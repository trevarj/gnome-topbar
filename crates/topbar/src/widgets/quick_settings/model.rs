//! What the panel and its button show, worked out without any GTK.
//!
//! The bar button's icon order, which of those icons are drawn right now, the
//! order of the toggle grid and how it wraps into rows, the bounds a slider is
//! allowed to move between, and which device in a list is the one in use.
//! Every one of these is a decision the eye notices when it is wrong, and
//! every one of them is a pure function here so it can be checked without a
//! display.

use topbar_services::{AudioState, BatteryState};

/// One status icon on the Quick Settings button.
///
/// The order of this enum is not the order they are drawn in — see [`ORDER`],
/// which is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Indicator {
    /// Wi-Fi or wired. Lands in M9b.
    Network,
    /// A VPN badge. Lands in M9b.
    Vpn,
    /// The output device's volume level. Always drawn.
    Audio,
    /// Bluetooth, when an adapter is powered. Lands in M9c.
    Bluetooth,
    /// The battery, when there is one.
    Battery,
    /// A dot saying something is listening.
    Microphone,
    /// A dot saying something is watching the screen. Lands in M9c.
    ScreenShare,
}

/// Left to right, the order the button draws its icons in.
///
/// Fixed rather than derived: the icons that come and go must not make the
/// others move about. M9b and M9c fill in the gaps without touching this list,
/// which is the point of writing it out in full now.
pub const ORDER: &[Indicator] = &[
    Indicator::Network,
    Indicator::Vpn,
    Indicator::Audio,
    Indicator::Bluetooth,
    Indicator::Battery,
    Indicator::Microphone,
    Indicator::ScreenShare,
];

/// What the button has to go on.
#[derive(Debug, Clone, Copy, Default)]
pub struct Indicators {
    /// Whether a microphone is in use right now.
    pub microphone: bool,
    /// Whether there is a battery to draw.
    pub battery: bool,
}

impl Indicators {
    /// Read the services the button subscribes to.
    pub fn read(audio: &AudioState, battery: &BatteryState, show_battery: bool) -> Self {
        Self {
            microphone: audio.source_in_use,
            battery: show_battery && battery.available,
        }
    }

    /// Whether `indicator` is drawn right now.
    ///
    /// The three M9b/M9c indicators answer `false` and will answer for
    /// themselves when their services arrive; the audio icon is always there,
    /// because a panel button with nothing on it is a panel button nobody
    /// finds.
    pub fn shows(self, indicator: Indicator) -> bool {
        match indicator {
            Indicator::Audio => true,
            Indicator::Battery => self.battery,
            Indicator::Microphone => self.microphone,
            Indicator::Network | Indicator::Vpn | Indicator::Bluetooth | Indicator::ScreenShare => {
                false
            }
        }
    }

    /// The icons on the button, in the order they are drawn.
    pub fn visible(self) -> Vec<Indicator> {
        ORDER
            .iter()
            .copied()
            .filter(|indicator| self.shows(*indicator))
            .collect()
    }
}

/// One pill in the toggle grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toggle {
    /// Wi-Fi. Lands in M9b.
    WiFi,
    /// Bluetooth. Lands in M9c.
    Bluetooth,
    /// VPN. Lands in M9b.
    Vpn,
    /// The idle inhibitor.
    Caffeine,
    /// The power-profiles daemon.
    PowerMode,
}

/// The order pills appear in, whichever of them exist.
///
/// The two that exist today land at the end, exactly where they will still be
/// once the first three arrive — so M9b and M9c add a pill without moving the
/// ones the user has learned the position of.
pub const GRID_ORDER: &[Toggle] = &[
    Toggle::WiFi,
    Toggle::Bluetooth,
    Toggle::Vpn,
    Toggle::Caffeine,
    Toggle::PowerMode,
];

/// How many pills fit across the panel.
pub const COLUMNS: usize = 2;

/// Sort `present` into grid order and cut it into rows.
///
/// The last row may be short; the grid pads it so a lone pill is half the
/// width of the panel rather than all of it, which is what a homogeneous row
/// does on its own.
pub fn grid_rows(present: &[Toggle]) -> Vec<Vec<Toggle>> {
    let ordered: Vec<Toggle> = GRID_ORDER
        .iter()
        .copied()
        .filter(|toggle| present.contains(toggle))
        .collect();
    ordered.chunks(COLUMNS).map(<[Toggle]>::to_vec).collect()
}

