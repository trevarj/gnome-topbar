//! Is this machine on the internet?
//!
//! The weather and crypto services — and, from M10, every `custom-*` widget
//! marked `requires_network` — have to know, because polling an HTTP endpoint
//! on a laptop in a tunnel is a request that will fail slowly and then be
//! retried.
//!
//! **This is no longer a service.** Until M9b it was: a NetworkManager
//! connection of its own that read one property. M9b's [network
//! service](crate::network) reads that property among many others, so this is
//! now a projection of that one snapshot down to the single boolean its
//! consumers care about — one NetworkManager client on the bus, not two. The
//! handle keeps its shape, so nothing that subscribes to it changed.
//!
//! The failure mode is chosen rather than inherited: a machine with no
//! NetworkManager on its system bus is assumed to be **online**. Guessing
//! "offline" there would leave the weather widget permanently blank on every
//! box that runs systemd-networkd, connman, or nothing at all, and a failed
//! fetch costs one timeout while a wrong guess costs the whole feature.

use std::sync::Arc;

use tokio::sync::watch;
use tracing::{debug, info};

use crate::network::{Network, NetworkState};

/// What the panel knows about reaching the internet.
///
/// One field on purpose. Anything richer is the network service's, and a
/// snapshot that grows speculative fields is a snapshot every consumer has to
/// re-read.
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

/// Whether the machine is online, as a subscription.
///
/// Cloning is cheap: it is one watch subscription.
#[derive(Clone)]
pub struct Connectivity {
    state: watch::Receiver<Arc<ConnectivityState>>,
    /// The service being followed, when nothing else is holding it.
    ///
    /// Production's lives in [`crate::Services`]; a test builds one of its own
    /// and would otherwise drop it on the floor the moment this is constructed.
    #[cfg(test)]
    #[expect(dead_code, reason = "held so the service it follows stays alive")]
    network: Option<Network>,
}

impl Connectivity {
    /// Follow the network service's idea of being online.
    pub(crate) fn from_network(network: &Network) -> Self {
        let mut source = network.state();
        let initial = ConnectivityState {
            online: source.borrow_and_update().online,
        };
        let (publisher, state) = watch::channel(Arc::new(initial));

        tokio::spawn(async move {
            while source.changed().await.is_ok() {
                let online = source.borrow_and_update().online;
                publish(&publisher, online);
            }
            // The network service stopped. Whatever replaced it — or nothing at
            // all — the panel must not be left believing the machine is offline
            // for good.
            info!("the network service stopped; assuming the machine is online");
            let _ = publisher.send(Arc::new(ConnectivityState::default()));
        });

        Self {
            state,
            #[cfg(test)]
            network: None,
        }
    }

    /// Start a network service and follow it. Tests only.
    ///
    /// The weather and crypto bus tests want a `Connectivity` pointed at a
    /// NetworkManager of their own, and this keeps them talking to the very
    /// same code path production uses rather than to a stub of it.
    #[cfg(test)]
    pub(crate) fn start(address: Option<String>) -> Self {
        let network = Network::start(address, crate::network::PersistedNetwork::default(), None);
        Self {
            network: Some(network.clone()),
            ..Self::from_network(&network)
        }
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

/// Publish a state, logging only the transitions.
fn publish(publisher: &watch::Sender<Arc<ConnectivityState>>, online: bool) {
    if publisher.borrow().online == online {
        debug!("still {}", label(online));
        return;
    }
    info!("the machine is {}", label(online));
    let _ = publisher.send(Arc::new(ConnectivityState { online }));
}

/// The word for a log line.
fn label(online: bool) -> &'static str {
    if online { "online" } else { "offline" }
}

/// Whether an `NM_STATE_*` value means "a request is worth making".
///
/// Re-exported from the network service's model, where the rule lives with the
/// rest of NetworkManager's constants.
pub use crate::network::online_from_state;

/// Where the state comes from, for a consumer that wants the whole picture.
impl From<&NetworkState> for ConnectivityState {
    fn from(state: &NetworkState) -> Self {
        Self {
            online: state.online,
        }
    }
}

/// A NetworkManager that only has a `State`, served on a bus of the test's own.
///
/// The weather service's own bus tests reach for it, which is why it is a
/// module rather than a few functions in one test file. It is deliberately
/// *minimal* — no devices, no settings store, no access points — because that
/// is also a test: the full network service has to cope with a NetworkManager
/// that answers one property and nothing else.
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
    fn the_panel_starts_optimistic() {
        assert!(ConnectivityState::default().online);
    }

    #[test]
    fn the_projection_carries_exactly_the_networks_answer() {
        let mut state = NetworkState::default();
        assert!(ConnectivityState::from(&state).online);
        state.online = false;
        assert!(!ConnectivityState::from(&state).online);
    }
}
