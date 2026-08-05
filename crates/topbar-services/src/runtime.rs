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
use topbar_core::Config;

use crate::audio::Audio;
use crate::battery::Battery;
use crate::bluetooth::Bluetooth;
use crate::brightness::Brightness;
use crate::connectivity::Connectivity;
use crate::crypto::Crypto;
use crate::inhibitor::Inhibitor;
use crate::ipc::Ipc;
use crate::media::Media;
use crate::network::Network;
use crate::niri::Niri;
use crate::notifications::Notifications;
use crate::power::Power;
use crate::power_profiles::PowerProfiles;
use crate::resources::Resources;
use crate::state_store::StateStore;
use crate::tray::{DEFAULT_ICON_SIZE, Tray};
use crate::updates::Updates;
use crate::weather::Weather;

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
    /// The network: Wi-Fi, Ethernet, VPN and the secret agent.
    pub network: Network,
    /// Bluetooth: the adapter, its paired devices, and the pairing agent.
    pub bluetooth: Bluetooth,
    /// Whether the machine is online. A projection of [`Services::network`].
    pub connectivity: Connectivity,
    /// The weather, as one cache for the whole panel.
    pub weather: Weather,
    /// How many updates are pending, on whatever distribution this is.
    pub updates: Updates,
    /// Crypto prices, as one cache for the whole panel.
    pub crypto: Crypto,
    /// The system tray.
    pub tray: Tray,
    /// The sound server.
    pub audio: Audio,
    /// The screen backlight.
    pub brightness: Brightness,
    /// The idle inhibitor.
    pub inhibitor: Inhibitor,
    /// The battery, and its charge limit.
    pub battery: Battery,
    /// CPU, memory and disks. Shared with M10's system_monitor widget.
    pub resources: Resources,
    /// The power-profiles daemon, when there is one.
    pub power_profiles: PowerProfiles,
    /// Shutting down, restarting and suspending.
    pub power: Power,
    /// The socket `topbar` commands arrive on.
    pub ipc: Ipc,
}

/// Points the battery and power-profiles clients at a bus of a test's own.
///
/// Debug builds only, and read once at start-up. The nested-niri smoke run
/// puts a stand-in UPower and power-profiles daemon on its private session
/// bus, because there is no way to fake the *system* bus inside the sandbox
/// and pointing the panel at the real one would mean a screenshot taken by
/// changing the developer's machine. logind is deliberately not covered: the
/// idle inhibitor keeps talking to the real one, as it has since M8.
const SMOKE_BUS: &str = "TOPBAR_SMOKE_POWER_BUS";
/// The same, for `/sys/class/power_supply`.
const SMOKE_SYSFS: &str = "TOPBAR_SMOKE_POWER_SYSFS";
/// The same again, for NetworkManager.
///
/// The real one is the machine's live network, and this is the switch that
/// makes the network service willing to change anything at all — see
/// [`crate::network::Access`]. A debug build with this unset reads and does
/// nothing else.
const SMOKE_NM_BUS: &str = "TOPBAR_SMOKE_NM_BUS";
/// And again, for BlueZ.
///
/// The real one is the machine's live adapter, and this is the switch that
/// makes the Bluetooth service willing to change anything at all — including
/// whether it registers a pairing agent, which on a real session would take
/// the prompts the user's own desktop is waiting for. A debug build with this
/// unset reads and does nothing else.
const SMOKE_BLUEZ_BUS: &str = "TOPBAR_SMOKE_BLUEZ_BUS";

/// A smoke override, in debug builds only.
fn smoke(variable: &str) -> Option<String> {
    if !cfg!(debug_assertions) {
        return None;
    }
    std::env::var(variable)
        .ok()
        .filter(|value| !value.is_empty())
}

impl Services {
    /// Start every service. Call once, from `main`, before GTK.
    ///
    /// Blocking here is deliberate and momentary: services are spawned onto
    /// the runtime, not awaited, so this returns as soon as their tasks exist.
    pub fn start(config: &Config) -> Self {
        let niri_socket = std::env::var_os(SOCKET_PATH_ENV).map(PathBuf::from);
        let weather = config.widgets.weather.clone();
        let crypto = config.widgets.crypto.clone();
        // The tray picks its pixmaps for the size the widget will draw them
        // at, so the icon that arrives is the icon that is shown.
        let icon_size = config
            .widgets
            .tray
            .pixmap_icon_size
            .map_or(DEFAULT_ICON_SIZE, |size| size as i32);
        let allow_overdrive = config.audio.allow_overdrive;
        let updates = config.updates.clone();
        let power_bus = smoke(SMOKE_BUS);
        let power_sysfs = smoke(SMOKE_SYSFS).map(PathBuf::from);
        let nm_bus = smoke(SMOKE_NM_BUS);
        let bluez_bus = smoke(SMOKE_BLUEZ_BUS);
        Runtime::handle().block_on(async move {
            // The state file is read once, here, so every service that
            // restores something starts from one consistent document.
            let (state, store) = StateStore::open();
            // The network first, and connectivity out of it: the weather and
            // crypto services subscribe to connectivity, and a subscriber built
            // before the service exists would have nothing to read on its first
            // frame.
            let network = Network::start(nm_bus, state.network, Some(store.clone()));
            let connectivity = Connectivity::from_network(&network);
            Self {
                niri: Niri::start(niri_socket),
                notifications: Notifications::start(state.notifications, store.clone(), None),
                media: Media::start(None),
                weather: Weather::start(&weather, state.weather, store.clone(), &connectivity),
                updates: Updates::start(&updates, &connectivity),
                crypto: Crypto::start(&crypto, state.crypto, store, &connectivity),
                tray: Tray::start(icon_size, None),
                audio: Audio::start(allow_overdrive),
                brightness: Brightness::start(None),
                inhibitor: Inhibitor::start(None),
                battery: Battery::start(power_bus.clone(), power_sysfs),
                power_profiles: PowerProfiles::start(power_bus),
                bluetooth: Bluetooth::start(bluez_bus),
                resources: Resources::start(),
                power: Power::new(None),
                ipc: Ipc::start(),
                network,
                connectivity,
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
