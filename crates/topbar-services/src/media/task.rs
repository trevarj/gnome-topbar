//! The one task that owns media state.
//!
//! ```text
//!   the bus ──NameOwnerChanged──▶ discover()  ─┐
//!                                              ├─▶ task.rs (the only owner)
//!   a player ──PropertiesChanged──▶ watch()  ──┘      │ ▲
//!                                                     │ └── relevance.rs, art.rs
//!                                                     ▼
//!   panel widgets ◀──────────────── watch<Arc<MediaState>>
//! ```
//!
//! One task per player reads that player and nothing else, and every answer —
//! a property change, a position poll, a downloaded cover — arrives here as a
//! message and is applied in order. Nothing awaits a player from inside the
//! loop: a browser that has stopped answering delays its own watcher task, not
//! the panel.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};
use zbus::Connection;
use zbus::fdo::{DBusProxy, PropertiesProxy};
use zbus::proxy::CacheProperties;
use zbus::zvariant::ObjectPath;

use crate::error::SvcError;

use super::art::{self, ArtCache, ArtDebounce};
use super::model::{ArtRef, MediaState, PlayerView, identity_from_bus_name};
use super::props::{PLAYER_INTERFACE, PlayerDelta};
use super::proxy::{ApplicationProxy, PlayerProxy};
use super::relevance::{self, Candidate};
use super::{MPRIS_PATH, MPRIS_PREFIX};

/// How often the position is read while a track plays and the panel looks.
const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// How long to wait after a track or status change before re-reading the
/// position. Players report the previous track's position for a moment after
/// they announce the new one.
const SETTLE: Duration = Duration::from_millis(120);
/// How long any single call to a player may take before it is given up on.
///
/// A player that does not answer must not leave a button spinning forever, and
/// must never hold a task open after the player itself has gone.
const CALL_TIMEOUT: Duration = Duration::from_secs(5);

/// Something the panel asks of the player it is showing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Control {
    /// Play if paused, pause if playing.
    PlayPause,
    /// Next track.
    Next,
    /// Previous track.
    Previous,
    /// Bring the player's own window forward.
    Raise,
    /// Jump to a position, in microseconds.
    SeekTo(i64),
}

/// Everything that can reach the media task.
#[derive(Debug)]
pub(super) enum Command {
    /// Act on the active player.
    Control(Control, oneshot::Sender<Result<(), SvcError>>),
    /// Show this player until it goes away.
    Select(String, oneshot::Sender<Result<(), SvcError>>),
    /// Whether the panel is looking at the position right now.
    Tracking(bool),
}

/// Everything the bus tells the media task.
enum Event {
    /// A name matching `org.mpris.MediaPlayer2.*` took an owner.
    Appeared(String),
    /// It lost its owner, or its watcher gave up on it.
    Vanished(String),
    /// A player answered its first questions.
    Ready(Box<Ready>),
    /// A player's properties changed.
    Changed(String, Box<PlayerDelta>),
    /// A position poll came back.
    Position(String, i64, Instant),
    /// An art fetch finished, for better or worse.
    Art(String, String, Option<ArtRef>),
}

/// What a watcher learned before it started listening.
struct Ready {
    bus_name: String,
    player: PlayerProxy<'static>,
    application: ApplicationProxy<'static>,
    identity: String,
    desktop_entry: Option<String>,
    delta: PlayerDelta,
}

/// One player the task is following.
struct Player {
    view: PlayerView,
    /// `None` until the player has answered its first questions.
    player: Option<PlayerProxy<'static>>,
    application: Option<ApplicationProxy<'static>>,
    /// `mpris:trackid`, so a seek cannot land in the wrong track.
    track_id: Option<String>,
    /// The art URL waiting out its grace period.
    art: ArtDebounce,
    /// The art URL the view currently reflects, so a fetch that finishes after
    /// the track moved on is dropped instead of drawn.
    art_url: Option<String>,
    /// When the status last changed, and when the player appeared.
    status_seq: u64,
    appeared_seq: u64,
    watcher: JoinHandle<()>,
}

impl Player {
    /// What the relevance rules need to know about this player.
    fn candidate(&self) -> Candidate {
        Candidate {
            status: self.view.status,
            has_track: self.view.has_track(),
            status_seq: self.status_seq,
            appeared_seq: self.appeared_seq,
        }
    }
}

