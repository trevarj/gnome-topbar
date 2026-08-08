//! Bluetooth: the adapter, the paired devices, and the agent that answers a
//! pairing somebody else started.
//!
//! ```text
//!   model.rs   the snapshot, the ordering, the icons, the passkey (pure)
//!   proxy.rs   exactly the BlueZ surface the panel touches
//!   agent.rs   the object BlueZ asks "does this code match?"
//!   task.rs    the one owner: one connection, one match rule
//!   fake.rs    a BlueZ of a test's own
//! ```
//!
//! ## What the panel does, and when the radio transmits
//!
//! It lists **paired** devices and connects them. Open the device list and it
//! also scans, lists what it found under an "Available devices" header, and
//! pairs with a row that is clicked — pair, trust, connect, the same three
//! calls GNOME's own pairing makes.
//!
//! The discovery is bounded by the *chevron*, not by the panel. v1 scanned for
//! ten seconds every time its card was opened, which is a radio transmitting
//! because somebody looked at a panel; here nothing happens until the device
//! list is deliberately opened, and it stops the moment the list is collapsed,
//! the popover is closed, the radio goes off, a pairing starts, or the process
//! goes away. `BtState` carries both halves separately —
//! [`BtState::browsing`] is whether found devices are listed and
//! [`BtState::scanning`] is whether the radio is actually looking — because a
//! pairing needs the first without the second.
//!
//! The agent serves the *other* direction, and predates all of it: a phone that
//! starts a pairing with this machine asks a question, BlueZ hands it to the
//! default agent, and on a niri desktop there is otherwise nobody to answer. So
//! the panel answers it, in a row with the six-digit code and two buttons — the
//! same row an outgoing pairing raises. See [`agent`] for the capability that
//! promise is made under.
//!
//! ## Safety
//!
//! Everything that changes the machine is gated on
//! [`Access`](crate::network::Access), exactly as the network service is. A
//! debug build talking to the real system bus **registers no agent** — which
//! would otherwise take the session's own pairing prompts — refuses the radio
//! switch, every connect and every pairing, and never makes the developer's
//! own adapter transmit. Tests and the smoke run point the service at
//! a BlueZ of their own with `TOPBAR_SMOKE_BLUEZ_BUS`, which is the signal that
//! changing things is safe.

mod agent;
pub mod model;
mod proxy;
mod task;

#[cfg(any(test, feature = "fake-bluez"))]
pub mod fake;

#[cfg(test)]
mod bus_tests;

use std::sync::Arc;

use tokio::sync::{mpsc, oneshot, watch};

use crate::error::SvcError;
use crate::network::Access;

pub use model::{BtDevice, BtState, IconKind, PairingPrompt, PromptKind};

/// How many commands may be in flight before a sender waits.
const QUEUE: usize = 8;

/// The Bluetooth service.
///
/// Cloning is cheap — a channel sender and a watch subscription.
#[derive(Clone)]
pub struct Bluetooth {
    handle: BluetoothHandle,
    state: watch::Receiver<Arc<BtState>>,
    task: crate::lazy::Deferred,
}

impl Bluetooth {
    /// Start following BlueZ.
    ///
    /// `address` overrides the system bus, which is how the bus tests and the
    /// smoke run point this at a BlueZ of their own. It is also the signal that
    /// changing things is safe — see [`Access`].
    /// `wanted` is whether the Quick Settings menu is on the bar; nothing else
    /// draws Bluetooth, and a panel without it registers no pairing agent.
    pub(crate) fn start(address: Option<String>, wanted: bool) -> Self {
        let access = Access::decide(address.as_deref(), packaged());
        Self::with_access(address, access, wanted)
    }

    /// The same, with the policy decided by the caller.
    ///
    /// Only the bus tests use this, and only to force `ReadOnly` against a
    /// BlueZ they own — which is the one way to check that policy without
    /// pointing a test at somebody's real adapter to see what it does not do.
    pub(crate) fn with_access(address: Option<String>, access: Access, wanted: bool) -> Self {
        let (commands, queue) = mpsc::channel(QUEUE);
        let (publisher, state) = watch::channel(Arc::new(BtState {
            access,
            ..BtState::default()
        }));
        let task =
            crate::lazy::Deferred::spawn(wanted, task::run(queue, publisher, address, access));
        Self {
            handle: BluetoothHandle { commands },
            state,
            task,
        }
    }

    /// Start the task if it was held back. Returns whether this call did it.
    pub(crate) fn ensure_started(&self) -> bool {
        self.task.start()
    }

    /// The handle commands are sent through.
    pub fn handle(&self) -> &BluetoothHandle {
        &self.handle
    }

