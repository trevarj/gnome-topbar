//! Shared battery formatting helpers for Quick Settings.

use crate::services::battery::{
    BatterySnapshot, STATE_CHARGING, STATE_DISCHARGING, STATE_FULLY_CHARGED, STATE_PENDING_CHARGE,
    STATE_PENDING_DISCHARGE,
};

const IDLE_CHARGE_RATE_WATTS: f64 = 0.1;

/// Round a floating-point percentage (0.0 - 100.0) to a u8, clamped.
///
/// NaN is treated as 0; infinities are clamped to the 0-100 range.
pub fn rounded_pct_value(percent: f64) -> u8 {
    if percent.is_nan() {
        return 0;
    }
    let clamped = percent.clamp(0.0, 100.0);
    clamped.round() as u8
}

/// Format a rounded percentage value as readable text, e.g. "57%".
pub fn readable_pct(percent: u8) -> String {
    format!("{}%", percent)
}

/// User-facing power state used for icon and tooltip selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatteryDisplayState {
    Charging,
    Discharging,
    FullyCharged,
    PluggedNotCharging,
    Unknown,
}

/// Return a symbolic icon name for the given battery level.
///
/// Returns names like "battery-full", "battery-high-charging",
/// "battery-full-charged", or "battery-plugged".
/// These are then resolved by `IconsService`.
pub fn battery_icon_name(percent: u8, state: BatteryDisplayState) -> String {
    if state == BatteryDisplayState::PluggedNotCharging {
        return "battery-plugged".to_string();
    }

    let level = if percent >= 95 {
        "full"
    } else if percent >= 80 {
        "high"
    } else if percent >= 60 {
        "medium-high"
    } else if percent >= 40 {
        "medium"
    } else if percent >= 25 {
        "medium-low"
    } else if percent >= 10 {
        "low"
    } else {
        "critical"
    };

    match state {
        BatteryDisplayState::Charging => format!("battery-{}-charging", level),
        BatteryDisplayState::FullyCharged => "battery-full-charged".to_string(),
        BatteryDisplayState::Discharging | BatteryDisplayState::Unknown => {
            format!("battery-{}", level)
        }
        BatteryDisplayState::PluggedNotCharging => unreachable!("handled before level selection"),
    }
}

pub fn battery_state_text(state: BatteryDisplayState) -> &'static str {
    match state {
        BatteryDisplayState::Charging => "Charging",
        BatteryDisplayState::Discharging => "Discharging",
        BatteryDisplayState::FullyCharged => "Full",
        BatteryDisplayState::PluggedNotCharging => "Plugged in, not charging",
        BatteryDisplayState::Unknown => "Unknown",
    }
}

/// Translate a raw UPower state code into display-oriented battery state.
pub fn battery_display_state(state: Option<u32>) -> BatteryDisplayState {
    match state {
        Some(STATE_CHARGING) => BatteryDisplayState::Charging,
        Some(STATE_DISCHARGING) => BatteryDisplayState::Discharging,
        Some(STATE_FULLY_CHARGED) => BatteryDisplayState::FullyCharged,
        Some(STATE_PENDING_CHARGE) | Some(STATE_PENDING_DISCHARGE) => {
            BatteryDisplayState::PluggedNotCharging
        }
        _ => BatteryDisplayState::Unknown,
    }
}

/// Derive display state from the full snapshot, correcting UPower's occasional
/// "charging" report when AC is connected but firmware charge limits are idling.
pub fn battery_display_state_from_snapshot(snapshot: &BatterySnapshot) -> BatteryDisplayState {
    let state = battery_display_state(snapshot.state);
    if state == BatteryDisplayState::Charging && appears_plugged_not_charging(snapshot) {
        return BatteryDisplayState::PluggedNotCharging;
    }
    state
}

