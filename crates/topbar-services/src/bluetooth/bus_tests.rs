//! Bluetooth against a BlueZ of the test's own.
//!
//! Everything here needs a `dbus-daemon`, which a Nix build sandbox has no
//! machine id for, so these run in the dev shell and on a real desktop and sit
//! out `nix flake check`. What is only covered here is the wire: whether the
//! object tree is read the way BlueZ publishes it, whether a pairing the panel
//! did not start reaches a row and comes back with an answer, and whether a
//! build that is not allowed to touch the machine keeps its hands off.
//!
//! Not one of these tests can reach the machine's real adapter: the fake owns
//! `org.bluez` on a bus that exists for the length of the test, and the panel
//! is pointed at it by address.

use std::sync::Arc;
use std::time::Duration;

use super::Bluetooth;
use super::fake::{self, Bluez, FakeDevice, Outcome};
use super::model::BtState;
use crate::network::Access;
use crate::private_bus::{PrivateBus, private_bus};

/// How long a test waits for the panel to catch up before failing.
const PATIENCE: Duration = Duration::from_secs(10);

/// Start the panel's Bluetooth service against `bus`.
fn panel(bus: &PrivateBus) -> Bluetooth {
    Bluetooth::start(Some(bus.address().to_string()))
}

/// Wait until the snapshot satisfies `wanted`, or fail saying what it was.
async fn settle(
    bluetooth: &Bluetooth,
    what: &str,
    wanted: impl Fn(&BtState) -> bool,
) -> Arc<BtState> {
    let mut state = bluetooth.state();
    let wait = async {
        loop {
            {
                let current = state.borrow_and_update();
                if wanted(&current) {
                    return current.clone();
                }
            }
            state.changed().await.expect("the service is alive");
        }
    };
    match tokio::time::timeout(PATIENCE, wait).await {
        Ok(state) => state,
        Err(_) => panic!(
            "timed out waiting for {what}; last state {:?}",
            bluetooth.current()
        ),
    }
}

/// Wait until `check` holds of the fake, or fail.
async fn until(bluez: &Arc<Bluez>, what: &str, check: impl Fn(&Arc<Bluez>) -> bool) {
    let wait = async {
        while !check(bluez) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    tokio::time::timeout(PATIENCE, wait)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}; calls {:?}", bluez.calls()));
}

/// A proxy onto the fake's control interface.
async fn control(served: &fake::Served) -> zbus::Proxy<'_> {
    zbus::Proxy::new(
        &served.connection,
        fake::BLUEZ_NAME,
        fake::CONTROL_PATH,
        "io.github.trevarj.topbar.FakeBluez1",
    )
    .await
    .expect("the control interface is there")
}

