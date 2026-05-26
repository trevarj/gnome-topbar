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

use crate::services::config_manager::ConfigManager;
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
    "forecast_days",
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

pub(crate) fn weather_config_from_widget_name(widget_name: &str) -> WeatherConfig {
    let config = ConfigManager::global();
    let default = WeatherConfig::default();
    let latitude = config
        .get_widget_option(widget_name, "latitude")
        .and_then(|v| v.as_float())
        .unwrap_or(default.latitude);
    let longitude = config
        .get_widget_option(widget_name, "longitude")
        .and_then(|v| v.as_float())
        .unwrap_or(default.longitude);
    let unit = WeatherUnit::from_config(
        config
            .get_widget_option(widget_name, "unit")
            .and_then(|v| v.as_str().map(str::to_string))
            .as_deref(),
    );
    let interval = config
        .get_widget_option(widget_name, "interval")
        .and_then(|v| v.as_integer())
        .filter(|v| *v >= 0)
        .map(|v| v as u64)
        .unwrap_or(default.interval);
    let tooltip = config
        .get_widget_option(widget_name, "tooltip")
        .and_then(|v| v.as_str().map(str::to_string))
        .filter(|s| !s.is_empty())
        .unwrap_or(default.tooltip);
    let max_chars = config
        .get_widget_option(widget_name, "max_chars")
        .and_then(|v| v.as_integer())
        .filter(|v| *v > 0)
        .map(|v| v as usize);

    WeatherConfig {
        latitude,
        longitude,
        unit,
        interval,
        tooltip,
        max_chars,
    }
}

pub(crate) fn forecast_days_from_widget_name(widget_name: &str) -> usize {
    ConfigManager::global()
        .get_widget_option(widget_name, "forecast_days")
        .and_then(|v| v.as_integer())
        .filter(|v| (3_i64..=5_i64).contains(v))
        .map(|v| v as usize)
        .unwrap_or(5)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WeatherDisplay {
    pub(crate) text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WeatherForecast {
    pub(crate) current: String,
    pub(crate) summary: String,
    pub(crate) days: Vec<WeatherForecastDay>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WeatherForecastDay {
    pub(crate) label: String,
    pub(crate) icon: &'static str,
    pub(crate) condition: &'static str,
    pub(crate) high: i64,
    pub(crate) low: i64,
    pub(crate) precipitation: Option<i64>,
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
                set_label_if_changed(&label, &truncate_label(&display.text, max_chars));
                set_visible_if_changed(&label, true);
            }
            Err(err) => {
                warn!("weather update failed: {:?}", err);
                set_label_if_changed(&label, FALLBACK_ICON);
                set_visible_if_changed(&label, true);
            }
        }
    });
}

fn set_label_if_changed(label: &Label, text: &str) {
    if label.label().as_str() != text {
        label.set_label(text);
    }
}

fn set_visible_if_changed<W: IsA<gtk4::Widget>>(widget: &W, visible: bool) {
    if widget.as_ref().is_visible() != visible {
        widget.as_ref().set_visible(visible);
    }
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
        .and_then(|body| serde_json::from_str::<OpenMeteoCurrentResponse>(&body).ok());

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
struct OpenMeteoCurrentResponse {
    current_weather: CurrentWeather,
}

#[derive(Debug, Deserialize)]
struct CurrentWeather {
    temperature: f64,
    weathercode: i64,
}

pub(crate) fn fetch_weather_forecast(
    config: &WeatherConfig,
    forecast_days: usize,
) -> WeatherForecast {
    let days = forecast_days.clamp(3, 5);
    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={}&longitude={}&current=temperature_2m,weather_code&daily=weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max&temperature_unit={}&forecast_days={}&timezone=auto",
        config.latitude,
        config.longitude,
        config.unit.api_value(),
        days
    );

    minreq::get(url)
        .with_timeout(8)
        .send()
        .ok()
        .and_then(|response| response.as_str().ok().map(str::to_string))
        .and_then(|body| serde_json::from_str::<OpenMeteoForecastResponse>(&body).ok())
        .map(|response| build_weather_forecast(response, config.unit))
        .unwrap_or_else(|| WeatherForecast {
            current: FALLBACK_ICON.to_string(),
            summary: "Forecast unavailable".to_string(),
            days: Vec::new(),
        })
}

