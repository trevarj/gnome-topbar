//! Who moved the volume, and how the panel works that out.
//!
//! The OSD is the reason this exists. A capsule that appears every time the
//! volume moves would appear while the user is dragging the Quick Settings
//! slider, directly under their pointer, restating the number they are already
//! looking at. GNOME does not do that, and neither does this panel.
//!
//! PulseAudio does not tell us who asked for a change: a sink event says only
//! that a sink now has a different volume. So the attribution is a **command
//! echo**: the moment a handle sends a command it records what it asked for,
//! for which field, on whose behalf, with a short deadline. When the change
//! comes back the service looks for a matching record — same field, same value
//! (±[`TOLERANCE`] for volumes, exactly for mutes), not yet expired — and takes
//! that record's source. Anything else is [`ChangeSource::External`]: `pactl`,
//! a headset button, another panel, the user's own `topbar volume` keybind.
//!
//! It is a heuristic, and the ways it can be wrong are worth stating:
//!
//! - Two changes to the same field inside [`WINDOW`] with the same value — a
//!   slider drag and a media key landing together — cannot be told apart, and
//!   the first record wins. The cost is one OSD that should not have shown, or
//!   one that should have.
//! - A backend that rounds a request to something more than [`TOLERANCE`] away
//!   makes its own echo unrecognisable, and the change reads as external. The
//!   cost is an OSD the user did not need; the alternative — trusting the
//!   window alone — would swallow a real external change.
//!
//! Both are cheap, and neither can wedge anything: records expire on their own
//! and nothing waits on one.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How long a command's echo is expected back.
///
/// Long enough for a round trip to PulseAudio or logind under load, short
/// enough that a genuinely external change arriving right after one of ours is
/// not mistaken for it.
pub const WINDOW: Duration = Duration::from_millis(400);

/// How far a returned volume may sit from the requested one and still count.
///
/// One percentage point: the conversion between a percentage and PulseAudio's
/// integer volume is lossy in both directions, so an exact match is not
/// something the backend can promise.
pub const TOLERANCE: u32 = 1;

/// Most records kept at once.
///
/// A ceiling rather than a size: records expire by time, and this only stops a
/// service that is being hammered from growing the queue without bound.
const CAPACITY: usize = 16;

/// Who asked for a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeSource {
    /// A `topbar volume`/`topbar brightness` command, or a media key bound to
    /// one. The user pressed something with no on-screen control attached, so
    /// the OSD is the only feedback there is.
    Cli,
    /// A control inside the panel — a Quick Settings slider (M9). The control
    /// itself is the feedback, so the OSD stays away.
    Ui,
    /// Something else on the machine entirely.
    External,
}

impl ChangeSource {
    /// Whether a change from here should raise the OSD.
    pub fn shows_osd(self) -> bool {
        matches!(self, Self::Cli | Self::External)
    }
}

/// A field a command can move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    /// The default sink's volume, as a percentage.
    SinkVolume,
    /// The default sink's mute flag, as 0 or 1.
    SinkMute,
    /// The default source's volume, as a percentage.
    SourceVolume,
    /// The default source's mute flag, as 0 or 1.
    SourceMute,
    /// The backlight, as a percentage.
    Brightness,
}

impl Field {
    /// Whether values of this field are compared with [`TOLERANCE`].
    fn is_continuous(self) -> bool {
        matches!(
            self,
            Self::SinkVolume | Self::SourceVolume | Self::Brightness
        )
    }
}

/// One change, as published on a snapshot.
///
/// The serial is what lets a subscriber tell a *new* change from a re-render:
/// a snapshot is republished whenever anything on it moves, including things
/// the OSD does not care about, and comparing values would make a volume
/// returning to a value it held a minute ago look like nothing happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Change {
    /// Who caused it.
    pub source: ChangeSource,
    /// Bumped once per change, never reused.
    pub serial: u64,
}

/// One outstanding command, waiting for its echo.
#[derive(Debug, Clone, Copy)]
struct Echo {
    field: Field,
    value: u32,
    source: ChangeSource,
    deadline: Instant,
}

/// The outstanding commands, oldest first.
#[derive(Debug, Default)]
pub struct Echoes {
    pending: VecDeque<Echo>,
    serial: u64,
}

impl Echoes {
    /// An empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Note that `source` asked for `field` to become `value`.
    ///
    /// Records from the same source for the same field replace each other: a
    /// slider being dragged posts a value every few milliseconds and only the
    /// last one will ever come back.
    pub fn record(&mut self, field: Field, value: u32, source: ChangeSource, now: Instant) {
        self.expire(now);
        self.pending
            .retain(|echo| echo.field != field || echo.source != source);
        if self.pending.len() >= CAPACITY {
            self.pending.pop_front();
        }
        self.pending.push_back(Echo {
            field,
            value,
            source,
            deadline: now + WINDOW,
        });
    }

    /// Attribute an observed change, consuming the record that explains it.
    ///
    /// Returns a [`Change`] carrying a fresh serial, so the caller can put it
    /// straight on the snapshot.
    pub fn attribute(&mut self, field: Field, value: u32, now: Instant) -> Change {
        self.expire(now);
        let matched = self.pending.iter().position(|echo| {
            echo.field == field
                && if field.is_continuous() {
                    echo.value.abs_diff(value) <= TOLERANCE
                } else {
                    echo.value == value
                }
        });
        let source = match matched {
            Some(index) => self
                .pending
                .remove(index)
                .map_or(ChangeSource::External, |echo| echo.source),
            None => ChangeSource::External,
        };
        self.serial = self.serial.wrapping_add(1);
        Change {
            source,
            serial: self.serial,
        }
    }

