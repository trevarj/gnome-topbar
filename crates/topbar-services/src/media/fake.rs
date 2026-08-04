//! A media player that exists only to be tested against.
//!
//! It serves the same two interfaces a real player does — enough of them for
//! the panel to treat it as one — plus a small control interface so a test (or
//! the visual smoke driver) can make it do things no button on the panel can:
//! change its track, hand it a different cover, or take a capability away.
//!
//! It is used two ways, and deliberately only written once:
//!
//! - the bus tests drive it in-process over a private `dbus-daemon`;
//! - `topbar-fake-player` (`--features fake-player`) runs it as a program, so
//!   the nested-niri smoke run can put two of them on its private bus.
//!
//! ```text
//! org.mpris.MediaPlayer2.<name>          the well-known name
//!   /org/mpris/MediaPlayer2
//!     org.mpris.MediaPlayer2             Identity, DesktopEntry, Raise
//!     org.mpris.MediaPlayer2.Player      the playback interface
//!     io.github.trevarj.topbar.FakePlayer1   SetTrack, SetStatus, Quit
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Notify;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};
use zbus::{Connection, interface};

use super::{MPRIS_PATH, MPRIS_PREFIX};

/// The control interface, so a test can move the fake player about.
pub const CONTROL_INTERFACE: &str = "io.github.trevarj.topbar.FakePlayer1";

/// How a fake player starts out.
#[derive(Debug, Clone)]
pub struct Recipe {
    /// The tail of the bus name, e.g. `fakeone`.
    pub name: String,
    /// What it calls itself.
    pub identity: String,
    /// The desktop entry it claims, if any.
    pub desktop_entry: Option<String>,
    /// `Playing`, `Paused` or `Stopped`.
    pub status: String,
    /// Track title.
    pub title: String,
    /// Track artist.
    pub artist: String,
    /// Album name.
    pub album: String,
    /// `mpris:artUrl`, if it has one.
    pub art_url: Option<String>,
    /// Track length in microseconds.
    pub length_us: i64,
    /// Where the track has got to.
    pub position_us: i64,
    /// Whether it offers a next track.
    pub can_go_next: bool,
    /// Whether it offers a previous one.
    pub can_go_previous: bool,
    /// Whether it may be seeked.
    pub can_seek: bool,
}

impl Default for Recipe {
    fn default() -> Self {
        Self {
            name: "fake".to_string(),
            identity: "Fake Player".to_string(),
            desktop_entry: None,
            status: "Paused".to_string(),
            title: "Untitled".to_string(),
            artist: "Nobody".to_string(),
            album: String::new(),
            art_url: None,
            length_us: 240_000_000,
            position_us: 0,
            can_go_next: true,
            can_go_previous: true,
            can_seek: true,
        }
    }
}

impl Recipe {
    /// The well-known name this player takes.
    pub fn bus_name(&self) -> String {
        format!("{MPRIS_PREFIX}{}", self.name)
    }
}

/// The root interface: who the player is.
struct Application {
    identity: String,
    desktop_entry: String,
    raised: Arc<Notify>,
}

#[interface(name = "org.mpris.MediaPlayer2")]
impl Application {
    /// Note that something asked the player to come forward.
    fn raise(&self) {
        self.raised.notify_waiters();
    }

    #[zbus(property)]
    fn identity(&self) -> &str {
        &self.identity
    }

    #[zbus(property)]
    fn desktop_entry(&self) -> &str {
        &self.desktop_entry
    }
}

/// The playback interface, and the state behind it.
struct Player {
    status: String,
    title: String,
    artist: String,
    album: String,
    art_url: Option<String>,
    length_us: i64,
    position_us: i64,
    track: u32,
    can_go_next: bool,
    can_go_previous: bool,
    can_seek: bool,
    /// Bumped by every command, so a test can wait for one to land.
    acted: Arc<Notify>,
    /// The last command that arrived, for the round-trip tests.
    last: Arc<std::sync::Mutex<Option<String>>>,
}

