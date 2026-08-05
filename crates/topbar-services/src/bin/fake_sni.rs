//! `topbar-fake-sni` — a tray application that exists to be looked at.
//!
//! Test support only: it is built behind the `fake-sni` feature and is not part
//! of the packaged panel. The nested-niri smoke run starts several of them on
//! its private bus so the tray has something to draw, and so the overflow
//! chevron has more icons than it can fit.
//!
//! ```text
//! topbar-fake-sni --name syncthing --title Syncthing \
//!     --icon-name folder-remote-symbolic --status NeedsAttention
//!
//! topbar-fake-sni --name colourful --pixmap 22x14:ff3584e4 --menu-file menu.json
//! ```
//!
//! It prints the bus name it took and the identifier the panel will know it by,
//! then runs until it is killed or `Quit` is called on
//! `io.github.trevarj.topbar.FakeSni1`.

use std::process::ExitCode;

use topbar_services::tray::fake::{DEFAULT_MENU, FakeSni, Recipe, parse_menu};

/// How many times registration is retried while no watcher is listening.
const REGISTRATION_ATTEMPTS: u32 = 60;
/// How long between those attempts.
const REGISTRATION_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

/// Parse `WIDTHxHEIGHT:AARRGGBB`, e.g. `22x14:ff3584e4`.
fn pixmap(spec: &str) -> Result<(i32, i32, u32), String> {
    let (size, colour) = spec
        .split_once(':')
        .ok_or_else(|| format!("`{spec}` is not WIDTHxHEIGHT:AARRGGBB"))?;
    let (width, height) = size
        .split_once('x')
        .ok_or_else(|| format!("`{size}` is not WIDTHxHEIGHT"))?;
    Ok((
        width.parse().map_err(|_| format!("bad width `{width}`"))?,
        height
            .parse()
            .map_err(|_| format!("bad height `{height}`"))?,
        u32::from_str_radix(colour, 16).map_err(|_| format!("bad colour `{colour}`"))?,
    ))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let mut recipe = Recipe::default();
    let mut arguments = std::env::args().skip(1);

    while let Some(flag) = arguments.next() {
        let mut value = || arguments.next().unwrap_or_default();
        match flag.as_str() {
            "--name" => recipe.name = value(),
            "--id" => recipe.id = value(),
            "--title" => recipe.title = value(),
            "--status" => recipe.status = value(),
            "--icon-name" => recipe.icon_name = Some(value()),
            "--no-icon-name" => recipe.icon_name = None,
            "--theme-path" => recipe.theme_path = Some(value()),
            "--tooltip" => recipe.tooltip_title = value(),
            "--tooltip-body" => recipe.tooltip_body = value(),
            "--item-is-menu" => recipe.item_is_menu = true,
            "--no-menu" => recipe.menu = None,
            "--menu" => recipe.menu = Some(value()),
            "--menu-file" => {
                let path = value();
                match std::fs::read_to_string(&path) {
                    Ok(json) => recipe.menu = Some(json),
                    Err(error) => {
                        eprintln!("topbar-fake-sni: cannot read {path}: {error}");
                        return ExitCode::FAILURE;
                    }
                }
            }
            "--default-menu" => recipe.menu = Some(DEFAULT_MENU.to_string()),
            "--pixmap" => {
                // Named without an icon name: a pixmap only reaches the screen
                // when there is no themed name to prefer over it.
                recipe.icon_name = None;
                match pixmap(&value()) {
                    Ok(pixmap) => recipe.pixmaps.push(pixmap),
                    Err(error) => {
                        eprintln!("topbar-fake-sni: {error}");
                        return ExitCode::FAILURE;
                    }
                }
            }
            other => {
                eprintln!("topbar-fake-sni: unknown argument `{other}`");
                return ExitCode::FAILURE;
            }
        }
    }

    if let Some(menu) = recipe.menu.as_deref()
        && let Err(error) = parse_menu(menu)
    {
        eprintln!("topbar-fake-sni: bad menu: {error}");
        return ExitCode::FAILURE;
    }

    let item = match FakeSni::start(&recipe, None).await {
        Ok(item) => item,
        Err(error) => {
            eprintln!("topbar-fake-sni: could not take a name on the bus: {error}");
            return ExitCode::FAILURE;
        }
    };
    // Real applications wait for a watcher rather than giving up on the first
    // refusal, and so does this one — which is also what lets the smoke run
    // start its applications before the panel that is going to draw them.
    let mut left = REGISTRATION_ATTEMPTS;
    loop {
        match item.register().await {
            Ok(()) => break,
            Err(error) if left == 0 => {
                eprintln!("topbar-fake-sni: no watcher took the registration: {error}");
                return ExitCode::FAILURE;
            }
            Err(_) => {
                left -= 1;
                tokio::time::sleep(REGISTRATION_INTERVAL).await;
            }
        }
    }

    // Both lines matter to the smoke driver: the first is what it addresses
    // the control interface with, the second is what the panel logs.
    println!("{}", item.bus_name());
    println!("{}", item.item_id().await);

    topbar_services::sidecar::park(item.connection(), "sni", item.stopped()).await;
    ExitCode::SUCCESS
}
