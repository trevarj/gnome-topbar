//! The MPRIS interfaces, trimmed to what the panel uses.
//!
//! Hand-written rather than generated from introspection: the specification is
//! stable, the panel needs about a third of it, and a trimmed proxy is a list
//! of exactly the calls that may be made. Nothing here touches
//! `LoopStatus`, `Shuffle`, `Volume` or `OpenUri` — the media card offers none
//! of them.
//!
//! Every proxy is built with property caching **off**. `Position` is the
//! reason: the specification marks it as not emitting `PropertiesChanged`, so
//! a cached read would answer with whatever was true when the track started.
//! Property *changes* arrive through one `PropertiesChanged` stream per player
//! instead (see [`super::task`]), which keeps one subscription per player
//! rather than one per property.

use std::collections::HashMap;

use zbus::zvariant::{ObjectPath, OwnedValue};

/// The root interface: who the player is.
#[zbus::proxy(
    interface = "org.mpris.MediaPlayer2",
    default_path = "/org/mpris/MediaPlayer2"
)]
pub(crate) trait Application {
    /// Bring the player's window to the front.
    fn raise(&self) -> zbus::Result<()>;

    /// The name the player calls itself, e.g. `Spotify`.
    #[zbus(property)]
    fn identity(&self) -> zbus::Result<String>;

    /// Its desktop entry, without the `.desktop` suffix.
    #[zbus(property)]
    fn desktop_entry(&self) -> zbus::Result<String>;
}

/// The player interface: what it is doing, and what may be done to it.
#[zbus::proxy(
    interface = "org.mpris.MediaPlayer2.Player",
    default_path = "/org/mpris/MediaPlayer2"
)]
pub(crate) trait Player {
    /// Play if paused, pause if playing.
    fn play_pause(&self) -> zbus::Result<()>;

    /// Skip to the next track.
    fn next(&self) -> zbus::Result<()>;

    /// Skip to the previous track.
    fn previous(&self) -> zbus::Result<()>;

    /// Stop playback and rewind.
    ///
    /// Only `topbar media stop` uses it: the media card has no stop button,
    /// GNOME's does not either, and a key bound to it should still work.
    fn stop(&self) -> zbus::Result<()>;

    /// Jump to `position` (microseconds) within `track_id`.
    ///
    /// The track id guards against a seek landing in the track that started
    /// while the request was in flight; a player that offers no track id gets
    /// [`PlayerProxy::seek`] instead.
    fn set_position(&self, track_id: &ObjectPath<'_>, position: i64) -> zbus::Result<()>;

    /// Move the position by `offset` microseconds, which may be negative.
    fn seek(&self, offset: i64) -> zbus::Result<()>;

    /// `Playing`, `Paused` or `Stopped`.
    #[zbus(property)]
    fn playback_status(&self) -> zbus::Result<String>;

    /// The track's metadata, in the xesam vocabulary.
    #[zbus(property)]
    fn metadata(&self) -> zbus::Result<HashMap<String, OwnedValue>>;

    /// Where the track has got to, in microseconds.
    #[zbus(property)]
    fn position(&self) -> zbus::Result<i64>;

    /// Playback speed, where 1.0 is normal.
    #[zbus(property)]
    fn rate(&self) -> zbus::Result<f64>;

    /// Whether `Play` would do anything.
    #[zbus(property)]
    fn can_play(&self) -> zbus::Result<bool>;

    /// Whether `Pause` would.
    #[zbus(property)]
    fn can_pause(&self) -> zbus::Result<bool>;

    /// Whether there is a next track.
    #[zbus(property)]
    fn can_go_next(&self) -> zbus::Result<bool>;

    /// Whether there is a previous one.
    #[zbus(property)]
    fn can_go_previous(&self) -> zbus::Result<bool>;

    /// Whether the position may be set.
    #[zbus(property)]
    fn can_seek(&self) -> zbus::Result<bool>;
}
