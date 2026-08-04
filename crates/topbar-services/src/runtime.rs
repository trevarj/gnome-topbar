//! The tokio runtime every service task shares, and the bundle of handles the
//! panel holds onto.
//!
//! [`Services::start`] runs before GTK is initialised and returns only handles
//! — watch receivers and `Clone` command handles. Nothing in the bundle can
//! reach a widget, and nothing a widget holds can block a service.

use std::path::PathBuf;
use std::sync::OnceLock;

use niri_ipc::socket::SOCKET_PATH_ENV;
use tokio::runtime;

use crate::media::Media;
use crate::niri::Niri;
use crate::notifications::Notifications;
use crate::state_store::StateStore;

static RUNTIME: OnceLock<runtime::Runtime> = OnceLock::new();

/// Accessor for the process-wide service runtime.
#[derive(Debug, Clone, Copy)]
pub struct Runtime;

impl Runtime {
    /// Start the runtime if it has not been started yet, returning its handle.
    ///
    /// Two worker threads are enough for the panel's workload: services are
    /// almost entirely I/O bound.
    pub fn handle() -> runtime::Handle {
        RUNTIME
            .get_or_init(|| {
                runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .thread_name("topbar-svc")
                    .enable_all()
                    .build()
                    .expect("service runtime should start")
            })
            .handle()
            .clone()
    }
}

/// Every running service, as handles the GTK side can hold.
///
/// New services become fields here; `start` is the one place that knows the
/// start-up order, so `main` does not change again as milestones land.
#[derive(Clone)]
pub struct Services {
    /// The niri compositor service.
    pub niri: Niri,
    /// The notification daemon.
    pub notifications: Notifications,
    /// The MPRIS media players.
    pub media: Media,
}

impl Services {
    /// Start every service. Call once, from `main`, before GTK.
    ///
    /// Blocking here is deliberate and momentary: services are spawned onto
    /// the runtime, not awaited, so this returns as soon as their tasks exist.
    pub fn start() -> Self {
        let niri_socket = std::env::var_os(SOCKET_PATH_ENV).map(PathBuf::from);
        Runtime::handle().block_on(async move {
            // The state file is read once, here, so every service that
            // restores something starts from one consistent document.
            let (state, store) = StateStore::open();
            Self {
                niri: Niri::start(niri_socket),
                notifications: Notifications::start(state.notifications, store, None),
                media: Media::start(None),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_is_stable_across_calls() {
        let first = Runtime::handle();
        let second = Runtime::handle();
        assert_eq!(first.id(), second.id());
    }

    #[test]
    fn runtime_can_run_a_task() {
        let value = Runtime::handle().block_on(async { 1 + 1 });
        assert_eq!(value, 2);
    }

    /// Compile-time proof that zbus links and its address parser works without
    /// a live bus, so M2 does not discover the native toolchain is broken.
    #[test]
    fn zbus_address_parsing_links() {
        let address: zbus::Address = "unix:path=/run/user/1000/bus"
            .try_into()
            .expect("well-formed bus address should parse");
        assert!(address.to_string().contains("/run/user/1000/bus"));
    }
}
