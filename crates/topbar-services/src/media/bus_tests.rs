//! The media service against real players on a real bus — a **private** one.
//!
//! The players are [`fake::FakePlayer`]s, which serve the same two interfaces
//! Spotify does plus a control interface no real player has, so a test can
//! change a track or take a capability away. Everything runs on a
//! `dbus-daemon` that exists for the length of one test: `cargo test` never
//! touches the music the developer is listening to.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use zbus::zvariant::ObjectPath;

use super::fake::{FakePlayer, Recipe};
use super::*;
use crate::private_bus::{PrivateBus, private_bus};

/// How long a test waits for the panel to catch up before failing.
const PATIENCE: Duration = Duration::from_secs(10);

/// The fake player's control interface, as a test drives it.
#[zbus::proxy(
    interface = "io.github.trevarj.topbar.FakePlayer1",
    default_path = "/org/mpris/MediaPlayer2"
)]
trait FakeControl {
    fn set_track(
        &self,
        title: &str,
        artist: &str,
        art_url: &str,
        length_us: i64,
    ) -> zbus::Result<()>;

    fn set_status(&self, status: &str) -> zbus::Result<()>;

    fn set_capability(&self, name: &str, allowed: bool) -> zbus::Result<()>;
}

/// The player's own interface, as another client sees it.
///
/// Used to move a player about behind the panel's back, which is what a media
/// key or the player's own window does.
#[zbus::proxy(
    interface = "org.mpris.MediaPlayer2.Player",
    default_path = "/org/mpris/MediaPlayer2"
)]
trait Playback {
    fn set_position(&self, track_id: &ObjectPath<'_>, position: i64) -> zbus::Result<()>;
}

/// A control proxy for one fake player.
async fn control(bus: &PrivateBus, player: &FakePlayer) -> FakeControlProxy<'static> {
    FakeControlProxy::builder(&bus.connect().await)
        .destination(player.bus_name().to_string())
        .expect("a well-formed bus name")
        .build()
        .await
        .expect("the fake player's control interface")
}

