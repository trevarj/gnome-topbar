//! What the panel knows about Bluetooth, and every decision with no bus in it.
//!
//! Which devices are worth a row, what order they go in, which picture goes
//! beside each one, and how a six-digit passkey is written down. All pure
//! functions, all testable without an adapter — which matters more here than
//! almost anywhere else in the panel, because the alternative is a test that
//! pairs something with the developer's laptop.

/// One device's kind, as far as the picture beside it is concerned.
///
/// BlueZ works the kind out from the device's class-of-device bits and
/// publishes it as `Icon` — a *freedesktop icon name*, not a symbolic one. The
/// panel maps it to the Adwaita symbolic set rather than using it verbatim the
/// way v1 did: `audio-headset` has no `-symbolic` variant in Adwaita, so v1's
/// pass-through drew a full-colour 48px headset in a 16px row on any theme
/// that happened to carry one, and nothing at all on the ones that did not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconKind {
    /// Headphones, with no microphone.
    Headphones,
    /// A headset, with one.
    Headset,
    /// A speaker, or a car stereo.
    Speaker,
    /// A keyboard.
    Keyboard,
    /// A mouse, a trackpad or a tablet.
    Mouse,
    /// A game controller.
    Gamepad,
    /// A phone.
    Phone,
    /// Another computer.
    Computer,
    /// A printer or a scanner.
    Printer,
    /// A camera.
    Camera,
    /// A screen, or something that plays to one.
    Display,
    /// Anything else, including a device BlueZ could not classify.
    Generic,
}

impl IconKind {
    /// Which kind BlueZ's `Icon` property describes.
    ///
    /// Everything unrecognised is [`IconKind::Generic`] rather than a guess: a
    /// Bluetooth logo beside an unknown device is honest, and a headset icon
    /// beside a heart-rate monitor is not.
    pub fn from_bluez(icon: Option<&str>) -> Self {
        match icon.unwrap_or_default() {
            "audio-headphones" => Self::Headphones,
            "audio-headset" => Self::Headset,
            "audio-card" | "audio-speakers" => Self::Speaker,
            "input-keyboard" => Self::Keyboard,
            "input-mouse" | "input-tablet" => Self::Mouse,
            "input-gaming" => Self::Gamepad,
            "phone" => Self::Phone,
            "computer" => Self::Computer,
            "printer" | "scanner" => Self::Printer,
            "camera-photo" | "camera-video" => Self::Camera,
            "video-display" | "multimedia-player" => Self::Display,
            _ => Self::Generic,
        }
    }

    /// The Adwaita symbolic name for this kind.
    pub fn icon(self) -> &'static str {
        match self {
            Self::Headphones => "audio-headphones-symbolic",
            Self::Headset => "audio-headset-symbolic",
            Self::Speaker => "audio-speakers-symbolic",
            Self::Keyboard => "input-keyboard-symbolic",
            Self::Mouse => "input-mouse-symbolic",
            Self::Gamepad => "input-gaming-symbolic",
            Self::Phone => "phone-symbolic",
            Self::Computer => "computer-symbolic",
            Self::Printer => "printer-symbolic",
            Self::Camera => "camera-photo-symbolic",
            Self::Display => "video-display-symbolic",
            Self::Generic => BLUETOOTH,
        }
    }
}

/// The plain Bluetooth logo.
pub const BLUETOOTH: &str = "bluetooth-symbolic";
/// The logo with something joined to it.
pub const BLUETOOTH_ACTIVE: &str = "bluetooth-active-symbolic";
/// The logo with the radio off.
pub const BLUETOOTH_DISABLED: &str = "bluetooth-disabled-symbolic";