/// The highest volume the output slider may be dragged to.
///
/// `max_volume_pct` comes from the sound server and honours
/// `[audio] allow_overdrive`; a slider whose range disagreed with the service's
/// ceiling would either refuse the last few percent or silently clamp.
pub fn slider_ceiling(audio: &AudioState) -> f64 {
    f64::from(audio.max_volume_pct.max(1))
}

/// A value clamped to what a slider may be set to.
pub fn clamp_volume(percent: u32, audio: &AudioState) -> u32 {
    percent.min(audio.max_volume_pct.max(1))
}

/// A scroll step, from `[widgets.quick_settings] audio_scroll_percentage`.
///
/// Configuration validation already rejects anything outside 1–25, so this
/// only covers a `Config` built by hand — but a step of zero would make the
/// wheel do nothing at all, which reads as a broken widget rather than as a
/// misconfiguration.
pub fn scroll_step(configured: u32) -> u32 {
    configured.clamp(1, 25)
}

/// Which entry of a device list is the one in use.
///
/// PulseAudio's own flag rather than a name comparison: descriptions are not
/// unique — two identical headsets are two identical strings — and the default
/// sink is an identity, not a label.
pub fn selected_device(devices: &[topbar_services::DeviceView]) -> Option<usize> {
    devices.iter().position(|device| device.is_default)
}

/// The devices worth offering, in the order they were given.
///
/// A device whose port says it is unplugged is left out: offering to send
/// audio to a headphone socket with nothing in it is offering silence.
pub fn choosable_devices(
    devices: &[topbar_services::DeviceView],
) -> Vec<&topbar_services::DeviceView> {
    devices
        .iter()
        .filter(|device| device.port_available != Some(false))
        .collect()
}

