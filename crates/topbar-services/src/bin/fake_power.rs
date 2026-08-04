//! `topbar-fake-power` — a UPower and a power-profiles daemon to photograph.
//!
//! Test support only: it is built behind the `fake-power` feature and is not
//! part of the packaged panel. The real ones live on the **system** bus, which
//! a smoke run has no business writing to and no way to replace, so the nested
//! session gets these on its private session bus instead and the panel is
//! pointed at them with `TOPBAR_SMOKE_POWER_BUS`.
//!
//! ```text
//! topbar-fake-power --profiles-name both --active balanced \
//!     --percent 62 --state 2 --time-to-empty 8100 \
//!     --thresholds 75:80 --sysfs /tmp/smoke-power-supply
//! ```
//!
//! It prints what it took and then runs until it is killed. The smoke driver
//! moves it about with `gdbus`: `Properties.Set` on `ActiveProfile` for the
//! power-profiles side, and `io.github.trevarj.topbar.FakePower1` for the
//! battery.

use std::path::PathBuf;
use std::process::ExitCode;

use topbar_services::battery::fake as battery_fake;
use topbar_services::power_profiles::fake as profiles_fake;

/// Everything the two fakes need, as the command line describes it.
struct Options {
    address: String,
    profiles: Option<profiles_fake::Names>,
    active: String,
    available: Vec<String>,
    battery: Option<battery_fake::Recipe>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            address: std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default(),
            profiles: Some(profiles_fake::Names::Both),
            active: "balanced".to_string(),
            available: ["power-saver", "balanced", "performance"]
                .iter()
                .map(|profile| (*profile).to_string())
                .collect(),
            battery: Some(battery_fake::Recipe::default()),
        }
    }
}

/// Parse `START:END`.
fn thresholds(value: &str) -> Option<(u32, u32)> {
    let (start, end) = value.split_once(':')?;
    Some((start.parse().ok()?, end.parse().ok()?))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let mut options = Options::default();
    let mut arguments = std::env::args().skip(1);

    while let Some(flag) = arguments.next() {
        let mut value = || arguments.next().unwrap_or_default();
        match flag.as_str() {
            "--address" => options.address = value(),
            "--profiles-name" => {
                let name = value();
                match profiles_fake::Names::parse(&name) {
                    Some(names) => options.profiles = Some(names),
                    None => {
                        eprintln!("unknown --profiles-name `{name}`");
                        return ExitCode::FAILURE;
                    }
                }
            }
            "--no-profiles" => options.profiles = None,
            "--active" => options.active = value(),
            "--profiles" => {
                options.available = value()
                    .split(',')
                    .filter(|profile| !profile.is_empty())
                    .map(str::to_string)
                    .collect();
            }
            "--no-battery" => options.battery = None,
            "--percent" => {
                if let (Some(battery), Ok(percent)) =
                    (options.battery.as_mut(), value().parse::<f64>())
                {
                    battery.percent = percent;
                }
            }
            "--state" => {
                if let (Some(battery), Ok(state)) =
                    (options.battery.as_mut(), value().parse::<u32>())
                {
                    battery.state = state;
                }
            }
            "--time-to-empty" => {
                if let (Some(battery), Ok(seconds)) =
                    (options.battery.as_mut(), value().parse::<i64>())
                {
                    battery.time_to_empty = seconds;
                }
            }
            "--time-to-full" => {
                if let (Some(battery), Ok(seconds)) =
                    (options.battery.as_mut(), value().parse::<i64>())
                {
                    battery.time_to_full = seconds;
                }
            }
            "--thresholds" => {
                let raw = value();
                match (options.battery.as_mut(), thresholds(&raw)) {
                    (Some(battery), Some((start, end))) => {
                        battery.threshold_supported = true;
                        battery.start_threshold = start;
                        battery.end_threshold = end;
                    }
                    (_, None) => {
                        eprintln!("--thresholds wants START:END, not `{raw}`");
                        return ExitCode::FAILURE;
                    }
                    _ => {}
                }
            }
            "--sysfs" => {
                let root = PathBuf::from(value());
                if let Some(battery) = options.battery.as_mut() {
                    battery.sysfs = Some(root);
                }
            }
            "--battery-name" => {
                if let Some(battery) = options.battery.as_mut() {
                    battery.battery = value();
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

    let mut held: Vec<zbus::Connection> = Vec::new();

    if let Some(names) = options.profiles {
        let borrowed: Vec<&str> = options
            .available
            .iter()
            .map(std::string::String::as_str)
            .collect();
        let state = profiles_fake::Shared::new(&options.active, &borrowed);
        match profiles_fake::serve(&options.address, names, &state).await {
            Ok(connections) => {
                println!("power profiles: {:?}, active {}", names, options.active);
                held.extend(connections);
            }
            Err(error) => {
                eprintln!("could not serve power profiles: {error}");
                return ExitCode::FAILURE;
            }
        }
    }

    // Kept alive for the life of the process; nothing else holds the bus name.
    let mut upower = None;
    if let Some(recipe) = options.battery {
        let description = format!("{}% state {}", recipe.percent, recipe.state);
        match battery_fake::serve(&options.address, recipe).await {
            Ok(fake) => {
                println!("upower: {description}");
                upower = Some(fake);
            }
            Err(error) => {
                eprintln!("could not serve UPower: {error}");
                return ExitCode::FAILURE;
            }
        }
    }

    if held.is_empty() && upower.is_none() {
        eprintln!("nothing to serve");
        return ExitCode::FAILURE;
    }

    println!("ready");
    std::future::pending::<()>().await;
    ExitCode::SUCCESS
}