/// The media task's whole state.
struct Media {
    connection: Connection,
    /// Every player, in the order they appeared.
    players: Vec<Player>,
    /// Index of the one the panel shows.
    active: Option<usize>,
    /// The player the user picked, until it goes away.
    pinned: Option<String>,
    /// Whether the panel is looking at the position right now.
    tracking: bool,
    /// When the next position poll is due.
    next_poll: Option<Instant>,
    /// Stamped on appearances and status changes; the relevance tie-breaker.
    seq: u64,
    cache: ArtCache,
    events: mpsc::Sender<Event>,
    publisher: watch::Sender<Arc<MediaState>>,
}

/// Connect to the bus and run the media task until every handle is dropped.
///
/// `address` overrides the session bus, which is how the integration tests
/// reach a private one instead of the developer's live desktop.
pub(super) async fn run(
    commands: mpsc::Receiver<Command>,
    publisher: watch::Sender<Arc<MediaState>>,
    address: Option<String>,
) {
    let built = match address {
        Some(address) => zbus::connection::Builder::address(address.as_str()),
        None => zbus::connection::Builder::session(),
    };
    let connection = match connect(built).await {
        Ok(connection) => connection,
        Err(error) => {
            // Not fatal, and not even loud: a session with no bus has no
            // players either, and the media card simply never appears.
            warn!("no media players will be found: {error}");
            return;
        }
    };

    let (events_tx, events_rx) = mpsc::channel(64);
    tokio::spawn(discover(connection.clone(), events_tx.clone()));

    Media {
        connection,
        players: Vec::new(),
        active: None,
        pinned: None,
        tracking: false,
        next_poll: None,
        seq: 0,
        cache: ArtCache::open(),
        events: events_tx,
        publisher,
    }
    .serve(commands, events_rx)
    .await;
}

/// Build the connection the media task uses for everything.
async fn connect(builder: zbus::Result<zbus::connection::Builder<'_>>) -> zbus::Result<Connection> {
    builder?.build().await
}

impl Media {
    /// Apply messages until the panel drops its last handle.
    async fn serve(
        mut self,
        mut commands: mpsc::Receiver<Command>,
        mut events: mpsc::Receiver<Event>,
    ) {
        self.publish();
        loop {
            let art_due = self
                .players
                .iter()
                .filter_map(|player| player.art.due())
                .min();
            tokio::select! {
                command = commands.recv() => match command {
                    Some(command) => self.apply_command(command),
                    None => break,
                },
                event = events.recv() => match event {
                    Some(event) => self.apply_event(event),
                    // Only possible if this task dropped its own sender.
                    None => break,
                },
                () = sleep_until(self.next_poll) => self.poll_position(),
                () = sleep_until(art_due) => self.settle_art(),
            }
            self.resync_polling();
            self.publish();
        }

        for player in &self.players {
            player.watcher.abort();
        }
        debug!("the media service is shutting down");
    }

    // -----------------------------------------------------------------------
    // Commands
    // -----------------------------------------------------------------------

    /// Apply one command from the panel.
    fn apply_command(&mut self, command: Command) {
        match command {
            Command::Control(control, reply) => {
                let _ = reply.send(self.control(control));
            }
            Command::Select(bus_name, reply) => {
                let _ = reply.send(self.select(bus_name));
            }
            Command::Tracking(tracking) => {
                if self.tracking != tracking {
                    debug!(
                        "position tracking is {}",
                        if tracking { "on" } else { "off" }
                    );
                    self.tracking = tracking;
                }
                // Even a repeated request polls once: the panel sends this
                // when its popover opens, and the position on screen has to be
                // right on the frame it appears, not a second later.
                if tracking {
                    self.next_poll = Some(Instant::now());
                }
            }
        }
    }

