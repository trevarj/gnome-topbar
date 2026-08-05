//! Exactly the NetworkManager surface the panel touches, and nothing else.
//!
//! NetworkManager's D-Bus API is enormous. Each trait here is trimmed to the
//! members one part of the panel actually uses, which is what keeps the client
//! auditable: a reader can see at a glance that the panel never writes a
//! routing table, never reads a DHCP lease, and never asks for a secret it did
//! not put a prompt on screen for.
//!
//! The type aliases at the top are the two shapes NetworkManager's own
//! documentation calls "a connection": a nested dictionary of settings groups.
//! They are spelled out once so the four methods that pass one agree.
//!
//! **No signals are declared here.** Every one NetworkManager emits arrives
//! through a single bus match rule in [`super::task`] instead: several of these
//! interfaces have a `StateChanged` of their own, and following them
//! object-by-object would mean one subscription per access point.

use std::collections::HashMap;

use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

/// One settings group — `802-11-wireless`, say — as it travels on the bus.
pub(crate) type Setting = HashMap<String, OwnedValue>;

/// A whole connection: settings groups by name.
///
/// `a{sa{sv}}` in NetworkManager's signatures.
pub(crate) type Connection = HashMap<String, Setting>;

/// The same, borrowed, for a dictionary the panel is building to send.
pub(crate) type ConnectionRef<'a> = HashMap<&'a str, HashMap<&'a str, Value<'a>>>;

/// The manager object: devices, global state, and the two ways to connect.
#[zbus::proxy(
    interface = "org.freedesktop.NetworkManager",
    default_service = "org.freedesktop.NetworkManager",
    default_path = "/org/freedesktop/NetworkManager"
)]
pub(crate) trait NetworkManager {
    /// Every device NetworkManager manages, real ones included.
    fn get_devices(&self) -> zbus::Result<Vec<OwnedObjectPath>>;

    /// Bring up a connection that already exists in Settings.
    ///
    /// `specific_object` is the access point for Wi-Fi and `/` for everything
    /// else, which is how NetworkManager is told *which* of two networks
    /// sharing an SSID the user meant.
    fn activate_connection(
        &self,
        connection: &zbus::zvariant::ObjectPath<'_>,
        device: &zbus::zvariant::ObjectPath<'_>,
        specific_object: &zbus::zvariant::ObjectPath<'_>,
    ) -> zbus::Result<OwnedObjectPath>;

    /// Save a new connection and bring it up in one call.
    ///
    /// Returns the settings object and the active connection, in that order.
    /// The settings path is what lets a failed first attempt clean up after
    /// itself instead of leaving a profile behind.
    fn add_and_activate_connection(
        &self,
        connection: ConnectionRef<'_>,
        device: &zbus::zvariant::ObjectPath<'_>,
        specific_object: &zbus::zvariant::ObjectPath<'_>,
    ) -> zbus::Result<(OwnedObjectPath, OwnedObjectPath)>;

    /// Take one active connection down.
    fn deactivate_connection(
        &self,
        active_connection: &zbus::zvariant::ObjectPath<'_>,
    ) -> zbus::Result<()>;

    /// `NM_STATE_*` for the machine as a whole.
    ///
    /// Renamed on the Rust side because zbus would derive `state` from it, and
    /// three of the interfaces here have a property or a signal of that name.
    #[zbus(property, name = "State")]
    fn nm_state(&self) -> zbus::Result<u32>;

    /// Whether the Wi-Fi radio is switched on. Writable: this is the toggle.
    #[zbus(property)]
    fn wireless_enabled(&self) -> zbus::Result<bool>;

    /// Switch the radio on or off.
    #[zbus(property)]
    fn set_wireless_enabled(&self, enabled: bool) -> zbus::Result<()>;

    /// Every connection that is up or coming up.
    #[zbus(property)]
    fn active_connections(&self) -> zbus::Result<Vec<OwnedObjectPath>>;
}

/// What every device has, whatever kind it is.
#[zbus::proxy(
    interface = "org.freedesktop.NetworkManager.Device",
    default_service = "org.freedesktop.NetworkManager"
)]
pub(crate) trait Device {
    /// Take this device's connection down, whatever it is.
    fn disconnect(&self) -> zbus::Result<()>;

    /// `NM_DEVICE_TYPE_*`.
    #[zbus(property)]
    fn device_type(&self) -> zbus::Result<u32>;

    /// The kernel's name for it: `wlp0s20f3`, `enp0s31f6`.
    #[zbus(property)]
    fn interface(&self) -> zbus::Result<String>;

    /// `NM_DEVICE_STATE_*`.
    #[zbus(property)]
    fn state(&self) -> zbus::Result<u32>;

    /// The connection running on it, or `/`.
    #[zbus(property)]
    fn active_connection(&self) -> zbus::Result<OwnedObjectPath>;
}