fn appears_plugged_not_charging(snapshot: &BatterySnapshot) -> bool {
    snapshot.ac_online == Some(true)
        && snapshot.health_limit_active()
        && snapshot
            .energy_rate
            .is_some_and(|rate| rate.abs() <= IDLE_CHARGE_RATE_WATTS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rounded_pct_value_basic() {
        assert_eq!(rounded_pct_value(0.0), 0);
        assert_eq!(rounded_pct_value(12.4), 12);
        assert_eq!(rounded_pct_value(12.5), 13);
        assert_eq!(rounded_pct_value(100.0), 100);
    }

    #[test]
    fn test_rounded_pct_value_clamps() {
        assert_eq!(rounded_pct_value(-5.0), 0);
        assert_eq!(rounded_pct_value(250.0), 100);
        assert_eq!(rounded_pct_value(f64::NAN), 0);
    }

    #[test]
    fn test_readable_pct() {
        assert_eq!(readable_pct(0), "0%");
        assert_eq!(readable_pct(57), "57%");
        assert_eq!(readable_pct(100), "100%");
    }

    #[test]
    fn test_battery_icon_name_thresholds() {
        assert_eq!(
            battery_icon_name(95, BatteryDisplayState::Discharging),
            "battery-full"
        );
        assert_eq!(
            battery_icon_name(80, BatteryDisplayState::Discharging),
            "battery-high"
        );
        assert_eq!(
            battery_icon_name(60, BatteryDisplayState::Discharging),
            "battery-medium-high"
        );
        assert_eq!(
            battery_icon_name(40, BatteryDisplayState::Discharging),
            "battery-medium"
        );
        assert_eq!(
            battery_icon_name(25, BatteryDisplayState::Discharging),
            "battery-medium-low"
        );
        assert_eq!(
            battery_icon_name(10, BatteryDisplayState::Discharging),
            "battery-low"
        );
        assert_eq!(
            battery_icon_name(9, BatteryDisplayState::Discharging),
            "battery-critical"
        );
    }

    #[test]
    fn test_battery_icon_name_charging() {
        assert_eq!(
            battery_icon_name(95, BatteryDisplayState::Charging),
            "battery-full-charging"
        );
        assert_eq!(
            battery_icon_name(60, BatteryDisplayState::Charging),
            "battery-medium-high-charging"
        );
        assert_eq!(
            battery_icon_name(9, BatteryDisplayState::Charging),
            "battery-critical-charging"
        );
    }

    #[test]
    fn test_battery_icon_name_plugged_states() {
        assert_eq!(
            battery_icon_name(100, BatteryDisplayState::FullyCharged),
            "battery-full-charged"
        );
        assert_eq!(
            battery_icon_name(57, BatteryDisplayState::PluggedNotCharging),
            "battery-plugged"
        );
    }

    #[test]
    fn test_battery_state_text() {
        assert_eq!(
            battery_state_text(BatteryDisplayState::Charging),
            "Charging"
        );
        assert_eq!(
            battery_state_text(BatteryDisplayState::PluggedNotCharging),
            "Plugged in, not charging"
        );
    }

    #[test]
    fn test_battery_display_state_from_upower_state() {
        assert_eq!(
            battery_display_state(Some(STATE_CHARGING)),
            BatteryDisplayState::Charging
        );
        assert_eq!(
            battery_display_state(Some(STATE_DISCHARGING)),
            BatteryDisplayState::Discharging
        );
        assert_eq!(
            battery_display_state(Some(STATE_FULLY_CHARGED)),
            BatteryDisplayState::FullyCharged
        );
        assert_eq!(
            battery_display_state(Some(STATE_PENDING_CHARGE)),
            BatteryDisplayState::PluggedNotCharging
        );
        assert_eq!(
            battery_display_state(Some(STATE_PENDING_DISCHARGE)),
            BatteryDisplayState::PluggedNotCharging
        );
        assert_eq!(battery_display_state(None), BatteryDisplayState::Unknown);
        assert_eq!(battery_display_state(Some(0)), BatteryDisplayState::Unknown);
    }

    #[test]
    fn test_battery_display_state_from_snapshot_detects_charge_limit_idle() {
        let mut snapshot = BatterySnapshot {
            available: true,
            state: Some(STATE_CHARGING),
            ac_online: Some(true),
            energy_rate: Some(0.0),
            charge_stop_threshold: Some(80),
            ..BatterySnapshot::unknown()
        };

        assert_eq!(
            battery_display_state_from_snapshot(&snapshot),
            BatteryDisplayState::PluggedNotCharging
        );

        snapshot.energy_rate = Some(6.5);
        assert_eq!(
            battery_display_state_from_snapshot(&snapshot),
            BatteryDisplayState::Charging
        );
    }
}
