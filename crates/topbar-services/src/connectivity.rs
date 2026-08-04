//! Is this machine on the internet?
//!
//! The weather service — and, from M10, every `custom-*` widget marked
//! `requires_network` — has to know, because polling an HTTP endpoint on a
//! laptop in a tunnel is a request that will fail slowly and then be retried.
//! One `State` property on NetworkManager answers it, so that is all this
//! module reads.
//!
//! **This is deliberately not the network service.** M9 builds that one:
//! devices, access points, VPN, the secret agent. When it lands it absorbs
//! this module — the `Connectivity` handle keeps its shape so the consumers do
//! not change, but the task behind it becomes one more subscriber to the full
//! NetworkManager state rather than a connection of its own.
//!
//! The failure mode is chosen rather than inherited: a machine with no
//! NetworkManager on its system bus is assumed to be **online**. Guessing
//! "offline" there would leave the weather widget permanently blank on every
//! box that runs systemd-networkd, connman, or nothing at all, and a failed
//! fetch costs one timeout while a wrong guess costs the whole feature.

use std::sync::Arc;

use futures_util::StreamExt;
use tokio::sync::watch;
use tracing::{debug, info, warn};
use zbus::Connection;

/// `NM_STATE_UNKNOWN` — NetworkManager is not sure, so neither are we.
const STATE_UNKNOWN: u32 = 0;
/// `NM_STATE_CONNECTED_SITE`: routable, but NM's own connectivity check did
/// not reach its test endpoint.
const STATE_CONNECTED_SITE: u32 = 60;

/// What the panel knows about the network.
///
/// One field on purpose. Anything richer is M9's, and a snapshot that grows
/// speculative fields is a snapshot every consumer has to re-read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectivityState {
    /// Whether a request to the internet is worth making.
    pub online: bool,
}

impl Default for ConnectivityState {
    /// Online, until something says otherwise.
    ///
    /// The panel starts before it has talked to any bus; starting "offline"
    /// would defer the first weather fetch behind a round trip that may never
    /// come back.
    fn default() -> Self {
        Self { online: true }
    }
}

/// The connectivity watcher.
///
/// Cloning is cheap: it is one watch subscription.
#[derive(Clone)]
pub struct Connectivity {
    state: watch::Receiver<Arc<ConnectivityState>>,
}

impl Connectivity {
    /// Start watching NetworkManager.
    ///
    /// `address` overrides the system bus, which is how the bus test points
    /// this at a `dbus-daemon` of its own instead of the machine's real
    /// NetworkManager.
    pub(crate) fn start(address: Option<String>) -> Self {
        let (publisher, state) = watch::channel(Arc::new(ConnectivityState::default()));
        tokio::spawn(run(publisher, address));
        Self { state }
    }

    /// Subscribe to the online/offline state.
    pub fn state(&self) -> watch::Receiver<Arc<ConnectivityState>> {
        self.state.clone()
    }

    /// Whether the machine is online right now.
    pub fn is_online(&self) -> bool {
        self.state.borrow().online
    }
}

/// Whether an `NM_STATE_*` value means "a request is worth making".
///
/// The threshold is `CONNECTED_SITE`, one step below v1's, which took
/// `CONNECTED_GLOBAL` (70) alone — v1 could afford that because it fell back
/// to link-level state (`wifi.connected || wired.connected`) whenever NM's own
/// answer was missing, and this module has no such fallback. A box whose
/// connectivity check is switched off, or whose captive-portal probe is
/// blocked, sits at 60 forever; refusing to fetch there would be the same
/// permanent blank the missing-NetworkManager case is written to avoid.
/// `UNKNOWN` is treated the same way, and for the same reason.
fn online_from_state(state: u32) -> bool {
    state == STATE_UNKNOWN || state >= STATE_CONNECTED_SITE
}

