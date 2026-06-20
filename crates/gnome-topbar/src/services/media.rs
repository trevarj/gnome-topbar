//! MediaService - MPRIS D-Bus integration for media player control.
//!
//! This service discovers and controls MPRIS-compatible media players on the session bus.
//! It provides:
//! - Player discovery (org.mpris.MediaPlayer2.*)
//! - Playback state monitoring (Playing/Paused/Stopped)
//! - Metadata access (title, artist, album, art URL, duration)
//! - Playback control (play/pause, next, previous, seek, volume)
//! - Position tracking with periodic polling when playing
//! - Multi-player support with automatic or manual player selection
//!
//! ## Architecture
//!
//! Unlike single-player designs, this service maintains connections to ALL discovered
//! MPRIS players simultaneously. This allows:
//! - Instant player switching (no reconnection delay)
//! - Real-time status for all players (for selector UI)
//! - Simple selection logic (just filter connected players)
//!
//! ## MPRIS D-Bus Interface
//!
//! - Bus: Session
//! - Service names: `org.mpris.MediaPlayer2.*` (e.g., `org.mpris.MediaPlayer2.spotify`)
//! - Object path: `/org/mpris/MediaPlayer2`
//! - Interfaces:
//!   - `org.mpris.MediaPlayer2` - Base interface (Identity, Quit, etc.)
//!   - `org.mpris.MediaPlayer2.Player` - Playback control and state

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gtk4::gio;
use gtk4::glib::{self, ControlFlow, Variant, clone};
use gtk4::prelude::*;
use sha2::{Digest, Sha256};
use tracing::{debug, error, trace, warn};

use super::callbacks::{CallbackId, Callbacks};

// D-Bus constants
const DBUS_NAME: &str = "org.freedesktop.DBus";
const DBUS_PATH: &str = "/org/freedesktop/DBus";
const DBUS_INTERFACE: &str = "org.freedesktop.DBus";
const MPRIS_PREFIX: &str = "org.mpris.MediaPlayer2.";
const MPRIS_PATH: &str = "/org/mpris/MediaPlayer2";
const MPRIS_PLAYER_INTERFACE: &str = "org.mpris.MediaPlayer2.Player";
const PROPERTIES_INTERFACE: &str = "org.freedesktop.DBus.Properties";
const MPD_PLAYER_NAME: &str = "mpd";
const MPD_HOST: &str = "127.0.0.1:6600";
const MPD_TIMEOUT_MS: u64 = 500;
const MPD_ART_CACHE_DIR: &str = "gnome-topbar-mpd-art";

/// Position polling interval when playing (in milliseconds).
const POSITION_POLL_INTERVAL_MS: u64 = 1000;
/// MPD discovery/status refresh interval.
const MPD_REFRESH_INTERVAL_MS: u64 = 5000;
/// Default timeout for D-Bus method calls (in milliseconds).
const DBUS_CALL_TIMEOUT_MS: i32 = 5000;
/// Shorter timeout for position polling queries.
const DBUS_POLL_TIMEOUT_MS: i32 = 1000;

// ========== Helper Functions ==========

/// Extract player ID from MPRIS bus name (e.g., "org.mpris.MediaPlayer2.spotify" -> "spotify").
fn player_id_from_bus_name(bus_name: &str) -> String {
    bus_name
        .strip_prefix(MPRIS_PREFIX)
        .map(|s| s.split('.').next().unwrap_or(s))
        .unwrap_or(bus_name)
        .to_string()
}

/// Capitalize the first character of a string (e.g., "spotify" -> "Spotify").
fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Extract MPRIS bus names from a D-Bus `(as)` reply.
fn mpris_names_from_reply(reply: &Variant) -> Vec<String> {
    reply
        .child_value(0)
        .iter()
        .filter_map(|v| v.get::<String>())
        .filter(|n| n.starts_with(MPRIS_PREFIX))
        .collect()
}

/// Playback status of the media player.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlaybackStatus {
    Playing,
    Paused,
    #[default]
    Stopped,
}

impl std::str::FromStr for PlaybackStatus {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "Playing" => Self::Playing,
            "Paused" => Self::Paused,
            _ => Self::Stopped,
        })
    }
}

/// Metadata about the currently playing track.
#[derive(Debug, Clone, Default)]
pub struct MediaMetadata {
    /// Track title (xesam:title).
    pub title: Option<String>,
    /// Artist name(s) (xesam:artist).
    pub artist: Option<String>,
    /// Album name (xesam:album).
    pub album: Option<String>,
    /// Album art URL (mpris:artUrl) - can be file:// or http(s)://.
    pub art_url: Option<String>,
    /// Track URL (xesam:url) - useful for identifying web players.
    pub url: Option<String>,
    /// Track duration in microseconds (mpris:length).
    pub length: Option<i64>,
    /// Track ID (mpris:trackid).
    pub track_id: Option<String>,
}

/// Info about a single player, for the player selector UI.
#[derive(Debug, Clone)]
pub struct PlayerInfo {
    /// Bus name (e.g., "org.mpris.MediaPlayer2.spotify").
    pub bus_name: String,
    /// Display name (e.g., "Spotify").
    pub player_name: String,
    /// Current playback status.
    pub playback_status: PlaybackStatus,
    /// Whether this is the currently active player.
    pub is_active: bool,
}

/// Canonical snapshot of media player state.
#[derive(Debug, Clone)]
pub struct MediaSnapshot {
    /// Whether any MPRIS player is available.
    pub available: bool,
    /// Raw player ID for icon lookup (e.g., "spotify", "firefox").
    pub player_id: Option<String>,
    /// Current playback status.
    pub playback_status: PlaybackStatus,
    /// Track metadata.
    pub metadata: MediaMetadata,
    /// Current position in microseconds.
    pub position: i64,
    /// Whether the player can play.
    pub can_play: bool,
    /// Whether the player can pause.
    pub can_pause: bool,
    /// Whether the player can go to next track.
    pub can_go_next: bool,
    /// Whether the player can go to previous track.
    pub can_go_previous: bool,
    /// Whether the player can seek.
    pub can_seek: bool,
}

impl Default for MediaSnapshot {
    fn default() -> Self {
        Self {
            available: false,
            player_id: None,
            playback_status: PlaybackStatus::Stopped,
            metadata: MediaMetadata::default(),
            position: 0,
            can_play: false,
            can_pause: false,
            can_go_next: false,
            can_go_previous: false,
            can_seek: false,
        }
    }
}

impl MediaSnapshot {
    /// Whether the snapshot contains meaningful track metadata (i.e. a non-empty title).
    pub fn has_metadata(&self) -> bool {
        self.metadata
            .title
            .as_ref()
            .is_some_and(|t| !t.trim().is_empty())
    }
}

/// State for a single connected MPRIS player.
struct MprisPlayer {
    bus_name: String,
    player_id: String,
    player_name: String,
    proxy: gio::DBusProxy,
    playback_status: PlaybackStatus,
    metadata: MediaMetadata,
    position: i64,
    can_play: bool,
    can_pause: bool,
    can_go_next: bool,
    can_go_previous: bool,
    can_seek: bool,
    can_control: bool,
    /// Signal subscription for PropertiesChanged (set after creation).
    _properties_subscription: Option<gio::SignalSubscription>,
    /// Track generation for invalidating stale position polls.
    track_generation: u64,
}

impl MprisPlayer {
    fn to_player_info(&self, is_active: bool) -> PlayerInfo {
        PlayerInfo {
            bus_name: self.bus_name.clone(),
            player_name: self.player_name.clone(),
            playback_status: self.playback_status,
            is_active,
        }
    }
}

/// State for a native MPD player discovered over the local MPD protocol.
#[derive(Debug, Clone)]
struct MpdPlayer {
    playback_status: PlaybackStatus,
    metadata: MediaMetadata,
    position: i64,
    can_seek: bool,
}

struct MpdBinaryChunk {
    total_size: usize,
    bytes: Vec<u8>,
}

impl MpdPlayer {
    fn to_player_info(&self, is_active: bool) -> PlayerInfo {
        PlayerInfo {
            bus_name: MPD_PLAYER_NAME.to_string(),
            player_name: "MPD".to_string(),
            playback_status: self.playback_status,
            is_active,
        }
    }
}

