//! Native weather widget.
//!
//! The widget intentionally uses the same script contract as custom widgets so
//! existing Waybar-style weather scripts can move over without rewrites.

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
    "position",
];

/// Configuration for the native weather widget.
#[derive(Debug, Clone)]
pub struct WeatherConfig {
    custom: CustomConfig,
}

impl WidgetConfig for WeatherConfig {
    fn from_entry(entry: &WidgetEntry) -> Self {
        warn_unknown_options("weather", entry, KNOWN_OPTIONS);

        let mut custom = CustomConfig::from_entry(entry);
        if custom.label.is_empty() {
            custom.label = "Weather".to_string();
        }
        if custom.tooltip.is_none() {
            custom.tooltip = Some("Weather".to_string());
        }
        if custom.max_chars.is_none() {
            custom.max_chars = Some(24);
        }

        Self { custom }
    }
}

impl Default for WeatherConfig {
    fn default() -> Self {
        Self {
            custom: CustomConfig {
                label: "Weather".to_string(),
                tooltip: Some("Weather".to_string()),
                max_chars: Some(24),
                ..Default::default()
            },
        }
    }
}

/// Bar widget for weather summary text.
pub struct WeatherWidget {
    inner: CustomWidget,
}

impl WeatherWidget {
    pub fn new(config: WeatherConfig) -> Self {
        Self {
            inner: CustomWidget::new_with_class("weather", wgt::WEATHER, config.custom),
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
            name: "weather".to_string(),
            options,
        }
    }

    #[test]
    fn weather_config_defaults_to_clock_left() {
        let config = WeatherConfig::from_entry(&make_entry(HashMap::new()));
        assert_eq!(config.custom.label, "Weather");
        assert_eq!(config.custom.tooltip.as_deref(), Some("Weather"));
        assert_eq!(config.custom.max_chars, Some(24));
    }

    #[test]
    fn weather_config_accepts_script_options() {
        let mut options = HashMap::new();
        options.insert("exec".to_string(), Value::String("weather.sh".to_string()));
        options.insert("position".to_string(), Value::String("right".to_string()));
        options.insert("interval".to_string(), Value::Integer(600));

        let config = WeatherConfig::from_entry(&make_entry(options));
        assert_eq!(config.custom.exec.as_deref(), Some("weather.sh"));
        assert_eq!(config.custom.interval, 600);
    }
}
