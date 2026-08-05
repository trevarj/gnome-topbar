//! What `headsetcontrol` says, and what of it is worth drawing.
//!
//! Pure, and tested against real output: the JSON below is what the tool
//! actually printed on the machine this panel is written for, including the
//! shapes that mean "there is nothing to report".

use serde::Deserialize;

/// A headset battery reading worth putting on a bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadsetReading {
    /// What the headset calls itself, for the tooltip.
    pub name: Option<String>,
    /// Charge, 0–100.
    pub percent: u8,
    /// Whether it is on its dock or its cable.
    pub charging: bool,
}

impl HeadsetReading {
    /// The tooltip: what it is, how full it is, and what it is doing.
    pub fn tooltip(&self) -> String {
        let what = if self.charging {
            "charging"
        } else {
            "discharging"
        };
        match &self.name {
            Some(name) => format!("{name}\n{}% · {what}", self.percent),
            None => format!("Headset\n{}% · {what}", self.percent),
        }
    }
}

/// Below this, the reading is drawn urgent.
pub const URGENT_PERCENT: u8 = 10;
/// Below this, it is drawn as a warning.
pub const WARNING_PERCENT: u8 = 25;

/// The top level of `headsetcontrol -o json`.
#[derive(Debug, Deserialize)]
struct Output {
    /// One entry per headset the tool found.
    #[serde(default)]
    devices: Vec<Device>,
}

/// One headset.
#[derive(Debug, Deserialize)]
struct Device {
    /// `"success"` when the tool could talk to it.
    #[serde(default)]
    status: String,
    /// Its name. Older versions call the same thing `product`.
    #[serde(default)]
    device: Option<String>,
    /// What the model is called.
    #[serde(default)]
    product: Option<String>,
    /// Who made it.
    #[serde(default)]
    vendor: Option<String>,
    /// The battery, present only once a reading has actually been taken.
    #[serde(default)]
    battery: Option<Battery>,
}

/// A device's battery block.
#[derive(Debug, Deserialize)]
struct Battery {
    /// `BATTERY_AVAILABLE`, `BATTERY_CHARGING` or `BATTERY_UNAVAILABLE`.
    #[serde(default)]
    status: String,
    /// The charge, as a number the tool sometimes writes with a decimal point.
    #[serde(default)]
    level: Option<f64>,
}