/// Shared, process-wide media service with multi-player support.
pub struct MediaService {
    /// Connection to the session bus.
    connection: RefCell<Option<gio::DBusConnection>>,
    /// All connected MPRIS players, keyed by bus name.
    players: RefCell<HashMap<String, Rc<RefCell<MprisPlayer>>>>,
    /// Native MPD fallback when no MPRIS bridge exposes MPD on D-Bus.
    mpd_player: RefCell<Option<MpdPlayer>>,
    /// Bus name of the currently active player.
    active_player: RefCell<Option<String>>,
    /// User's manual selection (None = auto mode).
    manual_selection: RefCell<Option<String>>,
    /// Last player that started playing (for auto-selection preference).
    last_playing: RefCell<Option<String>>,
    /// Signal subscription for NameOwnerChanged (player appear/disappear).
    _name_owner_subscription: RefCell<Option<gio::SignalSubscription>>,
    /// Timer for position polling when playing.
    position_poll_source: RefCell<Option<glib::SourceId>>,
    /// Timer for periodically discovering and refreshing MPD.
    mpd_refresh_source: RefCell<Option<glib::SourceId>>,
    /// Cancellable for position polling D-Bus calls.
    poll_cancellable: RefCell<gio::Cancellable>,
    /// Live media snapshot listeners.
    callbacks: Callbacks<MediaSnapshot>,
}

impl MediaService {
    fn new() -> Rc<Self> {
        let service = Rc::new(Self {
            connection: RefCell::new(None),
            players: RefCell::new(HashMap::new()),
            mpd_player: RefCell::new(None),
            active_player: RefCell::new(None),
            manual_selection: RefCell::new(None),
            last_playing: RefCell::new(None),
            _name_owner_subscription: RefCell::new(None),
            position_poll_source: RefCell::new(None),
            mpd_refresh_source: RefCell::new(None),
            poll_cancellable: RefCell::new(gio::Cancellable::new()),
            callbacks: Callbacks::new(),
        });

        Self::init_dbus(&service);
        Self::init_mpd(&service);
        service
    }

    /// Get the global MediaService singleton.
    pub fn global() -> Rc<Self> {
        thread_local! {
            static INSTANCE: Rc<MediaService> = MediaService::new();
        }
        INSTANCE.with(|s| s.clone())
    }

    /// Get a clone of the current snapshot.
    pub fn snapshot(&self) -> MediaSnapshot {
        self.build_snapshot()
    }

    /// Register for live media snapshot updates.
    pub fn connect<F>(&self, callback: F) -> CallbackId
    where
        F: Fn(&MediaSnapshot) + 'static,
    {
        let id = self.callbacks.register(callback);
        self.callbacks.notify_single(id, &self.build_snapshot());
        id
    }

    /// Unregister a media snapshot callback.
    pub fn disconnect(&self, id: CallbackId) -> bool {
        self.callbacks.unregister(id)
    }

    /// Get info about all available players (for selector UI).
    pub fn available_players(&self) -> Vec<PlayerInfo> {
        let players = self.players.borrow();
        let mpd_player = self.mpd_player.borrow();
        let active = self.active_player.borrow();

        let mut infos: Vec<PlayerInfo> = players
            .values()
            .map(|p| {
                let p = p.borrow();
                let is_active = active.as_ref() == Some(&p.bus_name);
                p.to_player_info(is_active)
            })
            .collect();

        if let Some(mpd) = mpd_player.as_ref() {
            infos.push(mpd.to_player_info(active.as_deref() == Some(MPD_PLAYER_NAME)));
        }

        infos
    }

    /// Manually select a specific player.
    pub fn set_active_player(self: &Rc<Self>, bus_name: &str) {
        let is_mpd = bus_name == MPD_PLAYER_NAME && self.mpd_player.borrow().is_some();
        if !is_mpd && !self.players.borrow().contains_key(bus_name) {
            warn!("Cannot select unknown player: {}", bus_name);
            return;
        }

        debug!("Manual player selection: {}", bus_name);
        self.manual_selection.replace(Some(bus_name.to_string()));
        self.update_active_player();
    }

    /// Switch to auto-selection mode.
    pub fn set_auto_selection(self: &Rc<Self>) {
        debug!("Switching to auto player selection");
        self.manual_selection.replace(None);
        self.update_active_player();
    }

    /// Check if auto-selection is active.
    pub fn is_auto_selection(&self) -> bool {
        self.manual_selection.borrow().is_none()
    }

    /// Write current active player to state file for CLI commands.
    fn write_ipc_state(&self) {
        let active = self.active_player.borrow();
        super::media_ipc::write_state(active.as_deref());
    }

    // ========== D-Bus Initialization ==========

    fn init_dbus(this: &Rc<Self>) {
        let this_weak = Rc::downgrade(this);

        gio::bus_get(
            gio::BusType::Session,
            None::<&gio::Cancellable>,
            move |res| {
                let Some(this) = this_weak.upgrade() else {
                    return;
                };

                let connection = match res {
                    Ok(c) => c,
                    Err(e) => {
                        error!("Failed to connect to session bus: {}", e);
                        return;
                    }
                };

                debug!("Connected to session bus for MPRIS");
                this.connection.replace(Some(connection.clone()));

                // Subscribe to NameOwnerChanged to detect player appear/disappear
                let this_weak = Rc::downgrade(&this);
                let subscription = connection.subscribe_to_signal(
                    Some(DBUS_NAME),
                    Some(DBUS_INTERFACE),
                    Some("NameOwnerChanged"),
                    Some(DBUS_PATH),
                    None,
                    gio::DBusSignalFlags::NONE,
                    move |signal| {
                        if let Some(name) = signal.parameters.child_value(0).str()
                            && name.starts_with(MPRIS_PREFIX)
                            && let Some(this) = this_weak.upgrade()
                        {
                            let old_owner_v = signal.parameters.child_value(1);
                            let new_owner_v = signal.parameters.child_value(2);
                            let old_owner = old_owner_v.str().unwrap_or("");
                            let new_owner = new_owner_v.str().unwrap_or("");

                            if old_owner.is_empty() && !new_owner.is_empty() {
                                // Player appeared
                                debug!("MPRIS player appeared: {}", name);
                                this.add_player(name);
                            } else if !old_owner.is_empty() && new_owner.is_empty() {
                                // Player disappeared
                                debug!("MPRIS player disappeared: {}", name);
                                this.remove_player(name);
                            }
                        }
                    },
                );
                this._name_owner_subscription.replace(Some(subscription));

                // Initial player discovery
                this.discover_players();
            },
        );
    }

    /// Discover all available MPRIS players on the bus.
    fn discover_players(self: &Rc<Self>) {
        let Some(connection) = self.connection.borrow().clone() else {
            return;
        };

        let this_weak = Rc::downgrade(self);
        let connection_for_activatable = connection.clone();
        connection.call(
            Some(DBUS_NAME),
            DBUS_PATH,
            DBUS_INTERFACE,
            "ListNames",
            None,
            Some(glib::VariantTy::new("(as)").unwrap()),
            gio::DBusCallFlags::NONE,
            DBUS_CALL_TIMEOUT_MS,
            None::<&gio::Cancellable>,
            move |res| {
                let Some(this) = this_weak.upgrade() else {
                    return;
                };

                let reply = match res {
                    Ok(r) => r,
                    Err(e) => {
                        warn!("Failed to list D-Bus names: {}", e);
                        return;
                    }
                };

                let owned_players = mpris_names_from_reply(&reply);
                let this_weak = Rc::downgrade(&this);

                // Some MPRIS bridges (including MPD bridges on Guix systems) are
                // D-Bus activatable and only take a well-known name once a client
                // asks for them. Include those names so discovery can activate them.
                connection_for_activatable.call(
                    Some(DBUS_NAME),
                    DBUS_PATH,
                    DBUS_INTERFACE,
                    "ListActivatableNames",
                    None,
                    Some(glib::VariantTy::new("(as)").unwrap()),
                    gio::DBusCallFlags::NONE,
                    DBUS_CALL_TIMEOUT_MS,
                    None::<&gio::Cancellable>,
                    move |res| {
                        let Some(this) = this_weak.upgrade() else {
                            return;
                        };

                        let mut players: HashSet<String> = owned_players.into_iter().collect();
                        match res {
                            Ok(reply) => players.extend(mpris_names_from_reply(&reply)),
                            Err(e) => {
                                warn!("Failed to list activatable D-Bus names: {}", e);
                            }
                        }

                        let mut players: Vec<String> = players.into_iter().collect();
                        players.sort();

                        debug!(
                            "Discovered {} MPRIS player(s): {:?}",
                            players.len(),
                            players
                        );

                        for bus_name in players {
                            this.add_player(&bus_name);
                        }
                    },
                );
            },
        );
    }

