//! Fixture tests for the wire format, and table tests for the resolution order.
//!
//! Every JSON body here is one Open-Meteo actually sends, recorded rather than
//! invented: the parse layer is the panel's only defence against a schema that
//! changes underneath it.

use super::*;
use crate::weather::api::{
    Endpoints, forecast_url, geocoding_url, parse_forecast, parse_geocoding,
};
use crate::weather::model::TemperatureUnit;

const CELSIUS: &str = include_str!("../../tests/fixtures/open-meteo-forecast-celsius.json");
const FAHRENHEIT: &str = include_str!("../../tests/fixtures/open-meteo-forecast-fahrenheit.json");
const GEOCODING: &str = include_str!("../../tests/fixtures/open-meteo-geocoding.json");
const GEOCODING_EMPTY: &str = include_str!("../../tests/fixtures/open-meteo-geocoding-empty.json");
const ERROR: &str = include_str!("../../tests/fixtures/open-meteo-error.json");
const RATE_LIMIT: &str = include_str!("../../tests/fixtures/open-meteo-rate-limit.json");

// ---------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------

#[test]
fn the_forecast_request_asks_for_everything_the_panel_draws() {
    let url = forecast_url(
        "https://api.open-meteo.com/v1/forecast",
        55.75204,
        37.61781,
        TemperatureUnit::Celsius,
        5,
    );

    assert!(url.starts_with("https://api.open-meteo.com/v1/forecast?"));
    for parameter in [
        "latitude=55.75204",
        "longitude=37.61781",
        "temperature_2m",
        "apparent_temperature",
        "weather_code",
        "is_day",
        "temperature_2m_max",
        "temperature_2m_min",
        "precipitation_probability_max",
        "temperature_unit=celsius",
        "forecast_days=5",
        "timezone=auto",
    ] {
        assert!(url.contains(parameter), "{url} is missing {parameter}");
    }
}

#[test]
fn the_unit_reaches_the_request() {
    let url = forecast_url("base", 0.0, 0.0, TemperatureUnit::Fahrenheit, 3);
    assert!(url.contains("temperature_unit=fahrenheit"));
    assert!(url.contains("forecast_days=3"));
}

#[test]
fn a_search_term_is_percent_encoded() {
    let url = geocoding_url(
        "https://geocoding-api.open-meteo.com/v1/search",
        "São Paulo",
    );
    assert!(url.contains("name=S%C3%A3o+Paulo"), "{url}");
    assert!(url.contains("count=5"));
    assert!(url.contains("language=en"));
}

#[test]
fn the_endpoints_default_to_open_meteo() {
    let endpoints = Endpoints::default();
    assert!(
        endpoints
            .forecast
            .starts_with("https://api.open-meteo.com/")
    );
    assert!(
        endpoints
            .geocoding
            .starts_with("https://geocoding-api.open-meteo.com/")
    );
}

// ---------------------------------------------------------------------------
// Forecast bodies
// ---------------------------------------------------------------------------

#[test]
fn a_celsius_forecast_reads_back_whole() {
    let data = parse_forecast(CELSIUS, TemperatureUnit::Celsius).expect("the fixture parses");

    assert_eq!(data.unit, TemperatureUnit::Celsius);
    assert!((data.current.temperature - 21.4).abs() < 1e-9);
    assert!((data.current.feels_like - 19.8).abs() < 1e-9);
    assert_eq!(data.current.code, 2);
    assert!(data.current.is_day);

    assert_eq!(data.days.len(), 5);
    assert_eq!(data.days[0].date, "2026-08-04");
    assert_eq!(data.days[0].code, 61);
    assert!((data.days[0].high - 24.2).abs() < 1e-9);
    assert!((data.days[0].low - 13.4).abs() < 1e-9);
    assert_eq!(data.days[0].precipitation, Some(70));
    // Zero is a real answer, not a missing one: the card hides the droplet
    // rather than the field.
    assert_eq!(data.days[1].precipitation, Some(0));
    assert_eq!(data.days[4].code, 95);
}

#[test]
fn a_fahrenheit_forecast_reads_back_whole() {
    let data = parse_forecast(FAHRENHEIT, TemperatureUnit::Fahrenheit).expect("the fixture parses");

    assert_eq!(data.unit, TemperatureUnit::Fahrenheit);
    assert!((data.current.temperature - 70.5).abs() < 1e-9);
    assert!((data.current.feels_like - 73.2).abs() < 1e-9);
    assert_eq!(data.current.code, 0);
    assert!(!data.current.is_day, "is_day: 0 means the night icon");

    assert_eq!(data.days.len(), 3);
    // A null in the probability array is "not reported", not zero.
    assert_eq!(data.days[0].precipitation, None);
    assert_eq!(data.days[1].precipitation, Some(12));
    assert_eq!(data.days[2].precipitation, Some(100));
}

