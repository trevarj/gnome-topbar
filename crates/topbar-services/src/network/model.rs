//! What the panel knows about the network, and every decision that can be made
//! without a bus.
//!
//! Reading an SSID, deciding whether an access point is secured, choosing which
//! of five bars to draw, collapsing the four radios one router advertises into
//! one row, ordering that list, and running the connect attempt's state
//! machine. All of it is a pure function or a pure type here, because all of it
//! is a thing the eye notices when it is wrong and none of it needs
//! NetworkManager to be checked.

use std::collections::HashMap;

/// `NM_STATE_UNKNOWN` — NetworkManager is not sure, so neither are we.
pub(crate) const STATE_UNKNOWN: u32 = 0;
/// `NM_STATE_CONNECTED_SITE`: routable, but NetworkManager's own connectivity
/// check did not reach its test endpoint.
pub(crate) const STATE_CONNECTED_SITE: u32 = 60;

/// `NM_DEVICE_TYPE_ETHERNET`.
pub(crate) const DEVICE_ETHERNET: u32 = 1;
/// `NM_DEVICE_TYPE_WIFI`.
pub(crate) const DEVICE_WIFI: u32 = 2;

/// `NM_DEVICE_STATE_ACTIVATED`.
pub(crate) const DEVICE_ACTIVATED: u32 = 100;

/// `NM_ACTIVE_CONNECTION_STATE_ACTIVATING`.
pub(crate) const ACTIVE_ACTIVATING: u32 = 1;
/// `NM_ACTIVE_CONNECTION_STATE_ACTIVATED`.
pub(crate) const ACTIVE_ACTIVATED: u32 = 2;
/// `NM_ACTIVE_CONNECTION_STATE_DEACTIVATED`.
pub(crate) const ACTIVE_DEACTIVATED: u32 = 4;

/// `NM_ACTIVE_CONNECTION_STATE_REASON_NO_SECRETS`.
pub(crate) const REASON_NO_SECRETS: u32 = 9;
/// `NM_ACTIVE_CONNECTION_STATE_REASON_LOGIN_FAILED`.
pub(crate) const REASON_LOGIN_FAILED: u32 = 10;

/// `NM_802_11_AP_FLAGS_PRIVACY` — the AP asks for a key.
///
/// The bit matters: `Flags` also carries the WPS bits, and v1's "any non-zero
/// flag means secured" test therefore drew a padlock on every open network
/// whose router advertised push-button setup.
pub(crate) const AP_FLAG_PRIVACY: u32 = 0x1;

/// `NM_SECRET_AGENT_GET_SECRETS_FLAG_ALLOW_INTERACTION`.
pub(crate) const SECRET_ALLOW_INTERACTION: u32 = 0x1;
/// `NM_SECRET_AGENT_GET_SECRETS_FLAG_REQUEST_NEW` — "the last one was wrong".
pub(crate) const SECRET_REQUEST_NEW: u32 = 0x2;

/// The settings group carrying a Wi-Fi password.
pub(crate) const WIFI_SECURITY_SETTING: &str = "802-11-wireless-security";
/// The settings group carrying an SSID.
pub(crate) const WIFI_SETTING: &str = "802-11-wireless";
/// The settings group every profile has.
pub(crate) const CONNECTION_SETTING: &str = "connection";
/// The key inside [`WIFI_SECURITY_SETTING`] that holds a pre-shared key.
pub(crate) const PSK_KEY: &str = "psk";

/// Connection types that are a VPN as far as the panel is concerned.
///
/// `vpn` is NetworkManager's own word for "a plugin does the work" — OpenVPN,
/// OpenConnect, l2tp and the rest all report it. `wireguard` is in-kernel and
/// reports itself.
pub(crate) const VPN_TYPES: &[&str] = &["vpn", "wireguard"];