    /// Act on the active player.
    fn control(&mut self, control: Control) -> Result<(), SvcError> {
        let now = Instant::now();
        let Some(player) = self.active.and_then(|index| self.players.get_mut(index)) else {
            return Err(SvcError::NoPlayer("nothing is playing".into()));
        };
        let bus_name = player.view.bus_name.clone();
        let Some(proxy) = player.player.clone() else {
            return Err(SvcError::NoPlayer(bus_name));
        };
        let application = player.application.clone();
        let track_id = player.track_id.clone();
        let current = player.view.position_at(now);

        if let Control::SeekTo(position) = control {
            // Optimistic: the thumb stays where the user left it rather than
            // snapping back until the next poll agrees with it.
            player.view.position_us = position.max(0);
            player.view.sampled_at = now;
        }

        tokio::spawn(async move {
            let call = async {
                match control {
                    Control::PlayPause => proxy.play_pause().await,
                    Control::Next => proxy.next().await,
                    Control::Previous => proxy.previous().await,
                    Control::Raise => match application {
                        Some(application) => application.raise().await,
                        None => Ok(()),
                    },
                    Control::SeekTo(position) => seek(&proxy, track_id, current, position).await,
                }
            };
            match tokio::time::timeout(CALL_TIMEOUT, call).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => warn!("{bus_name} refused {control:?}: {error}"),
                Err(_) => warn!("{bus_name} did not answer {control:?}"),
            }
        });
        Ok(())
    }

    /// Pin a player, for as long as it is on the bus.
    fn select(&mut self, bus_name: String) -> Result<(), SvcError> {
        if !self
            .players
            .iter()
            .any(|player| player.view.bus_name == bus_name)
        {
            return Err(SvcError::NoPlayer(bus_name));
        }
        info!("showing {bus_name} until it goes away");
        self.pinned = Some(bus_name);
        self.reselect();
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Events
    // -----------------------------------------------------------------------

    /// Apply one message from the bus.
    fn apply_event(&mut self, event: Event) {
        let now = Instant::now();
        match event {
            Event::Appeared(bus_name) => self.add(bus_name),
            Event::Vanished(bus_name) => self.remove(&bus_name),
            Event::Ready(ready) => self.introduce(*ready, now),
            Event::Changed(bus_name, delta) => self.change(&bus_name, *delta, now),
            Event::Position(bus_name, position_us, at) => {
                if let Some(player) = self.player_mut(&bus_name) {
                    player.view.position_us = position_us.max(0);
                    player.view.sampled_at = at;
                }
            }
            Event::Art(bus_name, url, art) => self.arrived_art(&bus_name, &url, art),
        }
    }

    /// A player appeared on the bus.
    fn add(&mut self, bus_name: String) {
        if self.player_mut(&bus_name).is_some() {
            return;
        }
        debug!("media player {bus_name} appeared");
        let seq = self.next_seq();
        let watcher = tokio::spawn(watch_player(
            bus_name.clone(),
            self.connection.clone(),
            self.events.clone(),
        ));
        self.players.push(Player {
            view: PlayerView::new(bus_name),
            player: None,
            application: None,
            track_id: None,
            art: ArtDebounce::default(),
            art_url: None,
            status_seq: seq,
            appeared_seq: seq,
            watcher,
        });
        self.reselect();
    }

    /// A player went away.
    fn remove(&mut self, bus_name: &str) {
        let Some(index) = self
            .players
            .iter()
            .position(|player| player.view.bus_name == bus_name)
        else {
            return;
        };
        debug!("media player {bus_name} went away");
        self.players.remove(index).watcher.abort();
        // The pin lasts exactly as long as the player it names.
        if self.pinned.as_deref() == Some(bus_name) {
            self.pinned = None;
        }
        self.reselect();
    }

    /// A player answered its first questions.
    fn introduce(&mut self, ready: Ready, now: Instant) {
        let seq = self.next_seq();
        let Some(player) = self.player_mut(&ready.bus_name) else {
            return;
        };
        info!("media player {} is {}", ready.bus_name, ready.identity);
        player.player = Some(ready.player);
        player.application = Some(ready.application);
        player.view.identity = ready.identity;
        player.view.desktop_entry = ready.desktop_entry;
        apply_delta(player, ready.delta, now, seq);
        self.reselect();
        self.poll_soon(now);
    }

    /// A player's properties changed.
    fn change(&mut self, bus_name: &str, delta: PlayerDelta, now: Instant) {
        let seq = self.next_seq();
        let Some(player) = self.player_mut(bus_name) else {
            return;
        };
        let before = player.view.status;
        apply_delta(player, delta, now, seq);
        let moved = player.view.status != before;
        self.reselect();
        if moved {
            self.resync_polling();
        }
        self.poll_soon(now);
    }

    /// An art fetch finished.
    fn arrived_art(&mut self, bus_name: &str, url: &str, art: Option<ArtRef>) {
        if let Some(art) = art.as_ref() {
            self.cache.record(art);
        }
        let Some(player) = self.player_mut(bus_name) else {
            return;
        };
        // The track moved on while the download was in flight: what came back
        // is a cover for a track nobody is looking at any more.
        if player.art_url.as_deref() != Some(url) {
            return;
        }
        if art.is_none() {
            debug!("no album art for {bus_name}");
        }
        player.view.art = art;
    }

    /// Act on every art URL whose grace period has run out.
    fn settle_art(&mut self) {
        let now = Instant::now();
        let mut wanted: Vec<(String, Option<String>)> = Vec::new();
        for player in &mut self.players {
            if let Some(url) = player.art.take_due(now) {
                player.art_url.clone_from(&url);
                wanted.push((player.view.bus_name.clone(), url));
            }
        }

        for (bus_name, url) in wanted {
            let Some(url) = url else {
                if let Some(player) = self.player_mut(&bus_name) {
                    player.view.art = None;
                }
                continue;
            };

            let key = art::key_for(&url);
            if let Some(path) = self.cache.hit(key)
                && let Some(player) = self.player_mut(&bus_name)
            {
                player.view.art = Some(ArtRef { key, path });
                continue;
            }

            // The old cover stays on screen while the new one is fetched:
            // blanking it first would flash a placeholder on every track.
            let destination = self.cache.path(key);
            let events = self.events.clone();
            tokio::spawn(async move {
                let art = art::fetch(url.clone(), destination).await;
                let _ = events.send(Event::Art(bus_name, url, art)).await;
            });
        }
    }

    // -----------------------------------------------------------------------
    // Position polling
    // -----------------------------------------------------------------------

    /// Whether the position is worth asking for.
    ///
    /// Both halves matter: a paused player's position does not move, and a
    /// closed panel is not looking at it. Between them, the panel makes no
    /// D-Bus calls at all while the user is working.
    fn should_poll(&self) -> bool {
        self.tracking
            && self
                .active
                .and_then(|index| self.players.get(index))
                .is_some_and(|player| player.view.status.is_playing())
    }

    /// Start or stop the poll timer to match the current state.
    fn resync_polling(&mut self) {
        if self.should_poll() {
            self.next_poll.get_or_insert_with(Instant::now);
        } else {
            self.next_poll = None;
        }
    }

    /// Bring the next poll forward, after something that moves the position.
    fn poll_soon(&mut self, now: Instant) {
        if !self.should_poll() {
            return;
        }
        let when = now + SETTLE;
        self.next_poll = Some(self.next_poll.map_or(when, |next| next.min(when)));
    }

    /// Ask the active player where it has got to.
    fn poll_position(&mut self) {
        self.next_poll = Some(Instant::now() + POLL_INTERVAL);
        let Some(player) = self.active.and_then(|index| self.players.get(index)) else {
            return;
        };
        let Some(proxy) = player.player.clone() else {
            return;
        };
        let bus_name = player.view.bus_name.clone();
        let events = self.events.clone();
        tokio::spawn(async move {
            let Ok(Ok(position)) = tokio::time::timeout(CALL_TIMEOUT, proxy.position()).await
            else {
                return;
            };
            let _ = events
                .send(Event::Position(bus_name, position, Instant::now()))
                .await;
        });
    }

    // -----------------------------------------------------------------------
    // Selection and publishing
    // -----------------------------------------------------------------------

    /// Work out which player the card shows.
    fn reselect(&mut self) {
        let pinned = self.pinned.as_ref().and_then(|bus_name| {
            self.players
                .iter()
                .position(|player| &player.view.bus_name == bus_name)
        });
        if self.pinned.is_some() && pinned.is_none() {
            self.pinned = None;
        }
        let candidates: Vec<Candidate> = self.players.iter().map(Player::candidate).collect();
        self.active = relevance::select(&candidates, pinned);
    }

    /// Publish the snapshot, unless nothing the panel draws has changed.
    fn publish(&self) {
        let state = MediaState {
            players: self
                .players
                .iter()
                .map(|player| player.view.clone())
                .collect(),
            active: self.active,
        };
        self.publisher.send_if_modified(|current| {
            if **current == state {
                return false;
            }
            *current = Arc::new(state);
            true
        });
    }

    /// The player with this bus name, if it is still here.
    fn player_mut(&mut self, bus_name: &str) -> Option<&mut Player> {
        self.players
            .iter_mut()
            .find(|player| player.view.bus_name == bus_name)
    }

    /// The next tie-breaking stamp.
    fn next_seq(&mut self) -> u64 {
        self.seq = self.seq.wrapping_add(1);
        self.seq
    }
}