    // ========== Native MPD fallback ==========

    fn init_mpd(this: &Rc<Self>) {
        this.refresh_mpd();

        let this_weak = Rc::downgrade(this);
        let source =
            glib::timeout_add_local(Duration::from_millis(MPD_REFRESH_INTERVAL_MS), move || {
                let Some(this) = this_weak.upgrade() else {
                    return ControlFlow::Break;
                };

                this.refresh_mpd();
                ControlFlow::Continue
            });
        this.mpd_refresh_source.replace(Some(source));
    }

    fn refresh_mpd(self: &Rc<Self>) {
        let old_player = self.mpd_player.borrow().clone();
        let new_player = Self::query_mpd_player().ok();

        let changed = old_player.as_ref().map(Self::mpd_signature)
            != new_player.as_ref().map(Self::mpd_signature);
        let appeared = old_player.is_none() && new_player.is_some();
        let disappeared = old_player.is_some() && new_player.is_none();

        self.mpd_player.replace(new_player);

        if appeared {
            debug!("Added native MPD player");
        } else if disappeared {
            debug!("Removed native MPD player");
            if self.manual_selection.borrow().as_deref() == Some(MPD_PLAYER_NAME) {
                self.manual_selection.replace(None);
            }
        }

        if appeared || disappeared {
            self.update_active_player();
        } else if changed {
            if self
                .mpd_player
                .borrow()
                .as_ref()
                .is_some_and(|p| p.playback_status == PlaybackStatus::Playing)
            {
                self.last_playing.replace(Some(MPD_PLAYER_NAME.to_string()));
            }
            self.update_active_player();
            if self.active_player.borrow().as_deref() == Some(MPD_PLAYER_NAME) {
                self.notify_listeners();
            }
        }
    }

    fn mpd_signature(player: &MpdPlayer) -> (PlaybackStatus, Option<String>, i64, Option<i64>) {
        (
            player.playback_status,
            player.metadata.track_id.clone(),
            player.position,
            player.metadata.length,
        )
    }

    fn query_mpd_player() -> Result<MpdPlayer, String> {
        let status = Self::mpd_query("status")?;
        let song = Self::mpd_query("currentsong").unwrap_or_default();
        let status = Self::parse_mpd_pairs(&status);
        let song = Self::parse_mpd_pairs(&song);

        let playback_status = match status.get("state").map(String::as_str) {
            Some("play") => PlaybackStatus::Playing,
            Some("pause") => PlaybackStatus::Paused,
            _ => PlaybackStatus::Stopped,
        };

        let title = song
            .get("Title")
            .cloned()
            .or_else(|| song.get("file").cloned());
        let artist = song.get("Artist").cloned();
        let album = song.get("Album").cloned();
        let length = song
            .get("duration")
            .or_else(|| status.get("duration"))
            .and_then(|v| Self::mpd_seconds_to_microseconds(v));
        let position = status
            .get("elapsed")
            .and_then(|v| Self::mpd_seconds_to_microseconds(v))
            .unwrap_or(0);
        let track_id = song
            .get("Id")
            .or_else(|| status.get("songid"))
            .map(|id| format!("mpd:{}", id));
        let art_url = song.get("file").and_then(|file| {
            let cache_key = track_id.as_deref().unwrap_or(file);
            Self::mpd_album_art_url(file, cache_key).ok().flatten()
        });

        Ok(MpdPlayer {
            playback_status,
            metadata: MediaMetadata {
                title,
                artist,
                album,
                art_url,
                length,
                track_id,
                ..Default::default()
            },
            position,
            can_seek: length.is_some(),
        })
    }

    fn mpd_query(command: &str) -> Result<Vec<String>, String> {
        let timeout = Duration::from_millis(MPD_TIMEOUT_MS);
        let mut stream =
            TcpStream::connect(MPD_HOST).map_err(|e| format!("connect to MPD failed: {}", e))?;
        stream
            .set_read_timeout(Some(timeout))
            .map_err(|e| format!("set MPD read timeout failed: {}", e))?;
        stream
            .set_write_timeout(Some(timeout))
            .map_err(|e| format!("set MPD write timeout failed: {}", e))?;

        let mut reader = BufReader::new(
            stream
                .try_clone()
                .map_err(|e| format!("clone MPD stream failed: {}", e))?,
        );

        let mut greeting = String::new();
        reader
            .read_line(&mut greeting)
            .map_err(|e| format!("read MPD greeting failed: {}", e))?;
        if !greeting.starts_with("OK MPD ") {
            return Err("invalid MPD greeting".to_string());
        }

        writeln!(stream, "{}", command).map_err(|e| format!("write MPD command failed: {}", e))?;
        writeln!(stream, "close").map_err(|e| format!("write MPD close failed: {}", e))?;
        stream
            .flush()
            .map_err(|e| format!("flush MPD command failed: {}", e))?;

        let mut lines = Vec::new();
        loop {
            let mut line = String::new();
            let bytes = reader
                .read_line(&mut line)
                .map_err(|e| format!("read MPD reply failed: {}", e))?;
            if bytes == 0 {
                break;
            }
            let line = line.trim_end().to_string();
            if line == "OK" {
                break;
            }
            if line.starts_with("ACK ") {
                return Err(line);
            }
            lines.push(line);
        }

        Ok(lines)
    }

