//! Built-in Open-Meteo weather widget.

use std::cell::RefCell;
use std::rc::Rc;

use gnome_topbar_core::config::WidgetEntry;
use gtk4::gio;
use gtk4::glib::{self, SourceId};
use gtk4::prelude::*;
use gtk4::{Box as GtkBox, Label};
use serde::Deserialize;
use tracing::warn;

use crate::styles::widget as wgt;
use crate::widgets::base::BaseWidget;
use crate::widgets::{WidgetConfig, warn_unknown_options};

const DEFAULT_LATITUDE: f64 = 0.0;
const DEFAULT_LONGITUDE: f64 = 0.0;
const DEFAULT_INTERVAL_SECS: u64 = 1800;
const FALLBACK_ICON: &str = "󰨹";

const KNOWN_OPTIONS: &[&str] = &[
    "latitude",
    "longitude",
    "unit",
    "interval",
    "tooltip",
    "max_chars",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WeatherUnit {
    Celsius,
    Fahrenheit,
}

impl WeatherUnit {
    fn from_config(value: Option<&str>) -> Self {
        match value.unwrap_or("celsius").to_ascii_lowercase().as_str() {
            "f" | "fahrenheit" => Self::Fahrenheit,
            _ => Self::Celsius,
        }
    }

    fn api_value(self) -> &'static str {
        match self {
            Self::Celsius => "celsius",
            Self::Fahrenheit => "fahrenheit",
        }
    }

    fn symbol(self) -> &'static str {
        match self {
            Self::Celsius => "°C",
            Self::Fahrenheit => "°F",
        }
    }
}

#[derive(Debug, Clone)]
pub struct WeatherConfig {
    latitude: f64,
    longitude: f64,
    unit: WeatherUnit,
    interval: u64,
    tooltip: String,
    max_chars: Option<usize>,
}

impl Default for WeatherConfig {
    fn default() -> Self {
        Self {
            latitude: DEFAULT_LATITUDE,
            longitude: DEFAULT_LONGITUDE,
            unit: WeatherUnit::Celsius,
            interval: DEFAULT_INTERVAL_SECS,
            tooltip: "Weather".to_string(),
            max_chars: None,
        }
    }
}

impl WidgetConfig for WeatherConfig {
    fn from_entry(entry: &WidgetEntry) -> Self {
        warn_unknown_options("weather", entry, KNOWN_OPTIONS);
        let default = Self::default();
        let latitude = entry
            .options
            .get("latitude")
            .and_then(|v| v.as_float())
            .unwrap_or(default.latitude);
        let longitude = entry
            .options
            .get("longitude")
            .and_then(|v| v.as_float())
            .unwrap_or(default.longitude);
        let unit = WeatherUnit::from_config(entry.options.get("unit").and_then(|v| v.as_str()));
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

        Self {
            latitude,
            longitude,
            unit,
            interval,
            tooltip,
            max_chars,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WeatherDisplay {
    pub(crate) text: String,
}

pub struct WeatherWidget {
    base: BaseWidget,
    _timer: Rc<RefCell<Option<SourceId>>>,
}

impl WeatherWidget {
    pub fn new(config: WeatherConfig) -> Self {
        let base = BaseWidget::new(&[wgt::WEATHER]);
        base.set_tooltip(&config.tooltip);
        let label = base.add_label(Some(FALLBACK_ICON), &[wgt::WEATHER]);
        label.set_xalign(0.5);

        let timer = Rc::new(RefCell::new(None));
        refresh_weather_label(&label, &config);

        if config.interval > 0 {
            let label_for_timer = label.clone();
            let config_for_timer = config.clone();
            let source = glib::timeout_add_seconds_local(config.interval as u32, move || {
                refresh_weather_label(&label_for_timer, &config_for_timer);
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

fn refresh_weather_label(label: &Label, config: &WeatherConfig) {
    let label = label.clone();
    let config = config.clone();
    let max_chars = config.max_chars;
    glib::spawn_future_local(async move {
        let result = gio::spawn_blocking(move || fetch_weather_display(&config)).await;
        match result {
            Ok(display) => {
                label.set_label(&truncate_label(&display.text, max_chars));
                label.set_visible(true);
            }
            Err(err) => {
                warn!("weather update failed: {:?}", err);
                label.set_label(FALLBACK_ICON);
                label.set_visible(true);
            }
        }
    });
}

impl Drop for WeatherWidget {
    fn drop(&mut self) {
        if let Some(source_id) = self._timer.borrow_mut().take() {
            source_id.remove();
        }
    }
}

pub(crate) fn fetch_weather_display(config: &WeatherConfig) -> WeatherDisplay {
    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={}&longitude={}&current_weather=true&temperature_unit={}",
        config.latitude,
        config.longitude,
        config.unit.api_value()
    );

    let response = minreq::get(url)
        .with_timeout(8)
        .send()
        .ok()
        .and_then(|response| response.as_str().ok().map(str::to_string))
        .and_then(|body| serde_json::from_str::<OpenMeteoResponse>(&body).ok());

    let Some(weather) = response.map(|r| r.current_weather) else {
        return WeatherDisplay {
            text: FALLBACK_ICON.to_string(),
        };
    };

    let temp = weather.temperature.round() as i64;
    WeatherDisplay {
        text: format!(
            "{}   {}{}",
            weather_icon(weather.weathercode),
            temp,
            config.unit.symbol()
        ),
    }
}

#[derive(Debug, Deserialize)]
struct OpenMeteoResponse {
    current_weather: CurrentWeather,
}

#[derive(Debug, Deserialize)]
struct CurrentWeather {
    temperature: f64,
    weathercode: i64,
}

fn weather_icon(code: i64) -> &'static str {
    match code {
        0 => "󰖙",
        1 | 2 => "󰖕",
        3 => "󰖐",
        45 | 48 => "󰖑",
        51 | 53 | 55 | 61 | 63 | 65 => "󰖗",
        56 | 57 | 66 | 67 => "󰖒",
        71 | 73 | 75 | 77 | 85 | 86 => "󰼶",
        80..=82 => "󰖖",
        95 | 96 | 99 => "󰙾",
        _ => FALLBACK_ICON,
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
    fn weather_unit_parses_common_values() {
        assert_eq!(
            WeatherUnit::from_config(Some("fahrenheit")),
            WeatherUnit::Fahrenheit
        );
        assert_eq!(WeatherUnit::from_config(Some("F")), WeatherUnit::Fahrenheit);
        assert_eq!(
            WeatherUnit::from_config(Some("celsius")),
            WeatherUnit::Celsius
        );
        assert_eq!(WeatherUnit::from_config(Some("bad")), WeatherUnit::Celsius);
    }

    #[test]
    fn weather_icon_maps_open_meteo_codes() {
        assert_eq!(weather_icon(0), "󰖙");
        assert_eq!(weather_icon(63), "󰖗");
        assert_eq!(weather_icon(95), "󰙾");
        assert_eq!(weather_icon(999), FALLBACK_ICON);
    }

    #[test]
    fn truncate_label_keeps_short_text() {
        assert_eq!(truncate_label("abcdef", Some(10)), "abcdef");
        assert_eq!(truncate_label("abcdef", Some(4)), "abc…");
    }
}
