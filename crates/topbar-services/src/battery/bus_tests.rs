//! The battery client against a UPower of the test's own.
//!
//! Nothing here touches the system bus, and nothing writes outside a temporary
//! directory: setting the developer's laptop to stop charging at 80% during
//! `cargo test` would be a rude surprise on a train.

use super::fake::{self, Recipe};
use super::model::BatteryStatus;
use super::sysfs::tests::{TempRoot, battery_with_thresholds};
use super::{Battery, sysfs};
use crate::logind::bus_tests::wait_for;
use crate::private_bus::{PrivateBus, private_bus};

/// Start the panel's client against `bus`, reading `root` for thresholds.
fn client(bus: &PrivateBus, root: &TempRoot) -> Battery {
    Battery::start(
        Some(bus.address().to_string()),
        Some(root.path().to_path_buf()),
        true,
    )
}

#[tokio::test]
async fn a_reading_arrives_and_a_change_to_it_propagates() {
    let bus = private_bus!();
    let root = TempRoot::new("bus-reading");
    root.supply("BAT0", &[("type", "Battery\n")]);
    let upower = fake::serve(
        bus.address(),
        Recipe {
            percent: 62.0,
            state: 2,
            time_to_empty: 8100,
            ..Recipe::default()
        },
    )
    .await
    .expect("the fake UPower starts");

    let battery = client(&bus, &root);
    wait_for("the first reading", || {
        battery.current().percent == Some(62.0)
    })
    .await;

    let current = battery.current();
    assert!(current.available);
    assert_eq!(current.status, BatteryStatus::Discharging);
    assert_eq!(current.time_to_empty, Some(8100));
    assert_eq!(current.icon(), "battery-level-60-symbolic");

    // The machine is plugged in and starts charging.
    fake::emit_percentage(upower.connection(), 15.0)
        .await
        .expect("the fake announces it");
    wait_for("the new charge", || battery.current().percent == Some(15.0)).await;
    assert!(battery.current().is_low(), "fifteen per cent on battery");

    fake::emit_state(upower.connection(), 1)
        .await
        .expect("the fake announces it");
    wait_for("charging", || {
        battery.current().status == BatteryStatus::Charging
    })
    .await;
    assert!(
        !battery.current().is_low(),
        "a battery on mains is not in trouble"
    );
    assert_eq!(
        battery.current().icon(),
        "battery-level-10-charging-symbolic"
    );
}

#[tokio::test]
async fn a_desktop_reports_no_battery() {
    let bus = private_bus!();
    let root = TempRoot::new("bus-desktop");
    root.supply("AC", &[("type", "Mains\n")]);
    let _upower = fake::serve(
        bus.address(),
        Recipe {
            present: false,
            ..Recipe::default()
        },
    )
    .await
    .expect("the fake UPower starts");

    let battery = client(&bus, &root);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        !battery.current().available,
        "the battery pill must not be drawn on a desktop"
    );
}

#[tokio::test]
async fn a_writable_limit_goes_through_sysfs_and_never_touches_upower() {
    let bus = private_bus!();
    let root = TempRoot::new("bus-sysfs-write");
    battery_with_thresholds(&root, 96, 100);
    let upower = fake::serve(
        bus.address(),
        Recipe {
            threshold_supported: true,
            start_threshold: 96,
            end_threshold: 100,
            ..Recipe::default()
        },
    )
    .await
    .expect("the fake UPower starts");

    let battery = client(&bus, &root);
    wait_for("the first reading", || battery.current().available).await;
    assert!(battery.current().can_set_thresholds());

    battery
        .handle()
        .set_thresholds(75, 80)
        .await
        .expect("the files take the write");

    assert_eq!(
        sysfs::read_thresholds(root.path()).map(|limits| (limits.start, limits.end)),
        Some((75, 80)),
        "the kernel's own files are what changed"
    );
    assert!(
        upower.calls().is_empty(),
        "UPower is the fallback, not the first choice"
    );
    wait_for("the card to catch up", || {
        battery
            .current()
            .thresholds
            .is_some_and(|limits| (limits.start, limits.end) == (75, 80))
    })
    .await;
    assert!(
        battery
            .current()
            .thresholds
            .is_some_and(super::Thresholds::limited)
    );
}