/// Whether the output chooser is worth showing at all.
pub fn wants_chooser(audio: &AudioState) -> bool {
    choosable_devices(&audio.sinks).len() > 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use topbar_services::{BatteryStatus, DeviceView};

    fn device(id: &str, is_default: bool, port_available: Option<bool>) -> DeviceView {
        DeviceView {
            id: id.to_string(),
            description: id.to_string(),
            is_default,
            port_available,
        }
    }

    #[test]
    fn the_icon_order_is_the_one_the_panel_promises() {
        assert_eq!(
            ORDER,
            &[
                Indicator::Network,
                Indicator::Vpn,
                Indicator::Audio,
                Indicator::Bluetooth,
                Indicator::Battery,
                Indicator::Microphone,
                Indicator::ScreenShare,
            ]
        );
    }

    #[test]
    fn the_audio_icon_is_always_there_and_the_seams_never_are() {
        let nothing = Indicators::default();
        assert_eq!(nothing.visible(), vec![Indicator::Audio]);
        for seam in [
            Indicator::Network,
            Indicator::Vpn,
            Indicator::Bluetooth,
            Indicator::ScreenShare,
        ] {
            assert!(
                !nothing.shows(seam),
                "{seam:?} belongs to a later milestone"
            );
        }
    }

    #[test]
    fn the_battery_comes_before_the_microphone_dot() {
        let both = Indicators {
            microphone: true,
            battery: true,
        };
        assert_eq!(
            both.visible(),
            vec![Indicator::Audio, Indicator::Battery, Indicator::Microphone]
        );
    }

    #[test]
    fn a_desktop_draws_no_battery() {
        let audio = AudioState::default();
        let battery = BatteryState::default();
        let indicators = Indicators::read(&audio, &battery, true);
        assert!(!indicators.shows(Indicator::Battery));
    }

    #[test]
    fn a_battery_switched_off_in_the_config_is_not_drawn_either() {
        let audio = AudioState::default();
        let battery = BatteryState {
            available: true,
            percent: Some(50.0),
            status: BatteryStatus::Discharging,
            ..BatteryState::default()
        };
        assert!(Indicators::read(&audio, &battery, true).shows(Indicator::Battery));
        assert!(!Indicators::read(&audio, &battery, false).shows(Indicator::Battery));
    }

    #[test]
    fn a_microphone_in_use_raises_the_dot() {
        let audio = AudioState {
            source_in_use: true,
            ..AudioState::default()
        };
        let indicators = Indicators::read(&audio, &BatteryState::default(), true);
        assert!(indicators.shows(Indicator::Microphone));
    }

    #[test]
    fn the_grid_keeps_its_order_whatever_order_it_is_given() {
        let rows = grid_rows(&[Toggle::PowerMode, Toggle::Caffeine]);
        assert_eq!(rows, vec![vec![Toggle::Caffeine, Toggle::PowerMode]]);
    }

    #[test]
    fn the_grid_wraps_at_two_and_the_last_row_may_be_short() {
        let rows = grid_rows(&[
            Toggle::WiFi,
            Toggle::Bluetooth,
            Toggle::Vpn,
            Toggle::Caffeine,
            Toggle::PowerMode,
        ]);
        assert_eq!(
            rows,
            vec![
                vec![Toggle::WiFi, Toggle::Bluetooth],
                vec![Toggle::Vpn, Toggle::Caffeine],
                vec![Toggle::PowerMode],
            ]
        );
    }

    #[test]
    fn adding_the_later_toggles_does_not_move_the_ones_that_exist_now() {
        // Caffeine and Power Mode are adjacent today...
        let today = grid_rows(&[Toggle::Caffeine, Toggle::PowerMode]);
        assert_eq!(today[0], vec![Toggle::Caffeine, Toggle::PowerMode]);
        // ...and still adjacent, in the same order, once M9b and M9c land.
        let later = grid_rows(GRID_ORDER);
        let flat: Vec<Toggle> = later.into_iter().flatten().collect();
        let caffeine = flat
            .iter()
            .position(|toggle| *toggle == Toggle::Caffeine)
            .expect("caffeine");
        assert_eq!(flat[caffeine + 1], Toggle::PowerMode);
    }

    #[test]
    fn an_empty_grid_has_no_rows() {
        assert!(grid_rows(&[]).is_empty());
    }

    #[test]
    fn the_slider_stops_where_the_sound_server_stops() {
        let plain = AudioState {
            max_volume_pct: 100,
            ..AudioState::default()
        };
        assert!((slider_ceiling(&plain) - 100.0).abs() < f64::EPSILON);
        assert_eq!(clamp_volume(150, &plain), 100);

        let overdriven = AudioState {
            max_volume_pct: 150,
            ..AudioState::default()
        };
        assert!((slider_ceiling(&overdriven) - 150.0).abs() < f64::EPSILON);
        assert_eq!(clamp_volume(150, &overdriven), 150);
        assert_eq!(clamp_volume(200, &overdriven), 150);
    }

    #[test]
    fn a_ceiling_of_zero_would_be_a_slider_with_no_range() {
        let broken = AudioState {
            max_volume_pct: 0,
            ..AudioState::default()
        };
        assert!(slider_ceiling(&broken) >= 1.0);
        assert_eq!(clamp_volume(50, &broken), 1);
    }

    #[test]
    fn the_scroll_step_stays_inside_the_range_the_config_documents() {
        assert_eq!(scroll_step(5), 5);
        assert_eq!(scroll_step(0), 1);
        assert_eq!(scroll_step(99), 25);
        assert_eq!(scroll_step(25), 25);
    }

    #[test]
    fn the_device_in_use_is_found_by_its_flag_not_its_name() {
        let devices = [
            device("hdmi", false, None),
            device("analog", true, None),
            device("usb", false, None),
        ];
        assert_eq!(selected_device(&devices), Some(1));
        assert_eq!(selected_device(&[]), None);
    }

    #[test]
    fn a_socket_with_nothing_plugged_into_it_is_not_offered() {
        let devices = [
            device("speakers", true, Some(true)),
            device("headphones", false, Some(false)),
            device("hdmi", false, None),
        ];
        let offered: Vec<&str> = choosable_devices(&devices)
            .iter()
            .map(|device| device.id.as_str())
            .collect();
        assert_eq!(
            offered,
            ["speakers", "hdmi"],
            "a device with no jack detection is not an unplugged one"
        );
    }

    #[test]
    fn the_chooser_appears_only_where_there_is_a_choice() {
        let one = AudioState {
            sinks: vec![device("speakers", true, None)],
            ..AudioState::default()
        };
        assert!(!wants_chooser(&one));

        let two = AudioState {
            sinks: vec![device("speakers", true, None), device("hdmi", false, None)],
            ..AudioState::default()
        };
        assert!(wants_chooser(&two));

        let one_unplugged = AudioState {
            sinks: vec![
                device("speakers", true, None),
                device("headphones", false, Some(false)),
            ],
            ..AudioState::default()
        };
        assert!(
            !wants_chooser(&one_unplugged),
            "an unplugged socket is not a second choice"
        );
    }
}