    /// Forget everything: the backend restarted and no echo is coming.
    pub fn clear(&mut self) {
        self.pending.clear();
    }

    /// Drop records nothing is going to answer.
    fn expire(&mut self, now: Instant) {
        self.pending.retain(|echo| echo.deadline > now);
    }

    /// How many records are outstanding. For the tests.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> Instant {
        Instant::now()
    }

    #[test]
    fn a_change_nobody_asked_for_is_external() {
        let mut echoes = Echoes::new();
        let change = echoes.attribute(Field::SinkVolume, 40, now());
        assert_eq!(change.source, ChangeSource::External);
    }

    #[test]
    fn a_command_claims_its_own_echo() {
        let mut echoes = Echoes::new();
        let start = now();
        echoes.record(Field::SinkVolume, 40, ChangeSource::Ui, start);
        assert_eq!(
            echoes.attribute(Field::SinkVolume, 40, start).source,
            ChangeSource::Ui
        );
        // Consumed: a second change with the same value is somebody else's.
        assert_eq!(
            echoes.attribute(Field::SinkVolume, 40, start).source,
            ChangeSource::External
        );
    }

    #[test]
    fn a_rounded_echo_still_counts_but_a_different_value_does_not() {
        let mut echoes = Echoes::new();
        let start = now();

        echoes.record(Field::SinkVolume, 40, ChangeSource::Cli, start);
        assert_eq!(
            echoes.attribute(Field::SinkVolume, 41, start).source,
            ChangeSource::Cli
        );

        echoes.record(Field::SinkVolume, 40, ChangeSource::Cli, start);
        assert_eq!(
            echoes.attribute(Field::SinkVolume, 45, start).source,
            ChangeSource::External,
            "five points away is somebody else's change"
        );
    }

    #[test]
    fn a_mute_has_to_match_exactly() {
        let mut echoes = Echoes::new();
        let start = now();
        echoes.record(Field::SinkMute, 1, ChangeSource::Ui, start);
        assert_eq!(
            echoes.attribute(Field::SinkMute, 0, start).source,
            ChangeSource::External,
            "unmuting is not the muting we asked for"
        );
    }

    #[test]
    fn an_echo_that_never_arrives_expires() {
        let mut echoes = Echoes::new();
        let start = now();
        echoes.record(Field::SinkVolume, 40, ChangeSource::Ui, start);

        let late = start + WINDOW + Duration::from_millis(1);
        assert_eq!(
            echoes.attribute(Field::SinkVolume, 40, late).source,
            ChangeSource::External
        );
        assert_eq!(echoes.len(), 0, "the stale record is gone");
    }

    #[test]
    fn fields_do_not_claim_each_others_echoes() {
        let mut echoes = Echoes::new();
        let start = now();
        echoes.record(Field::SinkVolume, 40, ChangeSource::Ui, start);
        assert_eq!(
            echoes.attribute(Field::SourceVolume, 40, start).source,
            ChangeSource::External
        );
        assert_eq!(
            echoes.attribute(Field::SinkVolume, 40, start).source,
            ChangeSource::Ui
        );
    }

    #[test]
    fn a_drag_leaves_one_record_per_source() {
        let mut echoes = Echoes::new();
        let start = now();
        for percent in [10, 20, 30, 40] {
            echoes.record(Field::SinkVolume, percent, ChangeSource::Ui, start);
        }
        assert_eq!(echoes.len(), 1, "only the last value can come back");
        assert_eq!(
            echoes.attribute(Field::SinkVolume, 40, start).source,
            ChangeSource::Ui
        );
    }

    #[test]
    fn two_sources_on_one_field_are_both_remembered() {
        let mut echoes = Echoes::new();
        let start = now();
        echoes.record(Field::SinkVolume, 30, ChangeSource::Ui, start);
        echoes.record(Field::SinkVolume, 70, ChangeSource::Cli, start);
        assert_eq!(echoes.len(), 2);
        assert_eq!(
            echoes.attribute(Field::SinkVolume, 70, start).source,
            ChangeSource::Cli
        );
        assert_eq!(
            echoes.attribute(Field::SinkVolume, 30, start).source,
            ChangeSource::Ui
        );
    }

    #[test]
    fn the_queue_is_bounded() {
        let mut echoes = Echoes::new();
        let start = now();
        for value in 0..(CAPACITY as u32 * 4) {
            // A distinct source is not available, so distinct fields are used
            // in rotation to defeat the same-source replacement.
            let field = match value % 4 {
                0 => Field::SinkVolume,
                1 => Field::SourceVolume,
                2 => Field::Brightness,
                _ => Field::SinkMute,
            };
            echoes.record(field, value, ChangeSource::Cli, start);
        }
        assert!(echoes.len() <= CAPACITY);
    }

    #[test]
    fn serials_only_ever_go_up() {
        let mut echoes = Echoes::new();
        let start = now();
        let first = echoes.attribute(Field::SinkVolume, 10, start).serial;
        let second = echoes.attribute(Field::SinkVolume, 10, start).serial;
        assert_eq!(second, first + 1, "the same value is still a new change");
    }

    #[test]
    fn clearing_forgets_everything_a_restart_orphaned() {
        let mut echoes = Echoes::new();
        let start = now();
        echoes.record(Field::SinkVolume, 40, ChangeSource::Ui, start);
        echoes.clear();
        assert_eq!(
            echoes.attribute(Field::SinkVolume, 40, start).source,
            ChangeSource::External
        );
    }

    #[test]
    fn only_the_cli_and_the_world_raise_the_capsule() {
        assert!(ChangeSource::Cli.shows_osd());
        assert!(ChangeSource::External.shows_osd());
        assert!(
            !ChangeSource::Ui.shows_osd(),
            "the slider is its own feedback"
        );
    }
}
