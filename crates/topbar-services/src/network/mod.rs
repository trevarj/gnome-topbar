//! The network: Wi-Fi, Ethernet, VPN, and the secret agent that makes joining
//! a network possible without ever putting a password in a command line.
//!
//! ```text
//!   model.rs         the snapshot, and every decision with no bus in it (pure)
//!   flow.rs          one connect attempt, as a state machine (pure)
//!   proxy.rs         exactly the NetworkManager D-Bus surface the panel uses
//!   secret_agent.rs  the object NetworkManager asks for passwords
//!   task.rs          the one owner: one client, one set of subscriptions
//!   fake.rs          a NetworkManager of a test's own
//! ```
//!
//! This module **absorbs the connectivity watcher**. Until M9b the panel had a
//! second NetworkManager connection whose only job was to read one property;
//! [`crate::connectivity::Connectivity`] keeps its shape so the weather and
//! crypto services did not change, but what is behind it is now a subscription
//! to this service's snapshot.
//!
//! ## Joining a network, and why it looks like this
//!
//! There were two ways to do it. The first is to put the password straight into
//! the `AddAndActivateConnection` dictionary — legitimate, and already an
//! enormous improvement on v1's `nmcli … password <it>`, which left the key
//! readable in `/proc` for the life of the process. The second is to send
//! **no** password at all and let NetworkManager ask the panel's own secret
//! agent for it.
//!
//! The panel does the second, for four reasons:
//!
//! 1. The key travels only in a reply to a question NetworkManager asked. The
//!    panel never composes a message with a password in it.
//! 2. NetworkManager builds the profile itself, from the access point it was
//!    pointed at — so WPA2, WPA3-SAE, OWE and WEP each get the right key
//!    management without the panel guessing at `key-mgmt`.
//! 3. A refused password is a *first-class signal*: NetworkManager comes back
//!    with `REQUEST_NEW` set. With the password in the dictionary the panel
//!    would have to infer "wrong password" from a deactivation reason after the
//!    attempt had already been abandoned.
//! 4. It is the same path a saved network takes when its stored secret has gone
//!    — one flow rather than two.
//!
//! The cost is that the agent has to exist and be registered before the first
//! activation, which is what [`task`] does at start-up.

mod flow;
pub mod model;
mod proxy;
mod secret_agent;
mod task;

#[cfg(any(test, feature = "fake-nm"))]
pub mod fake;

#[cfg(test)]
mod bus_tests;

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot, watch};

use crate::error::SvcError;
use crate::state_store::StateStore;

pub use model::{
    Access, ApView, NetworkState, Pending, PendingPrompt, VpnKind, VpnView, WifiState, WiredState,
    online_from_state, ssid_text, strength_bucket,
};
pub use secret_agent::Secret;

/// How many commands may be in flight before a sender waits.
const QUEUE: usize = 8;

/// What the panel remembers about the network between runs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PersistedNetwork {
    /// The last VPN profile that was actually up.
    ///
    /// It sorts to the top of the list under whatever is running now, which on
    /// a machine with six profiles is the difference between one click and
    /// reading a list every time.
    pub last_vpn_uuid: Option<String>,
}

/// The network service.
///
/// Cloning is cheap — a channel sender and a watch subscription.
#[derive(Clone)]
pub struct Network {
    handle: NetworkHandle,
    state: watch::Receiver<Arc<NetworkState>>,
}

impl Network {
    /// Start following NetworkManager.
    ///
    /// `address` overrides the system bus, which is how the bus tests and the
    /// smoke run point this at a NetworkManager of their own. It is also the
    /// signal that changing things is safe — see [`Access`].
    pub(crate) fn start(
        address: Option<String>,
        persisted: PersistedNetwork,
        store: Option<StateStore>,
    ) -> Self {
        let (commands, queue) = mpsc::channel(QUEUE);
        let (publisher, state) = watch::channel(Arc::new(NetworkState {
            access: Access::decide(address.as_deref(), packaged()),
            ..NetworkState::default()
        }));
        tokio::spawn(task::run(
            queue,
            publisher,
            address,
            packaged(),
            persisted.last_vpn_uuid,
            store,
        ));
        Self {
            handle: NetworkHandle { commands },
            state,
        }
    }

