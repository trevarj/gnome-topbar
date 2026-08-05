//! One attempt to join a Wi-Fi network, as a state machine with no bus in it.
//!
//! Joining a secured network the machine has never seen is a conversation, not
//! a call:
//!
//! ```text
//!   panel   AddAndActivateConnection(no password)   → NetworkManager
//!   NM      GetSecrets(802-11-wireless-security)    → the panel's agent
//!   panel   [password row appears]
//!   user    types it
//!   panel   { psk: … }                              → NetworkManager
//!   NM      ActiveConnection.StateChanged           → the panel
//! ```
//!
//! and the interesting part is every way that goes wrong: the password is
//! refused and NetworkManager asks again with `REQUEST_NEW`; or the attempt
//! gives up and deactivates with `NO_SECRETS`, leaving behind the profile the
//! panel added a moment earlier; or the user presses Escape while the card is
//! still trying.
//!
//! All of that is here, as a type that takes events and returns what to do,
//! because all of it is testable without a radio.

use super::model::{
    ACTIVE_ACTIVATED, ACTIVE_DEACTIVATED, REASON_LOGIN_FAILED, REASON_NO_SECRETS,
    SECRET_REQUEST_NEW,
};

/// Where an attempt has got to.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Phase {
    /// NetworkManager is working on it.
    Activating,
    /// The panel is waiting for the user to type something.
    Prompting,
}

/// One attempt to join one network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Attempt {
    /// The network being joined.
    ssid: String,
    /// The settings object the panel created for this attempt, if any.
    ///
    /// `None` for a network with a saved profile: that one belongs to the user
    /// and is never deleted, however the attempt ends.
    added: Option<String>,
    /// How many passwords have been asked for.
    attempt: u32,
    phase: Phase,
}

/// Something that happened to an attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Event {
    /// NetworkManager asked the panel's agent for a Wi-Fi password.
    SecretsRequested {
        /// Whether the flags carry `REQUEST_NEW` — "the last one was wrong".
        request_new: bool,
    },
    /// The user typed one and pressed Connect.
    SecretSubmitted,
    /// The user gave up.
    Cancelled,
    /// The active connection moved.
    ActiveChanged {
        /// `NM_ACTIVE_CONNECTION_STATE_*`.
        state: u32,
        /// `NM_ACTIVE_CONNECTION_STATE_REASON_*`.
        reason: u32,
    },
    /// Nothing happened for too long.
    TimedOut,
}

/// Why an attempt ended badly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Failure {
    /// The user pressed Cancel, or closed the panel.
    Cancelled,
    /// Nothing came back.
    TimedOut,
    /// NetworkManager refused, for a reason of its own.
    Refused(u32),
}

/// What the caller should do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Step {
    /// Keep waiting.
    Wait,
    /// Put a password row on screen. `attempt` is 1 the first time.
    Prompt {
        /// Which ask this is, from 1.
        attempt: u32,
    },
    /// The password was refused. Bin `delete`, say so, and ask again.
    ///
    /// Both halves matter. Leaving the profile behind is what filled v1's
    /// network list with dead duplicates of the same SSID; leaving the prompt
    /// up is what stops the user having to find the network in the list again
    /// just because they mistyped a character.
    Reprompt {
        /// Which ask this is, from 2 — the first was refused.
        attempt: u32,
        /// The settings object to delete, if the panel added one.
        delete: Option<String>,
    },
    /// It worked.
    Connected,
    /// It did not, and there is no point asking again.
    Failed {
        /// Why.
        reason: Failure,
        /// The settings object to delete, if the panel added one.
        delete: Option<String>,
    },
}

impl Attempt {
    /// Begin an attempt on `ssid`.
    ///
    /// `added` is the settings object the panel created, and is `None` when the
    /// network already had a profile — which is the whole of the rule about
    /// what may be deleted afterwards.
    pub(crate) fn new(ssid: String, added: Option<String>) -> Self {
        Self {
            ssid,
            added,
            attempt: 0,
            phase: Phase::Activating,
        }
    }

    /// The network being joined.
    pub(crate) fn ssid(&self) -> &str {
        &self.ssid
    }

    /// Record the settings object a restarted attempt created.
    pub(crate) fn set_added(&mut self, added: Option<String>) {
        self.added = added;
    }

