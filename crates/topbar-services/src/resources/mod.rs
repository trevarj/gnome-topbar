//! CPU, memory and disks: what `/proc` says, five seconds at a time.
//!
//! ```text
//!   model.rs   the /proc parsers, the arithmetic, the formatting (pure)
//!   task.rs    the sampler: one timer, one snapshot
//! ```
//!
//! Two consumers, one service. Quick Settings draws a card from this snapshot,
//! and M10's `system_monitor` widget draws a bar-mounted indicator and a
//! details popover from the same one — which is why the interval is a
//! [`configure`](ResourcesHandle::configure) seam rather than a constant: that
//! widget's `interval` key belongs to it, and it must not mean starting a
//! second sampler reading the same three files.
//!
//! Sampling reads `/proc/stat`, `/proc/meminfo` and `/proc/self/mountinfo` and
//! calls `statvfs` once per disk. All of it is read-only and none of it needs a
//! bus, which is why this is the one service with no fake behind it: the tests
//! read fixture strings for the parsing and the real `/` for the one syscall.

pub mod model;
mod task;

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};

pub use model::{CpuSample, Disk, Memory, ResourceState};

/// How often the panel samples `/proc` by default.
///
/// v1's interval, and the right one: a bar that moved every second would be
/// something the eye keeps going back to, and one that moved every thirty would
/// be reporting the past.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(5);

/// The shortest interval the sampler will run at.
///
/// Reading three files and calling `statvfs` per disk is cheap but not free,
/// and a configuration asking for it a hundred times a second would be a panel
/// that spun a core to draw a bar.
pub const MIN_INTERVAL: Duration = Duration::from_secs(1);

/// The resources service.
#[derive(Clone)]
pub struct Resources {
    handle: ResourcesHandle,
    state: watch::Receiver<Arc<ResourceState>>,
}

impl Resources {
    /// Start sampling.
    pub(crate) fn start() -> Self {
        let (commands, queue) = mpsc::channel(4);
        let (publisher, state) = watch::channel(Arc::new(ResourceState::default()));
        tokio::spawn(task::run(queue, publisher, DEFAULT_INTERVAL));
        Self {
            handle: ResourcesHandle { commands },
            state,
        }
    }

    /// The handle the interval is set through.
    pub fn handle(&self) -> &ResourcesHandle {
        &self.handle
    }

    /// Subscribe to resource state.
    pub fn state(&self) -> watch::Receiver<Arc<ResourceState>> {
        self.state.clone()
    }

    /// The state as of right now.
    pub fn current(&self) -> Arc<ResourceState> {
        self.state.borrow().clone()
    }
}

/// What the panel may ask of the sampler.
#[derive(Clone)]
pub struct ResourcesHandle {
    commands: mpsc::Sender<Duration>,
}

impl ResourcesHandle {
    /// Sample at `interval` from now on.
    ///
    /// The seam M10's `system_monitor` uses: its `[widgets.system_monitor]
    /// interval` is the rate *that widget* wants, and honouring it must not
    /// mean a second sampler reading the same three files. Clamped to
    /// [`MIN_INTERVAL`].
    ///
    /// Not `#[must_use]`, and not fallible: there is nothing a caller could do
    /// about a sampler that has stopped, and nothing to report if it has.
    pub async fn configure(&self, interval: Duration) {
        let _ = self.commands.send(interval.max(MIN_INTERVAL)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_interval_is_the_one_v1_used() {
        assert_eq!(DEFAULT_INTERVAL, Duration::from_secs(5));
        assert!(MIN_INTERVAL <= DEFAULT_INTERVAL);
    }

    #[tokio::test]
    async fn configuring_a_stopped_sampler_is_not_an_error_anybody_has_to_handle() {
        let (commands, queue) = mpsc::channel(1);
        drop(queue);
        ResourcesHandle { commands }
            .configure(Duration::from_secs(2))
            .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_sampler_publishes_this_machine_within_two_intervals() {
        // The one test that reads the real /proc, and it only reads. What it
        // proves is that the three files exist, parse, and reach a snapshot —
        // the arithmetic itself is checked against fixtures in `model`.
        let resources = Resources::start();
        resources.handle().configure(MIN_INTERVAL).await;

        let mut state = resources.state();
        let settled = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                {
                    let current = state.borrow_and_update();
                    if current.cpu_pct.is_some() && current.memory.total_kib > 0 {
                        return current.clone();
                    }
                }
                state.changed().await.expect("the sampler is alive");
            }
        })
        .await
        .expect("a reading from this machine");

        assert!(settled.memory.used_pct <= 100);
        assert!(
            !settled.disks.is_empty(),
            "every machine this runs on has a root filesystem"
        );
        assert!(settled.disks.iter().all(|disk| disk.total > 0));
    }
}
