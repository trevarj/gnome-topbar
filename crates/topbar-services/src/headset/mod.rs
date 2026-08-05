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

use tokio::sync::watch;
use topbar_core::config::HeadsetConfig;

pub use model::HeadsetReading;

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
}

impl Headset {
    /// Start polling `headsetcontrol`.
    pub(crate) fn start(config: &HeadsetConfig) -> Self {
        let (publisher, state) = watch::channel(Arc::new(HeadsetState::default()));
        tokio::spawn(task::run(
            publisher,
            config.command.clone(),
            Duration::from_secs(config.interval.max(1)),
        ));
        Self { state }
    }

    /// Subscribe to the headset battery.
    pub fn state(&self) -> watch::Receiver<Arc<HeadsetState>> {
        self.state.clone()
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
        let headset = Headset::start(&HeadsetConfig {
            command: "topbar-no-such-headsetcontrol".to_string(),
            interval: 1,
            ..HeadsetConfig::default()
        });
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(headset.state().borrow().reading, None);
    }
}