    fn mpd_album_art_url(song_file: &str, cache_key: &str) -> Result<Option<String>, String> {
        let cache_path = Self::mpd_art_cache_path(cache_key)?;
        if cache_path.exists() {
            return Ok(Some(gio::File::for_path(&cache_path).uri().to_string()));
        }

        let Some(bytes) = Self::mpd_album_art_bytes(song_file)? else {
            return Ok(None);
        };

        if let Some(parent) = cache_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("create MPD art cache failed: {}", e))?;
        }
        fs::write(&cache_path, bytes).map_err(|e| format!("write MPD art cache failed: {}", e))?;

        Ok(Some(gio::File::for_path(&cache_path).uri().to_string()))
    }

    fn mpd_art_cache_path(cache_key: &str) -> Result<PathBuf, String> {
        let base_dir = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let mut hasher = Sha256::new();
        hasher.update(cache_key.as_bytes());
        let digest = hasher.finalize();
        let name = digest
            .iter()
            .take(16)
            .map(|b| format!("{:02x}", b))
            .collect::<String>();

        Ok(base_dir
            .join(MPD_ART_CACHE_DIR)
            .join(format!("{name}.cover")))
    }

    fn mpd_album_art_bytes(song_file: &str) -> Result<Option<Vec<u8>>, String> {
        let quoted_file = Self::mpd_quote_arg(song_file);
        Self::mpd_binary_query_chunks("albumart", &quoted_file)
            .or_else(|_| Self::mpd_binary_query_chunks("readpicture", &quoted_file))
    }

    fn mpd_binary_query_chunks(
        command: &str,
        quoted_file: &str,
    ) -> Result<Option<Vec<u8>>, String> {
        let mut offset = 0usize;
        let mut total_size = None;
        let mut bytes = Vec::new();

        loop {
            let query = format!("{command} {quoted_file} {offset}");
            let Some(chunk) = Self::mpd_binary_query(&query)? else {
                return Ok(None);
            };

            if total_size.is_none() {
                total_size = Some(chunk.total_size);
            }
            if chunk.bytes.is_empty() {
                break;
            }

            offset += chunk.bytes.len();
            bytes.extend(chunk.bytes);

            if let Some(size) = total_size
                && offset >= size
            {
                break;
            }
        }

        if bytes.is_empty() {
            Ok(None)
        } else {
            Ok(Some(bytes))
        }
    }

    fn mpd_binary_query(command: &str) -> Result<Option<MpdBinaryChunk>, String> {
        let timeout = Duration::from_millis(MPD_TIMEOUT_MS);
        let mut stream =
            TcpStream::connect(MPD_HOST).map_err(|e| format!("connect to MPD failed: {}", e))?;
        stream
            .set_read_timeout(Some(timeout))
            .map_err(|e| format!("set MPD read timeout failed: {}", e))?;
        stream
            .set_write_timeout(Some(timeout))
            .map_err(|e| format!("set MPD write timeout failed: {}", e))?;

        let mut reader = BufReader::new(
            stream
                .try_clone()
                .map_err(|e| format!("clone MPD stream failed: {}", e))?,
        );

        let mut greeting = String::new();
        reader
            .read_line(&mut greeting)
            .map_err(|e| format!("read MPD greeting failed: {}", e))?;
        if !greeting.starts_with("OK MPD ") {
            return Err("invalid MPD greeting".to_string());
        }

        writeln!(stream, "{}", command).map_err(|e| format!("write MPD command failed: {}", e))?;
        writeln!(stream, "close").map_err(|e| format!("write MPD close failed: {}", e))?;
        stream
            .flush()
            .map_err(|e| format!("flush MPD command failed: {}", e))?;

        let mut total_size = None;
        loop {
            let mut line = String::new();
            let bytes_read = reader
                .read_line(&mut line)
                .map_err(|e| format!("read MPD binary header failed: {}", e))?;
            if bytes_read == 0 {
                return Ok(None);
            }
            let line = line.trim_end();

            if let Some(value) = line.strip_prefix("size: ") {
                total_size = value.parse::<usize>().ok();
            } else if let Some(value) = line.strip_prefix("binary: ") {
                let chunk_size = value
                    .parse::<usize>()
                    .map_err(|e| format!("invalid MPD binary size: {}", e))?;
                let mut bytes = vec![0; chunk_size];
                reader
                    .read_exact(&mut bytes)
                    .map_err(|e| format!("read MPD binary payload failed: {}", e))?;

                // MPD terminates the binary payload with a newline before OK.
                let mut separator = [0; 1];
                let _ = reader.read_exact(&mut separator);
                Self::mpd_expect_ok(reader)?;

                return Ok(Some(MpdBinaryChunk {
                    total_size: total_size.unwrap_or(chunk_size),
                    bytes,
                }));
            } else if line == "OK" {
                return Ok(None);
            } else if line.starts_with("ACK ") {
                return Err(line.to_string());
            }
        }
    }

    fn mpd_expect_ok(mut reader: BufReader<TcpStream>) -> Result<(), String> {
        loop {
            let mut line = String::new();
            let bytes_read = reader
                .read_line(&mut line)
                .map_err(|e| format!("read MPD command trailer failed: {}", e))?;
            if bytes_read == 0 {
                return Ok(());
            }
            let line = line.trim_end();
            if line == "OK" {
                return Ok(());
            }
            if line.starts_with("ACK ") {
                return Err(line.to_string());
            }
        }
    }

    fn mpd_quote_arg(value: &str) -> String {
        let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"{}\"", escaped)
    }

    fn parse_mpd_pairs(lines: &[String]) -> HashMap<String, String> {
        lines
            .iter()
            .filter_map(|line| line.split_once(": "))
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    fn mpd_seconds_to_microseconds(value: &str) -> Option<i64> {
        let seconds = value.parse::<f64>().ok()?;
        Some((seconds * MICROSECONDS_PER_SECOND as f64).round() as i64)
    }

    /// Add a new player (creates proxy and subscribes to signals).
    fn add_player(self: &Rc<Self>, bus_name: &str) {
        if self.players.borrow().contains_key(bus_name) {
            return;
        }

        let Some(connection) = self.connection.borrow().clone() else {
            return;
        };

        let bus_name_owned = bus_name.to_string();
        let this_weak = Rc::downgrade(self);

        gio::DBusProxy::for_bus(
            gio::BusType::Session,
            gio::DBusProxyFlags::NONE,
            None::<&gio::DBusInterfaceInfo>,
            &bus_name_owned,
            MPRIS_PATH,
            MPRIS_PLAYER_INTERFACE,
            None::<&gio::Cancellable>,
            clone!(
                #[strong]
                bus_name_owned,
                move |res| {
                    let Some(this) = this_weak.upgrade() else {
                        return;
                    };

                    let proxy = match res {
                        Ok(p) => p,
                        Err(e) => {
                            warn!("Failed to create MPRIS proxy for {}: {}", bus_name_owned, e);
                            return;
                        }
                    };

                    // Extract player ID and name
                    let player_id = player_id_from_bus_name(&bus_name_owned);
                    let player_name = capitalize_first(&player_id);

                    // Create the player with initial state from proxy
                    let player = Rc::new(RefCell::new(MprisPlayer {
                        bus_name: bus_name_owned.clone(),
                        player_id,
                        player_name: player_name.clone(),
                        proxy: proxy.clone(),
                        playback_status: PlaybackStatus::Stopped,
                        metadata: MediaMetadata::default(),
                        position: 0,
                        can_play: false,
                        can_pause: false,
                        can_go_next: false,
                        can_go_previous: false,
                        can_seek: false,
                        can_control: true,
                        _properties_subscription: None,
                        track_generation: 0,
                    }));

                    // Update state from cached properties
                    let _ = Self::update_player_from_proxy(&player);

                    // Subscribe to PropertiesChanged for this player
                    let player_weak = Rc::downgrade(&player);
                    let this_weak = Rc::downgrade(&this);
                    let subscription = connection.subscribe_to_signal(
                        Some(&bus_name_owned),
                        Some(PROPERTIES_INTERFACE),
                        Some("PropertiesChanged"),
                        Some(MPRIS_PATH),
                        None,
                        gio::DBusSignalFlags::NONE,
                        move |_signal| {
                            let Some(player) = player_weak.upgrade() else {
                                return;
                            };
                            let Some(this) = this_weak.upgrade() else {
                                return;
                            };

                            let old_status = player.borrow().playback_status;
                            let track_changed = Self::update_player_from_proxy(&player);
                            let new_status = player.borrow().playback_status;
                            let status_changed = old_status != new_status;
                            let bus_name = player.borrow().bus_name.clone();
                            let was_active =
                                this.active_player.borrow().as_ref() == Some(&bus_name);

                            // Track the most recently playing player
                            if new_status == PlaybackStatus::Playing
                                && old_status != PlaybackStatus::Playing
                            {
                                let bus_name = player.borrow().bus_name.clone();
                                this.last_playing.replace(Some(bus_name));
                            }

                            // In auto mode, if this player just started playing, make it active
                            if this.is_auto_selection() && status_changed {
                                if new_status == PlaybackStatus::Playing {
                                    // This player just started playing - make it the active player
                                    let current_active = this.active_player.borrow().clone();
                                    if current_active.as_ref() != Some(&bus_name) {
                                        debug!("Switching to newly playing player: {}", bus_name);
                                        this.active_player.replace(Some(bus_name));
                                        this.on_active_player_changed();
                                    } else {
                                        // Same player resumed - restart position polling
                                        this.start_position_polling();
                                    }
                                } else {
                                    // Player stopped/paused - re-evaluate to find best player
                                    this.update_active_player();
                                }
                            } else if status_changed {
                                // Manual mode: if the active player changed status, handle polling
                                let is_active =
                                    this.active_player.borrow().as_ref() == Some(&bus_name);
                                if is_active {
                                    if new_status == PlaybackStatus::Playing {
                                        this.start_position_polling();
                                    } else {
                                        this.stop_position_polling();
                                    }
                                }
                            }

                            if was_active || track_changed || status_changed {
                                this.notify_listeners();
                            }

                            // Some players (notably YouTube Music) report stale
                            // position data immediately after a track or status change.
                            // Give them a moment to sort themselves out, then re-poll.
                            if track_changed || status_changed {
                                let this_weak = Rc::downgrade(&this);
                                glib::timeout_add_local_once(
                                    Duration::from_millis(100),
                                    move || {
                                        if let Some(this) = this_weak.upgrade() {
                                            this.poll_position();
                                        }
                                    },
                                );
                            }
                        },
                    );

                    player.borrow_mut()._properties_subscription = Some(subscription);

                    debug!("Added MPRIS player: {} ({})", player_name, bus_name_owned);
                    this.players.borrow_mut().insert(bus_name_owned, player);

                    // Update active player selection
                    this.update_active_player();
                }
            ),
        );
    }

    /// Remove a player that disappeared.
    fn remove_player(self: &Rc<Self>, bus_name: &str) {
        let removed = self.players.borrow_mut().remove(bus_name);

        if removed.is_some() {
            debug!("Removed MPRIS player: {}", bus_name);

            // Clear manual selection if it was this player
            if self.manual_selection.borrow().as_deref() == Some(bus_name) {
                self.manual_selection.replace(None);
            }

            self.update_active_player();
        }
    }

    /// Update player state from its proxy's cached properties.
    /// Returns `true` if a track change was detected.
    fn update_player_from_proxy(player: &Rc<RefCell<MprisPlayer>>) -> bool {
        // Read all properties first (need to read from proxy without holding borrow_mut)
        let (
            playback_status,
            metadata,
            can_play,
            can_pause,
            can_go_next,
            can_go_previous,
            can_seek,
            can_control,
        ) = {
            let p = player.borrow();
            let proxy = &p.proxy;

            let playback_status = proxy
                .cached_property("PlaybackStatus")
                .and_then(|v| v.get::<String>())
                .map(|s| s.parse().unwrap_or_default())
                .unwrap_or(PlaybackStatus::Stopped);

            let metadata = proxy
                .cached_property("Metadata")
                .map(|m| Self::parse_metadata(&m))
                .unwrap_or_default();

            let can_play = proxy
                .cached_property("CanPlay")
                .and_then(|v| v.get::<bool>())
                .unwrap_or(false);
            let can_pause = proxy
                .cached_property("CanPause")
                .and_then(|v| v.get::<bool>())
                .unwrap_or(false);
            let can_go_next = proxy
                .cached_property("CanGoNext")
                .and_then(|v| v.get::<bool>())
                .unwrap_or(false);
            let can_go_previous = proxy
                .cached_property("CanGoPrevious")
                .and_then(|v| v.get::<bool>())
                .unwrap_or(false);
            let can_seek = proxy
                .cached_property("CanSeek")
                .and_then(|v| v.get::<bool>())
                .unwrap_or(false);
            let can_control = proxy
                .cached_property("CanControl")
                .and_then(|v| v.get::<bool>())
                .unwrap_or(true);

            (
                playback_status,
                metadata,
                can_play,
                can_pause,
                can_go_next,
                can_go_previous,
                can_seek,
                can_control,
            )
        };

        // Now mutate with all the values we read
        let mut p = player.borrow_mut();
        let old_track_id = p.metadata.track_id.clone();
        let old_title = p.metadata.title.clone();

        p.playback_status = playback_status;
        p.metadata = metadata;
        p.can_play = can_play;
        p.can_pause = can_pause;
        p.can_go_next = can_go_next;
        p.can_go_previous = can_go_previous;
        p.can_seek = can_seek;
        p.can_control = can_control;

        // Track change detection
        let track_id_changed = old_track_id != p.metadata.track_id;
        let title_changed =
            old_title.is_some() && p.metadata.title.is_some() && old_title != p.metadata.title;

        if track_id_changed || title_changed {
            p.position = 0;
            p.track_generation += 1;
            true
        } else {
            false
        }
    }

    /// Determine which player should be active.
    fn update_active_player(self: &Rc<Self>) {
        let players = self.players.borrow();
        let mpd_player = self.mpd_player.borrow().clone();
        let old_active = self.active_player.borrow().clone();

        // Honor manual selection if still valid
        if let Some(manual) = self.manual_selection.borrow().as_ref() {
            let manual_is_valid =
                players.contains_key(manual) || (manual == MPD_PLAYER_NAME && mpd_player.is_some());
            if manual_is_valid {
                if old_active.as_ref() != Some(manual) {
                    debug!("Active player (manual): {}", manual);
                    self.active_player.replace(Some(manual.clone()));
                    drop(players);
                    self.on_active_player_changed();
                }
                return;
            }
            // Manual selection is no longer valid
            drop(players);
            self.manual_selection.replace(None);
            let players = self.players.borrow();
            let mpd_player = self.mpd_player.borrow().clone();
            self.select_best_player_auto(&players, mpd_player.as_ref(), &old_active);
            return;
        }

        self.select_best_player_auto(&players, mpd_player.as_ref(), &old_active);
    }

    /// Auto-select the best player (last playing > other playing > current paused > other paused > any).
    fn select_best_player_auto(
        self: &Rc<Self>,
        players: &HashMap<String, Rc<RefCell<MprisPlayer>>>,
        mpd_player: Option<&MpdPlayer>,
        old_active: &Option<String>,
    ) {
        // First, check if last_playing is still playing - prefer it
        if let Some(ref last) = *self.last_playing.borrow()
            && let Some(player) = players.get(last)
            && player.borrow().playback_status == PlaybackStatus::Playing
        {
            if old_active.as_ref() != Some(last) {
                debug!("Active player (auto, last playing): {}", last);
                self.active_player.replace(Some(last.clone()));
                self.on_active_player_changed();
            }
            return;
        }
        if self.last_playing.borrow().as_deref() == Some(MPD_PLAYER_NAME)
            && mpd_player.is_some_and(|p| p.playback_status == PlaybackStatus::Playing)
        {
            if old_active.as_deref() != Some(MPD_PLAYER_NAME) {
                debug!("Active player (auto, last playing): {}", MPD_PLAYER_NAME);
                self.active_player
                    .replace(Some(MPD_PLAYER_NAME.to_string()));
                self.on_active_player_changed();
            }
            return;
        }

        // Otherwise prefer any playing player
        let playing = players
            .values()
            .find(|p| p.borrow().playback_status == PlaybackStatus::Playing)
            .map(|p| p.borrow().bus_name.clone());

        if let Some(bus_name) = playing {
            if old_active.as_ref() != Some(&bus_name) {
                debug!("Active player (auto, playing): {}", bus_name);
                self.active_player.replace(Some(bus_name));
                self.on_active_player_changed();
            }
            return;
        }
        if mpd_player.is_some_and(|p| p.playback_status == PlaybackStatus::Playing) {
            if old_active.as_deref() != Some(MPD_PLAYER_NAME) {
                debug!("Active player (auto, playing): {}", MPD_PLAYER_NAME);
                self.active_player
                    .replace(Some(MPD_PLAYER_NAME.to_string()));
                self.on_active_player_changed();
            }
            return;
        }

        // If last_playing is paused with metadata, prefer it
        if let Some(ref last) = *self.last_playing.borrow()
            && let Some(player) = players.get(last)
        {
            let p = player.borrow();
            if p.playback_status == PlaybackStatus::Paused && p.metadata.title.is_some() {
                if old_active.as_ref() != Some(last) {
                    debug!("Active player (auto, last playing paused): {}", last);
                    drop(p);
                    self.active_player.replace(Some(last.clone()));
                    self.on_active_player_changed();
                }
                return;
            }
        }
        if self.last_playing.borrow().as_deref() == Some(MPD_PLAYER_NAME)
            && mpd_player.is_some_and(|p| {
                p.playback_status == PlaybackStatus::Paused && p.metadata.title.is_some()
            })
        {
            if old_active.as_deref() != Some(MPD_PLAYER_NAME) {
                debug!(
                    "Active player (auto, last playing paused): {}",
                    MPD_PLAYER_NAME
                );
                self.active_player
                    .replace(Some(MPD_PLAYER_NAME.to_string()));
                self.on_active_player_changed();
            }
            return;
        }

        // If current player is paused with metadata, keep it (don't switch between paused players)
        if let Some(current) = old_active
            && let Some(player) = players.get(current)
        {
            let p = player.borrow();
            if p.playback_status == PlaybackStatus::Paused && p.metadata.title.is_some() {
                return;
            }
        }
        if old_active.as_deref() == Some(MPD_PLAYER_NAME)
            && mpd_player.is_some_and(|p| {
                p.playback_status == PlaybackStatus::Paused && p.metadata.title.is_some()
            })
        {
            return;
        }

        // Find any paused player with metadata
        let paused_with_meta = players
            .values()
            .find(|p| {
                let p = p.borrow();
                p.playback_status == PlaybackStatus::Paused && p.metadata.title.is_some()
            })
            .map(|p| p.borrow().bus_name.clone());

        if let Some(bus_name) = paused_with_meta {
            if old_active.as_ref() != Some(&bus_name) {
                debug!("Active player (auto, paused with metadata): {}", bus_name);
                self.active_player.replace(Some(bus_name));
                self.on_active_player_changed();
            }
            return;
        }
        if mpd_player.is_some_and(|p| {
            p.playback_status == PlaybackStatus::Paused && p.metadata.title.is_some()
        }) {
            if old_active.as_deref() != Some(MPD_PLAYER_NAME) {
                debug!(
                    "Active player (auto, paused with metadata): {}",
                    MPD_PLAYER_NAME
                );
                self.active_player
                    .replace(Some(MPD_PLAYER_NAME.to_string()));
                self.on_active_player_changed();
            }
            return;
        }

        // Keep current if still valid
        if let Some(current) = old_active
            && players.contains_key(current)
        {
            return;
        }
        if old_active.as_deref() == Some(MPD_PLAYER_NAME) && mpd_player.is_some() {
            return;
        }

        // Pick any available player
        let any = players
            .keys()
            .next()
            .cloned()
            .or_else(|| mpd_player.map(|_| MPD_PLAYER_NAME.to_string()));
        if any != *old_active {
            if let Some(ref bus_name) = any {
                debug!("Active player (auto, fallback): {}", bus_name);
            } else {
                debug!("No active player");
            }
            self.active_player.replace(any);
            self.on_active_player_changed();
        }
    }

    /// Called when the active player changes.
    fn on_active_player_changed(self: &Rc<Self>) {
        self.stop_position_polling();
        self.poll_cancellable.borrow().cancel();
        self.poll_cancellable.replace(gio::Cancellable::new());

        // Write state for CLI to read
        self.write_ipc_state();

        // Fetch position immediately and start polling if playing
        self.poll_position();

        let should_poll = {
            let players = self.players.borrow();
            let mpd_player = self.mpd_player.borrow();
            let active = self.active_player.borrow();
            if active.as_deref() == Some(MPD_PLAYER_NAME) {
                mpd_player
                    .as_ref()
                    .is_some_and(|p| p.playback_status == PlaybackStatus::Playing)
            } else {
                active
                    .as_ref()
                    .and_then(|bus| players.get(bus))
                    .is_some_and(|p| p.borrow().playback_status == PlaybackStatus::Playing)
            }
        };

        if should_poll {
            self.start_position_polling();
        }

        self.notify_listeners();
    }

    fn notify_listeners(&self) {
        self.callbacks.notify(&self.build_snapshot());
    }

    /// Build the current snapshot from active player state.
    fn build_snapshot(&self) -> MediaSnapshot {
        let players = self.players.borrow();
        let mpd_player = self.mpd_player.borrow();
        let active_bus = self.active_player.borrow();

        if active_bus.as_deref() == Some(MPD_PLAYER_NAME)
            && let Some(p) = mpd_player.as_ref()
        {
            return MediaSnapshot {
                available: true,
                player_id: Some(MPD_PLAYER_NAME.to_string()),
                playback_status: p.playback_status,
                metadata: p.metadata.clone(),
                position: p.position,
                can_play: true,
                can_pause: true,
                can_go_next: true,
                can_go_previous: true,
                can_seek: p.can_seek,
            };
        }

        let active_player = active_bus
            .as_ref()
            .and_then(|bus| players.get(bus))
            .map(|p| p.borrow());

        match active_player {
            Some(p) => MediaSnapshot {
                available: true,
                player_id: Some(p.player_id.clone()),
                playback_status: p.playback_status,
                metadata: p.metadata.clone(),
                position: p.position,
                can_play: p.can_play,
                can_pause: p.can_pause,
                can_go_next: p.can_go_next,
                can_go_previous: p.can_go_previous,
                can_seek: p.can_seek,
            },
            None => MediaSnapshot {
                available: !players.is_empty() || mpd_player.is_some(),
                ..Default::default()
            },
        }
    }

    // ========== Metadata Parsing ==========

    fn parse_metadata(variant: &Variant) -> MediaMetadata {
        let mut meta = MediaMetadata::default();

        if let Some(dict) = variant.get::<HashMap<String, Variant>>() {
            if let Some(title) = dict.get("xesam:title") {
                meta.title = title.get::<String>();
            }

            if let Some(artist) = dict.get("xesam:artist") {
                if let Some(artists) = artist.get::<Vec<String>>() {
                    meta.artist = Some(artists.join(", "));
                } else if let Some(artist_str) = artist.get::<String>() {
                    meta.artist = Some(artist_str);
                }
            }

            if let Some(album) = dict.get("xesam:album") {
                meta.album = album.get::<String>();
            }

            if let Some(art_url) = dict.get("mpris:artUrl") {
                meta.art_url = art_url.get::<String>();
            }

            if let Some(url) = dict.get("xesam:url") {
                meta.url = url.get::<String>();
            }

            if let Some(length) = dict.get("mpris:length") {
                meta.length = length
                    .get::<i64>()
                    .or_else(|| length.get::<u64>().map(|v| v as i64));
            }

            if let Some(track_id) = dict.get("mpris:trackid") {
                if let Some(id) = track_id.get::<String>() {
                    meta.track_id = Some(id);
                } else if let Some(path) = track_id.get::<glib::variant::ObjectPath>() {
                    meta.track_id = Some(path.to_string());
                }
            }
        }

        meta
    }

    // ========== Position Polling ==========

    fn start_position_polling(self: &Rc<Self>) {
        self.stop_position_polling();

        trace!("Starting position polling");
        let this_weak = Rc::downgrade(self);
        let source = glib::timeout_add_local(
            Duration::from_millis(POSITION_POLL_INTERVAL_MS),
            move || {
                let Some(this) = this_weak.upgrade() else {
                    return ControlFlow::Break;
                };

                let should_continue = {
                    let players = this.players.borrow();
                    let mpd_player = this.mpd_player.borrow();
                    let active = this.active_player.borrow();
                    if active.as_deref() == Some(MPD_PLAYER_NAME) {
                        mpd_player
                            .as_ref()
                            .is_some_and(|p| p.playback_status == PlaybackStatus::Playing)
                    } else {
                        active
                            .as_ref()
                            .and_then(|bus| players.get(bus))
                            .is_some_and(|p| p.borrow().playback_status == PlaybackStatus::Playing)
                    }
                };

                if !should_continue {
                    this.position_poll_source.replace(None);
                    return ControlFlow::Break;
                }

                this.poll_position();
                ControlFlow::Continue
            },
        );
        self.position_poll_source.replace(Some(source));
    }

    fn stop_position_polling(&self) {
        if let Some(source) = self.position_poll_source.take() {
            trace!("Stopping position polling");
            source.remove();
        }
    }

    fn poll_position(self: &Rc<Self>) {
        if self.active_player.borrow().as_deref() == Some(MPD_PLAYER_NAME) {
            let old_position = self.mpd_player.borrow().as_ref().map(|p| p.position);
            if let Ok(player) = Self::query_mpd_player() {
                let changed = old_position != Some(player.position);
                self.mpd_player.replace(Some(player));
                if changed {
                    self.notify_listeners();
                }
            }
            return;
        }

        let (bus_name, generation) = {
            let players = self.players.borrow();
            let active = self.active_player.borrow();
            let Some(bus) = active.as_ref() else {
                return;
            };
            let Some(player) = players.get(bus) else {
                return;
            };
            (bus.clone(), player.borrow().track_generation)
        };

        let Some(connection) = self.connection.borrow().clone() else {
            return;
        };

        let cancellable = self.poll_cancellable.borrow().clone();

        connection.call(
            Some(&bus_name),
            MPRIS_PATH,
            PROPERTIES_INTERFACE,
            "Get",
            Some(&(MPRIS_PLAYER_INTERFACE, "Position").to_variant()),
            Some(glib::VariantTy::new("(v)").unwrap()),
            gio::DBusCallFlags::NONE,
            DBUS_POLL_TIMEOUT_MS,
            Some(&cancellable),
            clone!(
                #[strong(rename_to = this)]
                self,
                #[strong]
                bus_name,
                move |res| {
                    let players = this.players.borrow();
                    let active = this.active_player.borrow();

                    // Verify we're still polling the same player/track
                    let Some(player) = active
                        .as_ref()
                        .filter(|b| *b == &bus_name)
                        .and_then(|bus| players.get(bus))
                    else {
                        return;
                    };

                    if player.borrow().track_generation != generation {
                        return;
                    }

                    match res {
                        Ok(reply) => {
                            let mut changed = false;
                            if let Some(inner) = reply.child_value(0).get::<Variant>()
                                && let Some(position) = inner.get::<i64>()
                            {
                                changed = player.borrow().position != position;
                                if changed {
                                    player.borrow_mut().position = position;
                                }
                            }
                            drop(active);
                            drop(players);
                            if changed {
                                this.notify_listeners();
                            }
                        }
                        Err(e) => {
                            if !e.matches(gio::IOErrorEnum::Cancelled) {
                                trace!("Position poll failed: {}", e);
                            }
                        }
                    }
                }
            ),
        );
    }

    // ========== Playback Control ==========

    pub fn play_pause(&self) {
        self.call_player_method("PlayPause");
    }

    pub fn next(&self) {
        self.call_player_method("Next");
    }

    pub fn previous(&self) {
        self.call_player_method("Previous");
    }

    /// Set absolute position (in microseconds).
    pub fn set_position(&self, position_us: i64) {
        if self.active_player.borrow().as_deref() == Some(MPD_PLAYER_NAME) {
            let seconds = position_us / MICROSECONDS_PER_SECOND;
            if let Err(e) = Self::mpd_query(&format!("seekcur {}", seconds)) {
                warn!("MPD seek failed: {}", e);
            }
            return;
        }

        let track_id = {
            let players = self.players.borrow();
            let active = self.active_player.borrow();
            active
                .as_ref()
                .and_then(|bus| players.get(bus))
                .and_then(|p| p.borrow().metadata.track_id.clone())
        };

        let Some(track_id) = track_id else {
            return;
        };

        let Some((connection, bus_name)) = self.get_active_connection() else {
            return;
        };

        let track_path = match glib::variant::ObjectPath::try_from(track_id.as_str()) {
            Ok(p) => p,
            Err(_) => {
                warn!("Invalid track ID for SetPosition: {}", track_id);
                return;
            }
        };

        // Optimistic update
        {
            let players = self.players.borrow();
            let active = self.active_player.borrow();
            if let Some(player) = active.as_ref().and_then(|bus| players.get(bus)) {
                player.borrow_mut().position = position_us;
            }
        }

        connection.call(
            Some(&bus_name),
            MPRIS_PATH,
            MPRIS_PLAYER_INTERFACE,
            "SetPosition",
            Some(&(track_path, position_us).to_variant()),
            None::<&glib::VariantTy>,
            gio::DBusCallFlags::NONE,
            DBUS_CALL_TIMEOUT_MS,
            None::<&gio::Cancellable>,
            |res| {
                if let Err(e) = res {
                    warn!("MPRIS SetPosition failed: {}", e);
                }
            },
        );
    }

    fn call_player_method(&self, method: &str) {
        if self.active_player.borrow().as_deref() == Some(MPD_PLAYER_NAME) {
            let command = match method {
                "PlayPause" => {
                    let is_playing = self
                        .mpd_player
                        .borrow()
                        .as_ref()
                        .is_some_and(|p| p.playback_status == PlaybackStatus::Playing);
                    if is_playing { "pause 1" } else { "play" }
                }
                "Next" => "next",
                "Previous" => "previous",
                "Stop" => "stop",
                _ => return,
            };

            if let Err(e) = Self::mpd_query(command) {
                warn!("MPD {} failed: {}", method, e);
            }
            return;
        }

        let Some((connection, bus_name)) = self.get_active_connection() else {
            return;
        };

        let method_owned = method.to_string();
        connection.call(
            Some(&bus_name),
            MPRIS_PATH,
            MPRIS_PLAYER_INTERFACE,
            method,
            None,
            None::<&glib::VariantTy>,
            gio::DBusCallFlags::NONE,
            DBUS_CALL_TIMEOUT_MS,
            None::<&gio::Cancellable>,
            move |res| {
                if let Err(e) = res {
                    warn!("MPRIS {} failed: {}", method_owned, e);
                }
            },
        );
    }

    fn get_active_connection(&self) -> Option<(gio::DBusConnection, String)> {
        let connection = self.connection.borrow().clone()?;
        let bus_name = self.active_player.borrow().clone()?;
        Some((connection, bus_name))
    }
}

