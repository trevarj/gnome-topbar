//! What the panel knows about the battery, and how it is drawn.
//!
//! Everything here is pure: the icon a percentage maps to, whether a pair of
//! charge thresholds is one the kernel will accept, and which order two
//! threshold files have to be written in. None of it needs a bus, which is why
//! all of it is tested.

use std::path::PathBuf;

/// What the battery is doing, as UPower reports it.
///
/// The numbers are UPower's own: they arrive over D-Bus as a `u32` and are
/// mapped here rather than passed around raw, so nothing downstream has to
/// remember that 4 means full.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BatteryStatus {
    /// UPower will not say.
    #[default]
    Unknown,
    /// Charging.
    Charging,
    /// Running off the battery.
    Discharging,
    /// Flat.
    Empty,
    /// Full, and on mains.
    Full,
    /// On mains but not taking charge — which is what a charge limit looks
    /// like once the battery has reached it.
    PendingCharge,
    /// On mains, about to start discharging.
    PendingDischarge,
}

impl BatteryStatus {
    /// Map UPower's `State` property.
    pub fn from_upower(state: u32) -> Self {
        match state {
            1 => Self::Charging,
            2 => Self::Discharging,
            3 => Self::Empty,
            4 => Self::Full,
            5 => Self::PendingCharge,
            6 => Self::PendingDischarge,
            _ => Self::Unknown,
        }
    }

    /// Whether the battery is taking charge, for the icon's charging variant.
    pub fn is_charging(self) -> bool {
        matches!(self, Self::Charging | Self::PendingCharge)
    }

    /// Whether the machine is running off the battery.
    pub fn is_discharging(self) -> bool {
        matches!(self, Self::Discharging | Self::PendingDischarge)
    }

    /// One word for the pill's second line.
    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "Unknown",
            Self::Charging => "Charging",
            Self::Discharging => "Discharging",
            Self::Empty => "Empty",
            Self::Full => "Fully charged",
            Self::PendingCharge => "Not charging",
            Self::PendingDischarge => "Pending discharge",
        }
    }
}

/// The charge limit the firmware is enforcing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Thresholds {
    /// Below this percentage, charging starts again.
    pub start: u8,
    /// At this percentage, charging stops.
    pub end: u8,
    /// Whether this process can write the files directly.
    ///
    /// False on a stock system: the kernel exposes the files root-owned, and
    /// the fix is a udev rule rather than anything the panel can do. See the
    /// battery-health card, which says so rather than pretending otherwise.
    pub writable: bool,
}

impl Thresholds {
    /// Whether a limit is actually in force, as opposed to "charge to full".
    pub fn limited(self) -> bool {
        self.end < 100
    }
}

/// The preset that charges to full.
///
/// A start of 96 rather than 100 because a battery that resumes charging at
/// 99% trickle-cycles all day, which is the wear the limit exists to avoid.
pub const FULL_PRESET: (u8, u8) = (96, 100);
/// The preset that stops at 80%, which is the one worth having.
pub const LIMIT_PRESET: (u8, u8) = (75, 80);

/// Everything the panel knows about the battery.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize)]
pub struct BatteryState {
    /// Whether there is a battery to report on at all.
    ///
    /// False on a desktop, and false while UPower has not answered. The bar's
    /// battery icon and the health card are both hidden outright rather than
    /// drawn empty.
    pub available: bool,
    /// Charge, 0–100, when it is known.
    pub percent: Option<f64>,
    /// What it is doing.
    pub status: BatteryStatus,
    /// Seconds until flat, when discharging and UPower has an estimate.
    pub time_to_empty: Option<i64>,
    /// Seconds until full, when charging and UPower has an estimate.
    pub time_to_full: Option<i64>,
    /// The charge limit, when the firmware exposes one.
    pub thresholds: Option<Thresholds>,
    /// Whether UPower offers to set the limit when the files will not take it.
    pub upower_thresholds: bool,
}

/// At or below this, and running off the battery, the icon turns urgent.
pub const LOW_PERCENT: f64 = 20.0;