/// Fold a delta into a player's view.
///
/// `seq` is spent only when the status actually moves, which is what makes the
/// relevance tie-breaker mean "most recently started or stopped" rather than
/// "most recently said anything at all".
fn apply_delta(player: &mut Player, delta: PlayerDelta, now: Instant, seq: u64) {
    if let Some(status) = delta.status
        && player.view.status != status
    {
        player.view.status = status;
        player.status_seq = seq;
    }

    if let Some(track) = delta.metadata {
        let changed = track.track_id != player.track_id || track.title != player.view.title;
        player.view.title = track.title;
        player.view.artist = track.artist;
        player.view.album = track.album;
        player.view.length_us = track.length_us;
        player.track_id = track.track_id;
        player.art.want(track.art_url.as_deref(), now);
        if changed {
            // A new track starts at the beginning; waiting for the next poll
            // to say so would leave the old track's position on the bar.
            player.view.position_us = 0;
            player.view.sampled_at = now;
        }
    }

    if let Some(position) = delta.position_us {
        player.view.position_us = position.max(0);
        player.view.sampled_at = now;
    }
    if let Some(rate) = delta.rate {
        player.view.rate = rate;
    }
    if let Some(can) = delta.can_play {
        player.view.can_play = can;
    }
    if let Some(can) = delta.can_pause {
        player.view.can_pause = can;
    }
    if let Some(can) = delta.can_go_next {
        player.view.can_go_next = can;
    }
    if let Some(can) = delta.can_go_previous {
        player.view.can_go_previous = can;
    }
    if let Some(can) = delta.can_seek {
        player.view.can_seek = can;
    }
}