    /// Feed the machine, and be told what to do.
    pub(crate) fn apply(&mut self, event: Event) -> Step {
        match event {
            Event::SecretsRequested { request_new } => {
                // A fresh ask starts at one; `REQUEST_NEW` means the answer to
                // the last one was refused, which is the only signal on the bus
                // that says "wrong password" while the attempt is still alive.
                self.attempt = if request_new {
                    self.attempt.max(1) + 1
                } else {
                    self.attempt.max(1)
                };
                self.phase = Phase::Prompting;
                if self.attempt > 1 {
                    // Nothing to delete: the attempt has not failed, so the
                    // profile is still the one being tried.
                    Step::Reprompt {
                        attempt: self.attempt,
                        delete: None,
                    }
                } else {
                    Step::Prompt {
                        attempt: self.attempt,
                    }
                }
            }
            Event::SecretSubmitted => {
                self.phase = Phase::Activating;
                Step::Wait
            }
            Event::Cancelled => Step::Failed {
                reason: Failure::Cancelled,
                delete: self.added.take(),
            },
            Event::TimedOut => Step::Failed {
                reason: Failure::TimedOut,
                delete: self.added.take(),
            },
            Event::ActiveChanged { state, reason } => self.active_changed(state, reason),
        }
    }

    /// What a move of the active connection means.
    fn active_changed(&mut self, state: u32, reason: u32) -> Step {
        if state == ACTIVE_ACTIVATED {
            // Nothing to clean up: the profile the panel added is the profile
            // the machine is now using.
            self.added = None;
            return Step::Connected;
        }
        if state != ACTIVE_DEACTIVATED {
            return Step::Wait;
        }
        if matches!(reason, REASON_NO_SECRETS | REASON_LOGIN_FAILED) {
            self.attempt = self.attempt.max(1) + 1;
            self.phase = Phase::Prompting;
            return Step::Reprompt {
                attempt: self.attempt,
                delete: self.added.take(),
            };
        }
        Step::Failed {
            reason: Failure::Refused(reason),
            delete: self.added.take(),
        }
    }
}

