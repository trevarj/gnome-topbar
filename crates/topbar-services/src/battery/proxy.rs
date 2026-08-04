//! The UPower interface, trimmed to what the panel reads.

/// The bus name UPower owns.
pub(crate) const BUS_NAME: &str = "org.freedesktop.UPower";
/// The composite device UPower publishes for "the battery", whatever the
/// machine actually has in it.
pub(crate) const DISPLAY_DEVICE: &str = "/org/freedesktop/UPower/devices/DisplayDevice";
/// Where the per-battery devices live.
pub(crate) const DEVICES: &str = "/org/freedesktop/UPower/devices";

/// A UPower device.
///
/// Property caching is deliberately left off where this is built: a battery
/// moves once a minute, the whole snapshot is four small reads, and a cache
/// that has to be invalidated is a cache that can be wrong.
#[zbus::proxy(
    interface = "org.freedesktop.UPower.Device",
    default_service = "org.freedesktop.UPower"
)]
pub(crate) trait Device {
    /// Charge, 0–100.
    #[zbus(property)]
    fn percentage(&self) -> zbus::Result<f64>;

    /// What the battery is doing. See `BatteryStatus::from_upower`.
    #[zbus(property)]
    fn state(&self) -> zbus::Result<u32>;

    /// Whether there is a battery at all.
    #[zbus(property)]
    fn is_present(&self) -> zbus::Result<bool>;

    /// Seconds until flat, or zero when UPower has no estimate.
    #[zbus(property)]
    fn time_to_empty(&self) -> zbus::Result<i64>;

    /// Seconds until full, likewise.
    #[zbus(property)]
    fn time_to_full(&self) -> zbus::Result<i64>;

    /// Where charging resumes, when the firmware exposes a limit.
    #[zbus(property)]
    fn charge_start_threshold(&self) -> zbus::Result<u32>;

    /// Where charging stops.
    #[zbus(property)]
    fn charge_end_threshold(&self) -> zbus::Result<u32>;

    /// Whether UPower can drive the limit on this machine.
    #[zbus(property)]
    fn charge_threshold_supported(&self) -> zbus::Result<bool>;

    /// Turn the firmware's charge limit on or off.
    ///
    /// UPower takes a flag rather than a pair of percentages: it owns the
    /// numbers, and the panel's two presets map onto "limited" and "full".
    fn enable_charge_threshold(&self, enable: bool) -> zbus::Result<()>;
}

/// The UPower object path for a sysfs battery directory.
///
/// `/sys/class/power_supply/BAT0` → `…/devices/battery_BAT0`. Everything that
/// is not alphanumeric or an underscore becomes an underscore, which is
/// UPower's own escaping.
pub(crate) fn device_path(battery: &std::path::Path) -> Option<String> {
    let name = battery.file_name()?.to_string_lossy();
    let escaped: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect();
    Some(format!("{DEVICES}/battery_{escaped}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn a_battery_directory_maps_to_its_upower_object() {
        assert_eq!(
            device_path(Path::new("/sys/class/power_supply/BAT0")).as_deref(),
            Some("/org/freedesktop/UPower/devices/battery_BAT0")
        );
    }

    #[test]
    fn awkward_names_are_escaped_the_way_upower_escapes_them() {
        assert_eq!(
            device_path(Path::new("/sys/class/power_supply/BAT-0")).as_deref(),
            Some("/org/freedesktop/UPower/devices/battery_BAT_0")
        );
    }

    #[test]
    fn a_path_with_no_name_has_no_object() {
        assert_eq!(device_path(Path::new("/")), None);
    }
}
