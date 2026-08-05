//! The network service against a NetworkManager of the test's own.
//!
//! Everything here needs a `dbus-daemon`, which a Nix build sandbox has no
//! machine id for, so these run in the dev shell and on a real desktop and sit
//! out `nix flake check`. What is only covered here is the wire protocol
//! itself — the conversation that joining a secured network actually is.
//!
//! Not one of these tests can reach the machine's real NetworkManager: the fake
//! owns the name on a bus that exists for the length of the test, and the panel
//! is pointed at it by address.

use std::sync::Arc;
use std::time::Duration;

use super::fake::{self, Ap, Nm, Outcome, Profile};
use super::model::{Access, NetworkState};
use super::{Network, PersistedNetwork, Secret};
use crate::private_bus::{PrivateBus, private_bus};

/// How long a test waits for the panel to catch up before failing.
const PATIENCE: Duration = Duration::from_secs(10);
/// The fake's own control interface.
const CONTROL: &str = "io.github.trevarj.topbar.FakeNm1";

/// Start the panel's network service against `bus`.
fn panel(bus: &PrivateBus) -> Network {
    Network::start(
        Some(bus.address().to_string()),
        PersistedNetwork::default(),
        None,
    )
}

/// Wait until the snapshot satisfies `wanted`, or fail saying what it was.
async fn settle(
    network: &Network,
    what: &str,
    wanted: impl Fn(&NetworkState) -> bool,
) -> Arc<NetworkState> {
    let mut state = network.state();
    let wait = async {
        loop {
            {
                let current = state.borrow_and_update();
                if wanted(&current) {
                    return current.clone();
                }
            }
            state.changed().await.expect("the network service is alive");
        }
    };
    match tokio::time::timeout(PATIENCE, wait).await {
        Ok(state) => state,
        Err(_) => panic!(
            "timed out waiting for {what}; last state {:?}",
            network.current()
        ),
    }
}

/// Wait until `check` holds of the fake, or fail.
async fn until(nm: &Arc<Nm>, what: &str, check: impl Fn(&Arc<Nm>) -> bool) {
    let wait = async {
        while !check(nm) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    tokio::time::timeout(PATIENCE, wait)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}; calls {:?}", nm.calls()));
}

/// Drive one of the fake's controls, the way the smoke driver does.
async fn drive<B>(bus: &PrivateBus, method: &str, body: &B)
where
    B: serde::ser::Serialize + zbus::zvariant::DynamicType,
{
    bus.connect()
        .await
        .call_method(
            Some(fake::NM_NAME),
            fake::CONTROL_PATH,
            Some(CONTROL),
            method,
            body,
        )
        .await
        .unwrap_or_else(|error| panic!("the control call {method} failed: {error}"));
}

/// A NetworkManager with one saved network, three in range, and a cable out.
fn furnished() -> Arc<Nm> {
    let nm = Nm::new();
    nm.seed_ap("1", Ap::secured("Home", 82));
    nm.seed_ap("2", Ap::secured("Cafe", 45));
    nm.seed_ap("3", Ap::open("Airport", 25));
    nm.seed_profile(Profile::wifi("Home", "Home"));
    nm.seed_active_ap("1");
    nm
}

#[tokio::test]
async fn the_panel_reads_devices_access_points_and_saved_profiles() {
    let bus = private_bus!();
    let nm = furnished();
    let _served = fake::serve(bus.address(), &nm)
        .await
        .expect("the fake starts");

    let network = panel(&bus);
    let state = settle(&network, "the first full read", |state| {
        state.available && state.wifi.list.len() == 3
    })
    .await;

    assert!(state.wifi.present);
    assert!(state.wifi.enabled);
    assert!(state.wired.present);
    assert_eq!(state.access, Access::Full, "an injected bus is ours to use");

    // Active first, then saved, then the rest by strength.
    let names: Vec<&str> = state.wifi.list.iter().map(|ap| ap.ssid.as_str()).collect();
    assert_eq!(names, ["Home", "Cafe", "Airport"]);

    let home = &state.wifi.list[0];
    assert!(home.active);
    assert!(home.known, "there is a profile for it");
    assert!(home.secured);
    assert_eq!(home.bucket, 4, "82 is five bars");

    let airport = &state.wifi.list[2];
    assert!(!airport.secured, "an open network has no padlock");
    assert!(!airport.known);

    assert_eq!(
        state.wifi.active.as_ref().map(|ap| ap.ssid.as_str()),
        Some("Home")
    );
}

