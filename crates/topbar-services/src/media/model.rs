//! What the panel knows about the players on the bus.
//!
//! Everything here is plain data: the snapshot the media task publishes and
//! the arithmetic the panel needs to draw a moving seek bar between polls.
//! MPRIS never signals `Position`, so the position in a snapshot is a *sample*
//! — a value plus the moment it was taken — and the panel extrapolates from it
//! rather than asking the player sixty times a second.

use std::path::PathBuf;
use std::time::Instant;

/// What a player is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlaybackStatus {
    /// Sound is coming out.
    Playing,
    /// A track is loaded and stopped where it stands.
    Paused,
    /// Nothing is loaded, or playback was ended rather than paused.
    #[default]
    Stopped,
}

impl PlaybackStatus {
    /// Read the `PlaybackStatus` property.
    ///
    /// The specification allows exactly three values; anything else is a
    /// player being creative, and "stopped" is the safe reading of it.
    pub fn parse(value: &str) -> Self {
        match value {
            "Playing" => Self::Playing,
            "Paused" => Self::Paused,
            _ => Self::Stopped,
        }
    }

    /// Whether the position is moving.
    pub fn is_playing(self) -> bool {
        matches!(self, Self::Playing)
    }
}

/// Album art that has been fetched and is ready to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtRef {
    /// Hash of the source URL.
    ///
    /// The panel keys its decoded-texture cache on this, so the same art
    /// arriving again — a track repeating, a player being re-selected — costs
    /// no decode at all.
    pub key: u64,
    /// The file to read. Either the player's own `file://` path or the copy
    /// the art cache downloaded.
    pub path: PathBuf,
}

/// One player, as the panel draws it.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerView {
    /// Its well-known name, e.g. `org.mpris.MediaPlayer2.spotify`.
    pub bus_name: String,
    /// The name it calls itself, e.g. `Spotify`.
    pub identity: String,
    /// Its desktop entry, when it declares one — the panel's icon lookup.
    pub desktop_entry: Option<String>,
    /// What it is doing.
    pub status: PlaybackStatus,
    /// Track title.
    pub title: Option<String>,
    /// Artist, with several artists already joined.
    pub artist: Option<String>,
    /// Album name.
    pub album: Option<String>,
    /// Album art, once it has been fetched.
    pub art: Option<ArtRef>,
    /// Position at [`PlayerView::sampled_at`], in microseconds.
    pub position_us: i64,
    /// Track length in microseconds, or 0 when the player does not say.
    pub length_us: i64,
    /// Playback rate: 1.0 is normal speed.
    pub rate: f64,
    /// Whether `Play` would do anything.
    pub can_play: bool,
    /// Whether `Pause` would.
    pub can_pause: bool,
    /// Whether there is a next track.
    pub can_go_next: bool,
    /// Whether there is a previous one.
    pub can_go_previous: bool,
    /// Whether the position may be set.
    pub can_seek: bool,
    /// When [`PlayerView::position_us`] was read.
    pub sampled_at: Instant,
}

impl PlayerView {
    /// A player that has answered nothing yet.
    pub(crate) fn new(bus_name: String) -> Self {
        Self {
            identity: identity_from_bus_name(&bus_name),
            bus_name,
            desktop_entry: None,
            status: PlaybackStatus::Stopped,
            title: None,
            artist: None,
            album: None,
            art: None,
            position_us: 0,
            length_us: 0,
            rate: 1.0,
            can_play: false,
            can_pause: false,
            can_go_next: false,
            can_go_previous: false,
            can_seek: false,
            sampled_at: Instant::now(),
        }
    }

    /// Whether there is anything worth putting on screen.
    pub fn has_track(&self) -> bool {
        self.title.as_ref().is_some_and(|title| !title.is_empty())
    }

    /// Where the track has got to by `now`.
    ///
    /// The panel calls this on its own tick, which is how the seek bar moves
    /// smoothly while the service polls the player only once a second.
    pub fn position_at(&self, now: Instant) -> i64 {
        let elapsed = now
            .checked_duration_since(self.sampled_at)
            .map_or(0, |elapsed| {
                elapsed.as_micros().min(i64::MAX as u128) as i64
            });
        advance(
            self.position_us,
            self.rate,
            self.length_us,
            self.status.is_playing(),
            elapsed,
        )
    }
}

/// Advance a sampled position by `elapsed_us`.
///
/// Pure, and the only place the extrapolation lives. A stopped or paused
/// player does not move, a rate that is zero or negative is treated as "do not
/// guess" rather than run backwards (some players report 0.0 while buffering),
/// and the result never leaves the track.
pub fn advance(position_us: i64, rate: f64, length_us: i64, playing: bool, elapsed_us: i64) -> i64 {
    let mut position = position_us.max(0);
    if playing && rate > 0.0 && elapsed_us > 0 {
        let travelled = (elapsed_us as f64 * rate) as i64;
        position = position.saturating_add(travelled);
    }
    if length_us > 0 {
        position = position.min(length_us);
    }
    position.max(0)
}