/// Follow NetworkManager's `State` for as long as it is on the bus.
async fn run(publisher: watch::Sender<Arc<ConnectivityState>>, address: Option<String>) {
    let connection = match connect(address).await {
        Ok(connection) => connection,
        Err(error) => {
            info!("no system bus ({error}); assuming the machine is online");
            return;
        }
    };

    let proxy = match NetworkManagerProxy::new(&connection).await {
        Ok(proxy) => proxy,
        Err(error) => {
            info!("no NetworkManager proxy ({error}); assuming the machine is online");
            return;
        }
    };

    // Subscribed before the first read, so a change that lands between the two
    // is queued rather than lost.
    let mut changes = match proxy.receive_state_changed().await {
        Ok(changes) => changes,
        Err(error) => {
            info!("NetworkManager is not answering ({error}); assuming the machine is online");
            return;
        }
    };

    match proxy.nm_state().await {
        Ok(state) => publish(&publisher, state),
        Err(error) => {
            info!("NetworkManager has no state to report ({error}); assuming online");
            return;
        }
    }

    while let Some(signal) = changes.next().await {
        match signal.args() {
            Ok(args) => publish(&publisher, args.state),
            Err(error) => warn!("unreadable NetworkManager state change: {error}"),
        }
    }

    // NetworkManager went away. Whatever replaced it — or nothing at all — the
    // panel must not be left believing the machine is offline for good.
    warn!("NetworkManager stopped reporting; assuming the machine is online");
    let _ = publisher.send(Arc::new(ConnectivityState::default()));
}

/// The system bus, or the address a test handed us.
async fn connect(address: Option<String>) -> zbus::Result<Connection> {
    match address {
        Some(address) => {
            zbus::connection::Builder::address(address.as_str())?
                .build()
                .await
        }
        None => Connection::system().await,
    }
}

/// Publish a state, logging only the transitions.
fn publish(publisher: &watch::Sender<Arc<ConnectivityState>>, state: u32) {
    let online = online_from_state(state);
    if publisher.borrow().online == online {
        debug!("NetworkManager state {state}; still {}", label(online));
        return;
    }
    info!(
        "NetworkManager state {state}; the machine is {}",
        label(online)
    );
    let _ = publisher.send(Arc::new(ConnectivityState { online }));
}

/// The word for a log line.
fn label(online: bool) -> &'static str {
    if online { "online" } else { "offline" }
}

/// The one property and the one signal this module reads.
///
/// The property is renamed on the Rust side: zbus derives
/// `receive_state_changed` from a property called `State` *and* from a signal
/// called `StateChanged`, and NetworkManager has both.
#[zbus::proxy(
    interface = "org.freedesktop.NetworkManager",
    default_service = "org.freedesktop.NetworkManager",
    default_path = "/org/freedesktop/NetworkManager"
)]
trait NetworkManager {
    /// `NM_STATE_*` for the machine as a whole.
    #[zbus(property, name = "State")]
    fn nm_state(&self) -> zbus::Result<u32>;

    /// Emitted whenever that value changes.
    #[zbus(signal)]
    fn state_changed(&self, state: u32) -> zbus::Result<()>;
}

/// A NetworkManager that only has a `State`, served on a bus of the test's
/// own. The weather service's own bus tests reach for it too, which is why it
/// is a module rather than a few functions in one test file.
#[cfg(test)]
pub(crate) mod bus_tests {
    use std::time::Duration;

    use zbus::object_server::SignalEmitter;

    use super::*;
    use crate::private_bus::{PrivateBus, private_bus};

    /// Where NetworkManager lives on the bus.
    const NM_PATH: &str = "/org/freedesktop/NetworkManager";
    /// The name the panel looks for.
    const NM_NAME: &str = "org.freedesktop.NetworkManager";
    /// How long a test waits for a transition before failing.
    const PATIENCE: Duration = Duration::from_secs(10);

    /// The one property and the one signal the panel reads.
    pub(crate) struct FakeNetworkManager {
        state: u32,
    }

    #[zbus::interface(name = "org.freedesktop.NetworkManager")]
    impl FakeNetworkManager {
        #[zbus(property)]
        fn state(&self) -> u32 {
            self.state
        }