#[tokio::test]
async fn the_saved_networks_cost_one_call_per_profile_and_not_one_more() {
    let bus = private_bus!();
    let nm = Nm::new();
    for index in 0..6 {
        nm.seed_profile(Profile::wifi(
            &format!("net{index}"),
            &format!("net{index}"),
        ));
    }
    let _served = fake::serve(bus.address(), &nm)
        .await
        .expect("the fake starts");

    let network = panel(&bus);
    settle(&network, "the profiles to be read", |state| state.available).await;
    until(&nm, "every profile to be read", |nm| {
        nm.count("GetSettings") >= 6
    })
    .await;
    // Let anything else the panel wanted to do arrive.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // The whole of the "which networks are saved" question: one list, one read
    // per profile. v1 ran `nmcli` once for the list and then once *more* per
    // profile for its SSID, every thirty seconds — the N+1 the plan names.
    assert_eq!(nm.count("ListConnections"), 1, "{:?}", nm.calls());
    assert_eq!(nm.count("GetSettings"), 6, "{:?}", nm.calls());
}

#[tokio::test]
async fn joining_a_saved_network_activates_its_own_profile() {
    let bus = private_bus!();
    let nm = Nm::new();
    nm.seed_ap("1", Ap::secured("Home", 70));
    nm.seed_profile(Profile::wifi("Home", "Home"));
    let _served = fake::serve(bus.address(), &nm)
        .await
        .expect("the fake starts");

    let network = panel(&bus);
    settle(&network, "the list", |state| state.wifi.list.len() == 1).await;
    until(&nm, "the agent to register", |nm| !nm.agents().is_empty()).await;

    network
        .handle()
        .connect("Home".to_string())
        .await
        .expect("the saved network comes up");

    assert!(
        nm.calls().contains(&"ActivateConnection".to_string()),
        "a saved profile is activated, not re-added: {:?}",
        nm.calls()
    );
    assert!(
        !nm.calls().contains(&"AddAndActivateConnection".to_string()),
        "a second profile for a network that has one is v1's litter"
    );
}

#[tokio::test]
async fn joining_an_unknown_secured_network_goes_through_the_secret_agent() {
    let bus = private_bus!();
    let nm = Nm::new();
    nm.seed_ap("1", Ap::secured("Cafe", 60));
    let _served = fake::serve(bus.address(), &nm)
        .await
        .expect("the fake starts");

    let network = panel(&bus);
    settle(&network, "the list", |state| state.wifi.list.len() == 1).await;
    until(&nm, "the agent to register", |nm| !nm.agents().is_empty()).await;
    assert_eq!(nm.agents(), vec!["io.github.trevarj.topbar".to_string()]);
    assert!(
        nm.calls()
            .contains(&"RegisterWithCapabilities:0".to_string()),
        "no VPN hints are claimed, because none are answered"
    );

    let handle = network.handle().clone();
    let joining = tokio::spawn(async move { handle.connect("Cafe".to_string()).await });

    // The password row appears because NetworkManager asked for it, not
    // because the panel guessed the network was secured.
    let state = settle(&network, "the password prompt", |state| {
        state.prompt.is_some()
    })
    .await;
    let prompt = state.prompt.as_ref().expect("a prompt");
    assert_eq!(prompt.ssid, "Cafe");
    assert_eq!(prompt.attempt, 1);
    assert!(!prompt.is_retry());

    network
        .handle()
        .submit_secret(Secret::new("hunter2".to_string()))
        .await
        .expect("the answer goes back");

    joining
        .await
        .expect("the task ran")
        .expect("the network comes up");

    // The password reached NetworkManager, and it reached it through the
    // agent's reply: the fake records only what came back from `GetSecrets`.
    assert_eq!(nm.secrets(), vec!["hunter2".to_string()]);
    let calls = nm.calls();
    assert!(calls.contains(&"AddAndActivateConnection".to_string()));
    assert!(calls.contains(&"GetSecrets".to_string()));
    assert!(
        !calls.contains(&"AddAndActivateConnection:with-settings".to_string()),
        "the connection dictionary must be empty; NetworkManager builds the profile"
    );
}