/// Jump to `position`, by track id where the player offers one.
///
/// `SetPosition` names the track it applies to, so a seek that arrives just
/// after the track changed is ignored by the player rather than applied to the
/// wrong song. `Seek` has no such guard, which is why it is the fallback.
async fn seek(
    proxy: &PlayerProxy<'static>,
    track_id: Option<String>,
    current: i64,
    position: i64,
) -> zbus::Result<()> {
    match track_id.and_then(|id| ObjectPath::try_from(id).ok()) {
        Some(track) => proxy.set_position(&track, position.max(0)).await,
        None => proxy.seek(position - current).await,
    }
}

/// Sleep until `deadline`, or forever when there is nothing to wait for.
async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline.into()).await,
        None => std::future::pending().await,
    }
}

// ---------------------------------------------------------------------------
// The bus side
// ---------------------------------------------------------------------------

/// Find the players that are already here, then follow the ones that arrive.
///
/// Subscribing before listing is deliberate: doing it the other way round
/// loses a player that starts up in between.
async fn discover(connection: Connection, events: mpsc::Sender<Event>) {
    let dbus = match DBusProxy::new(&connection).await {
        Ok(proxy) => proxy,
        Err(error) => {
            warn!("cannot watch the bus for media players: {error}");
            return;
        }
    };
    let mut changes = match dbus.receive_name_owner_changed().await {
        Ok(changes) => changes,
        Err(error) => {
            warn!("cannot watch the bus for media players: {error}");
            return;
        }
    };

    match dbus.list_names().await {
        Ok(names) => {
            for name in names
                .iter()
                .map(ToString::to_string)
                .filter(|name| is_mpris(name))
            {
                if events.send(Event::Appeared(name)).await.is_err() {
                    return;
                }
            }
        }
        Err(error) => warn!("cannot list the names on the bus: {error}"),
    }

    while let Some(signal) = changes.next().await {
        let Ok(args) = signal.args() else {
            continue;
        };
        let name = args.name().to_string();
        if !is_mpris(&name) {
            continue;
        }
        let event = if args.new_owner().is_some() {
            Event::Appeared(name)
        } else {
            Event::Vanished(name)
        };
        if events.send(event).await.is_err() {
            return;
        }
    }
}

