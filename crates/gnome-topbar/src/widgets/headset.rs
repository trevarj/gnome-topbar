//! Built-in headset battery widget backed by headsetcontrol.

use std::cell::RefCell;
use std::process::{Command, Stdio};
use std::rc::Rc;

use gnome_topbar_core::config::WidgetEntry;
use gtk4::gio;
use gtk4::glib::{self, SourceId};
use gtk4::prelude::*;
use gtk4::{Box as GtkBox, Label};
use serde_json::Value;
use tracing::warn;

use crate::services::tooltip::TooltipManager;
use crate::styles::widget as wgt;
use crate::widgets::base::BaseWidget;
use crate::widgets::{WidgetConfig, warn_unknown_options};

const DEFAULT_INTERVAL_SECS: u64 = 5;
const KNOWN_OPTIONS: &[&str] = &["interval", "tooltip", "max_chars", "command"];

#[derive(Debug, Clone)]
pub struct HeadsetConfig {
    interval: u64,
    tooltip: String,
    max_chars: Option<usize>,
    command: String,
}

impl Default for HeadsetConfig {
    fn default() -> Self {
        Self {
            interval: DEFAULT_INTERVAL_SECS,
            tooltip: "Headset battery".to_string(),
            max_chars: None,
            command: "headsetcontrol".to_string(),
        }
    }
}

