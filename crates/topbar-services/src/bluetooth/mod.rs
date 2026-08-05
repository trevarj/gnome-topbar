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
//! ## What the panel does, and what it does not
//!
//! It lists **paired** devices and connects them, which is what GNOME's own
//! Quick Settings does. It does not scan, and it does not pair: pairing wants a
//! device list, a scan and a trust decision, and that is a Settings dialog
//! rather than a row in a menu. v1 did all of it, and the cost was a ten-second
//! discovery burst every time its card was opened — a radio transmitting
//! because somebody looked at a panel.
//!
//! The agent is the exception, and it is there for the *other* direction: a
//! phone that starts a pairing with this machine asks a question, BlueZ hands
//! it to the default agent, and on a niri desktop there is otherwise nobody to
//! answer. So the panel answers it, in a row with the six-digit code and two
//! buttons. See [`agent`] for the capability that promise is made under.
//!
//! ## Safety
//!
//! Everything that changes the machine is gated on
//! [`Access`](crate::network::Access), exactly as the network service is. A
//! debug build talking to the real system bus **registers no agent** — which
//! would otherwise take the session's own pairing prompts — and refuses the
//! radio switch and every connect. Tests and the smoke run point the service at
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

pub use model::{
    BLUETOOTH, BLUETOOTH_ACTIVE, BLUETOOTH_DISABLED, BtDevice, BtState, IconKind, PairingPrompt,
    PromptKind,
};

/// How many commands may be in flight before a sender waits.
const QUEUE: usize = 8;

/// The Bluetooth service.
///
/// Cloning is cheap — a channel sender and a watch subscription.
#[derive(Clone)]
pub struct Bluetooth {
    handle: BluetoothHandle,
    state: watch::Receiver<Arc<BtState>>,
}

impl Bluetooth {
    /// Start following BlueZ.
    ///
    /// `address` overrides the system bus, which is how the bus tests and the
    /// smoke run point this at a BlueZ of their own. It is also the signal that
    /// changing things is safe — see [`Access`].
    pub(crate) fn start(address: Option<String>) -> Self {
        let access = Access::decide(address.as_deref(), packaged());
        Self::with_access(address, access)
    }

    /// The same, with the policy decided by the caller.
    ///
    /// Only the bus tests use this, and only to force `ReadOnly` against a
    /// BlueZ they own — which is the one way to check that policy without
    /// pointing a test at somebody's real adapter to see what it does not do.
    pub(crate) fn with_access(address: Option<String>, access: Access) -> Self {
        let (commands, queue) = mpsc::channel(QUEUE);
        let (publisher, state) = watch::channel(Arc::new(BtState {
            access,
            ..BtState::default()
        }));
        tokio::spawn(task::run(queue, publisher, address, access));
        Self {
            handle: BluetoothHandle { commands },
            state,
        }
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