impl Drop for MediaService {
    fn drop(&mut self) {
        trace!("MediaService dropping, cleaning up resources");
        self.poll_cancellable.borrow().cancel();
        if let Some(source) = self.position_poll_source.take() {
            source.remove();
        }
        if let Some(source) = self.mpd_refresh_source.take() {
            source.remove();
        }
        self._name_owner_subscription.take();
        self.players.borrow_mut().clear();
    }
}

const MICROSECONDS_PER_SECOND: i64 = 1_000_000;
const SECONDS_PER_MINUTE: i64 = 60;
const SECONDS_PER_HOUR: i64 = 3600;

/// Format microseconds as MM:SS or H:MM:SS.
pub fn format_duration(microseconds: i64) -> String {
    if microseconds < 0 {
        return "0:00".to_string();
    }

    let total_seconds = microseconds / MICROSECONDS_PER_SECOND;
    let hours = total_seconds / SECONDS_PER_HOUR;
    let minutes = (total_seconds % SECONDS_PER_HOUR) / SECONDS_PER_MINUTE;
    let seconds = total_seconds % SECONDS_PER_MINUTE;

    if hours > 0 {
        format!("{}:{:02}:{:02}", hours, minutes, seconds)
    } else {
        format!("{}:{:02}", minutes, seconds)
    }
}

