//! Shutting the machine down, restarting it, and putting it to sleep.
//!
//! These go to logind over D-Bus rather than through `loginctl`, which is what
//! v1 did: a subprocess for something this consequential adds a fork, a PATH
//! lookup and a class of failure ("command not found") that has nothing to do
//! with whether the machine may be shut down.
//!
//! There is no long-running task and no state to publish. A power action
//! happens at most once per boot, so the connection is opened when one is
//! asked for and closed when it is answered — the alternative is a system-bus
//! connection held open for months to be used never.
//!
//! Logging out is *not* here: under niri that is the compositor's own business
//! and goes through [`crate::niri`].
//!
//! And it obeys [`Access`], for the bluntest reason in the project: logind is on
//! the **system** bus, there is no way to put a stand-in one in front of a test,
//! and a hold that ran to the end in a smoke session would suspend the machine
//! the session is running on. Every other service whose daemon is something of
//! the user's already reads-only in a development build; this is the one where
//! forgetting costs a reboot mid-test.

use crate::error::SvcError;
use crate::logind::{self, ManagerProxy};
use crate::network::Access;

/// One thing logind can be asked to do to the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerAction {
    /// Sleep.
    Suspend,
    /// Restart.
    Restart,
    /// Power off.
    ShutDown,
}

impl PowerAction {
    /// The label on the row.
    pub fn label(self) -> &'static str {
        match self {
            Self::Suspend => "Suspend",
            Self::Restart => "Restart",
            Self::ShutDown => "Shut Down",
        }
    }

    /// The logind method name, for the log line.
    pub fn method(self) -> &'static str {
        match self {
            Self::Suspend => "Suspend",
            Self::Restart => "Reboot",
            Self::ShutDown => "PowerOff",
        }
    }
}

/// Whether an answer from logind's `Can…` methods means "go ahead".
///
/// `challenge` counts as yes: it means polkit will ask, which is a dialog the
/// user can answer, not a refusal. `na` means the machine cannot do it at all
/// — no suspend support in firmware — and `no` means policy forbids it.
pub fn permits(answer: &str) -> bool {
    !matches!(answer.trim(), "no" | "na")
}

/// The power actions, as a handle the panel can hold.
///
/// Cloning is free: it is a bus address and nothing else.
#[derive(Debug, Clone, Default)]
pub struct Power {
    /// Overrides the system bus, for the bus tests.
    address: Option<String>,
}

impl Power {
    /// Describe how to reach logind.
    pub(crate) fn new(address: Option<String>) -> Self {
        Self { address }
    }

    /// Do it.
    ///
    /// Non-interactive: the panel has already asked, by making the user hold
    /// the row down for two thirds of a second. A second dialog on top of that
    /// would be one confirmation too many — and where policy insists on one,
    /// polkit raises it anyway and this call fails with a message the row
    /// shows.
    pub async fn act(&self, action: PowerAction) -> Result<(), SvcError> {
        if !self.access().writable() {
            return Err(SvcError::PowerAction(
                "this build does not act on the machine".into(),
            ));
        }
        let manager = self.manager().await?;
        let answer = match action {
            PowerAction::Suspend => manager.suspend(false).await,
            PowerAction::Restart => manager.reboot(false).await,
            PowerAction::ShutDown => manager.power_off(false).await,
        };
        answer.map_err(|error| SvcError::PowerAction(format!("{}: {error}", action.method())))
    }

    /// Whether logind will allow `action` at all.
    ///
    /// A machine that cannot suspend should not offer to. When logind cannot
    /// be reached the answer is yes: the row stays live and the *attempt*
    /// explains itself inline, which is more use than a permanently disabled
    /// row on a machine where the bus was merely slow to start.
    pub async fn allows(&self, action: PowerAction) -> bool {
        let Ok(manager) = self.manager().await else {
            return true;
        };
        let answer = match action {
            PowerAction::Suspend => manager.can_suspend().await,
            PowerAction::Restart => manager.can_reboot().await,
            PowerAction::ShutDown => manager.can_power_off().await,
        };
        answer.as_deref().map_or(true, permits)
    }

    /// Whether this build may act on the machine.
    ///
    /// An explicit address is a logind of the caller's own — the bus tests
    /// bring one up. Without one, only the packaged panel touches the real
    /// thing; a development build asks, and is told no, and the row says so.
    fn access(&self) -> Access {
        Access::decide(self.address.as_deref(), !cfg!(debug_assertions))
    }