/// Whether a bus name belongs to a media player.
fn is_mpris(name: &str) -> bool {
    name.starts_with(MPRIS_PREFIX)
}

/// Follow one player until it stops answering.
///
/// The task ends of its own accord when the player quits — its signal stream
/// ends — and says so, so the panel drops the player even if the bus's own
/// `NameOwnerChanged` never reaches it.
async fn watch_player(bus_name: String, connection: Connection, events: mpsc::Sender<Event>) {
    // Subscribed *before* the first read, and deliberately so: a player that
    // starts a track in the moment between reading its properties and
    // listening for changes would otherwise sit on the card as a paused player
    // with the wrong title until it next did something.
    let subscription = async {
        let properties = properties_proxy(&bus_name, &connection).await?;
        let changes = properties.receive_properties_changed().await?;
        zbus::Result::Ok((properties, changes))
    };
    let (properties, mut changes) = match subscription.await {
        Ok(subscription) => subscription,
        Err(error) => {
            debug!("cannot follow {bus_name}: {error}");
            let _ = events.send(Event::Vanished(bus_name)).await;
            return;
        }
    };

    let introduction =
        tokio::time::timeout(CALL_TIMEOUT, introduce(&bus_name, &connection, &properties)).await;
    let ready = match introduction {
        Ok(Ok(ready)) => ready,
        Ok(Err(error)) => {
            debug!("media player {bus_name} did not introduce itself: {error}");
            let _ = events.send(Event::Vanished(bus_name)).await;
            return;
        }
        Err(_) => {
            debug!("media player {bus_name} did not answer in time");
            let _ = events.send(Event::Vanished(bus_name)).await;
            return;
        }
    };

    if events.send(Event::Ready(Box::new(ready))).await.is_err() {
        return;
    }

    while let Some(signal) = changes.next().await {
        let Ok(args) = signal.args() else {
            continue;
        };
        if args.interface_name().as_str() != PLAYER_INTERFACE {
            continue;
        }
        let delta = PlayerDelta::parse(
            args.changed_properties()
                .iter()
                .map(|(name, value)| (*name, value)),
        );
        if delta.is_empty() {
            continue;
        }
        if events
            .send(Event::Changed(bus_name.clone(), Box::new(delta)))
            .await
            .is_err()
        {
            return;
        }
    }

    let _ = events.send(Event::Vanished(bus_name)).await;
}

/// Ask a player who it is and what it is doing.
///
/// One `GetAll` rather than ten `Get`s: a player that has just started is busy,
/// and every round trip is one more chance to be waiting on it.
async fn introduce(
    bus_name: &str,
    connection: &Connection,
    properties: &PropertiesProxy<'static>,
) -> zbus::Result<Ready> {
    let player = PlayerProxy::builder(connection)
        .destination(bus_name.to_string())?
        .path(MPRIS_PATH)?
        .cache_properties(CacheProperties::No)
        .build()
        .await?;
    let application = ApplicationProxy::builder(connection)
        .destination(bus_name.to_string())?
        .path(MPRIS_PATH)?
        .cache_properties(CacheProperties::No)
        .build()
        .await?;

    let all = properties.get_all(PLAYER_INTERFACE.try_into()?).await?;
    let delta = PlayerDelta::parse(all.iter().map(|(name, value)| (name.as_str(), &**value)));

    // A player that will not say who it is still gets a name, taken from the
    // bus name it registered under.
    let identity = application
        .identity()
        .await
        .ok()
        .map(|identity| identity.trim().to_string())
        .filter(|identity| !identity.is_empty())
        .unwrap_or_else(|| identity_from_bus_name(bus_name));
    let desktop_entry = application
        .desktop_entry()
        .await
        .ok()
        .map(|entry| entry.trim().to_string())
        .filter(|entry| !entry.is_empty());

    Ok(Ready {
        bus_name: bus_name.to_string(),
        player,
        application,
        identity,
        desktop_entry,
        delta,
    })
}

/// The standard properties interface of one player.
async fn properties_proxy(
    bus_name: &str,
    connection: &Connection,
) -> zbus::Result<PropertiesProxy<'static>> {
    PropertiesProxy::builder(connection)
        .destination(bus_name.to_string())?
        .path(MPRIS_PATH)?
        .build()
        .await
}
