//! Shared battery formatting helpers for Quick Settings.

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

/// Return a symbolic icon name for the given battery level.
///
/// Returns names like "battery-full", "battery-high-charging", etc.
/// These are then resolved by `IconsService`.
pub fn battery_icon_name(percent: u8, charging: bool) -> String {
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

    if charging {
        format!("battery-{}-charging", level)
    } else {
        format!("battery-{}", level)
    }
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
        assert_eq!(battery_icon_name(95, false), "battery-full");
        assert_eq!(battery_icon_name(80, false), "battery-high");
        assert_eq!(battery_icon_name(60, false), "battery-medium-high");
        assert_eq!(battery_icon_name(40, false), "battery-medium");
        assert_eq!(battery_icon_name(25, false), "battery-medium-low");
        assert_eq!(battery_icon_name(10, false), "battery-low");
        assert_eq!(battery_icon_name(9, false), "battery-critical");
    }

    #[test]
    fn test_battery_icon_name_charging() {
        assert_eq!(battery_icon_name(95, true), "battery-full-charging");
        assert_eq!(battery_icon_name(60, true), "battery-medium-high-charging");
        assert_eq!(battery_icon_name(9, true), "battery-critical-charging");
    }
}