    /// A manager proxy on a connection of this call's own.
    async fn manager(&self) -> Result<ManagerProxy<'static>, SvcError> {
        let connection = logind::connect(self.address.as_deref())
            .await
            .map_err(|error| SvcError::PowerAction(format!("no system bus: {error}")))?;
        ManagerProxy::new(&connection)
            .await
            .map_err(|error| SvcError::PowerAction(format!("no logind: {error}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_has_a_label_and_a_method() {
        for action in [
            PowerAction::Suspend,
            PowerAction::Restart,
            PowerAction::ShutDown,
        ] {
            assert!(!action.label().is_empty());
            assert!(!action.method().is_empty());
        }
        assert_eq!(PowerAction::ShutDown.method(), "PowerOff");
        assert_eq!(PowerAction::Restart.method(), "Reboot");
        assert_eq!(PowerAction::ShutDown.label(), "Shut Down");
    }

    #[test]
    fn a_challenge_is_permission_and_na_is_not() {
        assert!(permits("yes"));
        assert!(permits("challenge"));
        assert!(permits("yes\n"));
        assert!(!permits("no"));
        assert!(!permits("na"));
        // Anything logind adds later is treated as permission: the call
        // itself is the authority, and a refusal is visible.
        assert!(permits("maybe"));
    }

    #[tokio::test]
    async fn a_development_build_never_reaches_the_real_logind() {
        // No address means the *system* bus, and there is no way to put a
        // stand-in in front of it. The pointer-driven smoke run holds the
        // power rows down; without this, one of them would suspend the machine
        // the run is inside.
        let power = Power::new(None);
        let error = power
            .act(PowerAction::ShutDown)
            .await
            .expect_err("a debug build must refuse before it connects");
        assert!(
            format!("{error}").contains("does not act on the machine"),
            "{error}"
        );
    }
}

/// Power actions against a logind of the test's own.
///
/// The system bus is never touched: a `cargo test` that shut the developer's
/// machine down would be the last one anybody ran.
#[cfg(test)]
mod bus_tests {
    use std::time::Duration;

    use super::*;
    use crate::logind::bus_tests::{Log, journal, serve_logind};
    use crate::private_bus::private_bus;

    #[tokio::test]
    async fn each_action_calls_the_method_logind_names_it_by() {
        let bus = private_bus!();
        let log = Log::default();
        let _logind = serve_logind(&bus, &log, Duration::ZERO, None).await;
        let power = Power::new(Some(bus.address().to_string()));

        power.act(PowerAction::Suspend).await.expect("logind is up");
        power.act(PowerAction::Restart).await.expect("logind is up");
        power
            .act(PowerAction::ShutDown)
            .await
            .expect("logind is up");

        assert_eq!(
            journal(&log).power,
            vec![
                ("Suspend".to_string(), false),
                ("Reboot".to_string(), false),
                ("PowerOff".to_string(), false),
            ],
            "each action is non-interactive: the hold was the confirmation"
        );
    }

    #[tokio::test]
    async fn nothing_is_called_until_a_hold_completes() {
        let bus = private_bus!();
        let log = Log::default();
        let _logind = serve_logind(&bus, &log, Duration::ZERO, None).await;
        let _power = Power::new(Some(bus.address().to_string()));

        // Building the handle must not, by itself, do anything to the machine.
        assert!(journal(&log).power.is_empty());
    }

    #[tokio::test]
    async fn a_machine_that_cannot_suspend_does_not_offer_to() {
        let bus = private_bus!();
        let log = Log::default();
        let _logind = serve_logind(&bus, &log, Duration::ZERO, None).await;
        let power = Power::new(Some(bus.address().to_string()));

        // The fake answers `na` for suspend and `yes` for the rest.
        assert!(!power.allows(PowerAction::Suspend).await);
        assert!(power.allows(PowerAction::Restart).await);
        assert!(power.allows(PowerAction::ShutDown).await);
    }

    #[tokio::test]
    async fn a_refused_action_says_so_rather_than_looking_like_it_worked() {
        let bus = private_bus!();
        let log = Log::default();
        let _logind = serve_logind(&bus, &log, Duration::ZERO, None).await;
        let power = Power::new(Some(bus.address().to_string()));

        // The fake refuses hibernation-by-another-name: `Reboot` is accepted,
        // so the refusal is arranged by asking a manager that has gone away.
        drop(_logind);
        let error = power
            .act(PowerAction::ShutDown)
            .await
            .expect_err("nothing is serving logind any more");
        assert_eq!(error.user_message(), "The system refused that power action");
    }
}
