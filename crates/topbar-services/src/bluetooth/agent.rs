//! The panel's BlueZ pairing agent.
//!
//! ## What it is for, and what it is not for
//!
//! **Pairing is not started from Quick Settings.** GNOME does not do it there
//! and neither does this panel: pairing is a dialog with a device list, a scan
//! and a trust decision in it, and that belongs in Settings. The panel's job
//! is the *other* half — a phone or a headset that initiates a pairing with
//! this machine asks a question, and somebody has to be there to answer it. On
//! a niri desktop with no GNOME Shell running, nobody is. That is what this
//! object is: the place a six-digit code appears with Confirm and Cancel under
//! it.
//!
//! ## Capability, and what BlueZ therefore asks
//!
//! The agent registers as [`CAPABILITY`] — `DisplayYesNo`, which is what
//! gnome-bluetooth uses. It is a promise about the *machine*: it has a screen
//! and two buttons, and it has no keypad the pairing can be typed into. BlueZ
//! reads it and picks a pairing method accordingly, so with this capability it
//! will ask for a confirmation or an authorization and will never ask the
//! panel to produce a PIN or a passkey out of nowhere.
//!
//! `RequestPinCode` and `RequestPasskey` are therefore implemented as
//! **refusals**. They are unreachable given the capability, and answering them
//! properly would mean a text entry that only exists for a case BlueZ has been
//! told not to create. v1 built that entry — four boxes for a PIN, six for a
//! passkey — and registered as `KeyboardDisplay`, which is the capability that
//! makes BlueZ ask for them. This is the smaller promise, kept.
//!
//! ## The default agent
//!
//! [`AgentManagerProxy::request_default_agent`] is the part that makes an
//! incoming pairing reach this object rather than being refused: BlueZ hands a
//! request nobody claimed to the default agent, and returns an error when
//! there is none. So the panel asks for it — and asks for it **only** under
//! [`Access::Full`](crate::network::Access), which on the real system bus
//! means a packaged build. A build being worked on registers nothing at all,
//! because taking the default agent away from a session's real panel is how a
//! developer discovers their headphones will not pair any more.

use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};
use zbus::zvariant::OwnedObjectPath;

/// Where the agent lives on the panel's own connection.
pub(crate) const AGENT_PATH: &str = "/io/github/trevarj/topbar/BluetoothAgent";

/// What the panel promises BlueZ it can do.
///
/// A screen and two buttons: it can show a code and say yes or no, and it
/// cannot take one in. See the module documentation.
pub(crate) const CAPABILITY: &str = "DisplayYesNo";

/// How long a pairing question may sit unanswered.
///
/// BlueZ has a timeout of its own — around thirty seconds for most pairing
/// methods — but it ends the *pairing*, not the panel's row. Without this, a
/// prompt the user walked away from would stay on screen until the popover was
/// closed.
pub(crate) const PROMPT_TIMEOUT: Duration = Duration::from_secs(45);

/// The errors the agent may answer BlueZ with.
///
/// The two names BlueZ documents and acts on: `Rejected` means "no, and do not
/// try again", `Canceled` means "the question went away". Spelled with one `l`,
/// which is how BlueZ spells it.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.bluez.Error")]
pub(crate) enum AgentError {
    /// Something went wrong on the bus itself.
    #[zbus(error)]
    ZBus(zbus::Error),
    /// The panel will not answer this, and it is not going to change its mind.
    Rejected(String),
    /// The user said no, or the prompt went away underneath the question.
    Canceled(String),
}

/// A question on its way to a row in the panel.
pub(crate) struct Question {
    /// Which device it is about.
    pub(crate) path: OwnedObjectPath,
    /// The code to show, already formatted, or nothing for an authorization.
    pub(crate) code: Option<String>,
    /// What sort of answer is wanted.
    pub(crate) kind: super::model::PromptKind,
    /// Where the answer goes. `None` for a display-only prompt.
    pub(crate) reply: Option<oneshot::Sender<bool>>,
}

/// What the agent sends the service task.
pub(crate) enum AgentMessage {
    /// Put a pairing question on screen.
    Ask(Box<Question>),
    /// Take it away: BlueZ gave up, or the pairing finished.
    Cancel,
}

/// The object BlueZ calls.
pub(crate) struct PairingAgent {
    questions: mpsc::Sender<AgentMessage>,
}

impl PairingAgent {
    /// Build an agent that forwards to `questions`.
    pub(crate) fn new(questions: mpsc::Sender<AgentMessage>) -> Self {
        Self { questions }
    }