/// Connection types that are a tunnel someone else brought up.
///
/// A `tun` active connection with no profile behind it is what an `openvpn`
/// run from a terminal, or a corporate client of its own, looks like on the
/// bus. v1 found these by reading `/sys/class/net` every five seconds; they are
/// right here, for free, in a list the panel already follows.
pub(crate) const TUNNEL_TYPES: &[&str] = &["tun", "wireguard", "vpn"];

/// Whether an `NM_STATE_*` value means "a request is worth making".
///
/// The threshold is `CONNECTED_SITE` rather than `CONNECTED_GLOBAL`: a box
/// whose connectivity check is switched off, or whose captive-portal probe is
/// blocked, sits at 60 for ever, and refusing to fetch there would leave the
/// weather widget permanently blank. `UNKNOWN` — and anything above the
/// documented range, which a newer NetworkManager may invent — is read the same
/// optimistic way.
pub fn online_from_state(state: u32) -> bool {
    state == STATE_UNKNOWN || state >= STATE_CONNECTED_SITE
}

/// An SSID, as something that can be put on a label.
///
/// 802.11 says an SSID is 32 bytes, not a string, and routers do ship names
/// that are not valid UTF-8. Lossy decoding keeps the readable part rather than
/// dropping the whole network the way v1's strict decode did — a name with one
/// bad byte in it is still the name the user recognises.
///
/// `None` means there is nothing to show: a zero-length SSID is a hidden
/// network, and a row labelled "&lt;unknown&gt;" that cannot be joined by name is a
/// row that does nothing.
pub fn ssid_text(bytes: &[u8]) -> Option<String> {
    let trimmed: &[u8] = match bytes.iter().position(|byte| *byte != 0) {
        Some(_) => bytes,
        None => &[],
    };
    if trimmed.is_empty() {
        return None;
    }
    let text = String::from_utf8_lossy(trimmed).trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// Which of the five signal icons a strength belongs in.
///
/// GNOME Shell's own thresholds — 20/40/60/80 — rather than v1's, which put
/// 70–79 in the top bucket and so drew five bars on a link GNOME draws four
/// for. Matching the desktop the panel is modelled on matters more here than
/// matching the panel it replaces.
pub fn strength_bucket(strength: u8) -> u8 {
    match strength {
        0..=19 => 0,
        20..=39 => 1,
        40..=59 => 2,
        60..=79 => 3,
        _ => 4,
    }
}

/// Whether an access point asks for a key.
///
/// Any RSN or WPA flag at all means yes; on the plain `Flags` word only the
/// privacy bit counts, because the others describe WPS, which an *open*
/// network may perfectly well advertise.
pub fn is_secured(flags: u32, wpa_flags: u32, rsn_flags: u32) -> bool {
    flags & AP_FLAG_PRIVACY != 0 || wpa_flags != 0 || rsn_flags != 0
}

/// One access point, or rather one network: several radios collapsed into one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApView {
    /// The name to draw.
    pub ssid: String,
    /// Signal strength, 0–100.
    pub strength: u8,
    /// Which of the five icons to draw, from [`strength_bucket`].
    pub bucket: u8,
    /// Whether joining it needs a key.
    pub secured: bool,
    /// Whether there is a saved profile for it.
    pub known: bool,
    /// Whether this is the one in use.
    pub active: bool,
    /// Whether the panel is in the middle of joining it.
    pub connecting: bool,
}

impl ApView {
    /// Build one from an access point's properties.
    pub(crate) fn new(ssid: String, strength: u8, secured: bool) -> Self {
        Self {
            ssid,
            strength,
            bucket: strength_bucket(strength),
            secured,
            known: false,
            active: false,
            connecting: false,
        }
    }
}

