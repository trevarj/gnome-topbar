//! `topbar-fake-nm` — a NetworkManager to photograph the panel against.
//!
//! Test support only: it is built behind the `fake-nm` feature and is not part
//! of the packaged panel. The real NetworkManager lives on the **system** bus
//! and *is* the developer's live network connection; a smoke run has no
//! business joining a network, switching a radio or registering a secret agent
//! there, so the nested session gets this on its private session bus and the
//! panel is pointed at it with `TOPBAR_SMOKE_NM_BUS`.
//!
//! ```text
//! topbar-fake-nm \
//!     --ap Home:82:secured --ap Cafe:45:secured --ap Airport:25:open \
//!     --saved Home --active Home \
//!     --vpn "Work:uuid-work:wireguard" \
//!     --carrier 1000
//! ```
//!
//! It prints what it took, then `ready`, and runs until it is killed. The smoke
//! driver moves it about with `gdbus` through
//! `io.github.trevarj.topbar.FakeNm1`: adding and removing access points,
//! moving signal strengths, queueing what the next activation should do, and
//! reading back every method the panel called.

use std::process::ExitCode;

use topbar_services::network::fake::{self, Ap, Nm, Outcome, Profile};

/// Everything the fake needs, as the command line describes it.
struct Options {
    address: String,
    has_wifi: bool,
    has_wired: bool,
    carrier: Option<u32>,
    state: u32,
    active: Option<String>,
    outcomes: Vec<Outcome>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            address: std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default(),
            has_wifi: true,
            has_wired: true,
            carrier: None,
            state: 70,
            active: None,
            outcomes: Vec::new(),
        }
    }
}

/// Parse `SSID:STRENGTH:open|secured`.
fn access_point(value: &str) -> Option<(String, Ap)> {
    let mut parts = value.split(':');
    let ssid = parts.next()?;
    let strength: u8 = parts.next()?.parse().ok()?;
    let secured = match parts.next() {
        Some("secured") => true,
        Some("open") | None => false,
        Some(_) => return None,
    };
    let ap = if secured {
        Ap::secured(ssid, strength)
    } else {
        Ap::open(ssid, strength)
    };
    Some((ssid.to_string(), ap))
}

/// Parse `ID:UUID:KIND[:SERVICE]`.
fn vpn(value: &str) -> Option<Profile> {
    let mut parts = value.split(':');
    let id = parts.next()?;
    let uuid = parts.next()?;
    let kind = parts.next()?;
    let service = parts.next().filter(|service| !service.is_empty());
    Some(Profile::vpn(id, uuid, kind, service))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let mut options = Options::default();
    let nm = Nm::new();
    let mut arguments = std::env::args().skip(1);
    let mut described = Vec::new();

    while let Some(flag) = arguments.next() {
        let mut value = || arguments.next().unwrap_or_default();
        match flag.as_str() {
            "--address" => options.address = value(),
            "--ap" => {
                let raw = value();
                match access_point(&raw) {
                    // The object name is the SSID, so the control interface can
                    // move an access point about by the name the driver knows.
                    Some((name, ap)) => {
                        described.push(format!("ap {raw}"));
                        nm.seed_ap(&name, ap);
                    }
                    None => {
                        eprintln!("--ap wants SSID:STRENGTH:open|secured, not `{raw}`");
                        return ExitCode::FAILURE;
                    }
                }
            }
            "--saved" => {
                let ssid = value();
                described.push(format!("saved {ssid}"));
                nm.seed_profile(Profile::wifi(&ssid, &ssid));
            }
            "--active" => options.active = Some(value()),
            "--vpn" => {
                let raw = value();
                match vpn(&raw) {
                    Some(profile) => {
                        described.push(format!("vpn {raw}"));
                        nm.seed_profile(profile);
                    }
                    None => {
                        eprintln!("--vpn wants ID:UUID:KIND[:SERVICE], not `{raw}`");
                        return ExitCode::FAILURE;
                    }
                }
            }
            "--vpn-active" => {
                let uuid = value();
                described.push(format!("vpn-active {uuid}"));
                nm.seed_vpn_active(&uuid);
            }
            "--carrier" => match value().parse::<u32>() {
                Ok(speed) => options.carrier = Some(speed),
                Err(_) => {
                    eprintln!("--carrier wants a link speed in Mb/s");
                    return ExitCode::FAILURE;
                }
            },
            "--state" => {
                if let Ok(state) = value().parse::<u32>() {
                    options.state = state;
                }
            }
            "--no-wifi" => options.has_wifi = false,
            "--no-wired" => options.has_wired = false,
            "--outcome" => {
                let raw = value();
                match Outcome::parse(&raw) {
                    Some(outcome) => options.outcomes.push(outcome),
                    None => {
                        eprintln!("--outcome wants success, auth_fail, slow or timeout");
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

    nm.set_has_wifi(options.has_wifi);
    nm.set_has_wired(options.has_wired);
    nm.set_state(options.state);
    if let Some(speed) = options.carrier {
        nm.set_carrier(true, speed);
        described.push(format!("carrier {speed} Mb/s"));
    }
    if let Some(active) = &options.active {
        nm.seed_active_ap(active);
        described.push(format!("active {active}"));
    }
    for outcome in options.outcomes {
        described.push(format!("outcome {outcome:?}"));
        nm.queue(outcome);
    }

    let mut served = match fake::serve(&options.address, &nm).await {
        Ok(served) => served,
        Err(error) => {
            eprintln!("could not serve NetworkManager: {error}");
            return ExitCode::FAILURE;
        }
    };

    for line in described {
        println!("{line}");
    }
    println!("ready");
    served.until_quit().await;
    ExitCode::SUCCESS
}