/// One paired device, as a row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtDevice {
    /// Its object path, which is what a command names it by.
    pub path: String,
    /// The name to draw: `Alias` if BlueZ has one, else the address.
    pub alias: String,
    /// Which picture goes beside it.
    pub icon: IconKind,
    /// Whether it is connected right now.
    pub connected: bool,
    /// Whether it is paired.
    pub paired: bool,
    /// Its battery level, when it publishes one.
    pub battery_pct: Option<u8>,
    /// Whether the panel is connecting or disconnecting it right now.
    pub pending: bool,
}

/// Put the device list in the order GNOME puts it in.
///
/// What is connected first, then everything else by name. Case-insensitive,
/// with the object path breaking a tie — two identical earbuds are two
/// identical strings, and a list that reordered itself between two readings
/// would move a row out from under the pointer.
pub fn order(devices: &mut [BtDevice]) {
    devices.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then_with(|| a.alias.to_lowercase().cmp(&b.alias.to_lowercase()))
            .then_with(|| a.path.cmp(&b.path))
    });
}

/// Whether a device belongs in the list at all.
///
/// Paired only, which is what GNOME's own Quick Settings shows. The panel is
/// not a pairing dialog — GNOME pairs in Settings and so does this desktop —
/// and a list that filled up with every phone in the building the moment the
/// adapter started scanning would be a list nobody could find their headphones
/// in.
pub fn listed(paired: bool) -> bool {
    paired
}

/// A passkey, written the way BlueZ's own documentation writes it.
///
/// Six digits, zero-padded: a passkey is a number between 0 and 999999 and the
/// *other* device is showing it with the leading zeros on. "42" beside
/// "000042" is a confirmation dialog that has failed at its one job.
pub fn passkey_text(passkey: u32) -> String {
    format!("{passkey:06}")
}

/// What kind of answer a pairing prompt is waiting for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    /// "Does this code match?" — the panel answers yes or no.
    Confirm,
    /// "Is this device allowed to pair?" — no code, just a decision.
    Authorize,
    /// "Type this on the other device." — nothing to answer; it goes away
    /// when the pairing finishes.
    Display,
}

/// A pairing question on screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingPrompt {
    /// The device's object path.
    pub path: String,
    /// Its name, as far as the panel could work one out.
    pub alias: String,
    /// The code to show, already formatted.
    pub code: Option<String>,
    /// What sort of answer is wanted.
    pub kind: PromptKind,
}

impl PairingPrompt {
    /// Whether this prompt has buttons under it.
    pub fn answerable(&self) -> bool {
        !matches!(self.kind, PromptKind::Display)
    }

    /// The sentence above the code.
    pub fn question(&self) -> String {
        match self.kind {
            PromptKind::Confirm => {
                format!("Confirm this code matches the one on {}", self.alias)
            }
            PromptKind::Authorize => format!("Allow {} to pair with this computer?", self.alias),
            PromptKind::Display => format!("Enter this code on {}", self.alias),
        }
    }
}

/// Everything the panel knows about Bluetooth.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BtState {
    /// Whether this machine has an adapter BlueZ is managing.
    pub available: bool,
    /// Whether the adapter's radio is on.
    pub powered: bool,
    /// Whether the panel is waiting on the radio switch.
    pub powering: bool,
    /// The paired devices, in order.
    pub devices: Vec<BtDevice>,
    /// The pairing question on screen, if any.
    pub prompt: Option<PairingPrompt>,
    /// Whether this panel may change anything here.
    pub access: crate::network::Access,
}

impl BtState {
    /// How many devices are connected.
    pub fn connected_count(&self) -> usize {
        self.devices
            .iter()
            .filter(|device| device.connected)
            .count()
    }

    /// The first connected device, which is what the collapsed pill names.
    pub fn first_connected(&self) -> Option<&BtDevice> {
        self.devices.iter().find(|device| device.connected)
    }

