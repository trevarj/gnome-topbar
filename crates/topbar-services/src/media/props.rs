//! Reading MPRIS properties, which is where the typed boundary sits.
//!
//! `Metadata` is an `a{sv}` and players fill it in with real variety: artists
//! arrive as a list, as one string, or occasionally boxed inside another
//! variant; lengths arrive signed or unsigned; track ids arrive as an object
//! path or as a string that looks like one. All of that is dealt with here,
//! once, and the rest of the crate sees [`TrackMetadata`] and [`PlayerDelta`].
//!
//! Both parsers read a sequence of `(name, value)` pairs rather than one
//! concrete map, because the same properties reach the panel two ways — a
//! `GetAll` reply and a `PropertiesChanged` signal — and they should not be
//! read by two different pieces of code.

use std::collections::HashMap;

use zbus::zvariant::{Dict, Value};

use super::model::PlaybackStatus;

/// The interface whose properties the media card draws.
pub(crate) const PLAYER_INTERFACE: &str = "org.mpris.MediaPlayer2.Player";

/// One property, as it arrives on the wire.
pub(crate) type Entry<'a> = (&'a str, &'a Value<'a>);

/// One track, as the panel needs it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TrackMetadata {
    /// `xesam:title`.
    pub title: Option<String>,
    /// `xesam:artist`, with several artists joined.
    pub artist: Option<String>,
    /// `xesam:album`.
    pub album: Option<String>,
    /// `mpris:artUrl`.
    pub art_url: Option<String>,
    /// `mpris:trackid`, needed to seek safely.
    pub track_id: Option<String>,
    /// `mpris:length`, in microseconds. 0 when the player does not say.
    pub length_us: i64,
}

impl TrackMetadata {
    /// Read a `Metadata` dictionary.
    pub(crate) fn parse<'a>(entries: impl IntoIterator<Item = Entry<'a>>) -> Self {
        let fields: HashMap<&str, &Value<'_>> = entries.into_iter().collect();
        let get = |key: &str| fields.get(key).copied();
        Self {
            title: get("xesam:title").and_then(as_text),
            artist: get("xesam:artist").and_then(as_people),
            album: get("xesam:album").and_then(as_text),
            art_url: get("mpris:artUrl").and_then(as_text),
            track_id: get("mpris:trackid").and_then(as_text),
            length_us: get("mpris:length").and_then(as_integer).unwrap_or(0).max(0),
        }
    }
}

/// The properties one reply or signal carried.
///
/// Every field is optional because a signal only names what changed; applying
/// a delta leaves everything it does not mention alone.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct PlayerDelta {
    /// `PlaybackStatus`.
    pub status: Option<PlaybackStatus>,
    /// `Metadata`, parsed.
    pub metadata: Option<TrackMetadata>,
    /// `Position`. Only a `GetAll` reply carries it: the specification marks
    /// it as never emitting a change signal, which is why the panel polls.
    pub position_us: Option<i64>,
    /// `Rate`.
    pub rate: Option<f64>,
    /// `CanPlay`.
    pub can_play: Option<bool>,
    /// `CanPause`.
    pub can_pause: Option<bool>,
    /// `CanGoNext`.
    pub can_go_next: Option<bool>,
    /// `CanGoPrevious`.
    pub can_go_previous: Option<bool>,
    /// `CanSeek`.
    pub can_seek: Option<bool>,
}

impl PlayerDelta {
    /// Read the properties of the `org.mpris.MediaPlayer2.Player` interface.
    pub(crate) fn parse<'a>(entries: impl IntoIterator<Item = Entry<'a>>) -> Self {
        let fields: HashMap<&str, &Value<'_>> = entries.into_iter().collect();
        let get = |key: &str| fields.get(key).copied();
        Self {
            status: get("PlaybackStatus")
                .and_then(as_text)
                .map(|value| PlaybackStatus::parse(&value)),
            metadata: get("Metadata")
                .and_then(as_dictionary)
                .map(|dict| TrackMetadata::parse(dict_entries(dict))),
            position_us: get("Position").and_then(as_integer),
            rate: get("Rate").and_then(as_number),
            can_play: get("CanPlay").and_then(as_flag),
            can_pause: get("CanPause").and_then(as_flag),
            can_go_next: get("CanGoNext").and_then(as_flag),
            can_go_previous: get("CanGoPrevious").and_then(as_flag),
            can_seek: get("CanSeek").and_then(as_flag),
        }
    }

    /// Whether this reply or signal said anything the panel draws.
    pub(crate) fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// The `(name, value)` pairs of a nested dictionary, skipping non-string keys.