impl BatteryState {
    /// Whether the charge limit can be changed right now.
    pub fn can_set_thresholds(&self) -> bool {
        self.thresholds.is_some_and(|limits| limits.writable) || self.upower_thresholds
    }

    /// Whether the reading should be drawn as a warning.
    pub fn is_low(&self) -> bool {
        self.status.is_discharging() && self.percent.is_some_and(|percent| percent <= LOW_PERCENT)
    }

    /// The symbolic icon for this reading.
    pub fn icon(&self) -> String {
        icon(self.percent, self.status)
    }

    /// The percentage, rounded the way it is written on the pill.
    pub fn rounded_percent(&self) -> Option<u32> {
        self.percent
            .map(|percent| percent.round().clamp(0.0, 100.0) as u32)
    }
}

/// The Adwaita icon for a charge and a state.
///
/// The level is floored to a multiple of ten, which is what GNOME Shell does:
/// 99% draws the 90 icon, and only a genuine 100 draws a full battery. A
/// reading with no percentage in it — UPower still starting up, a battery that
/// has just been removed — draws the missing-battery icon rather than an empty
/// one, because an empty battery outline means "flat", which is a different
/// and much more alarming claim.
///
/// Shared with the headset widget, which has the same three facts to draw and
/// no reason to name a second set of icons for them.
pub fn icon(percent: Option<f64>, status: BatteryStatus) -> String {
    let Some(percent) = percent else {
        return "battery-missing-symbolic".to_string();
    };
    let level = level(percent);
    // Adwaita has `battery-level-100-charged-symbolic` and no
    // `battery-level-100-charging-symbolic`: a battery that has reached full
    // while still on its cable is *charged*, and asking for the name that does
    // not exist would draw the missing-icon glyph.
    if status == BatteryStatus::Full || (status.is_charging() && level == 100) {
        return "battery-level-100-charged-symbolic".to_string();
    }

    if status.is_charging() {
        format!("battery-level-{level}-charging-symbolic")
    } else {
        format!("battery-level-{level}-symbolic")
    }
}

/// A percentage floored to the multiple of ten its icon is named after.
fn level(percent: f64) -> u32 {
    let clamped = percent.clamp(0.0, 100.0);
    ((clamped / 10.0).floor() as u32 * 10).min(100)
}

/// Why a pair of thresholds cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThresholdError(pub String);

impl std::fmt::Display for ThresholdError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Whether the kernel will take this pair.
///
/// A start at or above the end is rejected outright: the kernel refuses it,
/// and some firmware takes the first write and then silently ignores the
/// second, leaving the machine limited to a number nobody asked for.
pub fn validate(start: u8, end: u8) -> Result<(), ThresholdError> {
    if end > 100 {
        return Err(ThresholdError(format!("stop {end}% must be at most 100%")));
    }
    if start >= end {
        return Err(ThresholdError(format!(
            "start {start}% must be below stop {end}%"
        )));
    }
    Ok(())
}

/// One file to write, and what to put in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Write {
    /// The threshold file.
    pub path: PathBuf,
    /// The percentage to write into it.
    pub value: u8,
}

/// The order two threshold files have to be written in.
///
/// Raising the start above the *current* end would give the kernel a moment in
/// which start ≥ end, which it refuses. Writing the end first avoids the
/// window entirely; lowering the start is safe either way and goes first so
/// the limit tightens before it loosens.
pub fn ordered_writes(
    start_path: &std::path::Path,
    end_path: &std::path::Path,
    start: u8,
    end: u8,
    current_end: Option<u8>,
) -> Vec<Write> {
    let start_write = Write {
        path: start_path.to_path_buf(),
        value: start,
    };
    let end_write = Write {
        path: end_path.to_path_buf(),
        value: end,
    };
    if current_end.is_some_and(|current| start >= current) {
        vec![end_write, start_write]
    } else {
        vec![start_write, end_write]
    }
}

