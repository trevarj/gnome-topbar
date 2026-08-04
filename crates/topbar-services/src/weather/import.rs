//! The one-time import of v1's saved weather location.
//!
//! v1 kept the coordinates its Configure dialog wrote in
//! `$XDG_CACHE_HOME/gnome-topbar/weather.toml`:
//!
//! ```toml
//! [location]
//! label = "Moscow"
//! latitude = 55.75204
//! longitude = 37.61781
//! ```
//!
//! v2 keeps them in `state.json` with everything else it remembers, so a user
//! upgrading in place would otherwise be sent back through the setup dialog
//! for a location they already chose. This reads that file **once**, when
//! there is no location from any other source, and never writes to it or
//! removes it: v1 may still be installed, and the file is its state, not ours.

use std::path::PathBuf;

use serde::Deserialize;
use tracing::{debug, info, warn};

use crate::weather::model::{LocationView, valid_coordinates};

/// Directory v1 wrote under, inside the cache home.
const V1_DIR: &str = "gnome-topbar";
/// The file itself.
const V1_FILE: &str = "weather.toml";

/// v1's file, as v1 wrote it.
#[derive(Debug, Deserialize)]
struct V1Runtime {
    location: Option<V1Location>,
}

#[derive(Debug, Deserialize)]
struct V1Location {
    label: Option<String>,
    latitude: f64,
    longitude: f64,
}

/// The location v1 saved, if there is one worth having.
pub fn from_v1() -> Option<LocationView> {
    let path = v1_path()?;
    let contents = match std::fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            debug!("no v1 weather location at {} to import", path.display());
            return None;
        }
        Err(error) => {
            warn!("could not read {}: {error}", path.display());
            return None;
        }
    };

    let location = parse(&contents)?;
    info!(
        "imported the weather location `{}` from {}",
        location.label,
        path.display()
    );
    Some(location)
}

/// Where v1 kept it.
fn v1_path() -> Option<PathBuf> {
    let cache = match std::env::var_os("XDG_CACHE_HOME") {
        Some(cache) if !cache.is_empty() => PathBuf::from(cache),
        _ => PathBuf::from(std::env::var_os("HOME")?).join(".cache"),
    };
    Some(cache.join(V1_DIR).join(V1_FILE))
}

/// Read v1's document. Pure, so the shape of that file is a test rather than
/// something to be rediscovered the next time someone upgrades.
fn parse(contents: &str) -> Option<LocationView> {
    let runtime: V1Runtime = match toml::from_str(contents) {
        Ok(runtime) => runtime,
        Err(error) => {
            warn!("the v1 weather location is not readable TOML: {error}");
            return None;
        }
    };

    let location = runtime.location?;
    if !valid_coordinates(location.latitude, location.longitude) {
        warn!("the v1 weather location is not a point on Earth; ignoring it");
        return None;
    }

    Some(LocationView::new(
        location.label.unwrap_or_default(),
        location.latitude,
        location.longitude,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Byte-for-byte what v1's `toml::to_string_pretty` wrote.
    const V1_DOCUMENT: &str =
        "[location]\nlabel = \"Moscow\"\nlatitude = 55.75204\nlongitude = 37.61781\n";

    #[test]
    fn v1s_own_file_reads_back_as_a_location() {
        let location = parse(V1_DOCUMENT).expect("v1's document is importable");
        assert_eq!(location.label, "Moscow");
        assert!((location.latitude - 55.75204).abs() < 1e-9);
        assert!((location.longitude - 37.61781).abs() < 1e-9);
    }

    #[test]
    fn a_v1_file_with_no_label_falls_back_to_its_coordinates() {
        let location =
            parse("[location]\nlatitude = 55.75204\nlongitude = 37.61781\n").expect("importable");
        assert_eq!(location.label, "55.7520, 37.6178");
    }

    #[test]
    fn a_v1_file_that_never_had_a_location_imports_nothing() {
        assert!(parse("").is_none());
    }

    #[test]
    fn nonsense_coordinates_are_not_imported() {
        assert!(parse("[location]\nlatitude = 1000.0\nlongitude = 0.0\n").is_none());
    }

    #[test]
    fn an_unreadable_file_is_a_log_line_not_a_panic() {
        assert!(parse("this is not toml {{{").is_none());
    }
}
