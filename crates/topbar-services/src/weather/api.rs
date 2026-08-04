//! Talking to Open-Meteo: the two URLs, and the two bodies that come back.
//!
//! The request layer is ported from v1 and the parse layer is new, because v1
//! parsed straight into display strings and v2 has three surfaces rendering
//! from one snapshot. The split here is the usual one: [`forecast_url`] and
//! [`parse_forecast`] are pure and fixture-tested, and [`fetch`] is the only
//! thing that touches the network.
//!
//! `minreq` is a blocking client, so every request runs inside
//! [`tokio::task::spawn_blocking`]. That is deliberate rather than lazy: an
//! async HTTP client would drag a second TLS stack and a whole executor's
//! worth of dependencies in for two requests every half hour.
//!
//! # Environment
//!
//! `TOPBAR_WEATHER_API` and `TOPBAR_GEOCODING_API` replace the base URLs.
//! They exist for the visual smoke run and the tests, which point them at a
//! local listener serving fixture JSON so a screenshot of a populated forecast
//! does not depend on somebody else's uptime. Unset — which is every real run
//! — they default to Open-Meteo itself.

use serde::Deserialize;
use tracing::debug;

use crate::error::SvcError;
use crate::weather::model::{
    CurrentWeather, DailyWeather, GeocodeResult, TemperatureUnit, WeatherData, valid_coordinates,
};

/// Open-Meteo's forecast endpoint.
const FORECAST_BASE: &str = "https://api.open-meteo.com/v1/forecast";
/// Open-Meteo's geocoding endpoint.
const GEOCODING_BASE: &str = "https://geocoding-api.open-meteo.com/v1/search";
/// Overrides [`FORECAST_BASE`].
const FORECAST_ENV: &str = "TOPBAR_WEATHER_API";
/// Overrides [`GEOCODING_BASE`].
const GEOCODING_ENV: &str = "TOPBAR_GEOCODING_API";

/// How long a request may take before it is abandoned.
///
/// Half v1's thirty seconds: the panel refreshes on a schedule and retries
/// with backoff, so a slow answer is worth less than a prompt failure.
const TIMEOUT_SECS: u64 = 15;
/// How many places a search may return.
pub const SEARCH_RESULTS: usize = 5;
/// Header budget for one answer. Open-Meteo sends a dozen; anything past this
/// is a captive portal, and reading it is not this module's job.
const MAX_HEADERS: usize = 16 * 1024;

/// Where the two requests go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    /// Base URL of the forecast endpoint.
    pub forecast: String,
    /// Base URL of the geocoding endpoint.
    pub geocoding: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            forecast: FORECAST_BASE.to_string(),
            geocoding: GEOCODING_BASE.to_string(),
        }
    }
}

impl Endpoints {
    /// The real endpoints, unless the environment names others.
    pub fn from_env() -> Self {
        let default = Self::default();
        let forecast = override_from(FORECAST_ENV).unwrap_or(default.forecast);
        let geocoding = override_from(GEOCODING_ENV).unwrap_or(default.geocoding);
        Self {
            forecast,
            geocoding,
        }
    }
}

/// A non-empty base URL from `name`.
fn override_from(name: &str) -> Option<String> {
    let value = std::env::var(name).ok()?;
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    debug!("{name} redirects weather requests to {value}");
    Some(value.to_string())
}

/// The forecast request for one location.
pub fn forecast_url(
    base: &str,
    latitude: f64,
    longitude: f64,
    unit: TemperatureUnit,
    days: u32,
) -> String {
    format!(
        "{base}?latitude={latitude}&longitude={longitude}\
         &current=temperature_2m,apparent_temperature,weather_code,is_day\
         &daily=weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max\
         &temperature_unit={unit}&forecast_days={days}&timezone=auto",
        unit = unit.api_value(),
    )
}

/// The geocoding request for one query.
pub fn geocoding_url(base: &str, query: &str) -> String {
    format!(
        "{base}?name={name}&count={SEARCH_RESULTS}&language=en&format=json",
        name = percent_encode(query),
    )
}

/// Percent-encode a search term. Ported from v1.
fn percent_encode(query: &str) -> String {
    let mut encoded = String::with_capacity(query.len());
    for byte in query.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char);
            }
            b' ' => encoded.push('+'),
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

/// Fetch `url`, or say why not.
///
/// The blocking call is confined to a blocking-pool thread; nothing on a
/// runtime worker ever waits on a socket here.
pub async fn fetch(url: String) -> Result<String, SvcError> {
    tokio::task::spawn_blocking(move || blocking_fetch(&url))
        .await
        .map_err(|error| SvcError::Http(format!("the request task failed: {error}")))?
}

/// The request itself, on a blocking thread.
fn blocking_fetch(url: &str) -> Result<String, SvcError> {
    let response = minreq::get(url)
        .with_timeout(TIMEOUT_SECS)
        .with_max_headers_size(MAX_HEADERS)
        .send()
        .map_err(|error| SvcError::Http(error.to_string()))?;

    let status = response.status_code;
    let body = response
        .as_str()
        .map_err(|error| SvcError::Http(format!("the answer was not text: {error}")))?;

    // Open-Meteo puts the useful part of an error in the body, including for a
    // 429, so the body is read before the status is judged.
    if let Some(reason) = api_error(body) {
        return Err(SvcError::Http(reason));
    }
    if !(200..300).contains(&status) {
        return Err(SvcError::Http(format!("the service answered {status}")));
    }
    Ok(body.to_string())
}