/// Collapse duplicate radios and put the list in the order it is read in.
///
/// The key is the SSID **and** whether it is secured, which is v1's rule and
/// the right one: a network broadcasting an open guest SSID and a secured one
/// under the same name is two networks, and merging them would offer a padlock
/// on a row that joins without a password. Everything else about a duplicate is
/// merged rather than dropped — the strongest radio's signal, and "active" or
/// "known" if any of them was.
///
/// The order is the one GNOME uses: what you are on, then what you have joined
/// before, then everything else, each group strongest first, ties broken by
/// name so the list does not shuffle between two equal readings.
pub fn collapse(aps: Vec<ApView>) -> Vec<ApView> {
    let mut merged: HashMap<(String, bool), ApView> = HashMap::new();
    for ap in aps {
        match merged.entry((ap.ssid.clone(), ap.secured)) {
            std::collections::hash_map::Entry::Occupied(mut slot) => {
                let kept = slot.get_mut();
                kept.strength = kept.strength.max(ap.strength);
                kept.bucket = strength_bucket(kept.strength);
                kept.active |= ap.active;
                kept.known |= ap.known;
                kept.connecting |= ap.connecting;
            }
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(ap);
            }
        }
    }

    let mut list: Vec<ApView> = merged.into_values().collect();
    list.sort_by(|a, b| {
        group(a)
            .cmp(&group(b))
            .then(b.strength.cmp(&a.strength))
            .then(a.ssid.cmp(&b.ssid))
    });
    list
}

/// Which band of the list a network belongs in.
fn group(ap: &ApView) -> u8 {
    if ap.active {
        0
    } else if ap.known {
        1
    } else {
        2
    }
}

/// The Wi-Fi half of the snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WifiState {
    /// Whether this machine has a wireless card at all.
    pub present: bool,
    /// Whether the radio is switched on.
    pub enabled: bool,
    /// Whether a scan is in flight.
    pub scanning: bool,
    /// The network in use, if any.
    pub active: Option<ApView>,
    /// Everything in range, collapsed and ordered.
    pub list: Vec<ApView>,
}

/// The wired half.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WiredState {
    /// Whether this machine has an Ethernet port.
    pub present: bool,
    /// Whether there is a cable in it with a link on the other end.
    pub carrier: bool,
    /// Whether a connection is actually up on it.
    ///
    /// Deliberately the device's own state rather than v1's "is Ethernet the
    /// primary route", which read as disconnected the moment a VPN or Wi-Fi
    /// took the default route.
    pub connected: bool,
    /// The profile's name, when one is up.
    pub id: Option<String>,
    /// Negotiated link speed in Mb/s, or 0 when the driver will not say.
    pub speed_mbps: u32,
}

impl WiredState {
    /// The link speed as a phrase, or nothing when the driver is silent.
    pub fn speed_label(&self) -> Option<String> {
        match self.speed_mbps {
            0 => None,
            speed if speed % 1000 == 0 => Some(format!("{} Gb/s", speed / 1000)),
            speed if speed > 1000 => Some(format!("{:.1} Gb/s", f64::from(speed) / 1000.0)),
            speed => Some(format!("{speed} Mb/s")),
        }
    }
}

/// What kind of tunnel a VPN profile is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VpnKind {
    /// In-kernel WireGuard.
    WireGuard,
    /// A NetworkManager VPN plugin, named if the panel recognises it.
    Plugin(&'static str),
    /// A tunnel something else brought up. Shown, never switched.
    External,
}

impl VpnKind {
    /// The word for the row's subtitle.
    pub fn label(self) -> &'static str {
        match self {
            Self::WireGuard => "WireGuard",
            Self::Plugin(name) => name,
            Self::External => "External",
        }
    }
}

/// Plugin service types the panel has a proper name for.
///
/// Everything else is simply "VPN": a plugin the panel has never heard of still
/// works, and a made-up name would be worse than the honest generic one.
const PLUGINS: &[(&str, &str)] = &[
    ("openvpn", "OpenVPN"),
    ("openconnect", "OpenConnect"),
    ("vpnc", "Cisco VPN"),
    ("l2tp", "L2TP"),
    ("sstp", "SSTP"),
    ("pptp", "PPTP"),
    ("libreswan", "IPsec"),
    ("strongswan", "IPsec"),
    ("fortisslvpn", "Fortinet SSL"),
    ("wireguard", "WireGuard"),
];

