//! Privacy: whether something is watching the screen.
//!
//! ```text
//!   graph.rs    the node-graph heuristic (pure)
//!   worker.rs   the thread that talks to PipeWire
//! ```
//!
//! The *other* privacy indicator — whether something is listening — comes from
//! the audio service's `source_in_use`, because PulseAudio already knows and a
//! second answer to the same question would be a second thing to get wrong.
//! This module answers only the screen.
//!
//! ## Why this exists as a service at all
//!
//! v1 ran `pw-dump` once a second and diffed the JSON: 86,400 process spawns a
//! day to notice something that happens twice. The plan names the polling as
//! the defect and this as its replacement — PipeWire's registry says when a
//! node or a link appears, changes or goes, and the thread sleeps in between.
//!
//! ## What it may do
//!
//! Read the graph, and nothing else. Every other service in this crate can be
//! pointed at a fake for its tests; this one cannot, because the graph *is* the
//! feature and a fake graph is [`graph::Graph`], which is where the interesting
//! part is tested. So the connection is read-only by construction: the registry
//! is enumerated and listened to, and nothing is ever created, destroyed or
//! configured on the user's real session.

pub mod graph;
mod worker;

use std::sync::Arc;

use tokio::sync::watch;

/// What the panel knows about being watched.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PrivacyState {
    /// Whether a screen is being shared right now.
    pub screen_sharing: bool,
}

/// The privacy service.
#[derive(Clone)]
pub struct Privacy {
    state: watch::Receiver<Arc<PrivacyState>>,
    task: crate::lazy::Deferred,
}

impl Privacy {
    /// Start following PipeWire's node graph.
    ///
    /// The thread reports nothing on a machine with no PipeWire — which is the
    /// same answer as "nothing is being shared", and the right one. `wanted` is
    /// whether the Quick Settings menu, which owns the privacy dots, is on the
    /// bar: without it there is nowhere for a dot to appear.
    pub(crate) fn start(wanted: bool) -> Self {
        let (publisher, state) = watch::channel(Arc::new(PrivacyState::default()));
        // A plain thread rather than a tokio task: PipeWire's main loop is a C
        // loop that owns its connection and blocks, and putting it on a runtime
        // worker would take that worker out of the pool for the session.
        let task = crate::lazy::Deferred::run(wanted, move || {
            std::thread::Builder::new()
                .name("topbar-privacy".to_string())
                .spawn(move || worker::run(publisher))
                .map_or_else(
                    |error| tracing::warn!("privacy: could not start the PipeWire thread: {error}"),
                    |_| (),
                );
        });
        Self { state, task }
    }

    /// Start the thread if it was held back. Returns whether this call did it.
    pub(crate) fn ensure_started(&self) -> bool {
        self.task.start()
    }

    /// Subscribe to privacy state.
    pub fn state(&self) -> watch::Receiver<Arc<PrivacyState>> {
        self.state.clone()
    }

    /// The state as of right now.
    pub fn current(&self) -> Arc<PrivacyState> {
        self.state.borrow().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panel_that_has_not_looked_yet_reports_nothing() {
        // The default has to be "not being watched": a dot that appeared for a
        // moment at start-up on every login would be a dot nobody trusts.
        assert!(!PrivacyState::default().screen_sharing);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_service_starts_whether_or_not_there_is_a_pipewire_to_follow() {
        // Two environments, one test. On the developer's desktop the thread
        // connects to the real PipeWire read-only — the one thing this service
        // can be pointed at. Inside `nix flake check` there is no PipeWire at
        // all, the connection is declined, and the thread ends.
        //
        // What is asserted is what has to hold in *both*: the snapshot is
        // readable and says nothing is being watched. Whether this developer is
        // sharing their screen right now is not something a test may have an
        // opinion about, and a test that required a live sound server to pass
        // is a test that fails in the gate.
        let privacy = Privacy::start(true);
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        assert!(
            !privacy.current().screen_sharing,
            "nothing in this test shares a screen, either way"
        );
        // Still readable after the follower has stopped, which is what a
        // machine with no PipeWire leaves behind.
        let subscription = privacy.state();
        assert!(!subscription.borrow().screen_sharing);
    }
}
