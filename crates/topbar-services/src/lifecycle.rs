//! Suspend and resume, as one subscriber.
//!
//! A laptop that has been asleep since last night wakes up with a panel full of
//! yesterday: prices from twelve hours ago, a weather forecast for a day that
//! has ended, a battery reading from before it was unplugged, and — worst of
//! all, because it is the one nobody forgives — a clock that has to be looked
//! at twice. Everything on it is stale in the same instant and for the same
//! reason, so exactly one thing should notice.
//!
//! v1 subscribed to `PrepareForSleep` twice, from two different places, and
//! neither took an inhibitor: the panel was told the machine was going to sleep
//! at roughly the same time the machine went to sleep, so half of what it did
//! about it was still queued when the CPU stopped. Here there is one subscriber
//! and it holds a **delay inhibitor** — a file descriptor from
//! `Manager.Inhibit` whose existence blocks the suspend — so the panel is told
//! first, gets to say so, and only then lets go.
//!
//! ```text
//!   PrepareForSleep(true)   -> publish Suspending, release the lock
//!                              (the machine sleeps here)
//!   PrepareForSleep(false)  -> take the lock again, publish Resumed
//! ```
//!
//! The lock is released **after** the state is published, which is the whole
//! point of taking it: the consumers' reaction to "we are going down" happens
//! before the machine goes down. On the way back the order is reversed for the
//! same reason — the lock is in hand again before anything else can suspend.
//!
//! What consumers do about it is [`Services::wake`](crate::Services::wake).
//! The clock is deliberately not one of them: its tick is a one-shot timer
//! re-armed from inside its own callback, so a deadline that passed during the
//! sleep fires immediately on resume and the next one is aligned again. It
//! corrects itself, and a hook would be a second mechanism doing what the first
//! already does.

use std::sync::Arc;

use tokio::sync::watch;
use tracing::{debug, info, warn};
use zbus::zvariant::OwnedFd;

use crate::logind::{self, ManagerProxy};

/// What the panel knows about the machine's sleep.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LifecycleState {
    /// How many times this session has been told the machine is going to sleep.
    pub suspends: u64,
    /// How many times it has been told the machine is back.
    ///
    /// The number, not a flag: a consumer that acts "on resume" is really
    /// acting on *this number changing*, and a flag would make two resumes in
    /// quick succession look like one.
    pub resumes: u64,
    /// Whether the machine is on its way down right now.
    pub suspending: bool,
    /// Whether the delay inhibitor is held.
    ///
    /// False either because the machine is mid-suspend, or because there is no
    /// logind here at all — which is not an error and not worth a warning on
    /// every panel that runs without one.
    pub inhibited: bool,
}

/// The sleep/resume service.
///
/// Cloning is cheap: it is one watch subscription.
#[derive(Clone)]
pub struct Lifecycle {
    state: watch::Receiver<Arc<LifecycleState>>,
}

impl Lifecycle {
    /// Start following logind. `address` overrides the system bus, for tests.
    pub(crate) fn start(address: Option<String>) -> Self {
        let (publisher, state) = watch::channel(Arc::new(LifecycleState::default()));
        tokio::spawn(run(publisher, address));
        Self { state }
    }

    /// Subscribe to the sleep state.
    pub fn state(&self) -> watch::Receiver<Arc<LifecycleState>> {
        self.state.clone()
    }

    /// The state as of right now.
    pub fn current(&self) -> Arc<LifecycleState> {
        self.state.borrow().clone()
    }
}

/// Why the panel is asking to be told before the machine sleeps.
const WHY: &str = "refresh the panel before sleeping";
/// What the inhibitor covers. `sleep` only: the panel has no business delaying
/// a shutdown or a lid switch.
const WHAT: &str = "sleep";
/// A delay, not a block: this asks for a moment, not for a veto.
const MODE: &str = "delay";

/// Hold a delay inhibitor and follow `PrepareForSleep` until the panel exits.
async fn run(publisher: watch::Sender<Arc<LifecycleState>>, address: Option<String>) {
    let connection = match logind::connect(address.as_deref()).await {
        Ok(connection) => connection,
        // A machine with no logind is a machine that does not suspend in a way
        // anything here can see. Not an error.
        Err(error) => {
            info!("no logind on the bus; suspend and resume will not be noticed ({error})");
            return;
        }
    };
    let manager = match ManagerProxy::new(&connection).await {
        Ok(manager) => manager,
        Err(error) => {
            warn!("could not reach logind's manager: {error}");
            return;
        }
    };

    let mut signals = match manager.receive_prepare_for_sleep().await {
        Ok(signals) => signals,
        Err(error) => {
            warn!("could not subscribe to PrepareForSleep: {error}");
            return;
        }
    };

    let mut lock = take_lock(&manager).await;
    publish(&publisher, |state| state.inhibited = lock.is_some());

    use futures_util::StreamExt as _;
    while let Some(signal) = signals.next().await {
        let Ok(args) = signal.args() else {
            continue;
        };
        if args.start {
            info!("the machine is going to sleep");
            publish(&publisher, |state| {
                state.suspending = true;
                state.suspends += 1;
                state.inhibited = false;
            });
            // Only now. Dropping the descriptor is what tells logind the panel
            // is done, and everything above had to have happened first.
            drop(lock.take());
        } else {
            info!("the machine is back");
            lock = take_lock(&manager).await;
            publish(&publisher, |state| {
                state.suspending = false;
                state.resumes += 1;
                state.inhibited = lock.is_some();
            });
        }
    }

    debug!("logind stopped sending sleep signals");
}