// ============================================================================
// CLI interface - synchronous, standalone (no GTK main loop required)
// ============================================================================

/// Synchronous media control for CLI usage.
///
/// This is a lightweight, standalone interface that doesn't require GTK or
/// a running main loop. It uses synchronous D-Bus calls to control MPRIS
/// media players.
pub struct MediaCli {
    connection: gio::DBusConnection,
    players: Vec<(String, String)>, // (bus_name, player_name)
    active_player: Option<String>,
}

impl MediaCli {
    /// Create a new CLI media controller.
    ///
    /// Returns `None` if D-Bus connection fails.
    pub fn new() -> Option<Self> {
        let connection =
            gio::bus_get_sync(gio::BusType::Session, None::<&gio::Cancellable>).ok()?;

        let mut cli = Self {
            connection,
            players: Vec::new(),
            active_player: None,
        };

        cli.discover_players();
        Some(cli)
    }

    fn discover_players(&mut self) {
        // Call ListNames to find currently owned MPRIS players.
        let owned_result = self.connection.call_sync(
            Some(DBUS_NAME),
            DBUS_PATH,
            DBUS_INTERFACE,
            "ListNames",
            None,
            Some(glib::VariantTy::new("(as)").unwrap()),
            gio::DBusCallFlags::NONE,
            DBUS_CALL_TIMEOUT_MS,
            None::<&gio::Cancellable>,
        );

        let Ok(owned_reply) = owned_result else {
            return;
        };

        let mut names: HashSet<String> = mpris_names_from_reply(&owned_reply).into_iter().collect();

        // Also include activatable services so an MPD MPRIS bridge can be found
        // before any other client has caused it to claim its well-known name.
        let activatable_result = self.connection.call_sync(
            Some(DBUS_NAME),
            DBUS_PATH,
            DBUS_INTERFACE,
            "ListActivatableNames",
            None,
            Some(glib::VariantTy::new("(as)").unwrap()),
            gio::DBusCallFlags::NONE,
            DBUS_CALL_TIMEOUT_MS,
            None::<&gio::Cancellable>,
        );

        if let Ok(activatable_reply) = activatable_result {
            names.extend(mpris_names_from_reply(&activatable_reply));
        }

        let mut names: Vec<String> = names.into_iter().collect();
        names.sort();

        // Build player list with display names
        self.players = names
            .iter()
            .map(|bus_name| {
                let player_id = player_id_from_bus_name(bus_name);
                let player_name = capitalize_first(&player_id);
                (bus_name.clone(), player_name)
            })
            .collect();

        if MediaService::query_mpd_player().is_ok()
            && !self.players.iter().any(|(bus, _)| bus == MPD_PLAYER_NAME)
        {
            self.players
                .push((MPD_PLAYER_NAME.to_string(), "MPD".to_string()));
        }

        // Check if the panel has a selected player via state file.
        // Use the panel's active player so CLI commands control the same player shown in the UI.
        if let Some(ref bus_name) = super::media_ipc::read_state()
            && self.players.iter().any(|(b, _)| b == bus_name)
        {
            self.active_player = Some(bus_name.clone());
            return;
        }

        // Fallback when panel is not running: first playing player, or first player
        self.active_player = self
            .find_playing_player()
            .or_else(|| self.players.first().map(|(bus, _)| bus.clone()));
    }

