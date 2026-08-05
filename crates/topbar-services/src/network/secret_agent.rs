//! The panel's NetworkManager secret agent.
//!
//! When NetworkManager needs a password it does not go looking for one; it asks
//! every registered agent, over D-Bus, and waits. That is the whole reason this
//! exists, and it is also why joining a network from the panel never puts a
//! password in a command line: the key travels in a reply to a question
//! NetworkManager asked, on the system bus, and nowhere else. v1 ran
//! `nmcli … password <it>` and the key was readable in `/proc` for as long as
//! the process lived.
//!
//! The whole agent is four methods, and three of them are one line. `GetSecrets`
//! is a `#[zbus::interface]` method that *awaits* — it hands the request to the
//! service task, which puts a password row on screen, and suspends until the
//! user answers. zbus keeps the D-Bus reply pending in the meantime, which is
//! precisely the delayed reply NetworkManager's protocol expects. v1 built the
//! same thing out of manual `GDBusMethodInvocation` bookkeeping across 1738
//! lines.
//!
//! ## VPN secrets are out of scope in this milestone
//!
//! A VPN plugin that needs a password asks through the same call with
//! `setting_name = "vpn"`, and answering it properly means running the plugin's
//! own auth-dialog binary and speaking its stdin protocol — which is what v1
//! did, and what the plan drops. The agent therefore answers `NoSecrets` for
//! VPN requests: WireGuard profiles carry their keys, and an OpenVPN profile
//! whose secrets are stored in the keyring connects without asking. A profile
//! that genuinely needs typing gets an inline error naming the limitation
//! rather than a prompt that could never be answered.

use std::collections::HashMap;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};
use zbus::zvariant::{OwnedObjectPath, Value};

use super::model::{PSK_KEY, WIFI_SECURITY_SETTING, WIFI_SETTING, ssid_text};
use super::proxy::Connection;

/// Where the agent lives on the panel's own connection.
pub(crate) const AGENT_PATH: &str = "/org/freedesktop/NetworkManager/SecretAgent";
/// How the panel names itself to NetworkManager.
pub(crate) const AGENT_IDENTIFIER: &str = "io.github.trevarj.topbar";
/// How long a password row may sit unanswered before the agent gives up.
///
/// NetworkManager has no timeout of its own here; without one, a panel the user
/// walked away from would hold the card in `NEED_AUTH` indefinitely.
pub(crate) const PROMPT_TIMEOUT: Duration = Duration::from_secs(60);

/// A password, kept away from anything that formats.
///
/// [`Debug`] never prints it, so it cannot reach a log line through a `{:?}` on
/// some struct three layers up, and [`Drop`] overwrites the bytes rather than
/// handing the allocator a buffer with a Wi-Fi key still in it.
pub struct Secret(String);

impl Secret {
    /// Wrap a password the user typed.
    pub fn new(value: String) -> Self {
        Self(value)
    }

    /// The password, for the one place that puts it on the bus.
    fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Secret(***)")
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        // Through the bytes rather than through the `String`: assigning an
        // empty string would drop the old buffer with the key still in it.
        //
        // SAFETY: every byte is overwritten with an ASCII space, so the buffer
        // is still valid UTF-8 when `String`'s own drop runs.
        unsafe {
            for byte in self.0.as_bytes_mut() {
                *byte = b' ';
            }
        }
    }
}

/// The errors the agent may answer NetworkManager with.
///
/// The names are the ones `nm-applet` uses, because NetworkManager reads them:
/// `UserCanceled` stops it re-asking, and anything else is a failure it may
/// retry.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.NetworkManager.SecretAgent")]
pub(crate) enum AgentError {
    /// Something went wrong on the bus itself.
    #[zbus(error)]
    ZBus(zbus::Error),
    /// The panel has nothing stored and is not going to ask.
    NoSecrets(String),
    /// The user said no, or said nothing for long enough.
    UserCanceled(String),
}

/// A request from NetworkManager, on its way to a password row.
pub(crate) struct SecretRequest {
    /// The network it is for, when the panel could work one out.
    pub(crate) ssid: Option<String>,
    /// The connection object it is about, for matching a later cancel.
    pub(crate) path: OwnedObjectPath,
    /// The `GetSecrets` flags, which say whether this is a retry.
    pub(crate) flags: u32,
    /// Where to send the answer.
    pub(crate) reply: oneshot::Sender<Option<Secret>>,
}

/// A cancellation from NetworkManager.
pub(crate) struct CancelRequest {
    /// The connection object whose prompt is to be dropped.
    pub(crate) path: OwnedObjectPath,
    /// The settings group it was for.
    pub(crate) setting: String,
}

/// What the agent sends to the service task.
pub(crate) enum AgentMessage {
    /// Somebody wants a password.
    Secrets(Box<SecretRequest>),
    /// Never mind.
    Cancel(CancelRequest),
}

/// The object NetworkManager calls.
pub(crate) struct SecretAgent {
    requests: mpsc::Sender<AgentMessage>,
}

impl SecretAgent {
    /// Build an agent that forwards to `requests`.
    pub(crate) fn new(requests: mpsc::Sender<AgentMessage>) -> Self {
        Self { requests }
    }
}