fn build_weather_forecast(
    response: OpenMeteoForecastResponse,
    unit: WeatherUnit,
) -> WeatherForecast {
    let current_temp = response.current.temperature_2m.round() as i64;
    let current_condition = weather_condition(response.current.weather_code);
    let current = format!(
        "{} {}{}, {}",
        weather_icon(response.current.weather_code),
        current_temp,
        unit.symbol(),
        current_condition
    );

    let days = response
        .daily
        .time
        .iter()
        .enumerate()
        .filter_map(|(index, date)| {
            let code = *response.daily.weather_code.get(index)?;
            let high = response.daily.temperature_2m_max.get(index)?.round() as i64;
            let low = response.daily.temperature_2m_min.get(index)?.round() as i64;
            let precipitation = response
                .daily
                .precipitation_probability_max
                .as_ref()
                .and_then(|values| values.get(index))
                .map(|value| value.round() as i64);

            Some(WeatherForecastDay {
                label: forecast_day_label(index, date),
                icon: weather_icon(code),
                condition: weather_condition(code),
                high,
                low,
                precipitation,
            })
        })
        .collect::<Vec<_>>();

    let summary = days.first().map_or_else(
        || current_condition.to_string(),
        |day| {
            let precipitation = day
                .precipitation
                .map(|value| format!(", {}% precipitation", value))
                .unwrap_or_default();
            format!(
                "Today: {}, high {}{}, low {}{}{}",
                day.condition,
                day.high,
                unit.symbol(),
                day.low,
                unit.symbol(),
                precipitation
            )
        },
    );

    WeatherForecast {
        current,
        summary,
        days,
    }
}

#[derive(Debug, Deserialize)]
struct OpenMeteoForecastResponse {
    current: ForecastCurrent,
    daily: ForecastDaily,
}

#[derive(Debug, Deserialize)]
struct ForecastCurrent {
    temperature_2m: f64,
    weather_code: i64,
}

#[derive(Debug, Deserialize)]
struct ForecastDaily {
    time: Vec<String>,
    weather_code: Vec<i64>,
    temperature_2m_max: Vec<f64>,
    temperature_2m_min: Vec<f64>,
    precipitation_probability_max: Option<Vec<f64>>,
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

fn weather_condition(code: i64) -> &'static str {
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

fn forecast_day_label(index: usize, date: &str) -> String {
    if index == 0 {
        return "Today".to_string();
    }
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .map(|date| date.format("%a").to_string())
        .unwrap_or_else(|_| date.to_string())
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
    fn weather_condition_maps_common_open_meteo_codes() {
        assert_eq!(weather_condition(0), "Clear");
        assert_eq!(weather_condition(2), "Partly cloudy");
        assert_eq!(weather_condition(63), "Rain");
        assert_eq!(weather_condition(999), "Weather unavailable");
    }

    #[test]
    fn forecast_day_label_uses_today_and_weekdays() {
        assert_eq!(forecast_day_label(0, "2026-05-26"), "Today");
        assert_eq!(forecast_day_label(1, "2026-05-27"), "Wed");
        assert_eq!(forecast_day_label(1, "bad-date"), "bad-date");
    }

    #[test]
    fn build_weather_forecast_formats_current_summary_and_days() {
        let response = OpenMeteoForecastResponse {
            current: ForecastCurrent {
                temperature_2m: 21.4,
                weather_code: 2,
            },
            daily: ForecastDaily {
                time: vec!["2026-05-26".to_string(), "2026-05-27".to_string()],
                weather_code: vec![61, 0],
                temperature_2m_max: vec![24.2, 26.0],
                temperature_2m_min: vec![17.6, 18.2],
                precipitation_probability_max: Some(vec![70.0, 5.0]),
            },
        };

        let forecast = build_weather_forecast(response, WeatherUnit::Celsius);

        assert_eq!(forecast.current, "󰖕 21°C, Partly cloudy");
        assert_eq!(
            forecast.summary,
            "Today: Rain, high 24°C, low 18°C, 70% precipitation"
        );
        assert_eq!(forecast.days.len(), 2);
        assert_eq!(forecast.days[0].label, "Today");
        assert_eq!(forecast.days[1].label, "Wed");
    }

    #[test]
    fn truncate_label_keeps_short_text() {
        assert_eq!(truncate_label("abcdef", Some(10)), "abcdef");
        assert_eq!(truncate_label("abcdef", Some(4)), "abc…");
    }
}