    /// The icon for the collapsed pill and the bar indicator.
    pub fn icon(&self) -> &'static str {
        if !self.powered {
            BLUETOOTH_DISABLED
        } else if self.connected_count() > 0 {
            BLUETOOTH_ACTIVE
        } else {
            BLUETOOTH
        }
    }

    /// What the collapsed pill is titled.
    ///
    /// The device in use, when there is exactly one: that is the answer to the
    /// question the user opened the panel to ask, the same way the Wi-Fi pill
    /// names the network. Two is a count, because two names do not fit in half
    /// a panel.
    pub fn title(&self) -> String {
        match self.connected_count() {
            1 => self
                .first_connected()
                .map_or_else(|| "Bluetooth".to_string(), |device| device.alias.clone()),
            _ => "Bluetooth".to_string(),
        }
    }

    /// What it says underneath.
    pub fn subtitle(&self) -> String {
        if !self.powered {
            return "Off".to_string();
        }
        match self.connected_count() {
            0 => "Not connected".to_string(),
            1 => "Connected".to_string(),
            many => format!("{many} connected"),
        }
    }

    /// Whether the bar indicator draws anything.
    pub fn indicated(&self) -> bool {
        self.powered && self.connected_count() > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(alias: &str, connected: bool) -> BtDevice {
        BtDevice {
            path: format!("/org/bluez/hci0/dev_{alias}"),
            alias: alias.to_string(),
            icon: IconKind::Generic,
            connected,
            paired: true,
            battery_pct: None,
            pending: false,
        }
    }

    #[test]
    fn bluez_device_classes_map_onto_the_symbolic_set() {
        for (bluez, kind) in [
            ("audio-headset", IconKind::Headset),
            ("audio-headphones", IconKind::Headphones),
            ("audio-card", IconKind::Speaker),
            ("input-keyboard", IconKind::Keyboard),
            ("input-mouse", IconKind::Mouse),
            ("input-tablet", IconKind::Mouse),
            ("input-gaming", IconKind::Gamepad),
            ("phone", IconKind::Phone),
            ("computer", IconKind::Computer),
            ("printer", IconKind::Printer),
            ("camera-photo", IconKind::Camera),
            ("video-display", IconKind::Display),
        ] {
            assert_eq!(IconKind::from_bluez(Some(bluez)), kind, "{bluez}");
        }
    }

    #[test]
    fn a_device_bluez_could_not_classify_gets_the_bluetooth_logo() {
        assert_eq!(IconKind::from_bluez(None), IconKind::Generic);
        assert_eq!(IconKind::from_bluez(Some("")), IconKind::Generic);
        // BlueZ ships icon names the panel has never heard of — a heart-rate
        // monitor, a beacon — and a guess would be worse than the logo.
        assert_eq!(
            IconKind::from_bluez(Some("network-wireless")),
            IconKind::Generic
        );
        assert_eq!(IconKind::Generic.icon(), BLUETOOTH);
    }

    #[test]
    fn every_device_icon_is_a_symbolic_name() {
        for kind in [
            IconKind::Headphones,
            IconKind::Headset,
            IconKind::Speaker,
            IconKind::Keyboard,
            IconKind::Mouse,
            IconKind::Gamepad,
            IconKind::Phone,
            IconKind::Computer,
            IconKind::Printer,
            IconKind::Camera,
            IconKind::Display,
            IconKind::Generic,
        ] {
            assert!(
                kind.icon().ends_with("-symbolic"),
                "{kind:?} draws {}",
                kind.icon()
            );
        }
        for name in [BLUETOOTH, BLUETOOTH_ACTIVE, BLUETOOTH_DISABLED] {
            assert!(name.ends_with("-symbolic"));
        }
    }

    #[test]
    fn the_list_puts_what_is_connected_first_and_then_sorts_by_name() {
        let mut devices = vec![
            device("Zulu", false),
            device("alpha", false),
            device("WH-1000XM4", true),
            device("Magic Keyboard", false),
        ];
        order(&mut devices);
        let names: Vec<&str> = devices.iter().map(|d| d.alias.as_str()).collect();
        assert_eq!(names, ["WH-1000XM4", "alpha", "Magic Keyboard", "Zulu"]);
    }

    #[test]
    fn two_devices_with_the_same_name_keep_a_stable_order() {
        let mut first = device("Earbuds", false);
        first.path = "/org/bluez/hci0/dev_AA".into();
        let mut second = device("earbuds", false);
        second.path = "/org/bluez/hci0/dev_BB".into();

        let mut devices = vec![second.clone(), first.clone()];
        order(&mut devices);
        assert_eq!(devices[0].path, first.path, "the path breaks the tie");

        let mut again = vec![first, second];
        order(&mut again);
        assert_eq!(again[0].path, devices[0].path, "and it does so every time");
    }

    #[test]
    fn only_paired_devices_are_listed() {
        assert!(listed(true));
        assert!(
            !listed(false),
            "the panel is not a pairing dialog; a phone walking past is not a row"
        );
    }

    #[test]
    fn a_passkey_is_written_with_its_leading_zeros_on() {
        assert_eq!(passkey_text(123_456), "123456");
        assert_eq!(passkey_text(42), "000042");
        assert_eq!(passkey_text(0), "000000");
        assert_eq!(passkey_text(999_999), "999999");
        // Out of range for the protocol; shown rather than truncated, because
        // a code that does not match the other screen has to look wrong.
        assert_eq!(passkey_text(1_234_567), "1234567");
    }

    #[test]
    fn the_collapsed_pill_names_one_device_and_counts_several() {
        let mut state = BtState {
            available: true,
            powered: true,
            devices: vec![device("WH-1000XM4", true), device("Mouse", false)],
            ..BtState::default()
        };
        assert_eq!(state.title(), "WH-1000XM4");
        assert_eq!(state.subtitle(), "Connected");
        assert_eq!(state.icon(), BLUETOOTH_ACTIVE);
        assert!(state.indicated());

        state.devices[1].connected = true;
        assert_eq!(state.title(), "Bluetooth");
        assert_eq!(state.subtitle(), "2 connected");

        state.devices.iter_mut().for_each(|d| d.connected = false);
        assert_eq!(state.title(), "Bluetooth");
        assert_eq!(state.subtitle(), "Not connected");
        assert_eq!(state.icon(), BLUETOOTH);
        assert!(!state.indicated(), "a radio with nothing on it is quiet");
    }

    #[test]
    fn a_radio_that_is_off_says_so_and_draws_nothing_on_the_bar() {
        let state = BtState {
            available: true,
            powered: false,
            devices: vec![device("WH-1000XM4", true)],
            ..BtState::default()
        };
        assert_eq!(state.subtitle(), "Off");
        assert_eq!(state.icon(), BLUETOOTH_DISABLED);
        assert!(
            !state.indicated(),
            "a stale Connected flag under a dead radio is not an indicator"
        );
    }

    #[test]
    fn a_display_prompt_has_nothing_to_answer_and_a_confirmation_does() {
        let confirm = PairingPrompt {
            path: "/org/bluez/hci0/dev_AA".into(),
            alias: "Pixel".into(),
            code: Some(passkey_text(42)),
            kind: PromptKind::Confirm,
        };
        assert!(confirm.answerable());
        assert!(confirm.question().contains("Pixel"));
        assert_eq!(confirm.code.as_deref(), Some("000042"));

        let display = PairingPrompt {
            kind: PromptKind::Display,
            ..confirm.clone()
        };
        assert!(!display.answerable());

        let authorize = PairingPrompt {
            code: None,
            kind: PromptKind::Authorize,
            ..confirm
        };
        assert!(authorize.answerable());
        assert!(authorize.question().contains("pair"));
    }

    #[test]
    fn a_machine_with_no_adapter_knows_it() {
        let state = BtState::default();
        assert!(!state.available);
        assert!(!state.powered);
        assert!(!state.indicated());
        assert_eq!(state.connected_count(), 0);
        assert!(state.first_connected().is_none());
    }
}
