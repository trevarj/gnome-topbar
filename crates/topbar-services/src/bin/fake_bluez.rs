//! `topbar-fake-bluez` — a BlueZ to photograph the panel against.
//!
//! Test support only: built behind the `fake-bluez` feature and never part of
//! the packaged panel. The real BlueZ lives on the **system** bus and *is* the
//! developer's headphones; a smoke run has no business switching that radio
//! off, disconnecting whatever is playing, or registering a pairing agent
//! there — so the nested session gets this on its private session bus and the
//! panel is pointed at it with `TOPBAR_SMOKE_BLUEZ_BUS`.
//!
//! ```text
//! topbar-fake-bluez \
//!     --device "buds|WH-1000XM4|AA:BB:CC:DD:EE:FF|audio-headset|connected|85" \
//!     --device "mouse|MX Master|11:22:33:44:55:66|input-mouse" \
//!     --outcome slow
//! ```
//!
//! It prints what it took, then `ready`, and runs until it is killed. The smoke
//! driver moves it about with `gdbus` through
//! `io.github.trevarj.topbar.FakeBluez1`: pairing devices, moving battery
//! levels, queueing what the next connect should do, starting a pairing from
//! the *other* side, and reading back every method the panel called.

use std::process::ExitCode;

use topbar_services::bluetooth::fake::{self, Bluez, FakeDevice, Outcome};

/// Everything the fake needs, as the command line describes it.
struct Options {
    address: String,
    has_adapter: bool,
    powered: bool,
    outcomes: Vec<Outcome>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            address: std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default(),
            has_adapter: true,
            powered: true,
            outcomes: Vec::new(),
        }
    }
}

/// Parse `NAME|ALIAS|ADDRESS|ICON[|connected][|BATTERY]`.
///
/// A vertical bar rather than a colon, because a Bluetooth address is six
/// colon-separated bytes and splitting on colons turned `AA:BB:CC:DD:EE:FF`
/// into six fields.
///
/// The trailing fields are order-free so the common cases stay short: a device
/// that is connected and reports a battery ends `|connected|85`, and one that
/// is neither is just the first four. `unpaired` is something a scan would
/// find; `nameless` replaces the alias with the address, which is what BlueZ
/// publishes for a device that has not answered a name request and what the
/// panel refuses to put in a list.
///
/// An underscore in the alias becomes a space. The smoke harness word-splits
/// the arguments it forwards, so `MX_Master_3S` is how a two-word name gets
/// here at all — and a Bluetooth alias with a real underscore in it is not
/// something a screenshot needs.
fn device(value: &str) -> Option<(String, FakeDevice)> {
    let mut parts = value.split('|');
    let name = parts.next()?.to_string();
    let alias = parts.next()?;
    let address = parts.next()?;
    let icon = parts.next()?;
    let mut device = FakeDevice::paired(&alias.replace('_', " "), address, icon);
    for extra in parts {
        match extra {
            "connected" => device = device.connected(),
            "unpaired" => device = device.unpaired(),
            "nameless" => device = device.nameless(),
            percent => device = device.with_battery(percent.parse().ok()?),
        }
    }
    Some((name, device))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let mut options = Options::default();
    let bluez = Bluez::new();
    let mut arguments = std::env::args().skip(1);
    let mut described = Vec::new();

    while let Some(flag) = arguments.next() {
        let mut value = || arguments.next().unwrap_or_default();
        match flag.as_str() {
            "--address" => options.address = value(),
            "--device" => {
                let raw = value();
                match device(&raw) {
                    Some((name, device)) => {
                        described.push(format!("device {raw}"));
                        bluez.seed_device(&name, device);
                    }
                    None => {
                        eprintln!(
                            "--device wants NAME|ALIAS|ADDRESS|ICON[|connected][|BATTERY], not `{raw}`"
                        );
                        return ExitCode::FAILURE;
                    }
                }
            }
            "--no-adapter" => options.has_adapter = false,
            "--off" => options.powered = false,
            "--outcome" => {
                let raw = value();
                match Outcome::parse(&raw) {
                    Some(outcome) => options.outcomes.push(outcome),
                    None => {
                        eprintln!("--outcome wants success, fail or slow");
                        return ExitCode::FAILURE;
                    }
                }
            }
            other => {
                eprintln!("unknown flag `{other}`");
                return ExitCode::FAILURE;
            }
        }
    }

    if options.address.is_empty() {
        eprintln!("no bus address: pass --address or set DBUS_SESSION_BUS_ADDRESS");
        return ExitCode::FAILURE;
    }

    bluez.set_has_adapter(options.has_adapter);
    bluez.set_powered(options.powered);
    if !options.powered {
        described.push("powered off".to_string());
    }
    for outcome in options.outcomes {
        described.push(format!("outcome {outcome:?}"));
        bluez.queue(outcome);
    }

    let mut served = match fake::serve(&options.address, &bluez).await {
        Ok(served) => served,
        Err(error) => {
            eprintln!("could not serve BlueZ: {error}");
            return ExitCode::FAILURE;
        }
    };

    for line in described {
        println!("{line}");
    }
    println!("ready");
    let connection = served.connection.clone();
    topbar_services::sidecar::park(&connection, "bluez", served.until_quit()).await;
    ExitCode::SUCCESS
}
