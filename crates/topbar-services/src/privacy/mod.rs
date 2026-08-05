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
}

impl Privacy {
    /// Start following PipeWire's node graph.
    ///
    /// The thread is spawned unconditionally and reports nothing on a machine
    /// with no PipeWire — which is the same answer as "nothing is being
    /// shared", and the right one.
    pub(crate) fn start() -> Self {
        let (publisher, state) = watch::channel(Arc::new(PrivacyState::default()));
        // A plain thread rather than a tokio task: PipeWire's main loop is a C
        // loop that owns its connection and blocks, and putting it on a runtime
        // worker would take that worker out of the pool for the session.
        std::thread::Builder::new()
            .name("topbar-privacy".to_string())
            .spawn(move || worker::run(publisher))
            .map_or_else(
                |error| tracing::warn!("privacy: could not start the PipeWire thread: {error}"),
                |_| (),
            );
        Self { state }
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
    async fn the_service_starts_and_reports_this_session() {
        // Read-only against the developer's real PipeWire, which is the one
        // thing this service can be pointed at. What it proves is that the
        // thread starts, the library links, and the connection is made or
        // cleanly declined — the *heuristic* is tested against fixtures in
        // `graph`, where a screen can be shared without asking anybody to.
        let privacy = Privacy::start();
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        // No assertion on the *value*: whether this developer is sharing their
        // screen right now is not something a test may have an opinion about.
        // What is asserted is that asking is possible at all — a thread that
        // panicked would take the channel with it.
        let subscription = privacy.state();
        assert!(subscription.has_changed().is_ok(), "the follower is alive");
        let _ = privacy.current().screen_sharing;
    }
}
