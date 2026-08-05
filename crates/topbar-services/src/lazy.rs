//! Service tasks that are described at start-up and started when needed.
//!
//! Most of what the panel can do, a given panel is not doing. A bar with no
//! `crypto` widget still had a crypto service asking CoinGecko for prices every
//! half hour; a bar with no `quick_settings` still had a battery, a Bluetooth
//! adapter and an update count being polled for nobody. That is a request to a
//! stranger's API, a subprocess, and a handful of D-Bus clients, all for a
//! surface that does not exist.
//!
//! The fix is not to make the service optional — every widget would then have
//! to cope with a service that is not there, which is the kind of `Option` that
//! spreads. Instead the *channels* are always built, so handles and
//! subscriptions work exactly as before and a dormant service simply publishes
//! its empty snapshot forever, while the **task** is held here until something
//! asks for it.
//!
//! What asks is [`Services::start`](crate::Services::start), from the
//! configuration's widget placement, and — after a hot reload adds a widget —
//! [`Services::start_if_needed`](crate::Services::start_if_needed). Starting is
//! idempotent and one-way: nothing ever stops a service again, because a widget
//! removed from the bar is far more likely to come back than to have cost
//! anything by staying subscribed.

use std::future::Future;
use std::sync::{Arc, Mutex};

use crate::runtime::Runtime;

/// A task that is either already running or waiting to be asked.
///
/// Cloning shares the one task: whichever clone starts it, every other clone
/// sees it as started.
#[derive(Clone, Default)]
pub(crate) struct Deferred(Arc<Mutex<Option<Start>>>);

/// What starting a held task actually does.
type Start = Box<dyn FnOnce() + Send>;

impl Deferred {
    /// Run `task` now if `wanted`, or hold on to it until [`Self::start`].
    ///
    /// A held task is spawned onto the process-wide service runtime rather
    /// than onto whatever runtime happens to be current, because the caller
    /// that wakes it is the GTK main thread reacting to a reload.
    pub(crate) fn spawn<F>(wanted: bool, task: F) -> Self
    where
        F: Future<Output = ()> + Send + 'static,
    {
        if wanted {
            tokio::spawn(task);
            return Self::running();
        }
        Self::run(false, move || {
            Runtime::handle().spawn(task);
        })
    }

    /// The same for something that is not a tokio task at all.
    ///
    /// The privacy service is a plain thread — PipeWire's main loop is a C loop
    /// that blocks — and gating it needs the same switch.
    pub(crate) fn run<F>(wanted: bool, start: F) -> Self
    where
        F: FnOnce() + Send + 'static,
    {
        if wanted {
            start();
            return Self::running();
        }
        Self(Arc::new(Mutex::new(Some(Box::new(start)))))
    }

    /// A task that was started the usual way and is not deferred at all.
    pub(crate) fn running() -> Self {
        Self(Arc::new(Mutex::new(None)))
    }

    /// Start the task unless it is already running.
    ///
    /// Returns whether *this* call is what started it, so the caller can say
    /// so once in the log rather than on every reload.
    pub(crate) fn start(&self) -> bool {
        let held = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        match held {
            Some(start) => {
                start();
                true
            }
            None => false,
        }
    }

    /// Whether the task is still waiting to be asked for.
    #[cfg(test)]
    pub(crate) fn is_waiting(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[tokio::test]
    async fn a_wanted_task_runs_without_being_asked() {
        let runs = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&runs);
        let deferred = Deferred::spawn(true, async move {
            counter.fetch_add(1, Ordering::SeqCst);
        });

        assert!(!deferred.is_waiting());
        tokio::task::yield_now().await;
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        assert!(!deferred.start(), "there was nothing left to start");
    }

    #[tokio::test]
    async fn an_unwanted_task_waits_and_then_runs_exactly_once() {
        let runs = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&runs);
        let deferred = Deferred::spawn(false, async move {
            counter.fetch_add(1, Ordering::SeqCst);
        });

        assert!(deferred.is_waiting());
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(runs.load(Ordering::SeqCst), 0, "nobody asked for it");

        // The clone is what asks; the original has to agree that it started.
        assert!(deferred.clone().start());
        assert!(!deferred.is_waiting());
        assert!(!deferred.start(), "asking twice starts nothing twice");

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }
}