    fn find_playing_player(&self) -> Option<String> {
        for (bus_name, _) in &self.players {
            if let Some(status) = self.get_playback_status(bus_name)
                && status == PlaybackStatus::Playing
            {
                return Some(bus_name.clone());
            }
        }
        None
    }

    fn get_playback_status(&self, bus_name: &str) -> Option<PlaybackStatus> {
        if bus_name == MPD_PLAYER_NAME {
            return MediaService::query_mpd_player()
                .ok()
                .map(|p| p.playback_status);
        }

        let result = self
            .connection
            .call_sync(
                Some(bus_name),
                MPRIS_PATH,
                PROPERTIES_INTERFACE,
                "Get",
                Some(&(MPRIS_PLAYER_INTERFACE, "PlaybackStatus").to_variant()),
                Some(glib::VariantTy::new("(v)").unwrap()),
                gio::DBusCallFlags::NONE,
                DBUS_CALL_TIMEOUT_MS,
                None::<&gio::Cancellable>,
            )
            .ok()?;

        result
            .child_value(0)
            .get::<Variant>()
            .and_then(|v| v.get::<String>())
            .map(|s| s.parse().unwrap_or(PlaybackStatus::Stopped))
    }

    /// Toggle play/pause on the active player.
    pub fn play_pause(&self) -> Result<(), String> {
        self.call_method("PlayPause")
    }