#[test]
fn a_current_block_without_the_extras_still_parses() {
    // What the endpoint returns when only the v1 parameters were asked for.
    let body = r#"{"current":{"temperature_2m":9.5,"weather_code":3},"daily":{"time":[],
        "weather_code":[],"temperature_2m_max":[],"temperature_2m_min":[]}}"#;
    let data = parse_forecast(body, TemperatureUnit::Celsius).expect("parses");

    assert!(
        (data.current.feels_like - 9.5).abs() < 1e-9,
        "falls back to the air temperature"
    );
    assert!(data.current.is_day, "daylight is the safe assumption");
    assert!(data.days.is_empty());
}

#[test]
fn ragged_daily_arrays_truncate_rather_than_panic() {
    let body = r#"{"current":{"temperature_2m":1.0,"weather_code":0,"is_day":1},
        "daily":{"time":["2026-08-04","2026-08-05","2026-08-06"],"weather_code":[0,1,2],
        "temperature_2m_max":[5.0,6.0],"temperature_2m_min":[1.0,2.0]}}"#;
    let data = parse_forecast(body, TemperatureUnit::Celsius).expect("parses");
    assert_eq!(data.days.len(), 2);
}

#[test]
fn an_error_body_is_an_error_however_it_arrives() {
    let error = parse_forecast(ERROR, TemperatureUnit::Celsius).expect_err("an error body");
    assert!(matches!(error, SvcError::Http(_)));
    assert!(error.to_string().contains("Latitude must be in range"));
    assert_eq!(error.user_message(), "Could not reach the service");
}

#[test]
fn a_rate_limit_body_says_which_limit() {
    let error = parse_forecast(RATE_LIMIT, TemperatureUnit::Celsius).expect_err("rate limited");
    assert!(error.to_string().contains("request limit exceeded"));
}

#[test]
fn a_body_that_is_not_json_at_all_is_a_protocol_error() {
    let error =
        parse_forecast("<html>502 Bad Gateway</html>", TemperatureUnit::Celsius).expect_err("html");
    assert!(matches!(error, SvcError::Protocol(_)));
}

// ---------------------------------------------------------------------------
// Geocoding bodies
// ---------------------------------------------------------------------------

#[test]
fn a_search_answers_with_at_most_five_labelled_places() {
    let results = parse_geocoding(GEOCODING).expect("the fixture parses");

    // The fixture holds six, one of them off the planet: five come back.
    assert_eq!(results.len(), 5);
    assert_eq!(results[0].label, "Moscow — Moscow, Russia");
    assert!((results[0].latitude - 55.75222).abs() < 1e-9);
    assert_eq!(results[1].label, "Moscow — Idaho, United States");
    // No admin1 on this one: the region is simply left out.
    assert_eq!(results[2].label, "Moscow — United States");
    assert!(
        results
            .iter()
            .all(|result| result.label != "Moskva-Nowhere"),
        "a result that is not on Earth must not be offered"
    );
}

#[test]
fn a_search_that_matched_nothing_is_an_empty_list_not_an_error() {
    let results = parse_geocoding(GEOCODING_EMPTY).expect("an empty body is valid");
    assert!(results.is_empty());
}

#[test]
fn a_geocoding_error_body_is_an_error() {
    let error = parse_geocoding(ERROR).expect_err("an error body");
    assert!(matches!(error, SvcError::Http(_)));
}

// ---------------------------------------------------------------------------
// Settings and the location resolution order
// ---------------------------------------------------------------------------

/// A `[widgets.weather]` section with coordinates in it.
fn config_with(latitude: Option<f64>, longitude: Option<f64>) -> WeatherConfig {
    WeatherConfig {
        latitude,
        longitude,
        ..WeatherConfig::default()
    }
}

fn saved(label: &str, latitude: f64, longitude: f64) -> PersistedLocation {
    PersistedLocation {
        label: label.to_string(),
        latitude,
        longitude,
    }
}

fn imported() -> Option<LocationView> {
    Some(LocationView::new("Moscow", 55.75204, 37.61781))
}

#[test]
fn the_settings_come_out_of_the_config_section() {
    let config = WeatherConfig {
        unit: "fahrenheit".to_string(),
        interval: 900,
        forecast_days: 4,
        ..WeatherConfig::default()
    };
    let settings = Settings::from_config(&config);
    assert_eq!(settings.unit, TemperatureUnit::Fahrenheit);
    assert_eq!(settings.interval, Duration::from_secs(900));
    assert_eq!(settings.days, 4);
}

#[test]
fn the_forecast_length_is_clamped_to_what_the_card_can_draw() {
    let too_many = WeatherConfig {
        forecast_days: 99,
        ..WeatherConfig::default()
    };
    assert_eq!(Settings::from_config(&too_many).days, 5);

    let too_few = WeatherConfig {
        forecast_days: 0,
        ..WeatherConfig::default()
    };
    assert_eq!(Settings::from_config(&too_few).days, 3);
}

#[test]
fn a_saved_location_beats_the_config_and_the_import() {
    let startup = startup_location(
        Some(saved("Berlin", 52.52, 13.405)),
        &config_with(Some(48.85), Some(2.35)),
        imported,
    );
    let location = startup.location.expect("the saved location");
    assert_eq!(location.label, "Berlin");
    assert!(!startup.persist, "it is already saved");
}