    /// Put a question on screen and wait for the answer.
    async fn ask(
        &self,
        path: OwnedObjectPath,
        code: Option<String>,
        kind: super::model::PromptKind,
    ) -> Result<(), AgentError> {
        let (reply, answer) = oneshot::channel();
        let question = Question {
            path,
            code,
            kind,
            reply: Some(reply),
        };
        if self
            .questions
            .send(AgentMessage::Ask(Box::new(question)))
            .await
            .is_err()
        {
            warn!("bluetooth agent: the service has stopped");
            return Err(AgentError::Canceled("the panel is shutting down".into()));
        }

        match tokio::time::timeout(PROMPT_TIMEOUT, answer).await {
            Ok(Ok(true)) => Ok(()),
            Ok(Ok(false)) => Err(AgentError::Rejected("refused in the panel".into())),
            // The task dropped the sender, which is what closing the panel or a
            // second pairing arriving does.
            Ok(Err(_)) => Err(AgentError::Canceled("the prompt went away".into())),
            Err(_) => {
                info!("bluetooth agent: nobody answered within {PROMPT_TIMEOUT:?}");
                Err(AgentError::Canceled("nobody answered the prompt".into()))
            }
        }
    }

    /// Show something the user has to type on the other device.
    ///
    /// Nothing to answer, so the reply goes back straight away and the row is
    /// cleared when the pairing finishes or BlueZ calls `Cancel`.
    async fn show(&self, path: OwnedObjectPath, code: String) {
        let question = Question {
            path,
            code: Some(code),
            kind: super::model::PromptKind::Display,
            reply: None,
        };
        let _ = self
            .questions
            .send(AgentMessage::Ask(Box::new(question)))
            .await;
    }
}

#[zbus::interface(name = "org.bluez.Agent1")]
impl PairingAgent {
    /// BlueZ is finished with this agent.
    async fn release(&self) {
        debug!("bluetooth agent: released");
        let _ = self.questions.send(AgentMessage::Cancel).await;
    }

    /// A device wants a PIN typed into this machine.
    ///
    /// Unreachable with [`CAPABILITY`]: a `DisplayYesNo` agent has told BlueZ
    /// it has no keypad. Refused rather than half-answered — see the module
    /// documentation for why the entry v1 built is not here.
    async fn request_pin_code(&self, device: OwnedObjectPath) -> Result<String, AgentError> {
        info!(
            "bluetooth agent: refusing a PIN request for {} (this agent has no keypad)",
            device.as_str()
        );
        Err(AgentError::Rejected(
            "topbar cannot take a PIN; pair from Settings".into(),
        ))
    }

    /// The same for a numeric passkey.
    async fn request_passkey(&self, device: OwnedObjectPath) -> Result<u32, AgentError> {
        info!(
            "bluetooth agent: refusing a passkey request for {} (this agent has no keypad)",
            device.as_str()
        );
        Err(AgentError::Rejected(
            "topbar cannot take a passkey; pair from Settings".into(),
        ))
    }

    /// Show a PIN for the user to type on the other device.
    async fn display_pin_code(
        &self,
        device: OwnedObjectPath,
        pincode: String,
    ) -> Result<(), AgentError> {
        debug!("bluetooth agent: showing a PIN for {}", device.as_str());
        self.show(device, pincode).await;
        Ok(())
    }

    /// Show a passkey, and how much of it has been typed so far.
    ///
    /// `entered` is deliberately ignored: it arrives once per keystroke, and
    /// a row that redrew six times while somebody typed would be a row that
    /// flickered for no information the user does not already have — they are
    /// looking at the keyboard they are typing on.
    async fn display_passkey(&self, device: OwnedObjectPath, passkey: u32, _entered: u16) {
        debug!("bluetooth agent: showing a passkey for {}", device.as_str());
        self.show(device, super::model::passkey_text(passkey)).await;
    }

    /// The one the whole agent exists for: "does this code match?"
    ///
    /// The reply is delayed for as long as the row is on screen, which is what
    /// the protocol is for. Answering `Ok` is Confirm; `Rejected` is Cancel.
    async fn request_confirmation(
        &self,
        device: OwnedObjectPath,
        passkey: u32,
    ) -> Result<(), AgentError> {
        info!(
            "bluetooth agent: {} wants a code confirmed",
            device.as_str()
        );
        self.ask(
            device,
            Some(super::model::passkey_text(passkey)),
            super::model::PromptKind::Confirm,
        )
        .await
    }

    /// "Just works" pairing: no code, only a decision.
    async fn request_authorization(&self, device: OwnedObjectPath) -> Result<(), AgentError> {
        info!("bluetooth agent: {} wants to pair", device.as_str());
        self.ask(device, None, super::model::PromptKind::Authorize)
            .await
    }

    /// A device that is already paired wants to use one of its profiles.
    ///
    /// Accepted. The question only reaches this agent for a device that has
    /// already been through a pairing the user confirmed, and putting a second
    /// dialog in front of somebody for "your headphones would like to be
    /// headphones" is the kind of prompt people learn to click through without
    /// reading — which costs more security than it buys.
    async fn authorize_service(
        &self,
        device: OwnedObjectPath,
        uuid: String,
    ) -> Result<(), AgentError> {
        debug!("bluetooth agent: allowing {uuid} for {}", device.as_str());
        Ok(())
    }

