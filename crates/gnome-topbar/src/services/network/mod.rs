use crate::services::callbacks::CallbackId;
use crate::services::vpn::VpnService;
use gtk4::glib;
use std::rc::Rc;

pub mod network_manager;
pub use network_manager::{NmService, NmSnapshot};

/// Generic failure reason for connection errors.
pub const CONNECTION_FAILURE_REASON: &str = "Connection failed";

/// Whether a Wi-Fi network requires authentication.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SecurityType {
    Open,
    Secured,
}

impl SecurityType {
    pub fn is_secured(self) -> bool {
        self == Self::Secured
    }
}

/// A Wi-Fi network visible in NetworkManager scan results.
#[derive(Debug, Clone)]
pub struct WifiNetwork {
    pub ssid: String,
    /// Signal strength percentage (0-100).
    pub strength: i32,
    pub security: SecurityType,
    /// Whether this is the currently connected network.
    pub active: bool,
    /// Whether this SSID has a saved connection profile.
    pub known: bool,
}

/// NetworkManager-backed network snapshot.
pub enum NetworkSnapshot {
    NetworkManager(NmSnapshot),
}

/// Connection state for Wi-Fi.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkConnectionState {
    Disconnected,
    Connecting,
    Connected,
}

impl NetworkSnapshot {
    /// SSID of the active/connecting network.
    pub fn active_ssid(&self) -> Option<&str> {
        match self {
            Self::NetworkManager(inner) => inner
                .wifi
                .connecting_ssid
                .as_deref()
                .or(inner.wifi.ssid.as_deref()),
        }
    }

    pub fn available(&self) -> bool {
        match self {
            Self::NetworkManager(inner) => inner.available,
        }
    }

    /// Whether Wi-Fi is connected (does not include wired or mobile).
    pub fn connected(&self) -> bool {
        match self {
            Self::NetworkManager(inner) => inner.wifi.connected,
        }
    }

    pub fn connecting_ssid(&self) -> Option<&str> {
        match self {
            Self::NetworkManager(inner) => inner.wifi.connecting_ssid.as_deref(),
        }
    }

    /// Whether the NetworkManager Wi-Fi device is connecting.
    pub fn wifi_device_connecting(&self) -> bool {
        match self {
            Self::NetworkManager(inner) => matches!(inner.wifi.device_state, Some(40..=90)),
        }
    }

    /// Whether Wi-Fi is in a connecting state that warrants a spinner.
    pub fn wifi_connecting(&self) -> bool {
        self.connecting_ssid().is_some() || self.wifi_device_connecting()
    }

    pub fn connection_state(&self) -> NetworkConnectionState {
        match self {
            Self::NetworkManager(inner) => {
                if inner.wifi.connecting_ssid.is_some()
                    || matches!(inner.wifi.device_state, Some(40..=90))
                {
                    NetworkConnectionState::Connecting
                } else if inner.wifi.connected {
                    NetworkConnectionState::Connected
                } else {
                    NetworkConnectionState::Disconnected
                }
            }
        }
    }

    /// Whether the system has an Ethernet (wired) network device.
    pub fn has_ethernet_device(&self) -> bool {
        match self {
            Self::NetworkManager(inner) => inner.wired.has_device,
        }
    }

    /// Whether the system has a modem (cellular) device.
    pub fn has_modem_device(&self) -> bool {
        match self {
            Self::NetworkManager(inner) => inner.mobile.has_device,
        }
    }

    /// Whether the system has any non-WiFi network device (Ethernet or cellular).
    pub fn has_non_wifi_device(&self) -> bool {
        self.has_ethernet_device() || self.has_modem_device()
    }

    /// Whether the system has Wi-Fi hardware.
    pub fn has_wifi_device(&self) -> bool {
        match self {
            Self::NetworkManager(inner) => inner.wifi.has_device,
        }
    }

    pub fn networks(&self) -> &[WifiNetwork] {
        match self {
            Self::NetworkManager(inner) => &inner.wifi.networks,
        }
    }

    pub fn scanning(&self) -> bool {
        match self {
            Self::NetworkManager(inner) => inner.wifi.scanning,
        }
    }

    pub fn wifi_enabled(&self) -> Option<bool> {
        match self {
            Self::NetworkManager(inner) => inner.wifi.enabled,
        }
    }

    pub fn wired_connected(&self) -> bool {
        match self {
            Self::NetworkManager(inner) => inner.wired.connected,
        }
    }

    /// Whether mobile data is the primary connection.
    pub fn mobile_is_primary(&self) -> bool {
        match self {
            Self::NetworkManager(inner) => inner.mobile.is_primary,
        }
    }

    /// Whether a GSM/CDMA connection is activated.
    pub fn mobile_active(&self) -> bool {
        match self {
            Self::NetworkManager(inner) => inner.mobile.active,
        }
    }

    /// Whether a GSM/CDMA connection is currently activating.
    pub fn mobile_connecting(&self) -> bool {
        match self {
            Self::NetworkManager(inner) => inner.mobile.connecting,
        }
    }

    /// Whether mobile networking is supported (modem + SIM + profile all present).
    pub fn mobile_supported(&self) -> bool {
        match self {
            Self::NetworkManager(inner) => inner.mobile.supported,
        }
    }

    /// Whether WWAN (mobile broadband) is enabled in NetworkManager, if known.
    pub fn mobile_enabled(&self) -> Option<bool> {
        match self {
            Self::NetworkManager(inner) => inner.mobile.enabled,
        }
    }