#[tokio::test]
async fn an_unwritable_limit_falls_back_to_upower_and_is_read_back_from_sysfs() {
    let bus = private_bus!();
    let root = TempRoot::new("bus-upower-write");
    let directory = battery_with_thresholds(&root, 96, 100);
    // Root-owned files, as a stock kernel exposes them.
    for file in [
        "charge_control_start_threshold",
        "charge_control_end_threshold",
    ] {
        let path = directory.join(file);
        let mut permissions = std::fs::metadata(&path).expect("the file").permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&path, permissions).expect("read-only");
    }

    // UPower writes through to the same tree, exactly as the real one writes
    // through to the kernel — but it is allowed to, and the panel is not.
    let writable_root = root.path().to_path_buf();
    let upower = fake::serve(
        bus.address(),
        Recipe {
            threshold_supported: true,
            start_threshold: 96,
            end_threshold: 100,
            sysfs: Some(writable_root),
            ..Recipe::default()
        },
    )
    .await
    .expect("the fake UPower starts");

    let battery = client(&bus, &root);
    wait_for("the first reading", || battery.current().available).await;
    assert!(
        battery
            .current()
            .thresholds
            .is_some_and(|limits| !limits.writable),
        "the files are not ours to write"
    );
    assert!(
        battery.current().upower_thresholds,
        "but UPower says it can do it"
    );
    assert!(battery.current().can_set_thresholds());

    battery
        .handle()
        .set_thresholds(75, 80)
        .await
        .expect("UPower takes it");

    assert_eq!(upower.calls(), vec![true], "UPower was asked to limit");
    assert_eq!(
        sysfs::read_thresholds(root.path()).map(|limits| (limits.start, limits.end)),
        Some((75, 80)),
        "and sysfs is still what the panel reads back"
    );
}

#[tokio::test]
async fn a_limit_nothing_can_write_is_refused_rather_than_pretended() {
    let bus = private_bus!();
    let root = TempRoot::new("bus-no-write");
    let directory = battery_with_thresholds(&root, 96, 100);
    for file in [
        "charge_control_start_threshold",
        "charge_control_end_threshold",
    ] {
        let path = directory.join(file);
        let mut permissions = std::fs::metadata(&path).expect("the file").permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&path, permissions).expect("read-only");
    }

    let _upower = fake::serve(
        bus.address(),
        Recipe {
            threshold_supported: false,
            ..Recipe::default()
        },
    )
    .await
    .expect("the fake UPower starts");

    let battery = client(&bus, &root);
    wait_for("the first reading", || battery.current().available).await;
    assert!(
        !battery.current().can_set_thresholds(),
        "root-owned files and no UPower is a card that explains itself"
    );

    let error = battery
        .handle()
        .set_thresholds(75, 80)
        .await
        .expect_err("nothing will take it");
    assert_eq!(error.user_message(), "Could not change the charge limit");
    assert_eq!(
        sysfs::read_thresholds(root.path()).map(|limits| (limits.start, limits.end)),
        Some((96, 100)),
        "and nothing changed"
    );
}

#[tokio::test]
async fn upower_thresholds_stand_in_only_where_sysfs_says_nothing() {
    let bus = private_bus!();
    let root = TempRoot::new("bus-upower-only");
    // A battery with no threshold files at all.
    root.supply("BAT0", &[("type", "Battery\n"), ("status", "Full\n")]);

    let _upower = fake::serve(
        bus.address(),
        Recipe {
            threshold_supported: true,
            start_threshold: 75,
            end_threshold: 80,
            ..Recipe::default()
        },
    )
    .await
    .expect("the fake UPower starts");

    let battery = client(&bus, &root);
    wait_for("UPower's own numbers", || {
        battery.current().thresholds.is_some()
    })
    .await;

    let limits = battery.current().thresholds.expect("thresholds");
    assert_eq!((limits.start, limits.end), (75, 80));
    assert!(
        !limits.writable,
        "whatever happens goes through UPower, not through us"
    );
}
