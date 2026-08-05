//! `topbar-fake-player` — a media player that exists to be looked at.
//!
//! Test support only: it is built behind the `fake-player` feature and is not
//! part of the packaged panel. The nested-niri smoke run starts two of them on
//! its private bus so the media card has something to draw, and so the player
//! switcher has more than one player to switch between.
//!
//! ```text
//! topbar-fake-player --name fakeone --identity "Fake One" \
//!     --title "Windowlicker" --artist "Aphex Twin" \
//!     --art file:///tmp/cover.png --status Playing --no-next
//! ```
//!
//! It runs until it is killed, or until `Quit` is called on
//! `io.github.trevarj.topbar.FakePlayer1`.

use std::process::ExitCode;

use topbar_services::media::fake::{FakePlayer, Recipe};

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let mut recipe = Recipe::default();
    let mut arguments = std::env::args().skip(1);

    while let Some(flag) = arguments.next() {
        let mut value = || arguments.next().unwrap_or_default();
        match flag.as_str() {
            "--name" => recipe.name = value(),
            "--identity" => recipe.identity = value(),
            "--desktop-entry" => recipe.desktop_entry = Some(value()),
            "--status" => recipe.status = value(),
            "--title" => recipe.title = value(),
            "--artist" => recipe.artist = value(),
            "--album" => recipe.album = value(),
            "--art" => recipe.art_url = Some(value()),
            "--length" => recipe.length_us = value().parse().unwrap_or(recipe.length_us),
            "--position" => recipe.position_us = value().parse().unwrap_or(0),
            "--no-next" => recipe.can_go_next = false,
            "--no-previous" => recipe.can_go_previous = false,
            "--no-seek" => recipe.can_seek = false,
            other => {
                eprintln!("topbar-fake-player: unknown argument `{other}`");
                return ExitCode::FAILURE;
            }
        }
    }

    let player = match FakePlayer::start(&recipe, None).await {
        Ok(player) => player,
        Err(error) => {
            eprintln!("topbar-fake-player: could not take a name on the bus: {error}");
            return ExitCode::FAILURE;
        }
    };
    println!("{}", player.bus_name());

    topbar_services::sidecar::park(player.connection(), "player", player.stopped()).await;
    ExitCode::SUCCESS
}