    pub fn wired_iface(&self) -> Option<&str> {
        match self {
            Self::NetworkManager(inner) => inner.wired.iface.as_deref(),
        }
    }

    pub fn wired_name(&self) -> Option<&str> {
        match self {
            Self::NetworkManager(inner) => inner.wired.name.as_deref(),
        }
    }

    pub fn wired_speed(&self) -> Option<u32> {
        match self {
            Self::NetworkManager(inner) => inner.wired.speed,
        }
    }

    /// Connection profile name for the active mobile connection.
    pub fn mobile_name(&self) -> Option<&str> {
        match self {
            Self::NetworkManager(inner) => inner.mobile.name.as_deref(),
        }
    }

    /// Mobile carrier / operator name reported by the 3GPP modem.
    pub fn mobile_operator(&self) -> Option<&str> {
        match self {
            Self::NetworkManager(inner) => inner.mobile.operator.as_deref(),
        }
    }

    /// Best available display name for the mobile connection.
    pub fn mobile_display_name(&self) -> &str {
        self.mobile_operator()
            .or_else(|| self.mobile_name())
            .unwrap_or("Mobile")
    }

    /// Radio access technology label (e.g., "LTE", "5G NR", "HSPA+").
    pub fn mobile_access_technology(&self) -> Option<&str> {
        match self {
            Self::NetworkManager(inner) => inner.mobile.access_technology.as_deref(),
        }
    }

    /// Signal quality percentage (0-100) from ModemManager.
    pub fn mobile_signal_quality(&self) -> Option<u32> {
        match self {
            Self::NetworkManager(inner) => inner.mobile.signal_quality,
        }
    }

    /// Whether the last mobile connection attempt failed.
    pub fn mobile_failed(&self) -> bool {
        match self {
            Self::NetworkManager(inner) => inner.mobile.failed,
        }
    }

    /// Get the SSID of the network that failed to connect, if any.
    pub fn failed_ssid(&self) -> Option<&str> {
        match self {
            Self::NetworkManager(inner) => inner.wifi.failed_ssid.as_deref(),
        }
    }

    /// Signal strength of the active network, or 0 if not connected.
    pub fn active_strength(&self) -> i32 {
        match self {
            Self::NetworkManager(inner) => {
                if inner.wifi.connected {
                    inner.wifi.strength
                } else {
                    0
                }
            }
        }
    }
}

/// NetworkManager-backed network service.
pub struct NetworkService {
    backend: Rc<NmService>,
}

impl NetworkService {
    fn new() -> Rc<Self> {
        let backend = NmService::global();
        let service = Rc::new(Self {
            backend: backend.clone(),
        });

        VpnService::global().connect(move |_| {
            backend.re_notify();
        });

        service
    }

    /// Get the global network service singleton.
    pub fn global() -> Rc<Self> {
        thread_local! {
            static INSTANCE: Rc<NetworkService> = NetworkService::new();
        }

        INSTANCE.with(|s| s.clone())
    }

    pub fn connect<F>(&self, callback: F) -> CallbackId
    where
        F: Fn(&NetworkSnapshot) + 'static,
    {
        self.backend.connect(move |snap| {
            let wrapped = NetworkSnapshot::NetworkManager(snap.clone());
            callback(&wrapped);
        })
    }

    pub fn unsubscribe(&self, id: CallbackId) {
        self.backend.unsubscribe(id);
    }

    /// Re-emit the current snapshot to all callbacks without any state change.
    pub fn re_notify(&self) {
        self.backend.re_notify();
    }

    pub fn connect_to_network(&self, ssid: &str, password: Option<&str>) {
        self.backend.connect_to_network(ssid, password);
    }

    pub fn disconnect(&self) {
        self.backend.disconnect();
    }

    pub fn forget(&self, ssid: &str) {
        self.backend.forget_network(ssid);
    }

    pub fn scan(&self) {
        self.backend.scan_networks();
    }

    pub fn set_wifi_enabled(&self, enabled: bool) {
        self.backend.set_wifi_enabled(enabled);
    }

    pub fn set_mobile_enabled(&self, enabled: bool) {
        self.backend.set_mobile_enabled(enabled);
    }

    pub fn connect_mobile(&self) {
        self.backend.connect_mobile();
    }

    pub fn disconnect_mobile(&self) {
        self.backend.disconnect_mobile();
    }

    pub fn snapshot(&self) -> NetworkSnapshot {
        NetworkSnapshot::NetworkManager(self.backend.snapshot())
    }

    /// Whether internet-backed widgets should run now.
    pub fn internet_available(&self) -> bool {
        self.backend.internet_available() && !VpnService::global().snapshot().connection_pending()
    }

    /// Clear the failed state (called when user cancels password dialog).
    pub fn clear_failed_state(&self) {
        self.backend.clear_failed_state();
    }

    /// Clear the mobile failed connection state (called by UI after showing error).
    pub fn clear_mobile_failed_state(&self) {
        self.backend.clear_mobile_failed_state();
    }
}

/// Extract a D-Bus object path (`type o`) from a [`glib::Variant`] as a `String`.
pub(super) fn objpath_to_string(v: &glib::Variant) -> Option<String> {
    v.get::<glib::variant::ObjectPath>()
        .map(|p| p.as_str().to_string())
}