#[tokio::test]
async fn a_refused_password_deletes_the_profile_and_asks_again() {
    let bus = private_bus!();
    let nm = Nm::new();
    nm.seed_ap("1", Ap::secured("Cafe", 60));
    nm.queue(Outcome::AuthFail);
    let _served = fake::serve(bus.address(), &nm)
        .await
        .expect("the fake starts");

    let network = panel(&bus);
    settle(&network, "the list", |state| state.wifi.list.len() == 1).await;
    until(&nm, "the agent to register", |nm| !nm.agents().is_empty()).await;

    let handle = network.handle().clone();
    let joining = tokio::spawn(async move { handle.connect("Cafe".to_string()).await });

    settle(&network, "the first prompt", |state| {
        state
            .prompt
            .as_ref()
            .is_some_and(|prompt| prompt.attempt == 1)
    })
    .await;
    network
        .handle()
        .submit_secret(Secret::new("wrong".to_string()))
        .await
        .expect("the answer goes back");

    // The attempt failed, the panel asks again, and it says so by counting.
    let state = settle(&network, "the second prompt", |state| {
        state
            .prompt
            .as_ref()
            .is_some_and(|prompt| prompt.attempt > 1)
    })
    .await;
    let prompt = state.prompt.as_ref().expect("a prompt");
    assert_eq!(prompt.ssid, "Cafe", "the row keeps the network it is for");
    assert!(prompt.is_retry(), "which is what puts the error caption up");

    // And the profile NetworkManager added for the failed attempt is gone
    // rather than left in the list as a dead duplicate.
    until(&nm, "the added profile to be deleted", |nm| {
        nm.profile_count() == 0
    })
    .await;
    assert!(nm.calls().contains(&"Delete".to_string()));

    // The second password works, on a profile added afresh.
    network
        .handle()
        .submit_secret(Secret::new("right".to_string()))
        .await
        .expect("the answer goes back");
    joining
        .await
        .expect("the task ran")
        .expect("the second attempt comes up");
    assert_eq!(nm.secrets(), vec!["wrong".to_string(), "right".to_string()]);
}

#[tokio::test]
async fn cancelling_an_attempt_removes_the_profile_it_would_have_used() {
    let bus = private_bus!();
    let nm = Nm::new();
    nm.seed_ap("1", Ap::secured("Cafe", 60));
    nm.queue(Outcome::Timeout);
    let _served = fake::serve(bus.address(), &nm)
        .await
        .expect("the fake starts");

    let network = panel(&bus);
    settle(&network, "the list", |state| state.wifi.list.len() == 1).await;
    until(&nm, "the agent to register", |nm| !nm.agents().is_empty()).await;

    let handle = network.handle().clone();
    let joining = tokio::spawn(async move { handle.connect("Cafe".to_string()).await });

    // Nothing prompts, because the queued outcome never asks — so the panel is
    // simply waiting, and the user gives up.
    settle(&network, "the attempt to start", |state| {
        state.pending.is_some()
    })
    .await;
    network
        .handle()
        .cancel_prompt()
        .await
        .expect("the prompt goes away");

    // Cancelling is not a failure to report at the user under a row they just
    // dismissed.
    joining.await.expect("the task ran").expect("no error");
    until(&nm, "the added profile to be deleted", |nm| {
        nm.profile_count() == 0
    })
    .await;

    let state = settle(&network, "the pending flag to clear", |state| {
        state.pending.is_none()
    })
    .await;
    assert!(state.prompt.is_none());
}

#[tokio::test]
async fn the_radio_switch_writes_the_property_and_the_list_empties() {
    let bus = private_bus!();
    let nm = furnished();
    let _served = fake::serve(bus.address(), &nm)
        .await
        .expect("the fake starts");

    let network = panel(&bus);
    settle(&network, "the list", |state| state.wifi.list.len() == 3).await;

    network
        .handle()
        .set_wifi_enabled(false)
        .await
        .expect("the radio goes off");
    assert!(nm.calls().contains(&"SetWirelessEnabled".to_string()));

    // A property write does not emit a change on its own, so the fake is
    // nudged the way an outside `nmcli radio wifi off` would nudge the panel.
    drive(&bus, "SetWirelessEnabled", &(false,)).await;

    let state = settle(&network, "the radio to read as off", |state| {
        !state.wifi.enabled
    })
    .await;
    assert!(
        state.wifi.list.is_empty(),
        "a list of networks nobody can join is a list nobody wants"
    );
    assert!(state.wifi.present, "the card is still in the machine");
    assert!(state.pending.is_none(), "the spinner stops when it settles");
}