/// Open-Meteo's error envelope: `{"error": true, "reason": "..."}`.
#[derive(Debug, Deserialize)]
struct ApiError {
    error: bool,
    reason: Option<String>,
}

/// The `reason` from an error body, if that is what this is.
fn api_error(body: &str) -> Option<String> {
    let error: ApiError = serde_json::from_str(body).ok()?;
    if !error.error {
        return None;
    }
    Some(
        error
            .reason
            .unwrap_or_else(|| "the service refused the request".to_string()),
    )
}

// ---------------------------------------------------------------------------
// Forecast
// ---------------------------------------------------------------------------

/// The forecast body, as it arrives.
#[derive(Debug, Deserialize)]
struct ForecastBody {
    current: CurrentBody,
    daily: DailyBody,
}

#[derive(Debug, Deserialize)]
struct CurrentBody {
    temperature_2m: f64,
    /// Open-Meteo omits this if the parameter was dropped from the request.
    apparent_temperature: Option<f64>,
    weather_code: u16,
    /// `1` while the sun is up. Absent on some historical responses.
    is_day: Option<u8>,
}

#[derive(Debug, Deserialize)]
struct DailyBody {
    time: Vec<String>,
    weather_code: Vec<u16>,
    temperature_2m_max: Vec<f64>,
    temperature_2m_min: Vec<f64>,
    precipitation_probability_max: Option<Vec<Option<f64>>>,
}

/// Read a forecast body into the panel's own shape.
///
/// The daily arrays are parallel and Open-Meteo has been known to return them
/// ragged, so a day is only produced when every array it needs has an entry —
/// the same defensive zip v1 used, and the reason a short array truncates the
/// forecast instead of panicking.
pub fn parse_forecast(body: &str, unit: TemperatureUnit) -> Result<WeatherData, SvcError> {
    if let Some(reason) = api_error(body) {
        return Err(SvcError::Http(reason));
    }

    let parsed: ForecastBody = serde_json::from_str(body)
        .map_err(|error| SvcError::Protocol(format!("unreadable forecast: {error}")))?;

    let current = CurrentWeather {
        temperature: parsed.current.temperature_2m,
        feels_like: parsed
            .current
            .apparent_temperature
            .unwrap_or(parsed.current.temperature_2m),
        code: parsed.current.weather_code,
        // Absent means "assume daylight": the day icons are the ones a user
        // recognises, and a wrong night icon at noon reads as a bug.
        is_day: parsed.current.is_day.is_none_or(|value| value != 0),
    };

    let daily = &parsed.daily;
    let days = daily
        .time
        .iter()
        .enumerate()
        .map_while(|(index, date)| {
            Some(DailyWeather {
                date: date.clone(),
                code: *daily.weather_code.get(index)?,
                high: *daily.temperature_2m_max.get(index)?,
                low: *daily.temperature_2m_min.get(index)?,
                precipitation: daily
                    .precipitation_probability_max
                    .as_ref()
                    .and_then(|values| values.get(index).copied().flatten())
                    .map(|value| value.round().clamp(0.0, 100.0) as u8),
            })
        })
        .collect();

    Ok(WeatherData {
        current,
        days,
        unit,
    })
}

// ---------------------------------------------------------------------------
// Geocoding
// ---------------------------------------------------------------------------

/// The geocoding body. `results` is absent entirely when nothing matched.
#[derive(Debug, Default, Deserialize)]
struct GeocodingBody {
    #[serde(default)]
    results: Vec<GeocodingResult>,
}

#[derive(Debug, Deserialize)]
struct GeocodingResult {
    name: String,
    latitude: f64,
    longitude: f64,
    country: Option<String>,
    admin1: Option<String>,
}

/// Read a geocoding body into at most [`SEARCH_RESULTS`] places.
///
/// Results whose coordinates are not on Earth are dropped rather than offered:
/// picking one would save a location the forecast endpoint then rejects.
pub fn parse_geocoding(body: &str) -> Result<Vec<GeocodeResult>, SvcError> {
    if let Some(reason) = api_error(body) {
        return Err(SvcError::Http(reason));
    }

    let parsed: GeocodingBody = serde_json::from_str(body)
        .map_err(|error| SvcError::Protocol(format!("unreadable search results: {error}")))?;

    Ok(parsed
        .results
        .into_iter()
        .filter(|result| valid_coordinates(result.latitude, result.longitude))
        .map(|result| GeocodeResult {
            label: place_label(&result),
            latitude: result.latitude,
            longitude: result.longitude,
        })
        .take(SEARCH_RESULTS)
        .collect())
}

/// `City — Region, Country`, dropping whichever parts are missing.
fn place_label(result: &GeocodingResult) -> String {
    let region = result.admin1.as_deref().filter(|value| !value.is_empty());
    let country = result.country.as_deref().filter(|value| !value.is_empty());

    match (region, country) {
        (Some(region), Some(country)) => format!("{} — {region}, {country}", result.name),
        (Some(region), None) => format!("{} — {region}", result.name),
        (None, Some(country)) => format!("{} — {country}", result.name),
        (None, None) => result.name.clone(),
    }
}