/// Which kind a connection type and service type describe.
pub fn vpn_kind(connection_type: &str, service_type: Option<&str>) -> VpnKind {
    if connection_type == "wireguard" {
        return VpnKind::WireGuard;
    }
    let plugin = service_type
        .and_then(|service| service.rsplit('.').next())
        .map(str::to_ascii_lowercase);
    let name = plugin
        .as_deref()
        .and_then(|plugin| {
            PLUGINS
                .iter()
                .find(|(key, _)| *key == plugin)
                .map(|(_, name)| *name)
        })
        .unwrap_or("VPN");
    VpnKind::Plugin(name)
}

/// One VPN profile, as a row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VpnView {
    /// The name the user gave it.
    pub id: String,
    /// Its stable identifier, and what a command names it by.
    pub uuid: String,
    /// What kind of tunnel it is.
    pub kind: VpnKind,
    /// Whether it is up.
    pub active: bool,
    /// Whether the panel is switching it right now.
    pub pending: bool,
}

impl VpnView {
    /// Whether this row is something the panel may switch.
    pub fn switchable(&self) -> bool {
        self.kind != VpnKind::External
    }
}

/// Put the VPN rows in order: what is up, then what was used last, then the
/// rest by name, with tunnels nobody here started at the bottom.
pub fn order_vpn(profiles: &mut [VpnView], last_used: Option<&str>) {
    profiles.sort_by(|a, b| {
        vpn_group(a, last_used)
            .cmp(&vpn_group(b, last_used))
            .then_with(|| a.id.to_lowercase().cmp(&b.id.to_lowercase()))
    });
}

/// Which band of the VPN list a profile belongs in.
fn vpn_group(profile: &VpnView, last_used: Option<&str>) -> u8 {
    if profile.kind == VpnKind::External {
        return 3;
    }
    if profile.active {
        0
    } else if last_used == Some(profile.uuid.as_str()) {
        1
    } else {
        2
    }
}

/// What the panel is waiting for, and which row should show a spinner.
///
/// Wi-Fi and VPN are pessimistic by policy: nothing on screen moves until
/// NetworkManager says it happened, and the row the user clicked spins in the
/// meantime. A toggle that flipped optimistically and flipped back four seconds
/// later would be a control that lied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pending {
    /// Joining, leaving, or forgetting one network.
    Wifi {
        /// The network being joined; empty while disconnecting.
        ssid: String,
    },
    /// Switching the radio.
    Radio,
    /// Switching one VPN profile.
    Vpn {
        /// The profile being switched.
        uuid: String,
    },
}

/// A password the panel is asking the user for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingPrompt {
    /// The network it is for.
    pub ssid: String,
    /// Which attempt this is, from 1.
    ///
    /// Anything past the first means NetworkManager came back asking for a new
    /// secret, which is exactly what a wrong password looks like on the bus.
    pub attempt: u32,
}

impl PendingPrompt {
    /// Whether the last password was refused.
    pub fn is_retry(&self) -> bool {
        self.attempt > 1
    }
}

/// Whether the panel is allowed to change anything on the daemon it is talking
/// to.
///
/// Shared by every service whose real daemon lives on the **system** bus and
/// *is* something of the user's — NetworkManager is their live connection,
/// BlueZ is the headphones they are listening to. A development build has no
/// business joining a network, switching a radio off, disconnecting whatever is
/// playing, or registering an agent that would intercept the prompts the
/// session's actual panel is waiting for; so a debug build talking to the real
/// bus reads and nothing else. Tests and the smoke run point the service at a
/// daemon of their own with an address, which is the signal that changing
/// things is safe.
///
/// It lives here, beside the service that needed it first, rather than in a
/// module of its own — one policy with one test, named once.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Access {
    /// List, follow, report. Refuse anything that would change the machine.
    ///
    /// The default, so a snapshot built before any policy has been decided
    /// cannot be one that permits a write.
    #[default]
    ReadOnly,
    /// The packaged panel, or a bus the test brought up itself.
    Full,
}