    /// BlueZ gave up on the question before the panel answered it.
    async fn cancel(&self) {
        debug!("bluetooth agent: BlueZ cancelled the pairing");
        let _ = self.questions.send(AgentMessage::Cancel).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_agent_promises_a_screen_and_two_buttons_and_nothing_more() {
        assert_eq!(CAPABILITY, "DisplayYesNo");
        assert!(AGENT_PATH.starts_with("/io/github/trevarj/topbar"));
    }

    #[tokio::test]
    async fn a_keypad_request_is_refused_rather_than_left_hanging() {
        let (questions, mut queue) = mpsc::channel(1);
        let agent = PairingAgent::new(questions);
        let device = OwnedObjectPath::try_from("/org/bluez/hci0/dev_AA").expect("a path");

        let error = agent
            .request_pin_code(device.clone())
            .await
            .expect_err("a DisplayYesNo agent has no keypad");
        assert!(matches!(error, AgentError::Rejected(_)));

        let error = agent
            .request_passkey(device)
            .await
            .expect_err("nor a keypad for passkeys");
        assert!(matches!(error, AgentError::Rejected(_)));

        assert!(
            queue.try_recv().is_err(),
            "a refusal must not put a row on screen"
        );
    }

    #[tokio::test]
    async fn a_confirmation_waits_for_the_row_and_carries_the_padded_code() {
        let (questions, mut queue) = mpsc::channel(1);
        let agent = PairingAgent::new(questions);
        let device = OwnedObjectPath::try_from("/org/bluez/hci0/dev_AA").expect("a path");

        let call = tokio::spawn(async move { agent.request_confirmation(device, 42).await });

        let AgentMessage::Ask(question) = queue.recv().await.expect("a question") else {
            panic!("expected a question");
        };
        assert_eq!(question.code.as_deref(), Some("000042"));
        assert_eq!(question.kind, super::super::model::PromptKind::Confirm);
        question
            .reply
            .expect("answerable")
            .send(true)
            .expect("sent");

        call.await.expect("joined").expect("confirmed");
    }

    #[tokio::test]
    async fn cancelling_the_row_refuses_the_pairing() {
        let (questions, mut queue) = mpsc::channel(1);
        let agent = PairingAgent::new(questions);
        let device = OwnedObjectPath::try_from("/org/bluez/hci0/dev_AA").expect("a path");

        let call = tokio::spawn(async move { agent.request_authorization(device).await });

        let AgentMessage::Ask(question) = queue.recv().await.expect("a question") else {
            panic!("expected a question");
        };
        assert!(question.code.is_none(), "an authorization has no code");
        question
            .reply
            .expect("answerable")
            .send(false)
            .expect("sent");

        let error = call.await.expect("joined").expect_err("refused");
        assert!(matches!(error, AgentError::Rejected(_)));
    }

    #[tokio::test]
    async fn a_prompt_dropped_underneath_the_question_reads_as_cancelled() {
        let (questions, mut queue) = mpsc::channel(1);
        let agent = PairingAgent::new(questions);
        let device = OwnedObjectPath::try_from("/org/bluez/hci0/dev_AA").expect("a path");

        let call = tokio::spawn(async move { agent.request_confirmation(device, 1).await });

        let AgentMessage::Ask(question) = queue.recv().await.expect("a question") else {
            panic!("expected a question");
        };
        // Dropping the sender is what a second pairing, or a closing panel,
        // does to the one already on screen.
        drop(question.reply);

        let error = call.await.expect("joined").expect_err("no answer");
        assert!(matches!(error, AgentError::Canceled(_)));
    }

    #[tokio::test]
    async fn a_display_prompt_answers_at_once_and_asks_for_nothing_back() {
        let (questions, mut queue) = mpsc::channel(1);
        let agent = PairingAgent::new(questions);
        let device = OwnedObjectPath::try_from("/org/bluez/hci0/dev_AA").expect("a path");

        agent
            .display_pin_code(device, "1234".into())
            .await
            .expect("nothing to wait for");

        let AgentMessage::Ask(question) = queue.recv().await.expect("a question") else {
            panic!("expected a question");
        };
        assert_eq!(question.code.as_deref(), Some("1234"));
        assert!(question.reply.is_none(), "there is nothing to answer");
        assert_eq!(question.kind, super::super::model::PromptKind::Display);
    }

    #[tokio::test]
    async fn bluez_giving_up_takes_the_row_away() {
        let (questions, mut queue) = mpsc::channel(1);
        let agent = PairingAgent::new(questions);
        agent.cancel().await;
        assert!(matches!(
            queue.recv().await.expect("a message"),
            AgentMessage::Cancel
        ));
    }
}
