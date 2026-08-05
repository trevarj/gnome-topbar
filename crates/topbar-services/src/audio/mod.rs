//! The sound server: what is playing out of, what is listening in, and how
//! loud either of them is.
//!
//! ```text
//!   volume.rs  percentages, PulseAudio volumes, the overdrive policy (pure)
//!   model.rs   the published snapshot                                (pure)
//!   worker.rs  the libpulse thread — the only file that links libpulse
//!   task.rs    the one owner: commands in, readings out
//!   cli.rs     the standalone path `topbar volume` takes with no panel running
//! ```
//!
//! Two things leave this module: an `Arc<AudioState>` on a watch channel, and
//! an [`AudioHandle`] whose every method returns a `Result`. Every mutating
//! method also takes a [`ChangeSource`], which is what lets the OSD tell "the
//! user pressed a media key" from "the user is dragging the slider they are
//! already looking at". See [`crate::change`] for how that is worked out.

mod model;
mod task;
mod volume;
mod worker;

pub mod cli;

use std::sync::Arc;

use tokio::sync::{mpsc, oneshot, watch};

use crate::change::ChangeSource;
use crate::error::SvcError;

pub use model::{AudioState, DeviceView};
pub use volume::{DEFAULT_STEP, max_percent as max_volume_percent, ui_max_percent};

use task::{Action, Command};

/// How many commands may be in flight before a sender waits.
const QUEUE: usize = 32;

/// The audio service.
///
/// Cloning is cheap — a channel sender and a watch subscription.
#[derive(Clone)]
pub struct Audio {
    handle: AudioHandle,
    state: watch::Receiver<Arc<AudioState>>,
}

impl Audio {
    /// Start following the sound server.
    ///
    /// `allow_overdrive` comes from `[audio] allow_overdrive` and fixes the
    /// ceiling for the life of the process; it is a policy, not a state, and
    /// nothing on the panel offers to change it.
    pub(crate) fn start(allow_overdrive: bool) -> Self {
        let (commands, queue) = mpsc::channel(QUEUE);
        let (publisher, state) = watch::channel(Arc::new(AudioState {
            max_volume_pct: volume::max_percent(allow_overdrive),
            ..AudioState::default()
        }));
        tokio::spawn(task::run(queue, publisher, allow_overdrive));
        Self {
            handle: AudioHandle { commands },
            state,
        }
    }

    /// The handle commands are sent through.
    pub fn handle(&self) -> &AudioHandle {
        &self.handle
    }

    /// Subscribe to audio state.
    pub fn state(&self) -> watch::Receiver<Arc<AudioState>> {
        self.state.clone()
    }

    /// The state as of right now.
    pub fn current(&self) -> Arc<AudioState> {
        self.state.borrow().clone()
    }
}

/// What the panel may ask of the sound server.
#[derive(Clone)]
pub struct AudioHandle {
    commands: mpsc::Sender<Command>,
}

impl AudioHandle {
    /// Set the default sink's volume, clamped to the overdrive policy.
    pub async fn set_sink_volume(
        &self,
        percent: u32,
        source: ChangeSource,
    ) -> Result<(), SvcError> {
        self.send(Action::SetSinkVolume(percent), source).await
    }

    /// Turn the default sink up by `step` percentage points.
    pub async fn inc_sink_volume(&self, step: u32, source: ChangeSource) -> Result<(), SvcError> {
        self.send(Action::StepSinkVolume(points(step)), source)
            .await
    }

    /// Turn the default sink down by `step` percentage points.
    pub async fn dec_sink_volume(&self, step: u32, source: ChangeSource) -> Result<(), SvcError> {
        self.send(Action::StepSinkVolume(-points(step)), source)
            .await
    }

    /// Mute or unmute the default sink.
    pub async fn set_sink_muted(&self, muted: bool, source: ChangeSource) -> Result<(), SvcError> {
        self.send(Action::SetSinkMuted(muted), source).await
    }

    /// Flip the default sink's mute.
    pub async fn toggle_sink_muted(&self, source: ChangeSource) -> Result<(), SvcError> {
        self.send(Action::ToggleSinkMuted, source).await
    }

    /// Set the default source's volume.
    pub async fn set_source_volume(
        &self,
        percent: u32,
        source: ChangeSource,
    ) -> Result<(), SvcError> {
        self.send(Action::SetSourceVolume(percent), source).await
    }

    /// Turn the default source up by `step` percentage points.
    pub async fn inc_source_volume(&self, step: u32, source: ChangeSource) -> Result<(), SvcError> {
        self.send(Action::StepSourceVolume(points(step)), source)
            .await
    }

    /// Turn the default source down by `step` percentage points.
    pub async fn dec_source_volume(&self, step: u32, source: ChangeSource) -> Result<(), SvcError> {
        self.send(Action::StepSourceVolume(-points(step)), source)
            .await
    }

    /// Mute or unmute the default source.
    pub async fn set_source_muted(
        &self,
        muted: bool,
        source: ChangeSource,
    ) -> Result<(), SvcError> {
        self.send(Action::SetSourceMuted(muted), source).await
    }

    /// Flip the default source's mute.
    pub async fn toggle_source_muted(&self, source: ChangeSource) -> Result<(), SvcError> {
        self.send(Action::ToggleSourceMuted, source).await
    }

    /// Make `id` the default output device.
    pub async fn set_default_sink(&self, id: String) -> Result<(), SvcError> {
        self.send(Action::SetDefaultSink(id), ChangeSource::Ui)
            .await
    }

    /// Make `id` the default input device.
    pub async fn set_default_source(&self, id: String) -> Result<(), SvcError> {
        self.send(Action::SetDefaultSource(id), ChangeSource::Ui)
            .await
    }

    /// Read the whole state again, for a panel that suspects it has drifted.
    pub async fn refresh(&self) -> Result<(), SvcError> {
        self.send(Action::Refresh, ChangeSource::External).await
    }

    /// Apply a changed `[audio] allow_overdrive`. Hot reload calls this.
    ///
    /// Nothing about the sound server is re-read: the ceiling is policy about
    /// what a slider may ask for and what the OSD draws as full.
    pub async fn set_allow_overdrive(&self, allow: bool) -> Result<(), SvcError> {
        self.send(Action::SetAllowOverdrive(allow), ChangeSource::External)
            .await
    }

    /// Post a command and wait for the task to accept it.
    async fn send(&self, action: Action, source: ChangeSource) -> Result<(), SvcError> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(Command {
                action,
                source,
                reply,
            })
            .await
            .map_err(|_| SvcError::ServiceStopped("audio"))?;
        answer
            .await
            .map_err(|_| SvcError::ServiceStopped("audio"))?
    }
}

/// A step as a signed number of percentage points, never wrapping.
fn points(step: u32) -> i32 {
    i32::try_from(step).unwrap_or(i32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_step_larger_than_an_i32_saturates_rather_than_wrapping() {
        assert_eq!(points(5), 5);
        assert_eq!(points(u32::MAX), i32::MAX);
        // Negating the saturated value must still be a decrease.
        assert!(-points(u32::MAX) < 0);
    }

    #[tokio::test]
    async fn commands_against_a_stopped_service_report_it() {
        let (commands, queue) = mpsc::channel(1);
        drop(queue);
        let handle = AudioHandle { commands };
        let error = handle
            .set_sink_volume(40, ChangeSource::Cli)
            .await
            .expect_err("nothing is listening");
        assert!(matches!(error, SvcError::ServiceStopped("audio")));
    }
}