/// A duration in seconds, written the way the health card writes it.
///
/// Deliberately coarse: a battery estimate that claims to know the seconds is
/// claiming more than it can, and "2h 15m" is what the user is reading for.
pub fn duration(seconds: i64) -> Option<String> {
    if seconds <= 0 {
        return None;
    }
    let minutes = seconds / 60;
    let (hours, minutes) = (minutes / 60, minutes % 60);
    Some(match (hours, minutes) {
        (0, 0) => "less than a minute".to_string(),
        (0, minutes) => format!("{minutes}m"),
        (hours, 0) => format!("{hours}h"),
        (hours, minutes) => format!("{hours}h {minutes}m"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upower_states_map_to_names() {
        assert_eq!(BatteryStatus::from_upower(1), BatteryStatus::Charging);
        assert_eq!(BatteryStatus::from_upower(2), BatteryStatus::Discharging);
        assert_eq!(BatteryStatus::from_upower(4), BatteryStatus::Full);
        assert_eq!(BatteryStatus::from_upower(5), BatteryStatus::PendingCharge);
        assert_eq!(BatteryStatus::from_upower(99), BatteryStatus::Unknown);
    }

    #[test]
    fn the_icon_table_matches_adwaita() {
        // Floored to tens, exactly as GNOME Shell does it.
        let table = [
            (0.0, "battery-level-0-symbolic"),
            (9.9, "battery-level-0-symbolic"),
            (10.0, "battery-level-10-symbolic"),
            (55.0, "battery-level-50-symbolic"),
            (99.0, "battery-level-90-symbolic"),
            (100.0, "battery-level-100-symbolic"),
        ];
        for (percent, expected) in table {
            assert_eq!(
                icon(Some(percent), BatteryStatus::Discharging),
                expected,
                "{percent}%"
            );
        }
    }

    #[test]
    fn charging_has_its_own_variant() {
        assert_eq!(
            icon(Some(42.0), BatteryStatus::Charging),
            "battery-level-40-charging-symbolic"
        );
        assert_eq!(
            icon(Some(42.0), BatteryStatus::PendingCharge),
            "battery-level-40-charging-symbolic",
            "a limited battery on mains still reads as plugged in"
        );
    }

    #[test]
    fn a_full_battery_has_its_own_icon_whatever_the_reading_says() {
        assert_eq!(
            icon(Some(97.0), BatteryStatus::Full),
            "battery-level-100-charged-symbolic"
        );
    }

    #[test]
    fn a_hundred_percent_on_a_cable_is_charged_rather_than_a_missing_icon() {
        // Adwaita ships no `battery-level-100-charging-symbolic`, so asking for
        // it draws the broken-image glyph. The headset widget reaches this the
        // moment a headset finishes charging on its dock.
        assert_eq!(
            icon(Some(100.0), BatteryStatus::Charging),
            "battery-level-100-charged-symbolic"
        );
        assert_eq!(
            icon(Some(99.0), BatteryStatus::Charging),
            "battery-level-90-charging-symbolic",
            "and everything below it still charges"
        );
    }

    #[test]
    fn an_unknown_charge_is_missing_rather_than_empty() {
        assert_eq!(
            icon(None, BatteryStatus::Unknown),
            "battery-missing-symbolic",
            "an empty outline would claim the battery is flat"
        );
    }

    #[test]
    fn a_percentage_outside_its_range_still_names_a_real_icon() {
        assert_eq!(
            icon(Some(140.0), BatteryStatus::Discharging),
            "battery-level-100-symbolic"
        );
        assert_eq!(
            icon(Some(-3.0), BatteryStatus::Discharging),
            "battery-level-0-symbolic"
        );
    }

    #[test]
    fn only_a_discharging_battery_reads_as_low() {
        let low = BatteryState {
            available: true,
            percent: Some(15.0),
            status: BatteryStatus::Discharging,
            ..BatteryState::default()
        };
        assert!(low.is_low());

        let charging = BatteryState {
            status: BatteryStatus::Charging,
            ..low.clone()
        };
        assert!(!charging.is_low(), "a battery on mains is not in trouble");

        let comfortable = BatteryState {
            percent: Some(21.0),
            ..low.clone()
        };
        assert!(!comfortable.is_low());

        let boundary = BatteryState {
            percent: Some(LOW_PERCENT),
            ..low
        };
        assert!(boundary.is_low(), "twenty per cent is already low");
    }

    #[test]
    fn thresholds_have_to_be_the_right_way_round() {
        assert!(validate(75, 80).is_ok());
        assert!(validate(0, 100).is_ok());
        assert!(validate(80, 80).is_err());
        assert!(validate(90, 80).is_err());
        assert!(validate(10, 101).is_err());
    }

    #[test]
    fn a_rejected_pair_says_why() {
        let error = validate(90, 80).expect_err("crossed");
        assert!(error.to_string().contains("below"));
    }

    #[test]
    fn raising_the_start_past_the_current_limit_writes_the_end_first() {
        let writes = ordered_writes(
            std::path::Path::new("start"),
            std::path::Path::new("end"),
            96,
            100,
            Some(80),
        );
        assert_eq!(writes[0].path, PathBuf::from("end"));
        assert_eq!(writes[0].value, 100);
        assert_eq!(writes[1].value, 96);
    }

    #[test]
    fn lowering_the_start_writes_it_first() {
        let writes = ordered_writes(
            std::path::Path::new("start"),
            std::path::Path::new("end"),
            75,
            80,
            Some(100),
        );
        assert_eq!(writes[0].path, PathBuf::from("start"));
        assert_eq!(writes[1].path, PathBuf::from("end"));
    }

    #[test]
    fn with_no_current_limit_the_start_goes_first() {
        let writes = ordered_writes(
            std::path::Path::new("start"),
            std::path::Path::new("end"),
            75,
            80,
            None,
        );
        assert_eq!(writes[0].path, PathBuf::from("start"));
    }

    #[test]
    fn a_limit_of_a_hundred_is_not_a_limit() {
        assert!(
            !Thresholds {
                start: 96,
                end: 100,
                writable: true
            }
            .limited()
        );
        assert!(
            Thresholds {
                start: 75,
                end: 80,
                writable: true
            }
            .limited()
        );
    }

    #[test]
    fn the_limit_can_only_be_changed_where_something_will_take_it() {
        let unwritable = BatteryState {
            available: true,
            thresholds: Some(Thresholds {
                start: 75,
                end: 80,
                writable: false,
            }),
            ..BatteryState::default()
        };
        assert!(
            !unwritable.can_set_thresholds(),
            "root-owned files and no UPower is a card that explains itself"
        );

        let through_upower = BatteryState {
            upower_thresholds: true,
            ..unwritable.clone()
        };
        assert!(through_upower.can_set_thresholds());

        let writable = BatteryState {
            thresholds: Some(Thresholds {
                start: 75,
                end: 80,
                writable: true,
            }),
            ..unwritable
        };
        assert!(writable.can_set_thresholds());
    }

    #[test]
    fn durations_are_written_the_way_a_person_reads_them() {
        assert_eq!(duration(8100).as_deref(), Some("2h 15m"));
        assert_eq!(duration(7200).as_deref(), Some("2h"));
        assert_eq!(duration(900).as_deref(), Some("15m"));
        assert_eq!(duration(30).as_deref(), Some("less than a minute"));
        assert_eq!(duration(0), None, "no estimate is not an estimate of zero");
        assert_eq!(duration(-5), None);
    }

    #[test]
    fn the_percentage_on_the_pill_is_rounded_not_truncated() {
        let state = BatteryState {
            percent: Some(85.6),
            ..BatteryState::default()
        };
        assert_eq!(state.rounded_percent(), Some(86));
    }

    #[test]
    fn nothing_is_reported_before_upower_answers() {
        let state = BatteryState::default();
        assert!(!state.available);
        assert_eq!(state.percent, None);
        assert_eq!(state.status, BatteryStatus::Unknown);
        assert!(!state.can_set_thresholds());
        assert_eq!(state.icon(), "battery-missing-symbolic");
    }
}