/// Wait until a published snapshot satisfies `predicate`.
async fn wait_for(
    state: &mut watch::Receiver<Arc<MediaState>>,
    what: &str,
    predicate: impl Fn(&MediaState) -> bool,
) -> Arc<MediaState> {
    let wait = async {
        loop {
            // Cloned out before testing: holding a read guard across an await
            // deadlocks against the task trying to publish the next one.
            let snapshot = state.borrow_and_update().clone();
            if predicate(&snapshot) {
                return snapshot;
            }
            state.changed().await.expect("the media service is alive");
        }
    };
    tokio::time::timeout(PATIENCE, wait)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

/// Whether every player on the bus has answered its first questions.
///
/// A player is on the card the moment its name appears, before it has said
/// anything; a test that acts before then is testing the race, not the rule.
fn all_read(state: &MediaState, count: usize) -> bool {
    state.players.len() == count && state.players.iter().all(PlayerView::has_track)
}

/// The recipe every test starts from, named after the test.
fn recipe(name: &str) -> Recipe {
    Recipe {
        name: name.to_string(),
        identity: format!("Fake {name}"),
        title: "Windowlicker".to_string(),
        artist: "Aphex Twin".to_string(),
        ..Recipe::default()
    }
}

#[tokio::test]
async fn a_player_already_on_the_bus_is_found_and_read() {
    let bus = private_bus!();
    let mut player = recipe("discovered");
    player.desktop_entry = Some("org.example.Fake".to_string());
    player.length_us = 361_000_000;
    let _player = FakePlayer::start(&player, Some(bus.address()))
        .await
        .expect("the fake player takes its name");

    let media = Media::start(Some(bus.address().to_string()));
    let mut state = media.state();
    let settled = wait_for(&mut state, "the player to be read", |state| {
        state.active().is_some_and(|view| view.title.is_some())
    })
    .await;

    let view = settled.active().expect("an active player");
    assert_eq!(view.bus_name, "org.mpris.MediaPlayer2.discovered");
    assert_eq!(view.identity, "Fake discovered");
    assert_eq!(view.desktop_entry.as_deref(), Some("org.example.Fake"));
    assert_eq!(view.title.as_deref(), Some("Windowlicker"));
    assert_eq!(view.artist.as_deref(), Some("Aphex Twin"));
    assert_eq!(view.length_us, 361_000_000);
    assert_eq!(view.status, PlaybackStatus::Paused);
    assert!(view.can_play && view.can_pause && view.can_go_next && view.can_seek);
}

#[tokio::test]
async fn a_player_that_starts_later_is_picked_up() {
    let bus = private_bus!();
    let media = Media::start(Some(bus.address().to_string()));
    let mut state = media.state();
    wait_for(&mut state, "an empty bus", |state| state.is_empty()).await;

    let _player = FakePlayer::start(&recipe("latecomer"), Some(bus.address()))
        .await
        .expect("the fake player takes its name");

    let settled = wait_for(&mut state, "the new player", |state| {
        state.active().is_some_and(|view| view.title.is_some())
    })
    .await;
    assert_eq!(settled.players.len(), 1);
    assert_eq!(
        settled.active().expect("an active player").identity,
        "Fake latecomer"
    );
}

#[tokio::test]
async fn a_property_change_reaches_the_panel() {
    let bus = private_bus!();
    let player = FakePlayer::start(&recipe("changing"), Some(bus.address()))
        .await
        .expect("the fake player takes its name");
    let control = control(&bus, &player).await;

    let media = Media::start(Some(bus.address().to_string()));
    let mut state = media.state();
    wait_for(&mut state, "the player to be read", |state| {
        state.active().is_some_and(|view| view.title.is_some())
    })
    .await;

    control
        .set_track("Avril 14th", "Aphex Twin", "", 120_000_000)
        .await
        .expect("the fake player changes track");

    let settled = wait_for(&mut state, "the new track", |state| {
        state
            .active()
            .is_some_and(|view| view.title.as_deref() == Some("Avril 14th"))
    })
    .await;
    let view = settled.active().expect("an active player");
    assert_eq!(view.length_us, 120_000_000);
    assert_eq!(view.position_us, 0, "a new track starts at the beginning");

    control
        .set_capability("CanGoNext", false)
        .await
        .expect("the fake player drops a capability");
    let settled = wait_for(&mut state, "the capability change", |state| {
        state.active().is_some_and(|view| !view.can_go_next)
    })
    .await;
    assert!(
        settled.active().expect("an active player").can_go_previous,
        "only the capability that changed changed"
    );
}

#[tokio::test]
async fn a_player_that_quits_is_dropped_and_taken_off_the_card() {
    let bus = private_bus!();
    let player = FakePlayer::start(&recipe("quitter"), Some(bus.address()))
        .await
        .expect("the fake player takes its name");

    let media = Media::start(Some(bus.address().to_string()));
    let mut state = media.state();
    wait_for(&mut state, "the player to be read", |state| {
        all_read(state, 1)
    })
    .await;

    player.quit().await;

    let settled = wait_for(&mut state, "the player to go", |state| state.is_empty()).await;
    assert_eq!(settled.active, None);

    let error = media
        .handle()
        .play_pause()
        .await
        .expect_err("there is nothing left to play");
    assert!(matches!(error, SvcError::NoPlayer(_)), "{error:?}");
    assert_eq!(error.user_message(), "No media player is available");
}

#[tokio::test]
async fn the_playing_player_takes_the_card_from_the_paused_one() {
    let bus = private_bus!();
    let quiet = FakePlayer::start(&recipe("quiet"), Some(bus.address()))
        .await
        .expect("a paused player");
    let loud = FakePlayer::start(&recipe("loud"), Some(bus.address()))
        .await
        .expect("a second paused player");
    let loud_control = control(&bus, &loud).await;

    let media = Media::start(Some(bus.address().to_string()));
    let mut state = media.state();
    wait_for(&mut state, "both players to be read", |state| {
        all_read(state, 2)
    })
    .await;

    loud_control
        .set_status("Playing")
        .await
        .expect("the second player starts playing");

    let settled = wait_for(&mut state, "the card to move", |state| {
        state
            .active()
            .is_some_and(|view| view.status == PlaybackStatus::Playing)
    })
    .await;
    assert_eq!(
        settled.active().expect("an active player").bus_name,
        loud.bus_name()
    );
    assert_eq!(
        settled.players.len(),
        2,
        "both players stay on the switcher"
    );
    assert!(
        settled
            .players
            .iter()
            .any(|view| view.bus_name == quiet.bus_name()),
        "the paused player is still there to switch back to"
    );
}

#[tokio::test]
async fn a_pinned_player_keeps_the_card_until_it_goes_away() {
    let bus = private_bus!();
    let pinned = FakePlayer::start(&recipe("pinned"), Some(bus.address()))
        .await
        .expect("a paused player");
    let other = FakePlayer::start(&recipe("other"), Some(bus.address()))
        .await
        .expect("a second player");
    let other_control = control(&bus, &other).await;

    let media = Media::start(Some(bus.address().to_string()));
    let mut state = media.state();
    wait_for(&mut state, "both players to be read", |state| {
        all_read(state, 2)
    })
    .await;

    media
        .handle()
        .select_player(pinned.bus_name().to_string())
        .await
        .expect("the pinned player is on the bus");
    let settled = wait_for(&mut state, "the pin", |state| {
        state
            .active()
            .is_some_and(|view| view.bus_name == pinned.bus_name())
    })
    .await;
    assert_eq!(settled.players.len(), 2);

    // Something else starts playing: the pin holds.
    other_control
        .set_status("Playing")
        .await
        .expect("the other player starts playing");
    let settled = wait_for(&mut state, "the other player to start", |state| {
        state
            .players
            .iter()
            .any(|view| view.status == PlaybackStatus::Playing)
    })
    .await;
    assert_eq!(
        settled.active().expect("an active player").bus_name,
        pinned.bus_name(),
        "a pin outranks what the bus is doing"
    );

    // The pinned player quits: the card goes to whatever is playing.
    pinned.quit().await;
    let settled = wait_for(&mut state, "the pin to lapse", |state| {
        state.players.len() == 1
    })
    .await;
    assert_eq!(
        settled.active().expect("an active player").bus_name,
        other.bus_name(),
        "a pin lasts exactly as long as the player it names"
    );

    let error = media
        .handle()
        .select_player("org.mpris.MediaPlayer2.imaginary".to_string())
        .await
        .expect_err("a player that is not there cannot be pinned");
    assert!(matches!(error, SvcError::NoPlayer(_)), "{error:?}");
}

#[tokio::test]
async fn a_command_reaches_the_player_it_is_meant_for() {
    let bus = private_bus!();
    let player = FakePlayer::start(&recipe("controlled"), Some(bus.address()))
        .await
        .expect("a paused player");

    let media = Media::start(Some(bus.address().to_string()));
    let mut state = media.state();
    wait_for(&mut state, "the player to be read", |state| {
        all_read(state, 1)
    })
    .await;

    media.handle().play_pause().await.expect("the call is sent");
    let command = tokio::time::timeout(PATIENCE, player.acted())
        .await
        .expect("PlayPause arrives at the player");
    assert_eq!(command, "PlayPause");

    // And the player's own answer comes back the other way.
    let settled = wait_for(&mut state, "the status to flip", |state| {
        state
            .active()
            .is_some_and(|view| view.status == PlaybackStatus::Playing)
    })
    .await;
    assert_eq!(
        settled.active().expect("active").status,
        PlaybackStatus::Playing
    );
    assert_eq!(player.status().await, "Playing");

    media
        .handle()
        .seek_to(45_000_000)
        .await
        .expect("the seek is sent");
    tokio::time::timeout(PATIENCE, async {
        while player.position().await != 45_000_000 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the seek arrives at the player");

    media.handle().raise().await.expect("the raise is sent");
    tokio::time::timeout(PATIENCE, player.raised())
        .await
        .expect("Raise arrives at the player");
}

#[tokio::test]
async fn the_position_is_polled_only_while_the_panel_is_looking() {
    let bus = private_bus!();
    let mut started = recipe("seeking");
    started.status = "Playing".to_string();
    started.position_us = 42_000_000;
    let player = FakePlayer::start(&started, Some(bus.address()))
        .await
        .expect("a playing player");

    // A client moving the player about behind the panel's back, which is what
    // a keyboard shortcut or the player's own window does.
    let playback = PlaybackProxy::builder(&bus.connect().await)
        .destination(player.bus_name().to_string())
        .expect("a well-formed bus name")
        .build()
        .await
        .expect("the player's playback interface");
    let track = ObjectPath::try_from("/io/github/trevarj/topbar/track/1").expect("a track path");

    let media = Media::start(Some(bus.address().to_string()));
    let mut state = media.state();
    let settled = wait_for(&mut state, "the player", |state| {
        state
            .active()
            .is_some_and(|view| view.status == PlaybackStatus::Playing)
    })
    .await;
    // The first GetAll carries Position, so the panel starts out right; what
    // it must not do is keep asking while nothing is looking at the answer.
    assert_eq!(settled.active().expect("active").position_us, 42_000_000);

    playback
        .set_position(&track, 90_000_000)
        .await
        .expect("the player jumps");
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    assert_eq!(
        state.borrow().active().expect("active").position_us,
        42_000_000,
        "with no popover open the panel never asked, so it never noticed"
    );

    media
        .handle()
        .set_position_tracking(true)
        .await
        .expect("tracking is switched on");

    // Switching tracking on polls immediately, so the bar is right on the
    // frame the popover appears rather than a second later.
    let settled = wait_for(&mut state, "an immediate poll", |state| {
        state
            .active()
            .is_some_and(|view| view.position_us == 90_000_000)
    })
    .await;
    let view = settled.active().expect("active");
    assert!(view.position_at(std::time::Instant::now()) >= 90_000_000);

    media
        .handle()
        .set_position_tracking(false)
        .await
        .expect("tracking is switched off");
    playback
        .set_position(&track, 5_000_000)
        .await
        .expect("the player jumps again");
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    assert_eq!(
        state.borrow().active().expect("active").position_us,
        90_000_000,
        "closing the popover stops the polling again"
    );
}

#[tokio::test]
async fn album_art_on_disk_is_read_and_handed_to_the_panel() {
    let bus = private_bus!();
    let art = std::env::temp_dir().join(format!("topbar-cover-{}.png", std::process::id()));
    std::fs::write(&art, b"pretend this is a cover").expect("write the cover");

    let mut started = recipe("illustrated");
    started.art_url = Some(format!("file://{}", art.display()));
    let player = FakePlayer::start(&started, Some(bus.address()))
        .await
        .expect("a player with a cover");
    let control = control(&bus, &player).await;

    let media = Media::start(Some(bus.address().to_string()));
    let mut state = media.state();
    let settled = wait_for(&mut state, "the cover", |state| {
        state.active().is_some_and(|view| view.art.is_some())
    })
    .await;

    let cover = settled
        .active()
        .expect("active")
        .art
        .clone()
        .expect("a cover");
    assert_eq!(cover.path, art, "a local cover is read where it lies");

    // A track with no cover clears the one before it, once the grace period
    // that protects against Chromium's temporary files has run out.
    control
        .set_track("Nothing To Look At", "Nobody", "", 60_000_000)
        .await
        .expect("the fake player changes track");
    wait_for(&mut state, "the cover to clear", |state| {
        state.active().is_some_and(|view| view.art.is_none())
    })
    .await;

    std::fs::remove_file(&art).expect("clean up");
}

#[tokio::test]
async fn a_player_that_says_nothing_at_all_is_still_a_player() {
    let bus = private_bus!();
    let quiet = Recipe {
        name: "silent".to_string(),
        identity: String::new(),
        title: String::new(),
        artist: String::new(),
        status: "Stopped".to_string(),
        length_us: 0,
        can_go_next: false,
        can_go_previous: false,
        can_seek: false,
        ..Recipe::default()
    };
    let _player = FakePlayer::start(&quiet, Some(bus.address()))
        .await
        .expect("a player with nothing to say");

    let media = Media::start(Some(bus.address().to_string()));
    let mut state = media.state();
    let settled = wait_for(&mut state, "the player", |state| !state.is_empty()).await;

    let view = settled.active().expect("an active player");
    assert_eq!(
        view.identity, "Silent",
        "a player with no Identity is named after its bus name"
    );
    assert_eq!(view.title, None);
    assert!(!view.has_track());
    assert!(!view.can_seek);
}