/// Take the delay inhibitor, or explain once why there is none.
async fn take_lock(manager: &ManagerProxy<'_>) -> Option<OwnedFd> {
    match manager.inhibit(WHAT, "topbar", WHY, MODE).await {
        Ok(fd) => {
            debug!("holding a delay inhibitor for sleep");
            Some(fd)
        }
        Err(error) => {
            warn!("could not take a sleep inhibitor; the panel will still be told: {error}");
            None
        }
    }
}

/// Edit the state and publish it.
fn publish(publisher: &watch::Sender<Arc<LifecycleState>>, edit: impl FnOnce(&mut LifecycleState)) {
    publisher.send_if_modified(|current| {
        let mut next = **current;
        edit(&mut next);
        if **current == next {
            return false;
        }
        *current = Arc::new(next);
        true
    });
}

#[cfg(test)]
pub(crate) mod bus_tests {
    use std::time::Duration;

    use super::*;
    use crate::logind::bus_tests::{Log, journal, serve_logind, wait_for};
    use crate::private_bus::private_bus;

    /// Tell every subscriber the machine is going to sleep, or coming back.
    async fn prepare_for_sleep(connection: &zbus::Connection, start: bool) {
        connection
            .emit_signal(
                None::<&str>,
                crate::logind::bus_tests::MANAGER_PATH,
                "org.freedesktop.login1.Manager",
                "PrepareForSleep",
                &(start,),
            )
            .await
            .expect("the signal goes out");
    }

    /// Wait until the published state satisfies `wanted`.
    async fn wait_for_state(
        lifecycle: &Lifecycle,
        what: &str,
        wanted: impl Fn(&LifecycleState) -> bool,
    ) {
        let mut state = lifecycle.state();
        let wait = async {
            while !wanted(&state.borrow_and_update()) {
                state.changed().await.expect("the service is alive");
            }
        };
        tokio::time::timeout(Duration::from_secs(10), wait)
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
    }

    #[tokio::test]
    async fn the_panel_is_told_first_and_lets_go_afterwards() {
        let bus = private_bus!();
        let log = Log::default();
        let logind = serve_logind(&bus, &log, Duration::ZERO, None).await;

        let lifecycle = Lifecycle::start(Some(bus.address().to_string()));

        // The lock is taken as soon as the service starts, before anything has
        // happened: a delay inhibitor asked for after the signal arrives is an
        // inhibitor that inhibits nothing.
        wait_for("the inhibitor to be taken", || {
            !journal(&log).inhibits.is_empty()
        })
        .await;
        wait_for_state(&lifecycle, "the lock to be reported", |state| {
            state.inhibited
        })
        .await;

        let taken = journal(&log).inhibits[0].clone();
        assert_eq!(taken.what, "sleep", "only sleep, never shutdown");
        assert_eq!(taken.mode, "delay", "a moment, not a veto");
        assert_eq!(taken.who, "topbar");

        // Going down.
        prepare_for_sleep(&logind, true).await;
        wait_for_state(&lifecycle, "the suspend to be published", |state| {
            state.suspending && state.suspends == 1
        })
        .await;

        // And only then is logind allowed to carry on. The fake kept the read
        // end of the pipe it handed out, so this is the descriptor's real
        // lifetime rather than an assumption about it.
        wait_for("the inhibitor to be released", || {
            let journal = journal(&log);
            crate::logind::bus_tests::is_released(&journal.locks[0])
        })
        .await;

        // Coming back: a fresh lock, then the resume.
        prepare_for_sleep(&logind, false).await;
        wait_for_state(&lifecycle, "the resume to be published", |state| {
            state.resumes == 1 && !state.suspending && state.inhibited
        })
        .await;
        assert_eq!(
            journal(&log).inhibits.len(),
            2,
            "the lock has to be taken again, or the next suspend is not delayed"
        );
    }

    #[tokio::test]
    async fn every_sleep_is_counted_separately() {
        let bus = private_bus!();
        let log = Log::default();
        let logind = serve_logind(&bus, &log, Duration::ZERO, None).await;
        let lifecycle = Lifecycle::start(Some(bus.address().to_string()));
        wait_for_state(&lifecycle, "the first lock", |state| state.inhibited).await;

        for round in 1..=3 {
            prepare_for_sleep(&logind, true).await;
            wait_for_state(&lifecycle, "a suspend", |state| state.suspends == round).await;
            prepare_for_sleep(&logind, false).await;
            wait_for_state(&lifecycle, "a resume", |state| state.resumes == round).await;
        }

        // Three round trips, three counted, and the panel is inhibiting again.
        let state = lifecycle.current();
        assert_eq!((state.suspends, state.resumes), (3, 3));
        assert!(state.inhibited);
    }

    #[tokio::test]
    async fn a_machine_with_no_logind_reports_nothing_and_does_not_fail() {
        let bus = private_bus!();
        // Nothing is serving login1 on this bus.
        let lifecycle = Lifecycle::start(Some(bus.address().to_string()));
        tokio::time::sleep(Duration::from_millis(300)).await;

        let state = lifecycle.current();
        assert_eq!(state.suspends, 0);
        assert!(!state.inhibited, "there is nothing to inhibit with");
    }
}
