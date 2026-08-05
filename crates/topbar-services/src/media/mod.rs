//! MPRIS media players: who is on the bus, what they are playing, and the
//! five things the panel may ask of them.
//!
//! ```text
//!   proxy.rs      the trimmed MPRIS interfaces
//!   props.rs      the wire format, parsed once, at the edge
//!   relevance.rs  which player the card shows            (pure)
//!   art.rs        when to fetch a cover and what to keep (pure + I/O)
//!   model.rs      the published snapshot                 (pure)
//!   task.rs       the one owner of all of it
//! ```
//!
//! Every player on the bus is followed at once, so switching between them is
//! instant and the switcher can show what each of them is doing. Nothing here
//! polls unless it has to: property changes arrive as signals, and the only
//! poll — `Position`, which MPRIS never signals — runs solely while a track is
//! playing *and* the panel has a popover open on it.

mod art;
mod model;
mod props;
mod proxy;
mod relevance;
mod task;

pub mod cli;

#[cfg(any(test, feature = "fake-player"))]
pub mod fake;

#[cfg(test)]
mod bus_tests;

use std::sync::Arc;

use tokio::sync::{mpsc, oneshot, watch};

use crate::error::SvcError;

pub use model::{ArtRef, MediaState, PlaybackStatus, PlayerView, advance};

use task::{Command, Control};

/// The bus-name prefix every MPRIS player registers under.
pub(crate) const MPRIS_PREFIX: &str = "org.mpris.MediaPlayer2.";
/// The object path every MPRIS player serves its interfaces at.
pub(crate) const MPRIS_PATH: &str = "/org/mpris/MediaPlayer2";

/// How many commands may be in flight before a sender waits.
const QUEUE: usize = 32;

/// The media service.
///
/// Cloning is cheap — a channel sender and a watch subscription — so every
/// widget that wants media state can hold its own copy.
#[derive(Clone)]
pub struct Media {
    handle: MediaHandle,
    state: watch::Receiver<Arc<MediaState>>,
    task: crate::lazy::Deferred,
}

impl Media {
    /// Start following the players on the bus.
    ///
    /// `address` overrides the session bus; production passes `None` and the
    /// tests pass a private bus, which is what keeps a test run from taking
    /// over the music the developer is listening to.
    /// `wanted` is whether anything draws media: today that is the clock's
    /// control panel. A panel with no control panel follows no players.
    pub(crate) fn start(address: Option<String>, wanted: bool) -> Self {
        let (commands, queue) = mpsc::channel(QUEUE);
        let (publisher, state) = watch::channel(Arc::new(MediaState::default()));
        let task = crate::lazy::Deferred::spawn(wanted, task::run(queue, publisher, address));
        Self {
            handle: MediaHandle { commands },
            state,
            task,
        }
    }

    /// Start the task if it was held back. Returns whether this call did it.
    pub(crate) fn ensure_started(&self) -> bool {
        self.task.start()
    }

    /// The handle commands are sent through.
    pub fn handle(&self) -> &MediaHandle {
        &self.handle
    }

    /// Subscribe to media state.
    pub fn state(&self) -> watch::Receiver<Arc<MediaState>> {
        self.state.clone()
    }
}

/// What the panel may ask of the players.
///
/// Every call returns a `Result` so a failure is reported rather than dropped
/// in a click handler — see `bridge::act` on the GTK side.
#[derive(Clone)]
pub struct MediaHandle {
    commands: mpsc::Sender<Command>,
}

impl MediaHandle {
    /// Play the active player if it is paused, pause it if it is playing.
    pub async fn play_pause(&self) -> Result<(), SvcError> {
        self.control(Control::PlayPause).await
    }

    /// Skip to the next track.
    pub async fn next(&self) -> Result<(), SvcError> {
        self.control(Control::Next).await
    }

    /// Skip to the previous track.
    pub async fn previous(&self) -> Result<(), SvcError> {
        self.control(Control::Previous).await
    }

    /// Bring the player's own window forward, GNOME style.
    pub async fn raise(&self) -> Result<(), SvcError> {
        self.control(Control::Raise).await
    }

    /// Jump to `position_us` microseconds into the current track.
    pub async fn seek_to(&self, position_us: i64) -> Result<(), SvcError> {
        self.control(Control::SeekTo(position_us)).await
    }

    /// Show `bus_name` until that player goes away.
    ///
    /// The pin lasts exactly as long as the player it names: quitting the
    /// player hands the card back to whatever is playing, which is what the
    /// user meant by quitting it.
    pub async fn select_player(&self, bus_name: String) -> Result<(), SvcError> {
        let (reply, answer) = oneshot::channel();
        self.send(Command::Select(bus_name, reply)).await?;
        answer
            .await
            .map_err(|_| SvcError::ServiceStopped("media"))?
    }

    /// Say whether the panel is looking at the playback position.
    ///
    /// The seek bar is the only thing that needs it, and it only exists while
    /// a popover is open, so this is what keeps the panel from asking every
    /// player where it is once a second all day. Switching it on also polls
    /// once immediately, so the bar is right on the frame it appears.
    pub async fn set_position_tracking(&self, tracking: bool) -> Result<(), SvcError> {
        self.send(Command::Tracking(tracking)).await
    }

    /// Send a control command and wait for the task to accept it.
    ///
    /// "Accepted" means the player exists and the call is on its way; the call
    /// itself is not awaited, because a player that has stopped answering must
    /// not leave a button waiting on it.
    async fn control(&self, control: Control) -> Result<(), SvcError> {
        let (reply, answer) = oneshot::channel();
        self.send(Command::Control(control, reply)).await?;
        answer
            .await
            .map_err(|_| SvcError::ServiceStopped("media"))?
    }

    /// Post a command, or report that the service has stopped.
    async fn send(&self, command: Command) -> Result<(), SvcError> {
        self.commands
            .send(command)
            .await
            .map_err(|_| SvcError::ServiceStopped("media"))
    }
}