/// Everything the panel knows about media right now.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MediaState {
    /// Every player on the bus, in the order they appeared.
    pub players: Vec<PlayerView>,
    /// Index into [`MediaState::players`] of the one the panel shows.
    pub active: Option<usize>,
}

impl MediaState {
    /// The player the panel shows, if there is one.
    pub fn active(&self) -> Option<&PlayerView> {
        self.active.and_then(|index| self.players.get(index))
    }

    /// Whether the media card should be on screen at all.
    pub fn is_empty(&self) -> bool {
        self.players.is_empty()
    }
}

/// A readable name for a player that has not told us its `Identity` yet.
///
/// `org.mpris.MediaPlayer2.spotify` becomes `Spotify`, which is what the
/// player itself would have said. Instances (`...firefox.instance_1_23`) keep
/// only the application part.
pub(crate) fn identity_from_bus_name(bus_name: &str) -> String {
    let tail = bus_name
        .strip_prefix(super::MPRIS_PREFIX)
        .unwrap_or(bus_name);
    let name = tail.split('.').next().unwrap_or(tail);
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => bus_name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    const SECOND: i64 = 1_000_000;

    #[test]
    fn playback_status_reads_the_three_documented_values() {
        assert_eq!(PlaybackStatus::parse("Playing"), PlaybackStatus::Playing);
        assert_eq!(PlaybackStatus::parse("Paused"), PlaybackStatus::Paused);
        assert_eq!(PlaybackStatus::parse("Stopped"), PlaybackStatus::Stopped);
        assert_eq!(PlaybackStatus::parse("dancing"), PlaybackStatus::Stopped);
        assert!(PlaybackStatus::Playing.is_playing());
        assert!(!PlaybackStatus::Paused.is_playing());
    }

    #[test]
    fn a_playing_track_advances_in_real_time() {
        assert_eq!(advance(0, 1.0, 0, true, SECOND), SECOND);
        assert_eq!(advance(5 * SECOND, 1.0, 0, true, 2 * SECOND), 7 * SECOND);
    }

    #[test]
    fn a_half_speed_track_advances_at_half_speed() {
        assert_eq!(advance(0, 0.5, 0, true, 4 * SECOND), 2 * SECOND);
        assert_eq!(advance(0, 2.0, 0, true, SECOND), 2 * SECOND);
    }

    #[test]
    fn a_paused_track_does_not_move() {
        assert_eq!(advance(3 * SECOND, 1.0, 0, false, 10 * SECOND), 3 * SECOND);
    }

    #[test]
    fn a_rate_that_is_not_positive_is_not_guessed_at() {
        // Some players report 0.0 while buffering, and a negative rate would
        // otherwise rewind the seek bar under the user's finger.
        assert_eq!(advance(3 * SECOND, 0.0, 0, true, SECOND), 3 * SECOND);
        assert_eq!(advance(3 * SECOND, -1.0, 0, true, SECOND), 3 * SECOND);
    }

    #[test]
    fn extrapolation_never_leaves_the_track() {
        assert_eq!(
            advance(9 * SECOND, 1.0, 10 * SECOND, true, 5 * SECOND),
            10 * SECOND
        );
        assert_eq!(advance(-SECOND, 1.0, 10 * SECOND, false, 0), 0);
        // No length means no clamp: a stream has no end to run into.
        assert_eq!(advance(9 * SECOND, 1.0, 0, true, 5 * SECOND), 14 * SECOND);
    }

    #[test]
    fn a_clock_that_went_backwards_does_not_move_the_position() {
        assert_eq!(advance(SECOND, 1.0, 0, true, -SECOND), SECOND);
    }

    #[test]
    fn a_view_extrapolates_from_when_it_was_sampled() {
        let mut view = PlayerView::new("org.mpris.MediaPlayer2.vlc".into());
        view.status = PlaybackStatus::Playing;
        view.position_us = SECOND;
        view.sampled_at = Instant::now() - Duration::from_millis(500);

        let position = view.position_at(Instant::now());
        assert!(
            (SECOND + 400_000..=SECOND + 700_000).contains(&position),
            "{position} should be about 1.5s"
        );
    }

    #[test]
    fn a_bus_name_stands_in_for_an_identity_until_the_player_answers() {
        assert_eq!(
            identity_from_bus_name("org.mpris.MediaPlayer2.spotify"),
            "Spotify"
        );
        assert_eq!(
            identity_from_bus_name("org.mpris.MediaPlayer2.firefox.instance_1_23"),
            "Firefox"
        );
        assert_eq!(identity_from_bus_name("weird"), "Weird");
    }

    #[test]
    fn an_empty_state_has_nothing_to_show() {
        let state = MediaState::default();
        assert!(state.is_empty());
        assert!(state.active().is_none());
    }
}