/// The first headset with a battery reading in it, if there is one.
///
/// Three shapes mean "draw nothing", and all three are normal rather than
/// exceptional — which is why they are a `None` and not an error:
///
/// - **no devices at all**, because the headset is switched off or unplugged;
/// - **`BATTERY_UNAVAILABLE`**, because it is connected but asleep;
/// - **capabilities but no `battery` block**, because the tool listed what the
///   device *can* do without being asked for a reading.
///
/// Devices are skipped rather than abandoned: a dongle that reports two
/// interfaces, one of them mute, must not hide the one that answers. v1
/// returned `None` from the whole function the moment a device had a battery
/// block with no `level` in it, which is that bug.
pub fn parse(json: &str) -> Option<HeadsetReading> {
    let output: Output = serde_json::from_str(json).ok()?;

    for device in output.devices {
        if device.status != "success" {
            continue;
        }
        let Some(battery) = device.battery else {
            continue;
        };
        if battery.status == "BATTERY_UNAVAILABLE" {
            continue;
        }
        let Some(level) = battery.level else {
            continue;
        };

        return Some(HeadsetReading {
            name: device
                .device
                .or(device.product)
                .or(device.vendor)
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty()),
            percent: level.floor().clamp(0.0, 100.0) as u8,
            charging: battery.status == "BATTERY_CHARGING",
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the tool prints with nothing plugged in — captured from the real
    /// binary on the machine this was written on.
    const NOTHING: &str = r#"{
      "name": "HeadsetControl",
      "api_version": "1.3",
      "device_count": 0,
      "devices": []
    }"#;

    #[test]
    fn the_first_device_with_a_reading_wins() {
        let raw = r#"{
          "devices": [
            {"status": "success", "battery": {"status": "BATTERY_UNAVAILABLE", "level": 0}},
            {"status": "success", "device": "Arctis Nova", "battery": {"status": "BATTERY_AVAILABLE", "level": 72.8}}
          ]
        }"#;
        assert_eq!(
            parse(raw),
            Some(HeadsetReading {
                name: Some("Arctis Nova".to_string()),
                percent: 72,
                charging: false,
            })
        );
    }

    #[test]
    fn a_battery_that_is_not_available_is_not_a_reading() {
        let raw = r#"{"devices":[{"status":"success","battery":{"status":"BATTERY_UNAVAILABLE","level":0}}]}"#;
        assert_eq!(parse(raw), None);
    }

    #[test]
    fn capabilities_without_a_reading_are_not_a_reading() {
        let raw = r#"{
          "devices": [
            {
              "status": "success",
              "device": "Arctis Nova",
              "capabilities": ["CAP_BATTERY_STATUS"],
              "capabilities_str": ["battery"]
            }
          ]
        }"#;
        assert_eq!(parse(raw), None);
    }

    #[test]
    fn no_headset_at_all_is_not_a_reading() {
        assert_eq!(parse(NOTHING), None);
    }

    #[test]
    fn a_device_the_tool_could_not_talk_to_is_skipped() {
        let raw = r#"{
          "devices": [
            {"status": "failure", "device": "Broken", "battery": {"status": "BATTERY_AVAILABLE", "level": 50}},
            {"status": "success", "device": "Working", "battery": {"status": "BATTERY_AVAILABLE", "level": 30}}
          ]
        }"#;
        assert_eq!(
            parse(raw).expect("the working one").name.as_deref(),
            Some("Working")
        );
    }

    #[test]
    fn a_battery_block_with_no_level_does_not_abandon_the_rest() {
        // v1's bug: the `?` on `level` returned from the whole function, so a
        // dongle whose first interface answers without a number hid a headset
        // that was reporting perfectly well.
        let raw = r#"{
          "devices": [
            {"status": "success", "device": "Dongle", "battery": {"status": "BATTERY_AVAILABLE"}},
            {"status": "success", "device": "Headset", "battery": {"status": "BATTERY_AVAILABLE", "level": 45}}
          ]
        }"#;
        assert_eq!(parse(raw).expect("the headset").percent, 45);
    }

    #[test]
    fn charging_is_read_out_of_the_battery_status() {
        let raw = r#"{"devices":[{"status":"success","device":"Arctis","battery":{"status":"BATTERY_CHARGING","level":45}}]}"#;
        let reading = parse(raw).expect("a reading");
        assert!(reading.charging);
        assert_eq!(reading.tooltip(), "Arctis\n45% · charging");
    }

    #[test]
    fn a_flat_headset_is_a_reading_rather_than_an_absence() {
        // v1 dropped any level of zero, which meant a headset about to die
        // silently disappeared from the bar — exactly when it mattered.
        let raw = r#"{"devices":[{"status":"success","battery":{"status":"BATTERY_AVAILABLE","level":0}}]}"#;
        let reading = parse(raw).expect("zero is a reading");
        assert_eq!(reading.percent, 0);
        assert_eq!(reading.tooltip(), "Headset\n0% · discharging");
    }

    #[test]
    fn a_name_is_looked_for_in_all_three_places() {
        for (key, expected) in [("device", "A"), ("product", "A"), ("vendor", "A")] {
            let raw = format!(
                r#"{{"devices":[{{"status":"success","{key}":"A","battery":{{"status":"BATTERY_AVAILABLE","level":50}}}}]}}"#
            );
            assert_eq!(
                parse(&raw).expect("a reading").name.as_deref(),
                Some(expected)
            );
        }
    }

    #[test]
    fn a_nameless_headset_still_has_a_tooltip() {
        let raw = r#"{"devices":[{"status":"success","battery":{"status":"BATTERY_AVAILABLE","level":50}}]}"#;
        assert_eq!(
            parse(raw).expect("a reading").tooltip(),
            "Headset\n50% · discharging"
        );
    }

    #[test]
    fn a_level_above_a_hundred_is_clamped_rather_than_wrapped() {
        let raw = r#"{"devices":[{"status":"success","battery":{"status":"BATTERY_AVAILABLE","level":140}}]}"#;
        assert_eq!(parse(raw).expect("a reading").percent, 100);
    }

    #[test]
    fn output_that_is_not_json_at_all_is_not_a_reading() {
        assert_eq!(parse("headsetcontrol: no supported device found"), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn the_tint_thresholds_are_the_ones_the_widget_paints_with() {
        const { assert!(URGENT_PERCENT < WARNING_PERCENT) };
        assert_eq!(URGENT_PERCENT, 10);
        assert_eq!(WARNING_PERCENT, 25);
    }
}