    /// Subscribe to Bluetooth state.
    pub fn state(&self) -> watch::Receiver<Arc<BtState>> {
        self.state.clone()
    }

    /// The state as of right now.
    pub fn current(&self) -> Arc<BtState> {
        self.state.borrow().clone()
    }
}

/// Whether this is the panel a user installed, rather than a build being worked
/// on.
///
/// The same distinction the network service draws, for the same reason: a debug
/// build talking to the machine's real BlueZ must not switch the radio off,
/// disconnect the headphones somebody is listening to, or register a pairing
/// agent that would take the session's own prompts.
const fn packaged() -> bool {
    !cfg!(debug_assertions)
}

/// What the panel may ask of Bluetooth.
#[derive(Clone)]
pub struct BluetoothHandle {
    commands: mpsc::Sender<task::Command>,
}

impl BluetoothHandle {
    /// Switch the adapter's radio on or off.
    ///
    /// Pessimistic: the pill does not move until BlueZ says the adapter did.
    pub async fn set_powered(&self, powered: bool) -> Result<(), SvcError> {
        self.send(|reply| task::Command::SetPowered { powered, reply })
            .await
    }

    /// Connect one device.
    ///
    /// Answers when BlueZ has finished, which for a device that is switched off
    /// is the better part of ten seconds — so the row spins for exactly as long
    /// as the attempt takes and reverts on something the user watched.
    pub async fn connect(&self, path: String) -> Result<(), SvcError> {
        self.send(|reply| task::Command::SetConnected {
            path,
            connected: true,
            reply,
        })
        .await
    }

    /// Disconnect one device.
    pub async fn disconnect(&self, path: String) -> Result<(), SvcError> {
        self.send(|reply| task::Command::SetConnected {
            path,
            connected: false,
            reply,
        })
        .await
    }

    /// Open the discovery session behind the device list.
    ///
    /// Called when the list is expanded, and paid for in radio time — so the
    /// caller must close it again on every path the list can leave the screen.
    pub async fn start_discovery(&self) -> Result<(), SvcError> {
        self.send(|reply| task::Command::SetDiscovery { on: true, reply })
            .await
    }

    /// Close it: stop looking, and drop what the scan found out of the list.
    pub async fn stop_discovery(&self) -> Result<(), SvcError> {
        self.send(|reply| task::Command::SetDiscovery { on: false, reply })
            .await
    }

    /// Pair with one of the devices the scan found, then connect it.
    ///
    /// Answers when the whole chain has finished. For a device that wants a
    /// code confirmed that is however long the user takes to look at the other
    /// screen — the question arrives in the panel's own pairing row while this
    /// is outstanding.
    pub async fn pair(&self, path: String) -> Result<(), SvcError> {
        self.send(|reply| task::Command::Pair { path, reply }).await
    }

    /// Answer the pairing row: Confirm.
    pub async fn confirm_pairing(&self) -> Result<(), SvcError> {
        self.send(|reply| task::Command::AnswerPrompt {
            confirm: true,
            reply,
        })
        .await
    }

    /// Answer it: Cancel.
    pub async fn cancel_pairing(&self) -> Result<(), SvcError> {
        self.send(|reply| task::Command::AnswerPrompt {
            confirm: false,
            reply,
        })
        .await
    }

    /// Send one command and wait for its answer.
    async fn send(
        &self,
        build: impl FnOnce(oneshot::Sender<Result<(), SvcError>>) -> task::Command,
    ) -> Result<(), SvcError> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(build(reply))
            .await
            .map_err(|_| SvcError::ServiceStopped("bluetooth"))?;
        answer
            .await
            .map_err(|_| SvcError::ServiceStopped("bluetooth"))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn commands_against_a_stopped_service_report_it() {
        let (commands, queue) = mpsc::channel(1);
        drop(queue);
        let handle = BluetoothHandle { commands };
        let error = handle
            .set_powered(true)
            .await
            .expect_err("nothing is listening");
        assert!(matches!(error, SvcError::ServiceStopped("bluetooth")));
    }

    #[test]
    fn only_a_packaged_panel_touches_an_adapter_it_was_not_given() {
        // Both arms asserted rather than whichever this build happens to be:
        // `nix flake check` runs the tests in *release*, where `packaged()` is
        // true.
        assert_eq!(Access::decide(None, false), Access::ReadOnly);
        assert_eq!(Access::decide(None, true), Access::Full);
        assert_eq!(Access::decide(Some("unix:path=/x"), false), Access::Full);
        assert_eq!(packaged(), !cfg!(debug_assertions));
    }
}