#[zbus::interface(name = "org.freedesktop.NetworkManager.SecretAgent")]
impl SecretAgent {
    /// NetworkManager needs a secret for `setting_name` of `connection`.
    ///
    /// The reply is delayed for as long as the password row is on screen — up
    /// to [`PROMPT_TIMEOUT`] — which is what the protocol is for.
    async fn get_secrets(
        &self,
        connection: Connection,
        connection_path: OwnedObjectPath,
        setting_name: String,
        hints: Vec<String>,
        flags: u32,
    ) -> Result<Connection, AgentError> {
        debug!(
            "secret agent: {setting_name} for {}, flags {flags:#x}, hints {hints:?}",
            connection_path.as_str()
        );

        if setting_name != WIFI_SECURITY_SETTING {
            // Including "vpn": see the module documentation for why, and for
            // what the user sees instead.
            info!("secret agent: nothing to offer for {setting_name}");
            return Err(AgentError::NoSecrets(format!(
                "topbar has no {setting_name} secrets"
            )));
        }

        if !super::flow::wants_interaction(flags) {
            // A probe rather than a request: NetworkManager is asking whether
            // anybody has it saved, not asking the user.
            return Err(AgentError::NoSecrets(
                "topbar stores no secrets of its own".into(),
            ));
        }

        let ssid = connection
            .get(WIFI_SETTING)
            .and_then(|wifi| wifi.get("ssid"))
            .and_then(|value| Vec::<u8>::try_from(value.try_clone().ok()?).ok())
            .as_deref()
            .and_then(ssid_text);

        let (reply, answer) = oneshot::channel();
        let request = SecretRequest {
            ssid,
            path: connection_path,
            flags,
            reply,
        };
        if self
            .requests
            .send(AgentMessage::Secrets(Box::new(request)))
            .await
            .is_err()
        {
            warn!("secret agent: the network service has stopped");
            return Err(AgentError::NoSecrets("the panel is shutting down".into()));
        }

        match tokio::time::timeout(PROMPT_TIMEOUT, answer).await {
            Ok(Ok(Some(secret))) => Ok(psk_reply(&secret)),
            Ok(Ok(None)) => Err(AgentError::UserCanceled("cancelled in the panel".into())),
            // The task dropped the sender, which is what closing the panel or
            // starting a different attempt does.
            Ok(Err(_)) => Err(AgentError::UserCanceled("the prompt went away".into())),
            Err(_) => {
                info!("secret agent: nobody answered within {PROMPT_TIMEOUT:?}");
                Err(AgentError::NoSecrets("nobody answered the prompt".into()))
            }
        }
    }

    /// NetworkManager gave up on a request before the panel answered it.
    async fn cancel_get_secrets(
        &self,
        connection_path: OwnedObjectPath,
        setting_name: String,
    ) -> Result<(), AgentError> {
        debug!("secret agent: cancelled {setting_name}");
        let _ = self
            .requests
            .send(AgentMessage::Cancel(CancelRequest {
                path: connection_path,
                setting: setting_name,
            }))
            .await;
        Ok(())
    }

    /// NetworkManager is telling agents to persist a connection's secrets.
    ///
    /// Deliberately nothing. NetworkManager stores what it is given — as
    /// system-owned settings or in the user's keyring, by its own policy — and
    /// a panel keeping a second copy of every Wi-Fi key would be a second thing
    /// to get wrong.
    fn save_secrets(&self, _connection: Connection, _connection_path: OwnedObjectPath) {}

    /// And the other half of the same non-decision.
    fn delete_secrets(&self, _connection: Connection, _connection_path: OwnedObjectPath) {}
}

/// The one place a password is put on the bus.
fn psk_reply(secret: &Secret) -> Connection {
    let mut security = HashMap::new();
    if let Ok(value) = Value::from(secret.expose()).try_to_owned() {
        security.insert(PSK_KEY.to_string(), value);
    }
    let mut reply = HashMap::new();
    reply.insert(WIFI_SECURITY_SETTING.to_string(), security);
    reply
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_never_prints_itself() {
        let secret = Secret::new("hunter2".to_string());
        assert_eq!(format!("{secret:?}"), "Secret(***)");
        assert!(!format!("{secret:?}").contains("hunter2"));
    }

    #[test]
    fn a_reply_puts_the_key_where_networkmanager_looks_for_it() {
        let reply = psk_reply(&Secret::new("hunter2".to_string()));
        let security = reply
            .get(WIFI_SECURITY_SETTING)
            .expect("the security group");
        let psk = security.get(PSK_KEY).expect("the key");
        assert_eq!(
            String::try_from(psk.try_clone().expect("clone"))
                .ok()
                .as_deref(),
            Some("hunter2")
        );
        assert_eq!(reply.len(), 1, "nothing else is sent back");
    }

    #[test]
    fn the_agent_identifies_itself_by_the_application_id() {
        assert_eq!(AGENT_IDENTIFIER, "io.github.trevarj.topbar");
        assert!(AGENT_PATH.starts_with("/org/freedesktop/NetworkManager"));
    }
}
