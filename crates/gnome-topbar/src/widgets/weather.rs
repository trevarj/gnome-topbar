//! Built-in Open-Meteo weather widget.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gnome_topbar_core::config::WidgetEntry;
use gtk4::gio;
use gtk4::glib::{self, SourceId};
use gtk4::prelude::*;
use gtk4::{Box as GtkBox, Button, Entry, GestureClick, Label, Orientation, Window, gdk};
use serde::Deserialize;
use tracing::warn;

use crate::services::callbacks::CallbackId;
use crate::services::config_manager::ConfigManager;
use crate::services::icons::CairoSpinner;
use crate::services::network::{NetworkService, NetworkSnapshot};
use crate::services::weather_runtime_config::{
    WeatherCoordinates, WeatherLocation, load_weather_location, save_weather_location,
    valid_coordinates,
};
use crate::styles::widget as wgt;
use crate::widgets::base::BaseWidget;
use crate::widgets::{WidgetConfig, warn_unknown_options};

const DEFAULT_INTERVAL_SECS: u64 = 1800;
const WEATHER_API_TIMEOUT_SECS: u64 = 30;
const FALLBACK_ICON: &str = "󰨹";
const CONFIGURE_LABEL: &str = "Configure...";

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

    fn toggled(self) -> Self {
        match self {
            Self::Celsius => Self::Fahrenheit,
            Self::Fahrenheit => Self::Celsius,
        }
    }
}

#[derive(Debug, Clone)]
pub struct WeatherConfig {
    coordinates: Option<WeatherCoordinates>,
    unit: WeatherUnit,
    interval: u64,
    tooltip: String,
    max_chars: Option<usize>,
}