/// A fake with a headset, a mouse and a phone nobody paired.
fn furnished() -> Arc<Bluez> {
    let bluez = Bluez::new();
    bluez.seed_device(
        "buds",
        FakeDevice::paired("WH-1000XM4", "AA:BB:CC:DD:EE:FF", "audio-headset")
            .connected()
            .with_battery(85),
    );
    bluez.seed_device(
        "mouse",
        FakeDevice::paired("MX Master", "11:22:33:44:55:66", "input-mouse"),
    );
    bluez.seed_device(
        "stranger",
        FakeDevice::paired("Somebody's Pixel", "99:88:77:66:55:44", "phone").unpaired(),
    );
    bluez
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_object_tree_becomes_a_list_of_paired_devices() {
    let bus = private_bus!();
    let bluez = furnished();
    let _served = fake::serve(bus.address(), &bluez)
        .await
        .expect("BlueZ serves");

    let bluetooth = panel(&bus);
    let state = settle(&bluetooth, "the device list", |state| {
        state.available && !state.devices.is_empty()
    })
    .await;

    assert!(state.powered);
    let names: Vec<&str> = state.devices.iter().map(|d| d.alias.as_str()).collect();
    assert_eq!(
        names,
        ["WH-1000XM4", "MX Master"],
        "connected first, then by name, and the unpaired phone is not a row"
    );
    assert_eq!(state.devices[0].battery_pct, Some(85));
    assert_eq!(state.devices[0].icon, super::IconKind::Headset);
    assert_eq!(state.devices[1].icon, super::IconKind::Mouse);
    assert_eq!(state.connected_count(), 1);
    assert!(state.indicated());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn switching_the_radio_writes_the_property_and_waits_for_it() {
    let bus = private_bus!();
    let bluez = furnished();
    let _served = fake::serve(bus.address(), &bluez)
        .await
        .expect("BlueZ serves");

    let bluetooth = panel(&bus);
    settle(&bluetooth, "the adapter", |state| state.available).await;

    bluetooth
        .handle()
        .set_powered(false)
        .await
        .expect("the radio switches");

    assert!(!bluez.powered(), "BlueZ was actually written to");
    assert!(
        bluez.called("Adapter1.Powered=false"),
        "calls {:?}",
        bluez.calls()
    );
    let state = settle(&bluetooth, "the radio going off", |state| !state.powered).await;
    assert!(
        !state.indicated(),
        "a dead radio draws nothing, whatever the stale flags say"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_connect_reaches_bluez_and_the_row_stops_spinning() {
    let bus = private_bus!();
    let bluez = Bluez::new();
    bluez.seed_device(
        "mouse",
        FakeDevice::paired("MX Master", "11:22:33:44:55:66", "input-mouse"),
    );
    let _served = fake::serve(bus.address(), &bluez)
        .await
        .expect("BlueZ serves");

    let bluetooth = panel(&bus);
    let state = settle(&bluetooth, "the mouse", |state| !state.devices.is_empty()).await;
    let path = state.devices[0].path.clone();

    bluetooth
        .handle()
        .connect(path.clone())
        .await
        .expect("the mouse connects");

    let state = settle(&bluetooth, "the mouse connecting", |state| {
        state.devices.iter().any(|device| device.connected)
    })
    .await;
    assert!(!state.devices[0].pending, "the spinner stops when it lands");
    assert!(bluez.called(&format!("Connect {path}")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_connect_that_fails_says_so_and_the_switch_reverts() {
    let bus = private_bus!();
    let bluez = Bluez::new();
    bluez.seed_device(
        "buds",
        FakeDevice::paired("WH-1000XM4", "AA:BB:CC:DD:EE:FF", "audio-headset"),
    );
    bluez.queue(Outcome::Fail);
    let _served = fake::serve(bus.address(), &bluez)
        .await
        .expect("BlueZ serves");

    let bluetooth = panel(&bus);
    let state = settle(&bluetooth, "the headset", |state| !state.devices.is_empty()).await;
    let path = state.devices[0].path.clone();

    let error = bluetooth
        .handle()
        .connect(path)
        .await
        .expect_err("a device in a drawer does not answer");
    assert!(
        error.to_string().contains("in range"),
        "unhelpful message: {error}"
    );

    let state = bluetooth.current();
    assert!(!state.devices[0].connected, "the switch reverts");
    assert!(!state.devices[0].pending, "and the spinner stops");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_battery_that_arrives_after_the_fact_reaches_the_row() {
    let bus = private_bus!();
    let bluez = Bluez::new();
    bluez.seed_device(
        "buds",
        FakeDevice::paired("WH-1000XM4", "AA:BB:CC:DD:EE:FF", "audio-headset").connected(),
    );
    let served = fake::serve(bus.address(), &bluez)
        .await
        .expect("BlueZ serves");

    let bluetooth = panel(&bus);
    let state = settle(&bluetooth, "the headset", |state| !state.devices.is_empty()).await;
    assert_eq!(state.devices[0].battery_pct, None, "it has not said yet");

    // A headset publishes `Battery1` a second or two after it connects, as an
    // interface *arriving* rather than a property changing — which is why the
    // task re-reads the tree instead of watching a fixed set of properties.
    control(&served)
        .await
        .call_method("SetBattery", &("buds", 72_u8))
        .await
        .expect("the battery arrives");

    let state = settle(&bluetooth, "the battery", |state| {
        state
            .devices
            .first()
            .and_then(|device| device.battery_pct)
            .is_some()
    })
    .await;
    assert_eq!(state.devices[0].battery_pct, Some(72));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_device_paired_from_outside_appears_without_a_restart() {
    let bus = private_bus!();
    let bluez = Bluez::new();
    let served = fake::serve(bus.address(), &bluez)
        .await
        .expect("BlueZ serves");

    let bluetooth = panel(&bus);
    settle(&bluetooth, "the adapter", |state| state.available).await;
    assert!(bluetooth.current().devices.is_empty());

    control(&served)
        .await
        .call_method(
            "AddDevice",
            &("kb", "Magic Keyboard", "AA:11", "input-keyboard"),
        )
        .await
        .expect("something paired in Settings");

    let state = settle(&bluetooth, "the keyboard", |state| {
        !state.devices.is_empty()
    })
    .await;
    assert_eq!(state.devices[0].alias, "Magic Keyboard");
    assert_eq!(state.devices[0].icon, super::IconKind::Keyboard);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pairing_the_panel_did_not_start_is_confirmed_in_the_panel() {
    let bus = private_bus!();
    let bluez = Bluez::new();
    bluez.seed_device("pixel", FakeDevice::paired("Pixel 8", "AA:11", "phone"));
    let served = fake::serve(bus.address(), &bluez)
        .await
        .expect("BlueZ serves");

    let bluetooth = panel(&bus);
    settle(&bluetooth, "the device list", |state| {
        !state.devices.is_empty()
    })
    .await;

    // The agent has to be on the bus before anything can call it.
    until(&bluez, "the agent to register", |bluez| {
        !bluez.agents().is_empty()
    })
    .await;
    assert_eq!(
        bluez.agents(),
        vec!["/io/github/trevarj/topbar/BluetoothAgent DisplayYesNo".to_string()],
        "the capability is the promise the panel keeps"
    );
    assert!(
        bluez.called("RequestDefaultAgent"),
        "an incoming pairing reaches nobody without this: {:?}",
        bluez.calls()
    );

    // A phone asks to pair. BlueZ calls the agent, and the agent's reply stays
    // outstanding until the row is answered.
    control(&served)
        .await
        .call_method("TriggerConfirmation", &("pixel", 42_u32))
        .await
        .expect("the phone asks");

    let state = settle(&bluetooth, "the pairing row", |state| {
        state.prompt.is_some()
    })
    .await;
    let prompt = state.prompt.as_ref().expect("a prompt");
    assert_eq!(prompt.alias, "Pixel 8");
    assert_eq!(
        prompt.code.as_deref(),
        Some("000042"),
        "the other screen is showing the leading zeros too"
    );
    assert!(prompt.answerable());

    bluetooth
        .handle()
        .confirm_pairing()
        .await
        .expect("the user confirms");

    until(&bluez, "the agent's answer", |bluez| {
        !bluez.replies().is_empty()
    })
    .await;
    assert_eq!(
        bluez.replies(),
        vec!["RequestConfirmation confirmed".to_string()]
    );
    assert!(
        bluetooth.current().prompt.is_none(),
        "the row goes when it is answered"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_the_row_refuses_the_pairing() {
    let bus = private_bus!();
    let bluez = Bluez::new();
    bluez.seed_device("pixel", FakeDevice::paired("Pixel 8", "AA:11", "phone"));
    let served = fake::serve(bus.address(), &bluez)
        .await
        .expect("BlueZ serves");

    let bluetooth = panel(&bus);
    until(&bluez, "the agent to register", |bluez| {
        !bluez.agents().is_empty()
    })
    .await;
    settle(&bluetooth, "the device list", |state| {
        !state.devices.is_empty()
    })
    .await;

    control(&served)
        .await
        .call_method("TriggerConfirmation", &("pixel", 999_999_u32))
        .await
        .expect("the phone asks");
    settle(&bluetooth, "the pairing row", |state| {
        state.prompt.is_some()
    })
    .await;

    bluetooth
        .handle()
        .cancel_pairing()
        .await
        .expect("the user says no");

    until(&bluez, "the agent's answer", |bluez| {
        !bluez.replies().is_empty()
    })
    .await;
    let replies = bluez.replies();
    assert!(
        replies[0].contains("refused"),
        "BlueZ has to be told no: {replies:?}"
    );
    assert!(bluetooth.current().prompt.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_only_panel_registers_no_agent_and_writes_nothing() {
    let bus = private_bus!();
    let bluez = furnished();
    let _served = fake::serve(bus.address(), &bluez)
        .await
        .expect("BlueZ serves");

    // Forced, because the only other way to produce this policy is to point a
    // test at the developer's own adapter and watch what it does not do.
    let bluetooth = Bluetooth::with_access(Some(bus.address().to_string()), Access::ReadOnly);
    let state = settle(&bluetooth, "the device list", |state| {
        !state.devices.is_empty()
    })
    .await;
    assert_eq!(state.access, Access::ReadOnly);
    assert!(state.powered, "reading is still allowed, and still happens");

    let error = bluetooth
        .handle()
        .set_powered(false)
        .await
        .expect_err("the radio is not this build's to switch");
    assert!(error.to_string().contains("only reads"), "{error}");

    let path = state.devices[0].path.clone();
    bluetooth
        .handle()
        .connect(path)
        .await
        .expect_err("nor are the headphones this build's to move");

    // The assertion that matters: nothing was even attempted.
    let calls = bluez.calls();
    assert!(
        !calls
            .iter()
            .any(|call| call.starts_with("Adapter1.Powered")),
        "a read-only panel wrote to the adapter: {calls:?}"
    );
    assert!(
        !calls.iter().any(|call| call.starts_with("Connect")),
        "a read-only panel called Connect: {calls:?}"
    );
    assert!(
        bluez.agents().is_empty(),
        "a read-only panel registered a pairing agent: {:?}",
        bluez.agents()
    );
    assert!(
        !calls.iter().any(|call| call.contains("Agent")),
        "a read-only panel went near the agent manager: {calls:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_machine_with_no_adapter_says_so_rather_than_pretending() {
    let bus = private_bus!();
    let bluez = Bluez::new();
    bluez.set_has_adapter(false);
    let _served = fake::serve(bus.address(), &bluez)
        .await
        .expect("BlueZ serves");

    let bluetooth = panel(&bus);
    // There is nothing to wait *for*, so the first read gets a beat and then
    // the absence is checked.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let state = bluetooth.current();
    assert!(!state.available, "a desktop with no dongle has no toggle");
    assert!(!state.powered);
    assert!(state.devices.is_empty());
}
