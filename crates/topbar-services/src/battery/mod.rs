//! The battery: how much is left, what it is doing, and where it stops
//! charging.
//!
//! ```text
//!   model.rs   the published snapshot, the icon table, threshold rules (pure)
//!   sysfs.rs   the kernel's charge-limit files — the source of truth
//!   proxy.rs   the UPower interface, trimmed
//!   task.rs    the one owner: joins the two, commands in, readings out
//!   fake.rs    a UPower of a test's own
//! ```
//!
//! The charge limit has two possible owners and they do not agree. The kernel
//! exposes `charge_control_{start,end}_threshold` under
//! `/sys/class/power_supply`, and those files are what the firmware actually
//! reads; UPower also offers to manage the limit, and its copy of the numbers
//! can lag a write by seconds. The panel therefore **prefers sysfs whenever it
//! can write it**, falls back to UPower's `EnableChargeThreshold` when it
//! cannot, and **always reads the numbers back out of sysfs** — so what the
//! card shows is what the machine is doing, not what something was asked for.
//!
//! On a stock system the files are root-owned and neither path is available.
//! That is not a bug to engineer around: the fix is a udev rule, the card says
//! so in as many words, and the controls are disabled rather than hidden so
//! the machine's own capability is visible.

#[cfg(any(test, feature = "fake-power"))]
pub mod fake;
mod model;
mod proxy;
mod sysfs;
mod task;

#[cfg(test)]
mod bus_tests;

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::{mpsc, oneshot, watch};

use crate::error::SvcError;

pub use model::{
    BatteryState, BatteryStatus, FULL_PRESET, LIMIT_PRESET, LOW_PERCENT, Thresholds, duration, icon,
};
pub use sysfs::POWER_SUPPLY;

use task::{Action, Command};

/// How many commands may be in flight before a sender waits.
const QUEUE: usize = 8;

/// The battery service.
///
/// Cloning is cheap — a channel sender and a watch subscription.
#[derive(Clone)]
pub struct Battery {
    handle: BatteryHandle,
    state: watch::Receiver<Arc<BatteryState>>,
}

impl Battery {
    /// Start following the battery.
    ///
    /// `address` overrides the system bus and `root` the location of the
    /// kernel's power supplies; both exist so a test — and the smoke run's
    /// stand-in UPower — can be pointed somewhere harmless.
    pub(crate) fn start(address: Option<String>, root: Option<PathBuf>) -> Self {
        let (commands, queue) = mpsc::channel(QUEUE);
        let (publisher, state) = watch::channel(Arc::new(BatteryState::default()));
        let root = root.unwrap_or_else(|| PathBuf::from(POWER_SUPPLY));
        tokio::spawn(task::run(queue, publisher, address, root));
        Self {
            handle: BatteryHandle { commands },
            state,
        }
    }

    /// The handle commands are sent through.
    pub fn handle(&self) -> &BatteryHandle {
        &self.handle
    }

    /// Subscribe to battery state.
    pub fn state(&self) -> watch::Receiver<Arc<BatteryState>> {
        self.state.clone()
    }

    /// The state as of right now.
    pub fn current(&self) -> Arc<BatteryState> {
        self.state.borrow().clone()
    }
}

/// What the panel may ask of the battery.
#[derive(Clone)]
pub struct BatteryHandle {
    commands: mpsc::Sender<Command>,
}

impl BatteryHandle {
    /// Stop charging at `end`, and start again below `start`.
    ///
    /// Pessimistic: the card shows what the kernel reports after the write,
    /// never what was asked for, because a limit the firmware quietly refused
    /// would otherwise be drawn as though it had taken.
    pub async fn set_thresholds(&self, start: u8, end: u8) -> Result<(), SvcError> {
        self.send(Action::SetThresholds { start, end }).await
    }

    /// Read everything again.
    pub async fn refresh(&self) -> Result<(), SvcError> {
        self.send(Action::Refresh).await
    }

    /// Post a command and wait for the task to answer it.
    async fn send(&self, action: Action) -> Result<(), SvcError> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(Command { action, reply })
            .await
            .map_err(|_| SvcError::ServiceStopped("battery"))?;
        answer
            .await
            .map_err(|_| SvcError::ServiceStopped("battery"))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn commands_against_a_stopped_service_report_it() {
        let (commands, queue) = mpsc::channel(1);
        drop(queue);
        let handle = BatteryHandle { commands };
        let error = handle
            .set_thresholds(75, 80)
            .await
            .expect_err("nothing is listening");
        assert!(matches!(error, SvcError::ServiceStopped("battery")));
    }

    #[test]
    fn the_two_presets_are_both_valid_pairs() {
        assert!(model::validate(FULL_PRESET.0, FULL_PRESET.1).is_ok());
        assert!(model::validate(LIMIT_PRESET.0, LIMIT_PRESET.1).is_ok());
        assert!(
            LIMIT_PRESET.1 < 100,
            "the point of the limit preset is that it is a limit"
        );
        assert_eq!(FULL_PRESET.1, 100, "charging to full means to full");
    }
}
