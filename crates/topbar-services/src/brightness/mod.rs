//! The screen backlight.
//!
//! ```text
//!   device.rs    finding a controller and scaling its numbers   (pure + sysfs)
//!   throttle.rs  one call in flight, always the latest value          (pure)
//!   model.rs     the published snapshot                               (pure)
//!   task.rs      the one owner: logind writes, udev reads
//!   cli.rs       the standalone path `topbar brightness` takes
//! ```

mod device;
mod model;
mod task;
mod throttle;

pub mod cli;

#[cfg(test)]
mod bus_tests;

use std::sync::Arc;

use tokio::sync::{mpsc, oneshot, watch};

use crate::change::ChangeSource;
use crate::error::SvcError;

pub use model::BrightnessState;

use task::{Action, Command};

/// The default step for `topbar brightness inc`/`dec`, in points.
pub const DEFAULT_STEP: u32 = 5;

/// How many commands may be in flight before a sender waits.
const QUEUE: usize = 32;

/// The brightness service.
#[derive(Clone)]
pub struct Brightness {
    handle: BrightnessHandle,
    state: watch::Receiver<Arc<BrightnessState>>,
}

impl Brightness {
    /// Start following the backlight.
    ///
    /// `address` overrides the system bus, which is how the bus test points
    /// this at a logind of its own.
    pub(crate) fn start(address: Option<String>) -> Self {
        Self::start_at(address, None)
    }

    /// The same, reading a sysfs tree of the caller's choosing.
    ///
    /// The seam exists for the bus test, which cannot use `/sys/class/backlight`
    /// — the developer's screen is not a fixture, and a machine with no
    /// backlight would have nothing to test against.
    pub(crate) fn start_at(address: Option<String>, root: Option<std::path::PathBuf>) -> Self {
        let (commands, queue) = mpsc::channel(QUEUE);
        let (publisher, state) = watch::channel(Arc::new(BrightnessState::default()));
        tokio::spawn(task::run(queue, publisher, address, root));
        Self {
            handle: BrightnessHandle { commands },
            state,
        }
    }

    /// The handle commands are sent through.
    pub fn handle(&self) -> &BrightnessHandle {
        &self.handle
    }

    /// Subscribe to brightness state.
    pub fn state(&self) -> watch::Receiver<Arc<BrightnessState>> {
        self.state.clone()
    }

    /// The state as of right now.
    pub fn current(&self) -> Arc<BrightnessState> {
        self.state.borrow().clone()
    }
}

/// What the panel may ask of the backlight.
#[derive(Clone)]
pub struct BrightnessHandle {
    commands: mpsc::Sender<Command>,
}

impl BrightnessHandle {
    /// Set the backlight to a percentage, clamped to 0–100.
    pub async fn set(&self, percent: u32, source: ChangeSource) -> Result<(), SvcError> {
        self.send(Action::Set(percent), source).await
    }

    /// Turn it up by `step` points.
    pub async fn inc(&self, step: u32, source: ChangeSource) -> Result<(), SvcError> {
        self.send(Action::Step(points(step)), source).await
    }

    /// Turn it down by `step` points.
    pub async fn dec(&self, step: u32, source: ChangeSource) -> Result<(), SvcError> {
        self.send(Action::Step(-points(step)), source).await
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
            .map_err(|_| SvcError::ServiceStopped("brightness"))?;
        answer
            .await
            .map_err(|_| SvcError::ServiceStopped("brightness"))?
    }
}

/// A step as a signed number of points, never wrapping.
fn points(step: u32) -> i32 {
    i32::try_from(step).unwrap_or(i32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_step_matches_the_volume_one() {
        assert_eq!(DEFAULT_STEP, crate::audio::DEFAULT_STEP);
    }

    #[tokio::test]
    async fn commands_against_a_stopped_service_report_it() {
        let (commands, queue) = mpsc::channel(1);
        drop(queue);
        let handle = BrightnessHandle { commands };
        let error = handle
            .set(40, ChangeSource::Cli)
            .await
            .expect_err("nothing is listening");
        assert!(matches!(error, SvcError::ServiceStopped("brightness")));
    }
}
