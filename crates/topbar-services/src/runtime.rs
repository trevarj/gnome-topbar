//! The tokio runtime every service task shares.
//!
//! The runtime is created once, before GTK starts, and handed out as a
//! `tokio::runtime::Handle`. Service bundles are added here from M2 onward.

use std::sync::OnceLock;

use tokio::runtime;

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