impl Access {
    /// What a panel with this address and this build may do.
    ///
    /// An explicit address is always a bus the caller owns. Without one, only
    /// a packaged build — the one a user installed to run their session — may
    /// touch the machine's network.
    pub(crate) fn decide(address: Option<&str>, packaged: bool) -> Self {
        if address.is_some() || packaged {
            Self::Full
        } else {
            Self::ReadOnly
        }
    }

    /// Whether a mutating call may go out.
    pub fn writable(self) -> bool {
        self == Self::Full
    }
}

/// Everything the panel knows about the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkState {
    /// Whether NetworkManager answered at all.
    pub available: bool,
    /// Whether a request to the internet is worth making.
    pub online: bool,
    /// The wireless card.
    pub wifi: WifiState,
    /// The Ethernet port.
    pub wired: WiredState,
    /// Every VPN profile, in order.
    pub vpn: Vec<VpnView>,
    /// What the panel is waiting for.
    pub pending: Option<Pending>,
    /// The password the panel is asking for, if any.
    pub prompt: Option<PendingPrompt>,
    /// Whether this panel may change anything.
    pub access: Access,
}

impl Default for NetworkState {
    /// Online, until something says otherwise.
    ///
    /// The panel starts before it has talked to any bus, and starting
    /// "offline" would defer the first weather fetch behind a round trip that
    /// may never come back.
    fn default() -> Self {
        Self {
            available: false,
            online: true,
            wifi: WifiState::default(),
            wired: WiredState::default(),
            vpn: Vec::new(),
            pending: None,
            prompt: None,
            access: Access::ReadOnly,
        }
    }
}

impl NetworkState {
    /// Whether any VPN profile is up.
    pub fn vpn_active(&self) -> bool {
        self.vpn.iter().any(|profile| profile.active)
    }

    /// Whether there is any network device at all.
    pub fn has_device(&self) -> bool {
        self.wifi.present || self.wired.present
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ap(ssid: &str, strength: u8, secured: bool) -> ApView {
        ApView::new(ssid.to_string(), strength, secured)
    }

    #[test]
    fn a_routable_machine_is_online() {
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
    fn an_unknown_or_future_state_is_treated_as_online() {
        assert!(online_from_state(STATE_UNKNOWN));
        assert!(online_from_state(80));
        assert!(online_from_state(u32::MAX));
    }

    #[test]
    fn an_ssid_is_read_as_text_even_when_it_is_not_quite_utf8() {
        assert_eq!(ssid_text(b"Cafe"), Some("Cafe".to_string()));
        // The Cyrillic name on the developer's own router, byte for byte.
        assert_eq!(
            ssid_text(&[
                0xd0, 0xa3, 0xd1, 0x81, 0xd0, 0xb0, 0xd0, 0xb4, 0xd1, 0x8c, 0xd0, 0xb1, 0xd0, 0xb0
            ]),
            Some("Усадьба".to_string())
        );
        // One bad byte does not cost the whole name.
        let lossy = ssid_text(b"Caf\xe9").expect("a name with a replacement in it");
        assert!(lossy.starts_with("Caf"), "{lossy} lost the readable part");
    }

    #[test]
    fn a_hidden_network_has_no_name_to_draw() {
        assert_eq!(ssid_text(b""), None);
        assert_eq!(ssid_text(&[0, 0, 0]), None);
        assert_eq!(ssid_text(b"   "), None);
    }

    #[test]
    fn the_five_signal_buckets_match_gnome_shells() {
        assert_eq!(strength_bucket(0), 0);
        assert_eq!(strength_bucket(19), 0);
        assert_eq!(strength_bucket(20), 1);
        assert_eq!(strength_bucket(39), 1);
        assert_eq!(strength_bucket(40), 2);
        assert_eq!(strength_bucket(59), 2);
        assert_eq!(strength_bucket(60), 3);
        assert_eq!(strength_bucket(79), 3);
        assert_eq!(strength_bucket(80), 4);
        assert_eq!(strength_bucket(100), 4);
    }

    #[test]
    fn a_wps_advertising_open_network_keeps_its_open_padlock_off() {
        // Flags 0x2 is WPS and nothing else; v1's "any non-zero flag" test drew
        // a padlock here and made an open network look like it needed one.
        assert!(!is_secured(0x2, 0, 0));
        assert!(!is_secured(0, 0, 0));
        // Privacy on its own is WEP, and WPA/RSN speak for themselves.
        assert!(is_secured(0x1, 0, 0));
        assert!(is_secured(0, 0x100, 0));
        // The developer's own router: PAIR_CCMP | GROUP_CCMP | KEY_MGMT_PSK.
        assert!(is_secured(0x3, 0, 392));
    }

    #[test]
    fn one_router_with_four_radios_is_one_row_at_its_best_signal() {
        let list = collapse(vec![
            ap("Home", 40, true),
            ap("Home", 82, true),
            ap("Home", 55, true),
        ]);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].strength, 82);
        assert_eq!(list[0].bucket, 4, "the bucket follows the merged strength");
    }