impl Player {
    /// The `Metadata` dictionary this player's current track produces.
    fn track_metadata(&self) -> HashMap<String, OwnedValue> {
        let mut metadata = HashMap::new();
        let path = ObjectPath::try_from(format!("/io/github/trevarj/topbar/track/{}", self.track))
            .expect("a generated track path is well formed");
        insert(&mut metadata, "mpris:trackid", Value::ObjectPath(path));
        insert(&mut metadata, "mpris:length", Value::I64(self.length_us));
        insert(
            &mut metadata,
            "xesam:title",
            Value::from(self.title.clone()),
        );
        insert(
            &mut metadata,
            "xesam:artist",
            Value::from(vec![self.artist.clone()]),
        );
        if !self.album.is_empty() {
            insert(
                &mut metadata,
                "xesam:album",
                Value::from(self.album.clone()),
            );
        }
        if let Some(art_url) = self.art_url.clone() {
            insert(&mut metadata, "mpris:artUrl", Value::from(art_url));
        }
        metadata
    }

    /// Record that a command arrived.
    fn note(&self, what: &str) {
        if let Ok(mut last) = self.last.lock() {
            *last = Some(what.to_string());
        }
        self.acted.notify_waiters();
    }
}

/// Add one metadata entry, ignoring values that cannot be owned.
fn insert(metadata: &mut HashMap<String, OwnedValue>, key: &str, value: Value<'_>) {
    if let Ok(value) = OwnedValue::try_from(value) {
        metadata.insert(key.to_string(), value);
    }
}

#[interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    /// Toggle between playing and paused, announcing the change.
    async fn play_pause(&mut self, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>) {
        self.status = if self.status == "Playing" {
            "Paused".to_string()
        } else {
            "Playing".to_string()
        };
        self.note("PlayPause");
        let _ = self.playback_status_changed(&emitter).await;
    }

    /// Move to the next track, announcing the new metadata.
    async fn next(&mut self, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>) {
        self.track = self.track.wrapping_add(1);
        self.title = format!("Track {}", self.track);
        self.position_us = 0;
        self.note("Next");
        let _ = self.metadata_changed(&emitter).await;
    }

    /// Move to the previous track.
    async fn previous(&mut self, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>) {
        self.track = self.track.wrapping_sub(1);
        self.title = format!("Track {}", self.track);
        self.position_us = 0;
        self.note("Previous");
        let _ = self.metadata_changed(&emitter).await;
    }

    /// Jump to a position within a named track.
    fn set_position(&mut self, _track_id: ObjectPath<'_>, position: i64) {
        self.position_us = position;
        self.note("SetPosition");
    }

    /// Move the position by an offset.
    fn seek(&mut self, offset: i64) {
        self.position_us = (self.position_us + offset).max(0);
        self.note("Seek");
    }

    #[zbus(property)]
    fn playback_status(&self) -> &str {
        &self.status
    }

    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        self.track_metadata()
    }

    /// Deliberately *not* signalled, exactly like a real player.
    #[zbus(property(emits_changed_signal = "false"))]
    fn position(&self) -> i64 {
        self.position_us
    }

    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn can_play(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_pause(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        self.can_go_next
    }

    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        self.can_go_previous
    }

    #[zbus(property)]
    fn can_seek(&self) -> bool {
        self.can_seek
    }
}

/// The control interface: what a test may do that a user could not.
struct Control {
    stopped: Arc<Notify>,
}