#[tokio::test]
async fn an_access_point_coming_and_going_moves_the_list() {
    let bus = private_bus!();
    let nm = Nm::new();
    nm.seed_ap("1", Ap::open("Cafe", 40));
    let _served = fake::serve(bus.address(), &nm)
        .await
        .expect("the fake starts");

    let network = panel(&bus);
    settle(&network, "the first list", |state| {
        state.wifi.list.len() == 1
    })
    .await;

    drive(&bus, "AddAp", &("2", "Library", 90_u8, true)).await;
    let state = settle(&network, "the new access point", |state| {
        state.wifi.list.len() == 2
    })
    .await;
    assert_eq!(state.wifi.list[0].ssid, "Library", "the strongest leads");
    assert!(state.wifi.list[0].secured);

    drive(&bus, "SetStrength", &("2", 10_u8)).await;
    let state = settle(&network, "the signal to drop", |state| {
        state.wifi.list.first().is_some_and(|ap| ap.ssid == "Cafe")
    })
    .await;
    assert_eq!(state.wifi.list[1].bucket, 0, "10 is no bars at all");

    drive(&bus, "RemoveAp", &("2",)).await;
    settle(&network, "the access point to go", |state| {
        state.wifi.list.len() == 1
    })
    .await;
}

#[tokio::test]
async fn a_cable_going_in_shows_up_as_a_wired_connection() {
    let bus = private_bus!();
    let nm = Nm::new();
    nm.set_has_wifi(false);
    let _served = fake::serve(bus.address(), &nm)
        .await
        .expect("the fake starts");

    let network = panel(&bus);
    let state = settle(&network, "the first read", |state| state.available).await;
    assert!(state.wired.present);
    assert!(!state.wired.connected);
    assert!(!state.wifi.present, "there is no card in this machine");

    drive(&bus, "SetCarrier", &(true, 1000_u32)).await;

    let state = settle(&network, "the cable", |state| state.wired.connected).await;
    assert!(state.wired.carrier);
    assert_eq!(state.wired.speed_mbps, 1000);
    assert_eq!(state.wired.speed_label().as_deref(), Some("1 Gb/s"));
}

#[tokio::test]
async fn vpn_profiles_are_listed_switched_and_followed() {
    let bus = private_bus!();
    let nm = Nm::new();
    nm.set_has_wifi(false);
    nm.seed_profile(Profile::vpn("Work", "uuid-work", "wireguard", None));
    nm.seed_profile(Profile::vpn(
        "Home",
        "uuid-home",
        "vpn",
        Some("org.freedesktop.NetworkManager.openvpn"),
    ));
    let _served = fake::serve(bus.address(), &nm)
        .await
        .expect("the fake starts");

    let network = panel(&bus);
    let state = settle(&network, "the profiles", |state| state.vpn.len() == 2).await;
    // Neither is up, so they read in name order.
    assert_eq!(state.vpn[0].id, "Home");
    assert_eq!(state.vpn[0].kind.label(), "OpenVPN");
    assert_eq!(state.vpn[1].kind.label(), "WireGuard");
    assert!(!state.vpn_active());

    network
        .handle()
        .set_vpn("uuid-work".to_string(), true)
        .await
        .expect("the tunnel comes up");

    let state = settle(&network, "the tunnel to be up", |state| state.vpn_active()).await;
    assert_eq!(state.vpn[0].id, "Work", "what is up leads the list");
    assert!(state.vpn[0].active);
    assert!(state.pending.is_none(), "the spinner stops when it settles");

    network
        .handle()
        .set_vpn("uuid-work".to_string(), false)
        .await
        .expect("the tunnel goes down");
    settle(&network, "the tunnel to be down", |state| {
        !state.vpn_active()
    })
    .await;

    let calls = nm.calls();
    assert!(calls.contains(&"ActivateConnection".to_string()));
    assert!(calls.contains(&"DeactivateConnection".to_string()));
}