impl WidgetConfig for HeadsetConfig {
    fn from_entry(entry: &WidgetEntry) -> Self {
        warn_unknown_options("headset", entry, KNOWN_OPTIONS);
        let default = Self::default();
        let interval = entry
            .options
            .get("interval")
            .and_then(|v| v.as_integer())
            .filter(|v| *v >= 0)
            .map(|v| v as u64)
            .unwrap_or(default.interval);
        let tooltip = entry
            .options
            .get("tooltip")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(&default.tooltip)
            .to_string();
        let max_chars = entry
            .options
            .get("max_chars")
            .and_then(|v| v.as_integer())
            .filter(|v| *v > 0)
            .map(|v| v as usize);
        let command = entry
            .options
            .get("command")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(&default.command)
            .to_string();

        Self {
            interval,
            tooltip,
            max_chars,
            command,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HeadsetDisplay {
    text: String,
    tooltip: String,
    percentage: u8,
}

pub struct HeadsetWidget {
    base: BaseWidget,
    _timer: Rc<RefCell<Option<SourceId>>>,
}

impl HeadsetWidget {
    pub fn new(config: HeadsetConfig) -> Self {
        let base = BaseWidget::new(&[wgt::HEADSET]);
        base.set_tooltip(&config.tooltip);
        let label = base.add_label(None, &[wgt::HEADSET]);
        label.set_xalign(0.5);
        base.widget().set_visible(false);

        let timer = Rc::new(RefCell::new(None));
        refresh_headset_label(base.widget(), &label, &config);

        if config.interval > 0 {
            let root_for_timer = base.widget().clone();
            let label_for_timer = label.clone();
            let config_for_timer = config.clone();
            let source = glib::timeout_add_seconds_local(config.interval as u32, move || {
                refresh_headset_label(&root_for_timer, &label_for_timer, &config_for_timer);
                glib::ControlFlow::Continue
            });
            *timer.borrow_mut() = Some(source);
        }

        Self {
            base,
            _timer: timer,
        }
    }

    pub fn widget(&self) -> &GtkBox {
        self.base.widget()
    }
}

fn refresh_headset_label(root: &GtkBox, label: &Label, config: &HeadsetConfig) {
    let root = root.clone();
    let label = label.clone();
    let config = config.clone();
    let max_chars = config.max_chars;
    glib::spawn_future_local(async move {
        let result = gio::spawn_blocking(move || fetch_headset_display(&config)).await;
        match result {
            Ok(Some(display)) => {
                label.set_label(&truncate_label(&display.text, max_chars));
                TooltipManager::global().set_styled_tooltip(&root, &display.tooltip);
                root.set_visible(true);
            }
            Ok(None) => {
                label.set_label("");
                root.set_visible(false);
            }
            Err(err) => {
                warn!("headset update failed: {:?}", err);
                root.set_visible(false);
            }
        }
    });
}

impl Drop for HeadsetWidget {
    fn drop(&mut self) {
        if let Some(source_id) = self._timer.borrow_mut().take() {
            source_id.remove();
        }
    }
}

fn fetch_headset_display(config: &HeadsetConfig) -> Option<HeadsetDisplay> {
    let output = Command::new(&config.command)
        // Request a live battery read; plain JSON output only reports capabilities.
        .args(["-b", "-o", "json"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_headsetcontrol_output(&output.stdout)
}

fn parse_headsetcontrol_output(bytes: &[u8]) -> Option<HeadsetDisplay> {
    let value: Value = serde_json::from_slice(bytes).ok()?;
    let devices = value.get("devices")?.as_array()?;

    for device in devices {
        let status = device.get("status").and_then(Value::as_str);
        if status != Some("success") {
            continue;
        }

        let battery = device.get("battery")?;
        if battery.get("status").and_then(Value::as_str) == Some("BATTERY_UNAVAILABLE") {
            continue;
        }

        let percentage = battery
            .get("level")
            .and_then(Value::as_f64)
            .map(|level| level.floor().clamp(0.0, 100.0) as u8)
            .filter(|level| *level > 0)?;

        let device_name = device
            .get("device")
            .or_else(|| device.get("product"))
            .or_else(|| device.get("vendor"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());

        let tooltip = device_name
            .map(|name| format!("{name}: {percentage}%"))
            .unwrap_or_else(|| format!("{percentage}%"));

        return Some(HeadsetDisplay {
            text: format!("󰋎 {}", headset_battery_icon(percentage)),
            tooltip,
            percentage,
        });
    }

    None
}

fn headset_battery_icon(percentage: u8) -> &'static str {
    match percentage {
        0 => "",
        1..=25 => "",
        26..=50 => "",
        51..=75 => "",
        _ => "",
    }
}

fn truncate_label(text: &str, max_chars: Option<usize>) -> String {
    let Some(max_chars) = max_chars else {
        return text.to_string();
    };
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    text.chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>()
        + "…"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_first_available_headset() {
        let raw = br#"{
          "devices": [
            {"status": "success", "battery": {"status": "BATTERY_UNAVAILABLE", "level": 0}},
            {"status": "success", "device": "Arctis Nova", "battery": {"status": "BATTERY_AVAILABLE", "level": 72.8}}
          ]
        }"#;
        let display = parse_headsetcontrol_output(raw).unwrap();
        assert_eq!(display.text, "󰋎 ");
        assert_eq!(display.tooltip, "Arctis Nova: 72%");
        assert_eq!(display.percentage, 72);
    }

    #[test]
    fn hides_when_no_battery_available() {
        let raw = br#"{"devices":[{"status":"success","battery":{"status":"BATTERY_UNAVAILABLE","level":0}}]}"#;
        assert_eq!(parse_headsetcontrol_output(raw), None);
    }

    #[test]
    fn hides_when_json_has_capabilities_but_no_battery_reading() {
        let raw = br#"{
          "devices": [
            {
              "status": "success",
              "device": "Arctis Nova",
              "capabilities": ["CAP_BATTERY_STATUS"],
              "capabilities_str": ["battery"]
            }
          ]
        }"#;
        assert_eq!(parse_headsetcontrol_output(raw), None);
    }

    #[test]
    fn battery_icons_match_existing_script_thresholds() {
        assert_eq!(headset_battery_icon(0), "");
        assert_eq!(headset_battery_icon(1), "");
        assert_eq!(headset_battery_icon(25), "");
        assert_eq!(headset_battery_icon(26), "");
        assert_eq!(headset_battery_icon(51), "");
        assert_eq!(headset_battery_icon(76), "");
        assert_eq!(headset_battery_icon(100), "");
    }
}
