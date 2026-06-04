//! Private runtime configuration for weather location.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tracing::warn;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WeatherCoordinates {
    pub latitude: f64,
    pub longitude: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WeatherLocation {
    pub label: Option<String>,
    pub latitude: f64,
    pub longitude: f64,
}

impl WeatherLocation {
    pub fn coordinates(&self) -> WeatherCoordinates {
        WeatherCoordinates {
            latitude: self.latitude,
            longitude: self.longitude,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WeatherRuntimeConfig {
    pub location: Option<WeatherLocation>,
}

pub fn valid_coordinates(latitude: f64, longitude: f64) -> bool {
    latitude.is_finite()
        && longitude.is_finite()
        && (-90.0..=90.0).contains(&latitude)
        && (-180.0..=180.0).contains(&longitude)
}

pub fn weather_cache_path() -> PathBuf {
    if let Some(cache_home) = std::env::var_os("XDG_CACHE_HOME")
        && !cache_home.is_empty()
    {
        return PathBuf::from(cache_home)
            .join("gnome-topbar")
            .join("weather.toml");
    }

    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    home.join(".cache")
        .join("gnome-topbar")
        .join("weather.toml")
}

pub fn load_weather_location() -> Option<WeatherLocation> {
    let path = weather_cache_path();
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            warn!(
                "failed to read weather runtime config {}: {}",
                path.display(),
                error
            );
            return None;
        }
    };

    let config = match toml::from_str::<WeatherRuntimeConfig>(&contents) {
        Ok(config) => config,
        Err(error) => {
            warn!(
                "failed to parse weather runtime config {}: {}",
                path.display(),
                error
            );
            return None;
        }
    };

    config.location.filter(|location| {
        if valid_coordinates(location.latitude, location.longitude) {
            true
        } else {
            warn!(
                "ignoring invalid weather runtime coordinates in {}",
                path.display()
            );
            false
        }
    })
}

pub fn save_weather_location(location: &WeatherLocation) -> Result<(), String> {
    if !valid_coordinates(location.latitude, location.longitude) {
        return Err("latitude or longitude is out of range".to_string());
    }

    let path = weather_cache_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {}", parent.display(), error))?;
    }

    let config = WeatherRuntimeConfig {
        location: Some(location.clone()),
    };
    let contents = toml::to_string_pretty(&config)
        .map_err(|error| format!("failed to serialize weather config: {}", error))?;
    fs::write(&path, contents)
        .map_err(|error| format!("failed to write {}: {}", path.display(), error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_coordinate_ranges() {
        assert!(valid_coordinates(0.0, 0.0));
        assert!(valid_coordinates(-90.0, 180.0));
        assert!(!valid_coordinates(-90.1, 0.0));
        assert!(!valid_coordinates(0.0, 180.1));
        assert!(!valid_coordinates(f64::NAN, 0.0));
    }
}