    /// The handle commands are sent through.
    pub fn handle(&self) -> &NetworkHandle {
        &self.handle
    }

    /// Subscribe to network state.
    pub fn state(&self) -> watch::Receiver<Arc<NetworkState>> {
        self.state.clone()
    }

    /// The state as of right now.
    pub fn current(&self) -> Arc<NetworkState> {
        self.state.borrow().clone()
    }
}

/// Whether this is the panel a user installed, rather than a build being worked
/// on.
///
/// The distinction exists for exactly one reason: a debug build talking to the
/// machine's real NetworkManager must not join networks, switch the radio, make
/// the card transmit, or register a secret agent that would take the session's
/// own panel out of the queue for its prompts. A packaged build *is* the
/// session's panel and does the whole job.
const fn packaged() -> bool {
    !cfg!(debug_assertions)
}

/// What the panel may ask of the network.
#[derive(Clone)]
pub struct NetworkHandle {
    commands: mpsc::Sender<task::Command>,
}

impl NetworkHandle {
    /// Switch the Wi-Fi radio on or off.
    ///
    /// Pessimistic, like everything else here: the toggle does not move until
    /// NetworkManager says the radio did.
    pub async fn set_wifi_enabled(&self, enabled: bool) -> Result<(), SvcError> {
        self.send(|reply| task::Command::SetWifiEnabled { enabled, reply })
            .await
    }

    /// Join a network by name.
    ///
    /// Answers when the attempt has finished — which may be a minute later, if
    /// a password row went up in the middle of it. A refused password is not an
    /// error here: the prompt stays up and asks again, and the panel reads that
    /// from the snapshot.
    pub async fn connect(&self, ssid: String) -> Result<(), SvcError> {
        self.send(|reply| task::Command::Connect { ssid, reply })
            .await
    }

    /// Leave the network the card is on.
    pub async fn disconnect_wifi(&self) -> Result<(), SvcError> {
        self.send(|reply| task::Command::DisconnectWifi { reply })
            .await
    }

    /// Ask the card to look around, at most once every ten seconds.
    pub async fn scan(&self) -> Result<(), SvcError> {
        self.send(|reply| task::Command::Scan { reply }).await
    }

    /// Answer the password row.
    pub async fn submit_secret(&self, secret: Secret) -> Result<(), SvcError> {
        self.send(|reply| task::Command::SubmitSecret { secret, reply })
            .await
    }

    /// Take the password row away.
    pub async fn cancel_prompt(&self) -> Result<(), SvcError> {
        self.send(|reply| task::Command::CancelPrompt { reply })
            .await
    }

    /// Bring one VPN profile up, or take it down.
    ///
    /// Answers when NetworkManager has finished, so the row's spinner runs for
    /// exactly as long as the tunnel takes and a failure reverts something the
    /// user watched rather than something they had forgotten about.
    pub async fn set_vpn(&self, uuid: String, active: bool) -> Result<(), SvcError> {
        self.send(|reply| task::Command::SetVpn {
            uuid,
            active,
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
            .map_err(|_| SvcError::ServiceStopped("network"))?;
        answer
            .await
            .map_err(|_| SvcError::ServiceStopped("network"))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn commands_against_a_stopped_service_report_it() {
        let (commands, queue) = mpsc::channel(1);
        drop(queue);
        let handle = NetworkHandle { commands };
        let error = handle
            .connect("Home".into())
            .await
            .expect_err("nothing is listening");
        assert!(matches!(error, SvcError::ServiceStopped("network")));
    }

    #[test]
    fn a_development_build_is_never_treated_as_the_session_panel() {
        // The tests only ever run in a debug build, and the whole safety story
        // rests on this: without an explicit address, such a build reads.
        assert!(!packaged());
        assert_eq!(Access::decide(None, packaged()), Access::ReadOnly);
    }

    #[test]
    fn the_last_used_vpn_survives_a_round_trip_through_the_state_file() {
        let persisted = PersistedNetwork {
            last_vpn_uuid: Some("abc".into()),
        };
        let json = serde_json::to_string(&persisted).expect("serialise");
        let back: PersistedNetwork = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(back, persisted);
        // An older state file with no network section still loads.
        let empty: PersistedNetwork = serde_json::from_str("{}").expect("an empty document");
        assert_eq!(empty.last_vpn_uuid, None);
    }
}