        /// Renamed on the Rust side: zbus derives `state_changed` from the
        /// `State` property's change notification as well, and NetworkManager
        /// really does have both.
        #[zbus(signal, name = "StateChanged")]
        async fn nm_state_changed(emitter: &SignalEmitter<'_>, state: u32) -> zbus::Result<()>;
    }

    /// Serve a NetworkManager on `bus`, starting at `state`.
    pub(crate) async fn serve_nm(bus: &PrivateBus, state: u32) -> zbus::Connection {
        zbus::connection::Builder::address(bus.address())
            .expect("a well-formed private bus address")
            .name(NM_NAME)
            .expect("a well-formed bus name")
            .serve_at(NM_PATH, FakeNetworkManager { state })
            .expect("the object path is free")
            .build()
            .await
            .expect("the fake NetworkManager starts")
    }

    /// Change the state and tell the bus about it.
    pub(crate) async fn set_nm_state(connection: &zbus::Connection, state: u32) {
        let iface = connection
            .object_server()
            .interface::<_, FakeNetworkManager>(NM_PATH)
            .await
            .expect("the fake is still served");
        iface.get_mut().await.state = state;
        iface
            .nm_state_changed(state)
            .await
            .expect("the signal goes out");
    }

    /// Wait until the panel's idea of being online is `wanted`.
    async fn wait_for_online(connectivity: &Connectivity, wanted: bool, what: &str) {
        let mut state = connectivity.state();
        let wait = async {
            while state.borrow_and_update().online != wanted {
                state.changed().await.expect("the watcher is alive");
            }
        };
        tokio::time::timeout(PATIENCE, wait)
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
    }

    #[tokio::test]
    async fn networkmanagers_state_becomes_the_panels_idea_of_being_online() {
        let bus = private_bus!();
        let nm = serve_nm(&bus, 20).await;

        let connectivity = Connectivity::start(Some(bus.address().to_string()));

        // 20 is DISCONNECTED, and the optimistic default has to give way to it.
        wait_for_online(&connectivity, false, "the initial read").await;

        set_nm_state(&nm, 70).await;
        wait_for_online(&connectivity, true, "CONNECTED_GLOBAL").await;

        set_nm_state(&nm, 50).await;
        wait_for_online(&connectivity, false, "CONNECTED_LOCAL to read as unusable").await;

        // A captive-portal probe that never answers leaves NetworkManager at
        // CONNECTED_SITE, which the panel still treats as worth a request.
        set_nm_state(&nm, 60).await;
        wait_for_online(&connectivity, true, "CONNECTED_SITE").await;
    }

    #[tokio::test]
    async fn a_bus_with_no_networkmanager_on_it_leaves_the_panel_online() {
        let bus = private_bus!();
        let connectivity = Connectivity::start(Some(bus.address().to_string()));

        // Nothing is serving the name, so the initial read fails and the panel
        // is left believing it can reach the internet — which on a machine
        // running systemd-networkd it can.
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(connectivity.is_online());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_routable_machine_is_online() {
        // 70 CONNECTED_GLOBAL, 60 CONNECTED_SITE.
        assert!(online_from_state(70));
        assert!(online_from_state(60));
    }

    #[test]
    fn a_machine_that_is_not_connected_yet_is_offline() {
        // 10 ASLEEP, 20 DISCONNECTED, 30 DISCONNECTING, 40 CONNECTING,
        // 50 CONNECTED_LOCAL — a link with no route off it.
        for state in [10, 20, 30, 40, 50] {
            assert!(!online_from_state(state), "state {state} is not usable");
        }
    }

    #[test]
    fn an_unknown_state_is_treated_as_online() {
        assert!(online_from_state(STATE_UNKNOWN));
    }

    #[test]
    fn a_value_networkmanager_has_never_defined_is_treated_as_online() {
        // Above every documented state: a newer NetworkManager adding a
        // "more connected" value must not read as a disconnection.
        assert!(online_from_state(80));
        assert!(online_from_state(u32::MAX));
    }

    #[test]
    fn the_panel_starts_optimistic() {
        assert!(ConnectivityState::default().online);
    }
}