#[test]
fn the_config_coordinates_are_used_when_nothing_was_saved() {
    let startup = startup_location(None, &config_with(Some(48.85), Some(2.35)), imported);
    let location = startup.location.expect("the config location");
    assert!((location.latitude - 48.85).abs() < 1e-9);
    assert_eq!(location.label, "48.8500, 2.3500", "no name to give it");
    assert!(!startup.persist, "the config is not runtime state");
}

#[test]
fn one_config_coordinate_on_its_own_is_not_a_location() {
    let startup = startup_location(None, &config_with(Some(48.85), None), || None);
    assert_eq!(startup.location, None);
}

#[test]
fn config_coordinates_that_are_not_on_earth_are_ignored() {
    let startup = startup_location(None, &config_with(Some(1000.0), Some(0.0)), || None);
    assert_eq!(startup.location, None);
}

#[test]
fn v1s_location_is_imported_only_when_there_is_nothing_else() {
    let startup = startup_location(None, &config_with(None, None), imported);
    let location = startup.location.expect("the imported location");
    assert_eq!(location.label, "Moscow");
    assert!(
        startup.persist,
        "an import is written down so it happens once"
    );
}

#[test]
fn the_import_never_shadows_coordinates_written_in_the_config() {
    let startup = startup_location(None, &config_with(Some(48.85), Some(2.35)), || {
        panic!("the import must not even be attempted")
    });
    assert!((startup.location.expect("config").latitude - 48.85).abs() < 1e-9);
}

#[test]
fn a_saved_location_that_is_not_on_earth_falls_through() {
    let startup = startup_location(
        Some(saved("Nowhere", 1000.0, 0.0)),
        &WeatherConfig::default(),
        || None,
    );
    assert_eq!(startup.location, None);
}

#[test]
fn a_panel_with_nothing_to_go_on_needs_a_location() {
    let startup = startup_location(None, &WeatherConfig::default(), || None);
    assert_eq!(
        startup,
        Startup {
            location: None,
            persist: false
        }
    );
}

// ---------------------------------------------------------------------------
// Coordinate validation at the handle
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// The real endpoints
// ---------------------------------------------------------------------------

/// Ask Open-Meteo itself, once, on purpose.
///
/// Ignored by default: `cargo test` must not depend on somebody else's uptime,
/// their rate limit, or the machine having a network at all. Run it by hand
/// when the wire format is in question —
/// `cargo test -p topbar-services -- --ignored --nocapture` — which is the one
/// thing the recorded fixtures cannot tell you.
#[tokio::test]
#[ignore = "talks to the real Open-Meteo"]
async fn the_real_endpoints_still_answer_the_way_the_fixtures_say() {
    let endpoints = Endpoints::default();

    let found = api::fetch(geocoding_url(&endpoints.geocoding, "Berlin"))
        .await
        .and_then(|body| parse_geocoding(&body))
        .expect("the geocoder answers");
    let berlin = found.first().expect("Berlin exists");
    println!(
        "geocoded {} at {}, {}",
        berlin.label, berlin.latitude, berlin.longitude
    );
    assert!(berlin.label.contains("Berlin"));

    let data = api::fetch(forecast_url(
        &endpoints.forecast,
        berlin.latitude,
        berlin.longitude,
        TemperatureUnit::Celsius,
        5,
    ))
    .await
    .and_then(|body| parse_forecast(&body, TemperatureUnit::Celsius))
    .expect("the forecast endpoint answers");

    println!(
        "{}°C, feels like {}°C, code {}, {} day(s)",
        data.current.temperature,
        data.current.feels_like,
        data.current.code,
        data.days.len()
    );
    assert_eq!(data.days.len(), 5, "five days were asked for");
    assert!(
        (-90.0..=60.0).contains(&data.current.temperature),
        "{}°C is not a temperature Berlin has",
        data.current.temperature
    );
}

#[tokio::test]
async fn coordinates_off_the_planet_are_refused_before_anything_is_saved() {
    let (_, connectivity) = watch::channel(Arc::new(crate::connectivity::ConnectivityState {
        online: true,
    }));
    let weather = Weather::start_with(
        Settings::from_config(&WeatherConfig::default()),
        Endpoints::default(),
        None,
        connectivity,
        None,
    );

    let error = weather
        .handle()
        .set_manual(91.0, 0.0, "Nowhere".to_string())
        .await
        .expect_err("a latitude of 91 is not a place");
    assert!(matches!(error, SvcError::Coordinates(_)));
    assert_eq!(error.user_message(), "Those coordinates are out of range");

    assert!(
        weather
            .handle()
            .set_manual(0.0, 181.0, String::new())
            .await
            .is_err()
    );
    // And the service is untouched: still no location at all.
    assert_eq!(weather.state().borrow().phase, Phase::NeedsLocation);
}
