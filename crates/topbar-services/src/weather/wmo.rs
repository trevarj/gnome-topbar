//! WMO weather-interpretation codes, as words and as icons.
//!
//! Open-Meteo reports conditions as a WMO 4677 code. The wording is ported
//! verbatim from v1 — it was reviewed once and there is nothing to improve —
//! but the icons are not: v1 drew Nerd Font glyphs, and v2 uses the Adwaita
//! symbolic set like every other icon in the panel, so a code maps to an icon
//! *name* and the stylesheet tints it.
//!
//! Adwaita has no night variant for most conditions — clouds look the same
//! after dark — so only the two that do (`weather-clear`, `weather-few-clouds`)
//! take `is_day` into account. Every name here exists in
//! `adwaita-icon-theme`'s `symbolic/status`; the test at the bottom is what
//! stops a typo becoming a missing-icon square on the panel.

/// Shown for a code no version of this table knows.
const UNKNOWN_ICON: &str = "weather-severe-alert-symbolic";

/// The condition, in words. Ported from v1.
pub fn condition(code: u16) -> &'static str {
    match code {
        0 => "Clear",
        1 => "Mostly clear",
        2 => "Partly cloudy",
        3 => "Overcast",
        45 | 48 => "Fog",
        51 | 53 | 55 => "Drizzle",
        56 | 57 => "Freezing drizzle",
        61 | 63 | 65 => "Rain",
        66 | 67 => "Freezing rain",
        71 | 73 | 75 => "Snow",
        77 => "Snow grains",
        80..=82 => "Showers",
        85 | 86 => "Snow showers",
        95 => "Thunderstorm",
        96 | 99 => "Thunderstorm with hail",
        _ => "Weather unavailable",
    }
}

/// The Adwaita symbolic icon for a code, by daylight.
pub fn icon(code: u16, is_day: bool) -> &'static str {
    match code {
        0 => day_night("weather-clear", is_day),
        1 | 2 => day_night("weather-few-clouds", is_day),
        3 => "weather-overcast-symbolic",
        45 | 48 => "weather-fog-symbolic",
        // Drizzle and its freezing form are light and intermittent, which is
        // what the scattered-showers glyph says at 16px.
        51 | 53 | 55 | 56 | 57 => "weather-showers-scattered-symbolic",
        61 | 63 | 65 | 66 | 67 => "weather-showers-symbolic",
        71 | 73 | 75 | 77 | 85 | 86 => "weather-snow-symbolic",
        80 => "weather-showers-scattered-symbolic",
        81 | 82 => "weather-showers-symbolic",
        95 | 96 | 99 => "weather-storm-symbolic",
        _ => UNKNOWN_ICON,
    }
}

/// Pick the night variant of a name after dark.
fn day_night(base: &'static str, is_day: bool) -> &'static str {
    match (base, is_day) {
        ("weather-clear", false) => "weather-clear-night-symbolic",
        ("weather-clear", true) => "weather-clear-symbolic",
        ("weather-few-clouds", false) => "weather-few-clouds-night-symbolic",
        _ => "weather-few-clouds-symbolic",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every icon name this table can produce.
    const NAMES: &[&str] = &[
        "weather-clear-symbolic",
        "weather-clear-night-symbolic",
        "weather-few-clouds-symbolic",
        "weather-few-clouds-night-symbolic",
        "weather-overcast-symbolic",
        "weather-fog-symbolic",
        "weather-showers-scattered-symbolic",
        "weather-showers-symbolic",
        "weather-snow-symbolic",
        "weather-storm-symbolic",
        UNKNOWN_ICON,
    ];

    #[test]
    fn every_condition_v1_worded_is_worded_the_same_way() {
        assert_eq!(condition(0), "Clear");
        assert_eq!(condition(1), "Mostly clear");
        assert_eq!(condition(2), "Partly cloudy");
        assert_eq!(condition(3), "Overcast");
        assert_eq!(condition(45), "Fog");
        assert_eq!(condition(48), "Fog");
        assert_eq!(condition(53), "Drizzle");
        assert_eq!(condition(57), "Freezing drizzle");
        assert_eq!(condition(63), "Rain");
        assert_eq!(condition(67), "Freezing rain");
        assert_eq!(condition(75), "Snow");
        assert_eq!(condition(77), "Snow grains");
        assert_eq!(condition(81), "Showers");
        assert_eq!(condition(86), "Snow showers");
        assert_eq!(condition(95), "Thunderstorm");
        assert_eq!(condition(99), "Thunderstorm with hail");
        assert_eq!(condition(1234), "Weather unavailable");
    }

    #[test]
    fn each_bucket_of_codes_has_an_icon() {
        assert_eq!(icon(0, true), "weather-clear-symbolic");
        assert_eq!(icon(1, true), "weather-few-clouds-symbolic");
        assert_eq!(icon(2, true), "weather-few-clouds-symbolic");
        assert_eq!(icon(3, true), "weather-overcast-symbolic");
        assert_eq!(icon(45, true), "weather-fog-symbolic");
        assert_eq!(icon(48, true), "weather-fog-symbolic");
        assert_eq!(icon(51, true), "weather-showers-scattered-symbolic");
        assert_eq!(icon(56, true), "weather-showers-scattered-symbolic");
        assert_eq!(icon(63, true), "weather-showers-symbolic");
        assert_eq!(icon(66, true), "weather-showers-symbolic");
        assert_eq!(icon(73, true), "weather-snow-symbolic");
        assert_eq!(icon(77, true), "weather-snow-symbolic");
        assert_eq!(icon(80, true), "weather-showers-scattered-symbolic");
        assert_eq!(icon(82, true), "weather-showers-symbolic");
        assert_eq!(icon(86, true), "weather-snow-symbolic");
        assert_eq!(icon(95, true), "weather-storm-symbolic");
        assert_eq!(icon(99, true), "weather-storm-symbolic");
        assert_eq!(icon(4711, true), UNKNOWN_ICON);
    }

    #[test]
    fn only_the_two_conditions_adwaita_draws_twice_change_after_dark() {
        assert_eq!(icon(0, false), "weather-clear-night-symbolic");
        assert_eq!(icon(1, false), "weather-few-clouds-night-symbolic");
        assert_eq!(icon(2, false), "weather-few-clouds-night-symbolic");

        // Everything else looks the same whatever the hour.
        for code in [3, 45, 51, 61, 71, 80, 95, 4711] {
            assert_eq!(
                icon(code, true),
                icon(code, false),
                "code {code} should not have a night variant"
            );
        }
    }

    #[test]
    fn every_code_maps_to_a_name_the_icon_theme_has() {
        for code in 0..=120u16 {
            for is_day in [true, false] {
                let name = icon(code, is_day);
                assert!(
                    NAMES.contains(&name),
                    "code {code} produced an unlisted icon `{name}`"
                );
                assert!(name.ends_with("-symbolic"), "{name} is not symbolic");
            }
        }
    }

    #[test]
    fn every_code_has_words_for_it() {
        for code in 0..=120u16 {
            assert!(!condition(code).is_empty());
        }
    }
}