fn dict_entries<'a>(dict: &'a Dict<'a, 'a>) -> impl Iterator<Item = Entry<'a>> {
    dict.iter().filter_map(|(key, value)| match key {
        Value::Str(key) => Some((key.as_str(), value)),
        _ => None,
    })
}

/// Unwrap a variant that some players wrap their values in a second time.
fn unbox<'a>(value: &'a Value<'a>) -> &'a Value<'a> {
    match value {
        Value::Value(inner) => unbox(inner),
        other => other,
    }
}

/// A string, a path, or nothing. Blank strings count as nothing: a title of
/// `""` is a track with no title, not a track called nothing.
fn as_text(value: &Value<'_>) -> Option<String> {
    let text = match unbox(value) {
        Value::Str(text) => text.as_str(),
        Value::ObjectPath(path) => path.as_str(),
        _ => return None,
    };
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// `xesam:artist` is documented as a list; plenty of players send one string.
fn as_people(value: &Value<'_>) -> Option<String> {
    if let Value::Array(array) = unbox(value) {
        let names: Vec<String> = array.iter().filter_map(as_text).collect();
        return (!names.is_empty()).then(|| names.join(", "));
    }
    as_text(value)
}

/// Any integer-ish number, widened. Lengths arrive signed and unsigned alike.
fn as_integer(value: &Value<'_>) -> Option<i64> {
    Some(match unbox(value) {
        Value::I64(number) => *number,
        Value::U64(number) => i64::try_from(*number).unwrap_or(i64::MAX),
        Value::I32(number) => i64::from(*number),
        Value::U32(number) => i64::from(*number),
        Value::I16(number) => i64::from(*number),
        Value::U16(number) => i64::from(*number),
        Value::U8(number) => i64::from(*number),
        Value::F64(number) => *number as i64,
        _ => return None,
    })
}

/// A floating-point number, or an integer standing in for one.
fn as_number(value: &Value<'_>) -> Option<f64> {
    match unbox(value) {
        Value::F64(number) => Some(*number),
        other => as_integer(other).map(|number| number as f64),
    }
}

/// A boolean.
fn as_flag(value: &Value<'_>) -> Option<bool> {
    match unbox(value) {
        Value::Bool(flag) => Some(*flag),
        _ => None,
    }
}

/// A nested `a{sv}`, which is what `Metadata` is.
fn as_dictionary<'a>(value: &'a Value<'a>) -> Option<&'a Dict<'a, 'a>> {
    match unbox(value) {
        Value::Dict(dict) => Some(dict),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use zbus::zvariant::{Array, ObjectPath, Signature, Str};

    use super::*;

    /// The `(name, value)` pairs a test builds by hand.
    fn entries<'a>(pairs: &'a [(&'a str, Value<'a>)]) -> impl Iterator<Item = Entry<'a>> {
        pairs.iter().map(|(name, value)| (*name, value))
    }

    fn artists(names: &[&str]) -> Value<'static> {
        let mut array = Array::new(&Signature::Str);
        for name in names {
            array
                .append(Value::Str(Str::from(name.to_string())))
                .expect("a string fits a string array");
        }
        Value::Array(array)
    }

    fn dict<'a>(pairs: &'a [(&'a str, Value<'a>)]) -> Value<'a> {
        let mut dict = Dict::new(&Signature::Str, &Signature::Variant);
        for (name, value) in pairs {
            dict.append(
                Value::Str(Str::from(name.to_string())),
                Value::Value(Box::new(value.try_clone().expect("a cloneable value"))),
            )
            .expect("a metadata entry");
        }
        Value::Dict(dict)
    }

    #[test]
    fn a_full_metadata_dictionary_is_read() {
        let pairs = [
            ("xesam:title", Value::from("Windowlicker")),
            ("xesam:artist", artists(&["Aphex Twin"])),
            ("xesam:album", Value::from("Windowlicker")),
            ("mpris:artUrl", Value::from("file:///tmp/art.png")),
            (
                "mpris:trackid",
                Value::ObjectPath(ObjectPath::try_from("/org/mpris/track/1").expect("a path")),
            ),
            ("mpris:length", Value::I64(361_000_000)),
        ];

        let track = TrackMetadata::parse(entries(&pairs));
        assert_eq!(track.title.as_deref(), Some("Windowlicker"));
        assert_eq!(track.artist.as_deref(), Some("Aphex Twin"));
        assert_eq!(track.album.as_deref(), Some("Windowlicker"));
        assert_eq!(track.art_url.as_deref(), Some("file:///tmp/art.png"));
        assert_eq!(track.track_id.as_deref(), Some("/org/mpris/track/1"));
        assert_eq!(track.length_us, 361_000_000);
    }

    #[test]
    fn several_artists_are_joined_the_way_a_person_would_write_them() {
        let pairs = [("xesam:artist", artists(&["Boards", "of", "Canada"]))];
        assert_eq!(
            TrackMetadata::parse(entries(&pairs)).artist.as_deref(),
            Some("Boards, of, Canada")
        );
    }

    #[test]
    fn an_artist_sent_as_a_bare_string_is_read_too() {
        let pairs = [("xesam:artist", Value::from("Autechre"))];
        assert_eq!(
            TrackMetadata::parse(entries(&pairs)).artist.as_deref(),
            Some("Autechre")
        );
    }

    #[test]
    fn a_value_boxed_twice_is_unwrapped() {
        let pairs = [("xesam:title", Value::Value(Box::new(Value::from("Nested"))))];
        assert_eq!(
            TrackMetadata::parse(entries(&pairs)).title.as_deref(),
            Some("Nested")
        );
    }

    #[test]
    fn a_length_sent_unsigned_is_read_as_microseconds() {
        let pairs = [("mpris:length", Value::U64(90_000_000))];
        assert_eq!(TrackMetadata::parse(entries(&pairs)).length_us, 90_000_000);
    }

    #[test]
    fn a_negative_length_is_no_length() {
        let pairs = [("mpris:length", Value::I64(-1))];
        assert_eq!(TrackMetadata::parse(entries(&pairs)).length_us, 0);
    }

    #[test]
    fn missing_and_blank_fields_are_no_fields() {
        let pairs = [
            ("xesam:title", Value::from("   ")),
            ("xesam:artist", artists(&[])),
        ];
        assert_eq!(
            TrackMetadata::parse(entries(&pairs)),
            TrackMetadata::default()
        );
        assert_eq!(
            TrackMetadata::parse(std::iter::empty()),
            TrackMetadata::default()
        );
    }

    #[test]
    fn a_value_of_the_wrong_type_is_ignored_rather_than_fatal() {
        let pairs = [
            ("xesam:title", Value::Bool(true)),
            ("mpris:length", Value::from("about four minutes")),
        ];
        let track = TrackMetadata::parse(entries(&pairs));
        assert_eq!(track.title, None);
        assert_eq!(track.length_us, 0);
    }

    #[test]
    fn a_get_all_reply_is_read_into_a_delta() {
        let inner = [
            ("xesam:title", Value::from("Avril 14th")),
            ("mpris:length", Value::I64(120_000_000)),
        ];
        let metadata = dict(&inner);
        let pairs = [
            ("PlaybackStatus", Value::from("Playing")),
            ("Rate", Value::F64(0.5)),
            ("Position", Value::I64(4_000_000)),
            ("CanGoNext", Value::Bool(false)),
            ("Metadata", metadata),
        ];

        let delta = PlayerDelta::parse(entries(&pairs));
        assert_eq!(delta.status, Some(PlaybackStatus::Playing));
        assert_eq!(delta.rate, Some(0.5));
        assert_eq!(delta.position_us, Some(4_000_000));
        assert_eq!(delta.can_go_next, Some(false));
        assert_eq!(delta.can_seek, None, "an unmentioned property is untouched");
        assert!(!delta.is_empty());

        let track = delta.metadata.expect("metadata came through");
        assert_eq!(track.title.as_deref(), Some("Avril 14th"));
        assert_eq!(track.length_us, 120_000_000);
    }

    #[test]
    fn a_signal_about_properties_we_do_not_draw_is_empty() {
        let pairs = [("Volume", Value::F64(0.4)), ("Shuffle", Value::Bool(true))];
        assert!(PlayerDelta::parse(entries(&pairs)).is_empty());
        assert!(PlayerDelta::parse(std::iter::empty()).is_empty());
    }
}