/// The Wi-Fi half of a wireless device.
#[zbus::proxy(
    interface = "org.freedesktop.NetworkManager.Device.Wireless",
    default_service = "org.freedesktop.NetworkManager"
)]
pub(crate) trait DeviceWireless {
    /// Ask the card to look around. `options` is empty for a plain scan.
    ///
    /// A *mutation* as far as the panel's safety rules are concerned: it makes
    /// the radio transmit, and it is refused against a bus the panel does not
    /// own. See [`super::Access`].
    fn request_scan(&self, options: HashMap<&str, Value<'_>>) -> zbus::Result<()>;

    /// Every access point the card can currently hear.
    #[zbus(property)]
    fn access_points(&self) -> zbus::Result<Vec<OwnedObjectPath>>;

    /// The one it is associated with, or `/`.
    #[zbus(property)]
    fn active_access_point(&self) -> zbus::Result<OwnedObjectPath>;
}

/// The Ethernet half of a wired device.
#[zbus::proxy(
    interface = "org.freedesktop.NetworkManager.Device.Wired",
    default_service = "org.freedesktop.NetworkManager"
)]
pub(crate) trait DeviceWired {
    /// Whether there is a cable in the socket.
    #[zbus(property)]
    fn carrier(&self) -> zbus::Result<bool>;

    /// Negotiated link speed in Mb/s, or 0 when the driver will not say.
    #[zbus(property)]
    fn speed(&self) -> zbus::Result<u32>;
}

/// One access point the card can hear.
#[zbus::proxy(
    interface = "org.freedesktop.NetworkManager.AccessPoint",
    default_service = "org.freedesktop.NetworkManager"
)]
pub(crate) trait AccessPoint {
    /// The SSID, as bytes. Not a string: 802.11 does not say it is one.
    #[zbus(property)]
    fn ssid(&self) -> zbus::Result<Vec<u8>>;

    /// Signal strength, 0–100.
    #[zbus(property)]
    fn strength(&self) -> zbus::Result<u8>;

    /// `NM_802_11_AP_FLAGS_*` — privacy and WPS.
    #[zbus(property)]
    fn flags(&self) -> zbus::Result<u32>;

    /// `NM_802_11_AP_SEC_*` for WPA.
    #[zbus(property)]
    fn wpa_flags(&self) -> zbus::Result<u32>;

    /// The same for RSN — WPA2 and WPA3.
    #[zbus(property)]
    fn rsn_flags(&self) -> zbus::Result<u32>;
}

/// The store of saved connection profiles.
#[zbus::proxy(
    interface = "org.freedesktop.NetworkManager.Settings",
    default_service = "org.freedesktop.NetworkManager",
    default_path = "/org/freedesktop/NetworkManager/Settings"
)]
pub(crate) trait Settings {
    /// Every profile on the machine.
    fn list_connections(&self) -> zbus::Result<Vec<OwnedObjectPath>>;
}

/// One saved profile.
#[zbus::proxy(
    interface = "org.freedesktop.NetworkManager.Settings.Connection",
    default_service = "org.freedesktop.NetworkManager"
)]
pub(crate) trait SettingsConnection {
    /// The profile, without its secrets.
    fn get_settings(&self) -> zbus::Result<Connection>;

    /// Delete it.
    ///
    /// Used for exactly one thing: removing the profile the panel itself added
    /// moments earlier when the password turned out to be wrong. v1 left those
    /// behind, and a Wi-Fi list slowly filling with dead duplicates of the same
    /// network was the visible symptom.
    fn delete(&self) -> zbus::Result<()>;
}

/// One connection that is up, or on its way there.
#[zbus::proxy(
    interface = "org.freedesktop.NetworkManager.Connection.Active",
    default_service = "org.freedesktop.NetworkManager"
)]
pub(crate) trait ActiveConnection {
    /// The profile's name, as the user named it.
    #[zbus(property)]
    fn id(&self) -> zbus::Result<String>;

    /// The connection type: `802-11-wireless`, `802-3-ethernet`, `vpn`, …
    #[zbus(property, name = "Type")]
    fn connection_type(&self) -> zbus::Result<String>;

    /// The profile's stable identifier.
    #[zbus(property)]
    fn uuid(&self) -> zbus::Result<String>;

    /// `NM_ACTIVE_CONNECTION_STATE_*`.
    #[zbus(property)]
    fn state(&self) -> zbus::Result<u32>;

    /// The settings object this was activated from.
    #[zbus(property)]
    fn connection(&self) -> zbus::Result<OwnedObjectPath>;

    /// Every device carrying it.
    #[zbus(property)]
    fn devices(&self) -> zbus::Result<Vec<OwnedObjectPath>>;
}

/// Where a secret agent signs up.
#[zbus::proxy(
    interface = "org.freedesktop.NetworkManager.AgentManager",
    default_service = "org.freedesktop.NetworkManager",
    default_path = "/org/freedesktop/NetworkManager/AgentManager"
)]
pub(crate) trait AgentManager {
    /// Register this connection's secret agent under `identifier`.
    fn register(&self, identifier: &str) -> zbus::Result<()>;

    /// The same, declaring what the agent can do.
    fn register_with_capabilities(&self, identifier: &str, capabilities: u32) -> zbus::Result<()>;

    /// Stand down.
    fn unregister(&self) -> zbus::Result<()>;
}
