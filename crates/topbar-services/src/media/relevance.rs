//! Which player the media card shows.
//!
//! A desktop routinely has three or four MPRIS players on the bus — a browser
//! tab that once played a video, a music player, a video call — and only one
//! of them is what the user means by "the media". The rule, in order:
//!
//! 1. the player the user picked, for as long as it is still there;
//! 2. whatever is playing;
//! 3. whatever is paused with a track loaded;
//! 4. the most recent status change breaks a tie;
//! 5. the most recently appeared player breaks what is left.
//!
//! Rule 4 is what makes the card *stick*: pausing Spotify and then pausing a
//! browser tab moves the card to the browser, but a browser tab that has been
//! sitting paused since this morning never steals the card from the music that
//! just stopped. It is also what v1 achieved with four special cases and a
//! `last_playing` field.

use super::model::PlaybackStatus;

/// One player, reduced to what the choice depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Candidate {
    /// What it is doing.
    pub status: PlaybackStatus,
    /// Whether it has a track loaded worth drawing.
    pub has_track: bool,
    /// Counter stamped when the status last changed. Higher is more recent.
    pub status_seq: u64,
    /// Counter stamped when the player appeared. Higher is more recent.
    pub appeared_seq: u64,
}

/// How much a player deserves the card, before recency is considered.
fn rank(candidate: &Candidate) -> u8 {
    match (candidate.status, candidate.has_track) {
        (PlaybackStatus::Playing, _) => 3,
        (PlaybackStatus::Paused, true) => 2,
        (PlaybackStatus::Paused, false) => 1,
        (PlaybackStatus::Stopped, _) => 0,
    }
}

/// Pick the player to show.
///
/// `pinned` is the index of the player the user selected by hand, already
/// resolved against the current list — a pin whose player has gone is passed
/// as `None`, which is what makes the pin last exactly as long as the player.
pub(crate) fn select(candidates: &[Candidate], pinned: Option<usize>) -> Option<usize> {
    if let Some(index) = pinned.filter(|index| *index < candidates.len()) {
        return Some(index);
    }
    candidates
        .iter()
        .enumerate()
        .max_by_key(|(index, candidate)| {
            (
                rank(candidate),
                candidate.status_seq,
                candidate.appeared_seq,
                *index,
            )
        })
        .map(|(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A candidate with everything explicit, so each test says what it means.
    fn player(
        status: PlaybackStatus,
        has_track: bool,
        status_seq: u64,
        appeared_seq: u64,
    ) -> Candidate {
        Candidate {
            status,
            has_track,
            status_seq,
            appeared_seq,
        }
    }

    fn playing(status_seq: u64, appeared_seq: u64) -> Candidate {
        player(PlaybackStatus::Playing, true, status_seq, appeared_seq)
    }

    fn paused(status_seq: u64, appeared_seq: u64) -> Candidate {
        player(PlaybackStatus::Paused, true, status_seq, appeared_seq)
    }

    fn stopped(status_seq: u64, appeared_seq: u64) -> Candidate {
        player(PlaybackStatus::Stopped, false, status_seq, appeared_seq)
    }

    #[test]
    fn nothing_on_the_bus_means_nothing_to_show() {
        assert_eq!(select(&[], None), None);
        assert_eq!(select(&[], Some(0)), None, "a pin cannot invent a player");
    }

    #[test]
    fn one_player_is_the_player() {
        assert_eq!(select(&[stopped(1, 1)], None), Some(0));
    }

    #[test]
    fn playing_beats_paused() {
        let players = [paused(9, 1), playing(2, 2)];
        assert_eq!(select(&players, None), Some(1));
    }

    #[test]
    fn paused_with_a_track_beats_paused_without_one() {
        let players = [
            player(PlaybackStatus::Paused, false, 9, 1),
            player(PlaybackStatus::Paused, true, 2, 2),
        ];
        assert_eq!(select(&players, None), Some(1));
    }

    #[test]
    fn paused_beats_stopped() {
        let players = [stopped(9, 1), paused(2, 2)];
        assert_eq!(select(&players, None), Some(1));
    }

    #[test]
    fn the_most_recent_change_wins_between_equals() {
        let players = [playing(5, 1), playing(7, 2), playing(6, 3)];
        assert_eq!(select(&players, None), Some(1));

        let paused = [paused(7, 1), paused(5, 2)];
        assert_eq!(select(&paused, None), Some(0));
    }

    #[test]
    fn the_newest_player_breaks_a_dead_heat() {
        // Two players that came up together and have not changed since: the
        // one that appeared last is the one the user just started.
        let players = [paused(0, 1), paused(0, 2)];
        assert_eq!(select(&players, None), Some(1));
    }

    #[test]
    fn a_paused_player_keeps_the_card_until_something_else_moves() {
        // Spotify (0) pauses at seq 5; a browser tab (1) has been paused since
        // seq 2. The card stays with Spotify.
        let players = [paused(5, 1), paused(2, 2)];
        assert_eq!(select(&players, None), Some(0));

        // The browser tab starts playing: it takes the card immediately.
        let players = [paused(5, 1), playing(6, 2)];
        assert_eq!(select(&players, None), Some(1));
    }

    #[test]
    fn a_pin_outranks_everything_the_bus_is_doing() {
        let players = [paused(1, 1), playing(9, 2)];
        assert_eq!(select(&players, Some(0)), Some(0));
    }

    #[test]
    fn a_pin_on_a_player_that_has_gone_falls_back_to_the_rules() {
        // The task resolves a pinned bus name to an index and passes None when
        // that name is no longer on the bus; a stale index is refused too.
        let players = [paused(1, 1), playing(9, 2)];
        assert_eq!(select(&players, Some(7)), Some(1));
        assert_eq!(select(&players, None), Some(1));
    }
}