    /// Skip to next track.
    pub fn next(&self) -> Result<(), String> {
        self.call_method("Next")
    }

    /// Go to previous track.
    pub fn previous(&self) -> Result<(), String> {
        self.call_method("Previous")
    }

    /// Stop playback.
    pub fn stop(&self) -> Result<(), String> {
        self.call_method("Stop")
    }

    /// Get current playback status and metadata.
    pub fn status(&self) -> Result<MediaCliStatus, String> {
        let bus_name = self
            .active_player
            .as_ref()
            .ok_or_else(|| "no media player found".to_string())?;

        if bus_name == MPD_PLAYER_NAME {
            let player = MediaService::query_mpd_player()?;
            return Ok(MediaCliStatus {
                player_name: "MPD".to_string(),
                playback_status: player.playback_status,
                title: player.metadata.title,
                artist: player.metadata.artist,
                position: player.position,
                length: player.metadata.length,
            });
        }

        // Get all properties at once
        let result = self
            .connection
            .call_sync(
                Some(bus_name),
                MPRIS_PATH,
                PROPERTIES_INTERFACE,
                "GetAll",
                Some(&(MPRIS_PLAYER_INTERFACE,).to_variant()),
                Some(glib::VariantTy::new("(a{sv})").unwrap()),
                gio::DBusCallFlags::NONE,
                DBUS_CALL_TIMEOUT_MS,
                None::<&gio::Cancellable>,
            )
            .map_err(|e| format!("failed to get player properties: {}", e))?;

        // Parse properties dict
        let props_variant = result.child_value(0);
        let props: std::collections::HashMap<String, Variant> =
            props_variant.get().unwrap_or_default();

        let playback_status = props
            .get("PlaybackStatus")
            .and_then(|v| v.get::<String>())
            .map(|s| s.parse().unwrap_or(PlaybackStatus::Stopped))
            .unwrap_or(PlaybackStatus::Stopped);

        let metadata = props
            .get("Metadata")
            .map(MediaService::parse_metadata)
            .unwrap_or_default();

        let position = props
            .get("Position")
            .and_then(|v| v.get::<i64>())
            .unwrap_or(0);

        // Get player display name
        let player_name = self
            .players
            .iter()
            .find(|(b, _)| b == bus_name)
            .map(|(_, name)| name.clone())
            .unwrap_or_else(|| bus_name.clone());

        Ok(MediaCliStatus {
            player_name,
            playback_status,
            title: metadata.title,
            artist: metadata.artist,
            position,
            length: metadata.length,
        })
    }

    fn call_method(&self, method: &str) -> Result<(), String> {
        let bus_name = self
            .active_player
            .as_ref()
            .ok_or_else(|| "no media player found".to_string())?;

        if bus_name == MPD_PLAYER_NAME {
            let command = match method {
                "PlayPause" => {
                    if MediaService::query_mpd_player()
                        .ok()
                        .is_some_and(|p| p.playback_status == PlaybackStatus::Playing)
                    {
                        "pause 1"
                    } else {
                        "play"
                    }
                }
                "Next" => "next",
                "Previous" => "previous",
                "Stop" => "stop",
                _ => return Ok(()),
            };
            MediaService::mpd_query(command)?;
            return Ok(());
        }

        self.connection
            .call_sync(
                Some(bus_name),
                MPRIS_PATH,
                MPRIS_PLAYER_INTERFACE,
                method,
                None,
                None,
                gio::DBusCallFlags::NONE,
                DBUS_CALL_TIMEOUT_MS,
                None::<&gio::Cancellable>,
            )
            .map_err(|e| format!("MPRIS {} failed: {}", method, e))?;

        Ok(())
    }
}

/// Status information returned by MediaCli::status().
#[derive(Debug)]
pub struct MediaCliStatus {
    pub player_name: String,
    pub playback_status: PlaybackStatus,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub position: i64,
    pub length: Option<i64>,
}

impl std::fmt::Display for MediaCliStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let status_icon = match self.playback_status {
            PlaybackStatus::Playing => "▶",
            PlaybackStatus::Paused => "⏸",
            PlaybackStatus::Stopped => "⏹",
        };

        write!(f, "{} {}", status_icon, self.player_name)?;

        if let Some(ref title) = self.title {
            write!(f, "\n  {}", title)?;
            if let Some(ref artist) = self.artist {
                write!(f, " - {}", artist)?;
            }
        }

        // Show position/duration if available
        if self.position > 0 || self.length.is_some() {
            let pos_str = format_duration(self.position);
            if let Some(length) = self.length {
                let len_str = format_duration(length);
                write!(f, "\n  {} / {}", pos_str, len_str)?;
            } else {
                write!(f, "\n  {}", pos_str)?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn test_playback_status_from_str() {
        assert_eq!("Playing".parse(), Ok(PlaybackStatus::Playing));
        assert_eq!("Paused".parse(), Ok(PlaybackStatus::Paused));
        assert_eq!("Stopped".parse(), Ok(PlaybackStatus::Stopped));
        assert_eq!("Unknown".parse(), Ok(PlaybackStatus::Stopped));
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(0), "0:00");
        assert_eq!(format_duration(30_000_000), "0:30");
        assert_eq!(format_duration(90_000_000), "1:30");
        assert_eq!(format_duration(3_661_000_000), "1:01:01");
        assert_eq!(format_duration(-1000), "0:00");
    }

    #[test]
    fn mpd_quote_arg_escapes_protocol_strings() {
        assert_eq!(
            MediaService::mpd_quote_arg("simple.opus"),
            "\"simple.opus\""
        );
        assert_eq!(
            MediaService::mpd_quote_arg("artist/quote\"slash\\.opus"),
            "\"artist/quote\\\"slash\\\\.opus\""
        );
    }

    #[test]
    fn mpd_seconds_to_microseconds_rounds_fractional_seconds() {
        assert_eq!(
            MediaService::mpd_seconds_to_microseconds("250.080"),
            Some(250_080_000)
        );
        assert_eq!(MediaService::mpd_seconds_to_microseconds("nope"), None);
    }

    #[test]
    fn test_media_snapshot_default() {
        let snapshot = MediaSnapshot::default();
        assert!(!snapshot.available);
        assert_eq!(snapshot.playback_status, PlaybackStatus::Stopped);
    }

    #[test]
    fn media_callbacks_receive_current_snapshot_and_disconnect() {
        let service = MediaService {
            connection: RefCell::new(None),
            players: RefCell::new(HashMap::new()),
            mpd_player: RefCell::new(None),
            active_player: RefCell::new(None),
            manual_selection: RefCell::new(None),
            last_playing: RefCell::new(None),
            _name_owner_subscription: RefCell::new(None),
            position_poll_source: RefCell::new(None),
            mpd_refresh_source: RefCell::new(None),
            poll_cancellable: RefCell::new(gio::Cancellable::new()),
            callbacks: Callbacks::new(),
        };

        let calls = Rc::new(Cell::new(0));
        let available = Rc::new(Cell::new(true));

        let calls_for_callback = Rc::clone(&calls);
        let available_for_callback = Rc::clone(&available);
        let id = service.connect(move |snapshot| {
            calls_for_callback.set(calls_for_callback.get() + 1);
            available_for_callback.set(snapshot.available);
        });

        assert_eq!(calls.get(), 1);
        assert!(!available.get());
        assert!(service.disconnect(id));

        service.notify_listeners();
        assert_eq!(calls.get(), 1);
    }
}