impl Default for WeatherConfig {
    fn default() -> Self {
        Self {
            coordinates: None,
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
        let coordinates = coordinates_from_options(
            entry.options.get("latitude").and_then(|v| v.as_float()),
            entry.options.get("longitude").and_then(|v| v.as_float()),
        )
        .or_else(|| load_weather_location().map(|location| location.coordinates()));
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
            coordinates,
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
    let coordinates = coordinates_from_options(
        config
            .get_widget_option(widget_name, "latitude")
            .and_then(|v| v.as_float()),
        config
            .get_widget_option(widget_name, "longitude")
            .and_then(|v| v.as_float()),
    )
    .or_else(|| load_weather_location().map(|location| location.coordinates()));
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
        coordinates,
        unit,
        interval,
        tooltip,
        max_chars,
    }
}

fn coordinates_from_options(
    latitude: Option<f64>,
    longitude: Option<f64>,
) -> Option<WeatherCoordinates> {
    let (Some(latitude), Some(longitude)) = (latitude, longitude) else {
        return None;
    };
    if valid_coordinates(latitude, longitude) {
        Some(WeatherCoordinates {
            latitude,
            longitude,
        })
    } else {
        warn!("ignoring invalid weather coordinates from config");
        None
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

pub(crate) fn weather_refresh_interval_from_widget_name(widget_name: &str) -> u64 {
    weather_config_from_widget_name(widget_name).interval
}

pub(crate) fn show_weather_config_window_for_widget(
    widget_name: &str,
    on_saved: impl Fn() + 'static,
) {
    let label = Label::new(None);
    let config = Rc::new(RefCell::new(weather_config_from_widget_name(widget_name)));
    let generation = Rc::new(Cell::new(0_u64));
    show_weather_config_window_with_callback(&label, &config, &generation, Some(on_saved));
}

pub(crate) fn weather_location_label_from_widget_name(widget_name: &str) -> String {
    let config = weather_config_from_widget_name(widget_name);
    weather_location_label(&config)
}

fn weather_location_label(config: &WeatherConfig) -> String {
    let Some(coordinates) = config.coordinates else {
        return "No location configured".to_string();
    };

    if let Some(location) = load_weather_location() {
        let saved_coordinates = location.coordinates();
        if coordinates_match(coordinates, saved_coordinates)
            && let Some(label) = location.label.as_deref().filter(|label| !label.is_empty())
        {
            return label.to_string();
        }
    }

    format!("{:.4}, {:.4}", coordinates.latitude, coordinates.longitude)
}

fn coordinates_match(left: WeatherCoordinates, right: WeatherCoordinates) -> bool {
    (left.latitude - right.latitude).abs() < 0.0001
        && (left.longitude - right.longitude).abs() < 0.0001
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WeatherDisplay {
    pub(crate) text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WeatherForecast {
    pub(crate) current: String,
    pub(crate) location: String,
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
    _spinner: Rc<CairoSpinner>,
    _timer: Rc<RefCell<Option<SourceId>>>,
    network_callback: Rc<RefCell<Option<CallbackId>>>,
}

impl WeatherWidget {
    pub fn new(config: WeatherConfig) -> Self {
        let config = Rc::new(RefCell::new(config));
        let refresh_generation = Rc::new(Cell::new(0_u64));
        let network_callback = Rc::new(RefCell::new(None));
        let base = BaseWidget::new(&[wgt::WEATHER]);
        let spinner = Rc::new(CairoSpinner::new(base.content()));
        spinner.set_size(12);
        base.content().append(spinner.widget());
        let label = base.add_label(None, &[wgt::WEATHER]);
        label.set_visible(false);
        label.set_xalign(0.5);

        let on_click_right_cmd = ConfigManager::global().get_click_handlers(wgt::WEATHER).1;

        if on_click_right_cmd.is_none() {
            let click = GestureClick::new();
            click.set_button(gdk::BUTTON_SECONDARY);
            let label_for_toggle = label.clone();
            let spinner_for_toggle = spinner.clone();
            let config_for_toggle = config.clone();
            let generation_for_toggle = refresh_generation.clone();
            click.connect_released(move |_gesture, _n_press, _x, _y| {
                let mut config = config_for_toggle.borrow_mut();
                config.unit = config.unit.toggled();
                let current_config = config.clone();
                drop(config);
                refresh_weather_label(
                    &label_for_toggle,
                    &spinner_for_toggle,
                    &current_config,
                    &generation_for_toggle,
                );
            });
            base.widget().add_controller(click);
        }

        {
            let snapshot = config.borrow().clone();
            base.set_tooltip(&snapshot.tooltip);
            refresh_weather_label_when_online(
                &label,
                &spinner,
                &snapshot,
                &refresh_generation,
                &network_callback,
            );
        }

        let interval = config.borrow().interval;
        if interval > 0 {
            let timer = Rc::new(RefCell::new(None));
            let label_for_timer = label.clone();
            let spinner_for_timer = spinner.clone();
            let config_for_timer = config.clone();
            let generation_for_timer = refresh_generation.clone();
            let network_callback_for_timer = network_callback.clone();
            let source = glib::timeout_add_seconds_local(interval as u32, move || {
                let snapshot = config_for_timer.borrow().clone();
                refresh_weather_label_when_online(
                    &label_for_timer,
                    &spinner_for_timer,
                    &snapshot,
                    &generation_for_timer,
                    &network_callback_for_timer,
                );
                glib::ControlFlow::Continue
            });
            *timer.borrow_mut() = Some(source);
            Self {
                base,
                _spinner: spinner,
                _timer: timer,
                network_callback,
            }
        } else {
            Self {
                base,
                _spinner: spinner,
                _timer: Rc::new(RefCell::new(None)),
                network_callback,
            }
        }
    }

    pub fn widget(&self) -> &GtkBox {
        self.base.widget()
    }
}

pub(crate) fn show_weather_config_window_with_callback<F>(
    label: &Label,
    config: &Rc<RefCell<WeatherConfig>>,
    generation: &Rc<Cell<u64>>,
    on_saved: Option<F>,
) where
    F: Fn() + 'static,
{
    let window = Window::builder()
        .title("Weather Location")
        .default_width(360)
        .default_height(220)
        .build();
    window.add_css_class("weather-config-window");

    let content = GtkBox::new(Orientation::Vertical, 10);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_margin_start(12);
    content.set_margin_end(12);

    let city_entry = Entry::new();
    city_entry.set_placeholder_text(Some("Search city"));

    let search_button = Button::with_label("Search");
    let search_row = GtkBox::new(Orientation::Horizontal, 8);
    search_row.append(&city_entry);
    search_row.append(&search_button);

    let status_label = Label::new(Some("Search for a city or enter coordinates."));
    status_label.set_xalign(0.0);
    status_label.set_wrap(true);

    let latitude_entry = Entry::new();
    latitude_entry.set_placeholder_text(Some("Latitude"));
    let longitude_entry = Entry::new();
    longitude_entry.set_placeholder_text(Some("Longitude"));
    if let Some(coordinates) = config.borrow().coordinates {
        latitude_entry.set_text(&coordinates.latitude.to_string());
        longitude_entry.set_text(&coordinates.longitude.to_string());
    }

    let coordinates_row = GtkBox::new(Orientation::Horizontal, 8);
    coordinates_row.append(&latitude_entry);
    coordinates_row.append(&longitude_entry);

    let save_button = Button::with_label("Save");

    content.append(&search_row);
    content.append(&status_label);
    content.append(&coordinates_row);
    content.append(&save_button);
    window.set_child(Some(&content));

    {
        let city_entry = city_entry.clone();
        let latitude_entry = latitude_entry.clone();
        let longitude_entry = longitude_entry.clone();
        let status_label = status_label.clone();
        search_button.connect_clicked(move |_| {
            let query = city_entry.text().trim().to_string();
            if query.is_empty() {
                status_label.set_label("Enter a city to search.");
                return;
            }

            status_label.set_label("Searching...");
            let latitude_entry = latitude_entry.clone();
            let longitude_entry = longitude_entry.clone();
            let status_label = status_label.clone();
            glib::spawn_future_local(async move {
                let result = gio::spawn_blocking(move || fetch_geocoding_result(&query)).await;
                match result {
                    Ok(Some(location)) => {
                        latitude_entry.set_text(&location.latitude.to_string());
                        longitude_entry.set_text(&location.longitude.to_string());
                        let label = location
                            .label
                            .as_deref()
                            .filter(|label| !label.is_empty())
                            .unwrap_or("selected location");
                        status_label.set_label(&format!("Selected {label}."));
                    }
                    Ok(None) => status_label.set_label("No city found."),
                    Err(error) => {
                        warn!("weather geocoding failed: {:?}", error);
                        status_label.set_label("City search failed.");
                    }
                }
            });
        });
    }

    {
        let label = label.clone();
        let config = config.clone();
        let generation = generation.clone();
        let status_label = status_label.clone();
        let city_entry = city_entry.clone();
        let latitude_entry = latitude_entry.clone();
        let longitude_entry = longitude_entry.clone();
        let window = window.clone();
        let on_saved = Rc::new(on_saved);
        save_button.connect_clicked(move |_| {
            let latitude = latitude_entry.text().trim().parse::<f64>();
            let longitude = longitude_entry.text().trim().parse::<f64>();
            let (Ok(latitude), Ok(longitude)) = (latitude, longitude) else {
                status_label.set_label("Latitude and longitude must be numbers.");
                return;
            };
            if !valid_coordinates(latitude, longitude) {
                status_label.set_label("Latitude or longitude is out of range.");
                return;
            }

            let label_text = city_entry.text().trim().to_string();
            let location = WeatherLocation {
                label: (!label_text.is_empty()).then_some(label_text),
                latitude,
                longitude,
            };
            if let Err(error) = save_weather_location(&location) {
                status_label.set_label(&error);
                return;
            }

            let mut weather_config = config.borrow_mut();
            weather_config.coordinates = Some(location.coordinates());
            let current_config = weather_config.clone();
            drop(weather_config);
            refresh_weather_label_without_loading(&label, &current_config, &generation);
            if let Some(on_saved) = on_saved.as_ref() {
                on_saved();
            }
            window.close();
        });
    }

    window.present();
}

fn refresh_weather_label_when_online(
    label: &Label,
    spinner: &Rc<CairoSpinner>,
    config: &WeatherConfig,
    generation: &Rc<Cell<u64>>,
    network_callback: &Rc<RefCell<Option<CallbackId>>>,
) {
    if NetworkService::global().internet_available() {
        refresh_weather_label(label, spinner, config, generation);
        return;
    }

    if network_callback.borrow().is_some() {
        return;
    }

    let label = label.clone();
    let spinner = spinner.clone();
    let config = config.clone();
    let generation = generation.clone();
    let callback_id_cell = network_callback.clone();
    let callback_id = NetworkService::global().connect(move |_snapshot: &NetworkSnapshot| {
        if !NetworkService::global().internet_available() {
            return;
        }

        if let Some(callback_id) = callback_id_cell.borrow_mut().take() {
            NetworkService::global().unsubscribe(callback_id);
        }
        refresh_weather_label(&label, &spinner, &config, &generation);
    });
    *network_callback.borrow_mut() = Some(callback_id);
}

fn refresh_weather_label(
    label: &Label,
    spinner: &Rc<CairoSpinner>,
    config: &WeatherConfig,
    generation: &Rc<Cell<u64>>,
) {
    if should_show_weather_loading(label, config) {
        label.set_visible(false);
        spinner.start();
    }

    let label = label.clone();
    let spinner = spinner.clone();
    let config = config.clone();
    let max_chars = config.max_chars;
    let request_generation = generation.get().wrapping_add(1);
    generation.set(request_generation);
    let generation = generation.clone();
    glib::spawn_future_local(async move {
        let result = gio::spawn_blocking(move || fetch_weather_display(&config)).await;
        if generation.get() != request_generation {
            return;
        }
        spinner.stop();
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

fn refresh_weather_label_without_loading(
    label: &Label,
    config: &WeatherConfig,
    generation: &Rc<Cell<u64>>,
) {
    let label = label.clone();
    let config = config.clone();
    let max_chars = config.max_chars;
    let request_generation = generation.get().wrapping_add(1);
    generation.set(request_generation);
    let generation = generation.clone();
    glib::spawn_future_local(async move {
        let result = gio::spawn_blocking(move || fetch_weather_display(&config)).await;
        if generation.get() != request_generation {
            return;
        }
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

fn should_show_weather_loading(label: &Label, config: &WeatherConfig) -> bool {
    config.coordinates.is_some() && (!label.is_visible() || label.label().as_str() == FALLBACK_ICON)
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
        if let Some(callback_id) = self.network_callback.borrow_mut().take() {
            NetworkService::global().unsubscribe(callback_id);
        }
    }
}

pub(crate) fn fetch_weather_display(config: &WeatherConfig) -> WeatherDisplay {
    let Some(coordinates) = config.coordinates else {
        return WeatherDisplay {
            text: CONFIGURE_LABEL.to_string(),
        };
    };

    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={}&longitude={}&current=temperature_2m,weather_code&temperature_unit={}",
        coordinates.latitude,
        coordinates.longitude,
        config.unit.api_value()
    );

    let response = minreq::get(url)
        .with_timeout(WEATHER_API_TIMEOUT_SECS)
        .send()
        .ok()
        .and_then(|response| response.as_str().ok().map(str::to_string))
        .and_then(|body| serde_json::from_str::<OpenMeteoCurrentResponse>(&body).ok());

    let Some(weather) = response.map(|r| r.current) else {
        return WeatherDisplay {
            text: FALLBACK_ICON.to_string(),
        };
    };

    build_weather_display(weather, config.unit)
}

#[derive(Debug, Deserialize)]
struct OpenMeteoCurrentResponse {
    current: ForecastCurrent,
}

fn build_weather_display(weather: ForecastCurrent, unit: WeatherUnit) -> WeatherDisplay {
    let temp = weather.temperature_2m.round() as i64;
    WeatherDisplay {
        text: format!(
            "{}   {}{}",
            weather_icon(weather.weather_code),
            temp,
            unit.symbol()
        ),
    }
}

pub(crate) fn fetch_weather_forecast(
    config: &WeatherConfig,
    forecast_days: usize,
) -> WeatherForecast {
    let Some(coordinates) = config.coordinates else {
        return WeatherForecast {
            current: CONFIGURE_LABEL.to_string(),
            location: "No location configured".to_string(),
            summary: "Weather location is not configured.".to_string(),
            days: Vec::new(),
        };
    };

    let days = forecast_days.clamp(3, 5);
    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={}&longitude={}&current=temperature_2m,weather_code&daily=weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max&temperature_unit={}&forecast_days={}&timezone=auto",
        coordinates.latitude,
        coordinates.longitude,
        config.unit.api_value(),
        days
    );

    minreq::get(url)
        .with_timeout(WEATHER_API_TIMEOUT_SECS)
        .send()
        .ok()
        .and_then(|response| response.as_str().ok().map(str::to_string))
        .and_then(|body| serde_json::from_str::<OpenMeteoForecastResponse>(&body).ok())
        .map(|response| build_weather_forecast(response, config))
        .unwrap_or_else(|| WeatherForecast {
            current: FALLBACK_ICON.to_string(),
            location: weather_location_label(config),
            summary: "Forecast unavailable".to_string(),
            days: Vec::new(),
        })
}

fn fetch_geocoding_result(query: &str) -> Option<WeatherLocation> {
    let url = format!(
        "https://geocoding-api.open-meteo.com/v1/search?name={}&count=1&language=en&format=json",
        percent_encode_query(query)
    );

    minreq::get(url)
        .with_timeout(WEATHER_API_TIMEOUT_SECS)
        .send()
        .ok()
        .and_then(|response| response.as_str().ok().map(str::to_string))
        .and_then(|body| serde_json::from_str::<OpenMeteoGeocodingResponse>(&body).ok())
        .and_then(|response| response.results.into_iter().next())
        .and_then(|result| {
            if !valid_coordinates(result.latitude, result.longitude) {
                return None;
            }
            let label = geocoding_label(&result);
            Some(WeatherLocation {
                label: Some(label),
                latitude: result.latitude,
                longitude: result.longitude,
            })
        })
}

fn percent_encode_query(query: &str) -> String {
    query
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                vec![byte as char]
            }
            b' ' => vec!['+'],
            _ => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}

fn geocoding_label(result: &OpenMeteoGeocodingResult) -> String {
    let mut parts = vec![result.name.clone()];
    if let Some(admin1) = result.admin1.as_ref().filter(|value| !value.is_empty()) {
        parts.push(admin1.clone());
    }
    if let Some(country) = result.country.as_ref().filter(|value| !value.is_empty()) {
        parts.push(country.clone());
    }
    parts.join(", ")
}

#[derive(Debug, Deserialize)]
struct OpenMeteoGeocodingResponse {
    results: Vec<OpenMeteoGeocodingResult>,
}

#[derive(Debug, Deserialize)]
struct OpenMeteoGeocodingResult {
    name: String,
    latitude: f64,
    longitude: f64,
    country: Option<String>,
    admin1: Option<String>,
}

fn build_weather_forecast(
    response: OpenMeteoForecastResponse,
    config: &WeatherConfig,
) -> WeatherForecast {
    let unit = config.unit;
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
        location: weather_location_label(config),
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
    fn build_weather_display_formats_current_conditions() {
        let display = build_weather_display(
            ForecastCurrent {
                temperature_2m: 21.4,
                weather_code: 2,
            },
            WeatherUnit::Celsius,
        );

        assert_eq!(display.text, "󰖕   21°C");
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

        let config = WeatherConfig {
            coordinates: Some(WeatherCoordinates {
                latitude: 12.3456,
                longitude: 65.4321,
            }),
            unit: WeatherUnit::Celsius,
            interval: DEFAULT_INTERVAL_SECS,
            tooltip: "Weather".to_string(),
            max_chars: None,
        };
        let forecast = build_weather_forecast(response, &config);

        assert_eq!(forecast.current, "󰖕 21°C, Partly cloudy");
        assert_eq!(forecast.location, "12.3456, 65.4321");
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
