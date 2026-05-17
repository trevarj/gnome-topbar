//! Native headset status widget.
//!
//! This starts with the same script-backed contract as the custom widget,
//! giving headsetcontrol and similar tools a stable first-class widget name.

use gnome_topbar_core::config::WidgetEntry;

use crate::styles::widget as wgt;
use crate::widgets::custom::{CustomConfig, CustomWidget};
use crate::widgets::{WidgetConfig, warn_unknown_options};

const KNOWN_OPTIONS: &[&str] = &[
    "icon",
    "label",
    "exec",
    "template",
    "interval",
    "on_click",
    "tooltip",
    "max_chars",
];

/// Configuration for the native headset widget.
#[derive(Debug, Clone)]
pub struct HeadsetConfig {
    custom: CustomConfig,
}

impl WidgetConfig for HeadsetConfig {
    fn from_entry(entry: &WidgetEntry) -> Self {
        warn_unknown_options("headset", entry, KNOWN_OPTIONS);

        let mut custom = CustomConfig::from_entry(entry);
        if custom.tooltip.is_none() {
            custom.tooltip = Some("Headset".to_string());
        }
        if custom.max_chars.is_none() {
            custom.max_chars = Some(12);
        }

        Self { custom }
    }
}

impl Default for HeadsetConfig {
    fn default() -> Self {
        Self {
            custom: CustomConfig {
                tooltip: Some("Headset".to_string()),
                max_chars: Some(12),
                ..Default::default()
            },
        }
    }
}

/// Bar widget for headset battery/status text.
pub struct HeadsetWidget {
    inner: CustomWidget,
}

impl HeadsetWidget {
    pub fn new(config: HeadsetConfig) -> Self {
        Self {
            inner: CustomWidget::new_with_class("headset", wgt::HEADSET, config.custom),
        }
    }

    pub fn widget(&self) -> &gtk4::Box {
        self.inner.widget()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use toml::Value;

    fn make_entry(options: HashMap<String, Value>) -> WidgetEntry {
        WidgetEntry {
            name: "headset".to_string(),
            options,
        }
    }

    #[test]
    fn headset_config_defaults() {
        let config = HeadsetConfig::from_entry(&make_entry(HashMap::new()));
        assert_eq!(config.custom.tooltip.as_deref(), Some("Headset"));
        assert_eq!(config.custom.max_chars, Some(12));
    }

    #[test]
    fn headset_config_accepts_script_options() {
        let mut options = HashMap::new();
        options.insert(
            "exec".to_string(),
            Value::String("headsetcontrol.sh".to_string()),
        );
        options.insert("interval".to_string(), Value::Integer(5));

        let config = HeadsetConfig::from_entry(&make_entry(options));
        assert_eq!(config.custom.exec.as_deref(), Some("headsetcontrol.sh"));
        assert_eq!(config.custom.interval, 5);
    }
}