/// Whether a `GetSecrets` call is one the panel should put a prompt up for.
///
/// NetworkManager probes agents without `ALLOW_INTERACTION` to find out whether
/// anyone has the secret stored. Answering "no" straight away is the correct
/// reply to that: putting a password row on screen because something asked a
/// question in passing is how a panel starts interrupting people.
pub(crate) fn wants_interaction(flags: u32) -> bool {
    flags & (super::model::SECRET_ALLOW_INTERACTION | SECRET_REQUEST_NEW) != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The path the panel's own `AddAndActivateConnection` came back with.
    const ADDED: &str = "/org/freedesktop/NetworkManager/Settings/42";

    fn unknown_network() -> Attempt {
        Attempt::new("Cafe".to_string(), Some(ADDED.to_string()))
    }

    fn saved_network() -> Attempt {
        Attempt::new("Home".to_string(), None)
    }

    #[test]
    fn a_probe_without_interaction_is_answered_rather_than_shown() {
        assert!(!wants_interaction(0));
        assert!(wants_interaction(0x1), "ALLOW_INTERACTION");
        assert!(wants_interaction(0x2), "REQUEST_NEW");
        assert!(wants_interaction(0x5), "ALLOW_INTERACTION | USER_REQUESTED");
    }

    #[test]
    fn the_happy_path_is_ask_answer_connected() {
        let mut attempt = unknown_network();
        assert_eq!(attempt.ssid(), "Cafe");

        assert_eq!(
            attempt.apply(Event::SecretsRequested { request_new: false }),
            Step::Prompt { attempt: 1 }
        );
        assert_eq!(attempt.apply(Event::SecretSubmitted), Step::Wait);

        assert_eq!(
            attempt.apply(Event::ActiveChanged {
                state: 1,
                reason: 0
            }),
            Step::Wait,
            "activating is not an answer"
        );
        assert_eq!(
            attempt.apply(Event::ActiveChanged {
                state: 2,
                reason: 0
            }),
            Step::Connected
        );
    }

    #[test]
    fn a_wrong_password_networkmanager_notices_at_once_asks_again() {
        let mut attempt = unknown_network();
        attempt.apply(Event::SecretsRequested { request_new: false });
        attempt.apply(Event::SecretSubmitted);

        // NetworkManager comes straight back rather than giving up: the profile
        // is still the one being tried, so nothing is deleted.
        assert_eq!(
            attempt.apply(Event::SecretsRequested { request_new: true }),
            Step::Reprompt {
                attempt: 2,
                delete: None
            }
        );
    }

    #[test]
    fn a_wrong_password_that_deactivates_takes_the_added_profile_with_it() {
        let mut attempt = unknown_network();
        attempt.apply(Event::SecretsRequested { request_new: false });
        attempt.apply(Event::SecretSubmitted);

        assert_eq!(
            attempt.apply(Event::ActiveChanged {
                state: 4,
                reason: REASON_NO_SECRETS
            }),
            Step::Reprompt {
                attempt: 2,
                delete: Some(ADDED.to_string())
            },
            "the profile the panel added must not outlive the attempt"
        );

        // ...and it is offered exactly once, so a second failure cannot ask for
        // the same object to be deleted twice.
        attempt.apply(Event::SecretSubmitted);
        assert_eq!(
            attempt.apply(Event::ActiveChanged {
                state: 4,
                reason: REASON_NO_SECRETS
            }),
            Step::Reprompt {
                attempt: 3,
                delete: None
            }
        );
    }

    #[test]
    fn a_rejected_login_reads_the_same_way_a_missing_secret_does() {
        let mut attempt = unknown_network();
        assert_eq!(
            attempt.apply(Event::ActiveChanged {
                state: 4,
                reason: REASON_LOGIN_FAILED
            }),
            Step::Reprompt {
                attempt: 2,
                delete: Some(ADDED.to_string())
            }
        );
    }

    #[test]
    fn a_saved_networks_profile_is_never_deleted_however_it_ends() {
        let mut attempt = saved_network();
        assert_eq!(
            attempt.apply(Event::ActiveChanged {
                state: 4,
                reason: REASON_NO_SECRETS
            }),
            Step::Reprompt {
                attempt: 2,
                delete: None
            },
            "the user's own profile belongs to the user"
        );
    }

    #[test]
    fn cancelling_cleans_up_after_itself() {
        let mut attempt = unknown_network();
        attempt.apply(Event::SecretsRequested { request_new: false });
        assert_eq!(
            attempt.apply(Event::Cancelled),
            Step::Failed {
                reason: Failure::Cancelled,
                delete: Some(ADDED.to_string())
            }
        );
    }

    #[test]
    fn a_prompt_nobody_answers_gives_up_and_tidies_up() {
        let mut attempt = unknown_network();
        attempt.apply(Event::SecretsRequested { request_new: false });
        assert_eq!(
            attempt.apply(Event::TimedOut),
            Step::Failed {
                reason: Failure::TimedOut,
                delete: Some(ADDED.to_string())
            }
        );
    }

    #[test]
    fn a_refusal_that_is_not_about_the_password_is_not_asked_about_again() {
        let mut attempt = unknown_network();
        // 3 DEVICE_DISCONNECTED — the card went away, or the AP did.
        assert_eq!(
            attempt.apply(Event::ActiveChanged {
                state: 4,
                reason: 3
            }),
            Step::Failed {
                reason: Failure::Refused(3),
                delete: Some(ADDED.to_string())
            }
        );
    }

    #[test]
    fn a_successful_connection_leaves_its_profile_alone() {
        let mut attempt = unknown_network();
        assert_eq!(
            attempt.apply(Event::ActiveChanged {
                state: ACTIVE_ACTIVATED,
                reason: 0
            }),
            Step::Connected
        );
        // A later teardown must not delete the profile the user is now using.
        assert_eq!(
            attempt.apply(Event::Cancelled),
            Step::Failed {
                reason: Failure::Cancelled,
                delete: None
            }
        );
    }

    #[test]
    fn a_restarted_attempt_remembers_the_new_profile_to_clean_up() {
        let mut attempt = unknown_network();
        attempt.apply(Event::ActiveChanged {
            state: 4,
            reason: REASON_NO_SECRETS,
        });
        attempt.set_added(Some("/second".to_string()));
        assert_eq!(
            attempt.apply(Event::Cancelled),
            Step::Failed {
                reason: Failure::Cancelled,
                delete: Some("/second".to_string())
            }
        );
    }
}