#[interface(name = "io.github.trevarj.topbar.FakePlayer1")]
impl Control {
    /// Replace the current track, cover and all.
    ///
    /// An empty `art_url` means "this track has no cover", which is how the
    /// clearing path is exercised.
    async fn set_track(
        &self,
        title: String,
        artist: String,
        art_url: String,
        length_us: i64,
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> zbus::fdo::Result<()> {
        let iface = server.interface::<_, Player>(MPRIS_PATH).await?;
        let mut player = iface.get_mut().await;
        player.track = player.track.wrapping_add(1);
        player.title = title;
        player.artist = artist;
        player.art_url = (!art_url.is_empty()).then_some(art_url);
        player.length_us = length_us;
        player.position_us = 0;
        player.metadata_changed(iface.signal_emitter()).await?;
        Ok(())
    }

    /// Set the playback status, announcing it the way a real player would.
    async fn set_status(
        &self,
        status: String,
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> zbus::fdo::Result<()> {
        let iface = server.interface::<_, Player>(MPRIS_PATH).await?;
        let mut player = iface.get_mut().await;
        player.status = status;
        player
            .playback_status_changed(iface.signal_emitter())
            .await?;
        Ok(())
    }

    /// Set a capability flag, so the panel's disabled states can be seen.
    async fn set_capability(
        &self,
        name: String,
        allowed: bool,
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> zbus::fdo::Result<()> {
        let iface = server.interface::<_, Player>(MPRIS_PATH).await?;
        let mut player = iface.get_mut().await;
        match name.as_str() {
            "CanGoNext" => player.can_go_next = allowed,
            "CanGoPrevious" => player.can_go_previous = allowed,
            "CanSeek" => player.can_seek = allowed,
            other => {
                return Err(zbus::fdo::Error::InvalidArgs(format!(
                    "no such capability: {other}"
                )));
            }
        }
        player.can_go_next_changed(iface.signal_emitter()).await?;
        player
            .can_go_previous_changed(iface.signal_emitter())
            .await?;
        player.can_seek_changed(iface.signal_emitter()).await?;
        Ok(())
    }

    /// Leave the bus.
    fn quit(&self) {
        self.stopped.notify_waiters();
    }
}

/// A fake player that is on the bus for as long as this value lives.
pub struct FakePlayer {
    connection: Connection,
    bus_name: String,
    acted: Arc<Notify>,
    raised: Arc<Notify>,
    stopped: Arc<Notify>,
    last: Arc<std::sync::Mutex<Option<String>>>,
}

impl FakePlayer {
    /// Put a player on the bus at `address`, or on the session bus.
    pub async fn start(recipe: &Recipe, address: Option<&str>) -> zbus::Result<Self> {
        let acted = Arc::new(Notify::new());
        let raised = Arc::new(Notify::new());
        let stopped = Arc::new(Notify::new());
        let last = Arc::new(std::sync::Mutex::new(None));

        let builder = match address {
            Some(address) => zbus::connection::Builder::address(address)?,
            None => zbus::connection::Builder::session()?,
        };

        let bus_name = recipe.bus_name();
        let connection = builder
            .name(bus_name.clone())?
            .serve_at(
                MPRIS_PATH,
                Application {
                    identity: recipe.identity.clone(),
                    desktop_entry: recipe.desktop_entry.clone().unwrap_or_default(),
                    raised: Arc::clone(&raised),
                },
            )?
            .serve_at(
                MPRIS_PATH,
                Player {
                    status: recipe.status.clone(),
                    title: recipe.title.clone(),
                    artist: recipe.artist.clone(),
                    album: recipe.album.clone(),
                    art_url: recipe.art_url.clone(),
                    length_us: recipe.length_us,
                    position_us: recipe.position_us,
                    track: 1,
                    can_go_next: recipe.can_go_next,
                    can_go_previous: recipe.can_go_previous,
                    can_seek: recipe.can_seek,
                    acted: Arc::clone(&acted),
                    last: Arc::clone(&last),
                },
            )?
            .serve_at(
                MPRIS_PATH,
                Control {
                    stopped: Arc::clone(&stopped),
                },
            )?
            .build()
            .await?;

        Ok(Self {
            connection,
            bus_name,
            acted,
            raised,
            stopped,
            last,
        })
    }

    /// The well-known name this player took.
    pub fn bus_name(&self) -> &str {
        &self.bus_name
    }

    /// The connection it is serving on, for tests that want to poke it.
    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// Wait until the panel sends the player a command.
    pub async fn acted(&self) -> String {
        loop {
            let waiter = self.acted.notified();
            if let Some(last) = self.last.lock().ok().and_then(|last| last.clone()) {
                return last;
            }
            waiter.await;
        }
    }

    /// Wait until something asks the player to come forward.
    pub async fn raised(&self) {
        self.raised.notified().await;
    }

    /// Wait until `Quit` is called on the control interface.
    pub async fn stopped(&self) {
        self.stopped.notified().await;
    }

    /// What the player is doing right now.
    pub async fn status(&self) -> String {
        let Ok(iface) = self
            .connection
            .object_server()
            .interface::<_, Player>(MPRIS_PATH)
            .await
        else {
            return String::new();
        };
        iface.get().await.status.clone()
    }

    /// Where the player thinks it is, in microseconds.
    pub async fn position(&self) -> i64 {
        let Ok(iface) = self
            .connection
            .object_server()
            .interface::<_, Player>(MPRIS_PATH)
            .await
        else {
            return 0;
        };
        iface.get().await.position_us
    }

    /// Leave the bus, as if the application had quit.
    pub async fn quit(self) {
        let _ = self.connection.release_name(self.bus_name.as_str()).await;
        drop(self.connection);
    }
}