#[tokio::test]
async fn a_vpn_added_from_outside_appears_without_a_restart() {
    let bus = private_bus!();
    let nm = Nm::new();
    nm.set_has_wifi(false);
    let _served = fake::serve(bus.address(), &nm)
        .await
        .expect("the fake starts");

    let network = panel(&bus);
    settle(&network, "the first read", |state| state.available).await;

    drive(
        &bus,
        "AddVpnProfile",
        &("Office", "uuid-office", "wireguard", ""),
    )
    .await;
    let state = settle(&network, "the new profile", |state| state.vpn.len() == 1).await;
    assert_eq!(state.vpn[0].id, "Office");

    drive(&bus, "SetVpnActive", &("uuid-office", true)).await;
    settle(&network, "it to come up on its own", |state| {
        state.vpn_active()
    })
    .await;
}

#[tokio::test]
async fn the_machines_overall_state_is_what_the_weather_service_reads() {
    let bus = private_bus!();
    let nm = Nm::new();
    let _served = fake::serve(bus.address(), &nm)
        .await
        .expect("the fake starts");

    let network = panel(&bus);
    // The fake starts at 70, CONNECTED_GLOBAL.
    settle(&network, "the first read", |state| {
        state.available && state.online
    })
    .await;

    drive(&bus, "SetState", &(20_u32,)).await;
    settle(&network, "the machine to read as offline", |state| {
        !state.online
    })
    .await;

    drive(&bus, "SetState", &(60_u32,)).await;
    settle(&network, "CONNECTED_SITE to read as usable", |state| {
        state.online
    })
    .await;
}

#[tokio::test]
async fn the_policy_is_decided_by_the_address_and_the_build() {
    // No address at all, and this is a debug build: the service decides it is
    // looking at somebody's live network and keeps its hands off.
    assert_eq!(Access::decide(None, false), Access::ReadOnly);
    assert!(!Access::ReadOnly.writable());
    assert_eq!(Access::decide(Some("unix:path=/x"), false), Access::Full);
    assert_eq!(Access::decide(None, true), Access::Full);
}

#[tokio::test]
async fn a_read_only_panel_lists_everything_and_touches_nothing() {
    let bus = private_bus!();
    let nm = furnished();
    let _served = fake::serve(bus.address(), &nm)
        .await
        .expect("the fake starts");

    // The policy is forced rather than inferred, because the only way to reach
    // it honestly would be to point the test at the machine's real
    // NetworkManager — which is precisely what it must never do.
    let network = Network::with_access(
        Some(bus.address().to_string()),
        Access::ReadOnly,
        PersistedNetwork::default(),
        None,
    );

    let state = settle(&network, "the first full read", |state| {
        state.available && state.wifi.list.len() == 3
    })
    .await;
    assert_eq!(state.access, Access::ReadOnly);
    assert_eq!(state.wifi.list[0].ssid, "Home", "reading is allowed");

    // Nothing may go out that would change the machine, and above all no agent
    // may be registered: a second one would sit in the queue for the prompts
    // the session's own panel is waiting for.
    network
        .handle()
        .scan()
        .await
        .expect("a refused scan is not an error");
    let error = network
        .handle()
        .connect("Cafe".into())
        .await
        .expect_err("joining is refused");
    assert_eq!(error.user_message(), "Could not change the network");
    network
        .handle()
        .set_wifi_enabled(false)
        .await
        .expect_err("the radio is not ours to switch");
    network
        .handle()
        .set_vpn("uuid-work".into(), true)
        .await
        .expect_err("nor is the tunnel");

    tokio::time::sleep(Duration::from_millis(300)).await;
    let calls = nm.calls();
    assert!(
        nm.agents().is_empty(),
        "a read-only panel registers no secret agent: {:?}",
        nm.agents()
    );
    for forbidden in [
        "Register",
        "RegisterWithCapabilities",
        "RequestScan",
        "ActivateConnection",
        "AddAndActivateConnection",
        "DeactivateConnection",
        "SetWirelessEnabled",
        "Delete",
    ] {
        assert!(
            !calls.contains(&forbidden.to_string()),
            "{forbidden} reached the bus: {calls:?}"
        );
    }
}