    #[test]
    fn an_open_and_a_secured_network_of_the_same_name_stay_two_rows() {
        let list = collapse(vec![ap("Cafe", 60, false), ap("Cafe", 70, true)]);
        assert_eq!(list.len(), 2, "one padlocked, one not");
    }

    #[test]
    fn merging_keeps_whichever_radio_was_the_active_or_known_one() {
        let mut weak_but_active = ap("Home", 30, true);
        weak_but_active.active = true;
        let mut strong_but_known = ap("Home", 90, true);
        strong_but_known.known = true;

        let list = collapse(vec![weak_but_active, strong_but_known]);
        assert_eq!(list.len(), 1);
        assert!(list[0].active);
        assert!(list[0].known);
        assert_eq!(list[0].strength, 90);
    }

    #[test]
    fn the_list_reads_active_then_saved_then_the_rest() {
        let mut active = ap("Active", 10, true);
        active.active = true;
        let mut known = ap("Saved", 20, true);
        known.known = true;
        let stranger = ap("Stranger", 99, false);

        let list = collapse(vec![stranger, known, active]);
        let names: Vec<&str> = list.iter().map(|ap| ap.ssid.as_str()).collect();
        assert_eq!(
            names,
            ["Active", "Saved", "Stranger"],
            "a strong stranger never outranks the network you are on"
        );
    }

    #[test]
    fn within_a_band_the_strongest_comes_first_and_ties_go_by_name() {
        let list = collapse(vec![
            ap("bravo", 50, false),
            ap("alpha", 50, false),
            ap("charlie", 90, false),
        ]);
        let names: Vec<&str> = list.iter().map(|ap| ap.ssid.as_str()).collect();
        assert_eq!(names, ["charlie", "alpha", "bravo"]);
    }

    #[test]
    fn an_empty_scan_is_an_empty_list() {
        assert!(collapse(Vec::new()).is_empty());
    }

    #[test]
    fn a_link_speed_is_written_the_way_a_box_is_labelled() {
        let at = |speed| {
            WiredState {
                speed_mbps: speed,
                ..WiredState::default()
            }
            .speed_label()
        };
        assert_eq!(at(0), None, "a driver that will not say says nothing");
        assert_eq!(at(100), Some("100 Mb/s".to_string()));
        assert_eq!(at(1000), Some("1 Gb/s".to_string()));
        assert_eq!(at(2500), Some("2.5 Gb/s".to_string()));
        assert_eq!(at(10000), Some("10 Gb/s".to_string()));
    }

