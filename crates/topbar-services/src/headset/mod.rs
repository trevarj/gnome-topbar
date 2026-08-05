//! The headset battery, as `headsetcontrol` reports it.
//!
//! ```text
//!   model.rs   the JSON contract and the hide rules (pure)
//!   task.rs    the one owner: a timer, a subprocess, a snapshot
//! ```
//!
//! A wireless headset is the one battery in the room that no bus knows about:
//! UPower sees the laptop's, BlueZ sees anything paired over Bluetooth, and a
//! 2.4GHz dongle appears as neither. `headsetcontrol` talks to it over raw HID,
//! so the panel asks that.
//!
//! **The widget is invisible unless there is a reading**, which is most of the
//! time on most machines — the tool is not installed, or the headset is off, or
//! it is connected but asleep. A permanently empty pill on the right of the bar
//! would be worse than no widget, and the three shapes that mean "nothing to
//! report" are all normal rather than exceptional. See
//! [`model::parse`](model::parse) for what they look like.

pub mod model;
mod task;

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use topbar_core::config::HeadsetConfig;

pub use model::HeadsetReading;

use crate::lazy::Deferred;
use task::Poll;

/// Everything the panel knows about the headset.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeadsetState {
    /// The last reading, or `None` when there is nothing to draw.
    pub reading: Option<HeadsetReading>,
}

/// The headset service.
#[derive(Clone)]
pub struct Headset {
    state: watch::Receiver<Arc<HeadsetState>>,
    commands: mpsc::Sender<Poll>,
    task: Deferred,
}

impl Headset {
    /// Start polling `headsetcontrol`.
    ///
    /// `wanted` is whether a `headset` widget is on the bar. Without one there
    /// is no subprocess every few seconds for a reading nobody draws.
    pub(crate) fn start(config: &HeadsetConfig, wanted: bool) -> Self {
        let (publisher, state) = watch::channel(Arc::new(HeadsetState::default()));
        let (commands, queue) = mpsc::channel(2);
        let task = Deferred::spawn(wanted, task::run(publisher, poll(config), queue));
        Self {
            state,
            commands,
            task,
        }
    }

    /// Start the task if it was held back. Returns whether this call did it.
    pub(crate) fn ensure_started(&self) -> bool {
        self.task.start()
    }

    /// Subscribe to the headset battery.
    pub fn state(&self) -> watch::Receiver<Arc<HeadsetState>> {
        self.state.clone()
    }

    /// Apply a changed `[widgets.headset]`. Hot reload is what calls this.
    ///
    /// Not fallible: a service that has stopped is a panel that is shutting
    /// down, and there is nothing a caller could do about either.
    pub async fn configure(&self, config: &HeadsetConfig) {
        let _ = self.commands.send(poll(config)).await;
    }
}

/// What the service polls, out of `[widgets.headset]`.
fn poll(config: &HeadsetConfig) -> Poll {
    Poll {
        command: config.command.clone(),
        interval: Duration::from_secs(config.interval.max(1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_connected_is_the_default_and_draws_nothing() {
        assert_eq!(HeadsetState::default().reading, None);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_command_that_is_not_installed_leaves_the_widget_quiet() {
        let headset = Headset::start(
            &HeadsetConfig {
                command: "topbar-no-such-headsetcontrol".to_string(),
                interval: 1,
                ..HeadsetConfig::default()
            },
            true,
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(headset.state().borrow().reading, None);
    }
}
