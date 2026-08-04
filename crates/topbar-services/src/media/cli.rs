//! The media path that does not need a panel.
//!
//! `topbar media play-pause` is bound to a key like the volume ones, and it
//! goes straight to the session bus for the same reason: a key that only works
//! while the panel happens to be up is not a key anybody can rely on.
//!
//! The choice of *which* player to act on is [`relevance::select`] — the very
//! rule the media card uses — so the key acts on the player the card would
//! have been showing. That is the whole point: v1 kept a state file the panel
//! wrote and the CLI read, which went stale the moment the panel was not
//! running, which is exactly when the CLI needed it.

use futures_util::future::join_all;
use zbus::Connection;

use crate::error::SvcError;
use crate::media::model::PlaybackStatus;
use crate::media::props::TrackMetadata;
use crate::media::proxy::{ApplicationProxy, PlayerProxy};
use crate::media::relevance::{self, Candidate};
use crate::media::{MPRIS_PATH, MPRIS_PREFIX};

/// What a key can ask of a player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    /// Play if paused, pause if playing.
    PlayPause,
    /// Skip forward.
    Next,
    /// Skip back.
    Previous,
    /// Stop.
    Stop,
}

/// One player on the bus, as the CLI sees it.
#[derive(Debug, Clone)]
pub struct PlayerLine {
    /// Its bus name.
    pub bus_name: String,
    /// What it calls itself.
    pub identity: String,
    /// What it is doing.
    pub status: PlaybackStatus,
    /// The track title, if it has one.
    pub title: Option<String>,
    /// The artist, if it names one.
    pub artist: Option<String>,
    /// Whether this is the one a key would act on.
    pub active: bool,
}

impl PlayerLine {
    /// The line `topbar media status` prints for this player.
    pub fn to_line(&self) -> String {
        let marker = if self.active { "*" } else { " " };
        let track = match (&self.title, &self.artist) {
            (Some(title), Some(artist)) => format!("{artist} — {title}"),
            (Some(title), None) => title.clone(),
            (None, _) => "(no track)".to_string(),
        };
        format!(
            "{marker} {:<20} {:<8} {track}",
            self.identity,
            self.status.as_str()
        )
    }
}

/// Act on the most relevant player.
pub async fn control(action: Control) -> Result<String, SvcError> {
    let connection = connect().await?;
    let players = survey(&connection).await;
    let chosen = players
        .iter()
        .find(|player| player.active)
        .ok_or_else(|| SvcError::NoPlayer("nothing is on the bus".to_string()))?;

    let proxy = player_proxy(&connection, &chosen.bus_name).await?;
    let result = match action {
        Control::PlayPause => proxy.play_pause().await,
        Control::Next => proxy.next().await,
        Control::Previous => proxy.previous().await,
        Control::Stop => proxy.stop().await,
    };
    result.map_err(|error| SvcError::Bus(error.to_string()))?;
    Ok(chosen.identity.clone())
}

/// Every player on the bus, most relevant first.
pub async fn status() -> Result<Vec<PlayerLine>, SvcError> {
    let connection = connect().await?;
    let mut players = survey(&connection).await;
    players.sort_by_key(|player| (!player.active, player.identity.clone()));
    Ok(players)
}

/// The session bus, or a plain explanation of why not.
async fn connect() -> Result<Connection, SvcError> {
    Connection::session()
        .await
        .map_err(|error| SvcError::Bus(error.to_string()))
}

/// Read every MPRIS player on the bus and mark the relevant one.
async fn survey(connection: &Connection) -> Vec<PlayerLine> {
    let Ok(dbus) = zbus::fdo::DBusProxy::new(connection).await else {
        return Vec::new();
    };
    let Ok(names) = dbus.list_names().await else {
        return Vec::new();
    };

    let buses: Vec<String> = names
        .into_iter()
        .map(|name| name.as_str().to_string())
        .filter(|name| name.starts_with(MPRIS_PREFIX))
        .collect();

    let mut players: Vec<PlayerLine> = join_all(
        buses
            .into_iter()
            .map(|bus_name| async move { read(connection, bus_name).await }),
    )
    .await
    .into_iter()
    .flatten()
    .collect();

    // A one-shot survey has no history, so the two recency counters the card
    // uses are flat: the choice falls back to status alone, which is the right
    // answer when there is nothing else to know.
    let candidates: Vec<Candidate> = players
        .iter()
        .map(|player| Candidate {
            status: player.status,
            has_track: player.title.is_some(),
            status_seq: 0,
            appeared_seq: 0,
        })
        .collect();
    if let Some(index) = relevance::select(&candidates, None)
        && let Some(player) = players.get_mut(index)
    {
        player.active = true;
    }
    players
}

/// Read one player, or nothing if it stopped answering mid-survey.
async fn read(connection: &Connection, bus_name: String) -> Option<PlayerLine> {
    let player = player_proxy(connection, &bus_name).await.ok()?;
    let status = player
        .playback_status()
        .await
        .map_or(PlaybackStatus::Stopped, |value| {
            PlaybackStatus::parse(&value)
        });
    let metadata = player.metadata().await.ok().map(|fields| {
        TrackMetadata::parse(fields.iter().map(|(key, value)| (key.as_str(), &**value)))
    });

    let identity = ApplicationProxy::builder(connection)
        .destination(bus_name.clone())
        .ok()?
        .path(MPRIS_PATH)
        .ok()?
        .build()
        .await
        .ok()?
        .identity()
        .await
        .unwrap_or_else(|_| crate::media::model::identity_from_bus_name(&bus_name));

    Some(PlayerLine {
        bus_name,
        identity,
        status,
        title: metadata.as_ref().and_then(|track| track.title.clone()),
        artist: metadata.as_ref().and_then(|track| track.artist.clone()),
        active: false,
    })
}

/// A player proxy addressed at `bus_name`.
async fn player_proxy(
    connection: &Connection,
    bus_name: &str,
) -> Result<PlayerProxy<'static>, SvcError> {
    let bus = |error: zbus::Error| SvcError::Bus(error.to_string());
    PlayerProxy::builder(connection)
        .destination(bus_name.to_string())
        .map_err(bus)?
        .path(MPRIS_PATH)
        .map_err(bus)?
        .build()
        .await
        .map_err(bus)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player(identity: &str, status: PlaybackStatus, title: Option<&str>) -> PlayerLine {
        PlayerLine {
            bus_name: format!("org.mpris.MediaPlayer2.{identity}"),
            identity: identity.to_string(),
            status,
            title: title.map(ToString::to_string),
            artist: None,
            active: false,
        }
    }

    #[test]
    fn the_active_player_is_marked_in_the_listing() {
        let mut playing = player(
            "spotify",
            PlaybackStatus::Playing,
            Some("Wish You Were Here"),
        );
        playing.active = true;
        assert!(playing.to_line().starts_with('*'));
        assert!(playing.to_line().contains("Wish You Were Here"));

        let idle = player("firefox", PlaybackStatus::Stopped, None);
        assert!(idle.to_line().starts_with(' '));
        assert!(idle.to_line().contains("(no track)"));
    }

    #[test]
    fn a_track_with_an_artist_reads_as_one_line() {
        let mut line = player("spotify", PlaybackStatus::Playing, Some("Time"));
        line.artist = Some("Pink Floyd".to_string());
        assert!(line.to_line().contains("Pink Floyd — Time"));
    }
}