    #[test]
    fn a_vpn_is_named_by_the_plugin_that_runs_it() {
        assert_eq!(vpn_kind("wireguard", None), VpnKind::WireGuard);
        assert_eq!(
            vpn_kind("vpn", Some("org.freedesktop.NetworkManager.openvpn")),
            VpnKind::Plugin("OpenVPN")
        );
        assert_eq!(
            vpn_kind("vpn", Some("org.freedesktop.NetworkManager.strongswan")),
            VpnKind::Plugin("IPsec")
        );
        // A plugin nobody here has heard of still works, and says so honestly.
        assert_eq!(
            vpn_kind("vpn", Some("com.example.SomeVpn")),
            VpnKind::Plugin("VPN")
        );
        assert_eq!(vpn_kind("vpn", None), VpnKind::Plugin("VPN"));
    }

    #[test]
    fn every_kind_has_a_word_for_its_row() {
        for kind in [
            VpnKind::WireGuard,
            VpnKind::Plugin("OpenVPN"),
            VpnKind::External,
        ] {
            assert!(!kind.label().is_empty());
        }
    }

    fn vpn(id: &str, uuid: &str, active: bool, kind: VpnKind) -> VpnView {
        VpnView {
            id: id.to_string(),
            uuid: uuid.to_string(),
            kind,
            active,
            pending: false,
        }
    }

    #[test]
    fn the_vpn_list_puts_what_is_up_first_and_what_was_last_used_next() {
        let mut profiles = vec![
            vpn("Zulu", "z", false, VpnKind::WireGuard),
            vpn("Alpha", "a", false, VpnKind::WireGuard),
            vpn("Work", "w", true, VpnKind::Plugin("OpenVPN")),
            vpn("Home", "h", false, VpnKind::WireGuard),
        ];
        order_vpn(&mut profiles, Some("h"));
        let names: Vec<&str> = profiles.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(names, ["Work", "Home", "Alpha", "Zulu"]);
    }

    #[test]
    fn a_tunnel_the_panel_did_not_start_sits_at_the_bottom_and_is_not_switchable() {
        let mut profiles = vec![
            vpn("tun0", "external", true, VpnKind::External),
            vpn("Home", "h", false, VpnKind::WireGuard),
        ];
        order_vpn(&mut profiles, None);
        assert_eq!(profiles[0].id, "Home", "an external tunnel never leads");
        assert!(!profiles[1].switchable());
        assert!(profiles[0].switchable());
    }

    #[test]
    fn a_second_password_prompt_is_a_refused_first_one() {
        assert!(
            !PendingPrompt {
                ssid: "Home".into(),
                attempt: 1
            }
            .is_retry()
        );
        assert!(
            PendingPrompt {
                ssid: "Home".into(),
                attempt: 2
            }
            .is_retry()
        );
    }

    #[test]
    fn only_a_packaged_panel_or_a_bus_of_our_own_may_change_the_network() {
        // The developer's live network, from a debug build: look, do not touch.
        assert_eq!(Access::decide(None, false), Access::ReadOnly);
        // The packaged panel is the session's own, and does the whole job.
        assert_eq!(Access::decide(None, true), Access::Full);
        // A test or the smoke run brought the bus up itself.
        assert_eq!(
            Access::decide(Some("unix:path=/tmp/x"), false),
            Access::Full
        );
        assert!(!Access::ReadOnly.writable());
        assert!(Access::Full.writable());
    }

    #[test]
    fn a_panel_that_has_not_reached_the_bus_yet_assumes_it_is_online() {
        let state = NetworkState::default();
        assert!(state.online);
        assert!(!state.available);
        assert!(!state.has_device());
        assert!(!state.vpn_active());
    }
}
