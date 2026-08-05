//! The power-profiles client against a daemon of the test's own.
//!
//! Nothing here touches the system bus. Changing the developer's CPU governor
//! during `cargo test` would be as rude as changing their volume.

use std::sync::Arc;

use super::fake::{self, Names, Shared};
use super::{PowerProfiles, ProfileView};
use crate::logind::bus_tests::wait_for;
use crate::private_bus::{PrivateBus, private_bus};

/// Start the panel's client against `bus`.
fn client(bus: &PrivateBus) -> PowerProfiles {
    PowerProfiles::start(Some(bus.address().to_string()), true)
}

/// Wait for the client to report a daemon.
async fn wait_available(profiles: &PowerProfiles) {
    wait_for("the daemon to answer", || profiles.current().available).await;
}

/// Wait for the client's idea of the active profile to be `wanted`.
async fn wait_active(profiles: &PowerProfiles, wanted: &str) {
    wait_for(wanted, || profiles.current().active_id() == Some(wanted)).await;
}

#[tokio::test]
async fn the_current_bus_name_is_found_and_its_profiles_are_published() {
    let bus = private_bus!();
    let state = Shared::new("balanced", &["power-saver", "balanced", "performance"]);
    let _daemon = fake::serve(bus.address(), Names::Modern, &state)
        .await
        .expect("the fake daemon starts");

    let profiles = client(&bus);
    wait_available(&profiles).await;

    let current = profiles.current();
    assert_eq!(current.active_id(), Some("balanced"));
    assert_eq!(
        current
            .profiles
            .iter()
            .map(|profile| profile.id.as_str())
            .collect::<Vec<_>>(),
        ["power-saver", "balanced", "performance"],
        "exactly the profiles the daemon reports, in its own order"
    );
    assert_eq!(
        current.active,
        Some(ProfileView::new("balanced")),
        "the active profile arrives ready to draw"
    );
}

#[tokio::test]
async fn the_legacy_bus_name_works_just_as_well() {
    let bus = private_bus!();
    let state = Shared::new("performance", &["balanced", "performance"]);
    let _daemon = fake::serve(bus.address(), Names::Legacy, &state)
        .await
        .expect("the fake daemon starts");

    let profiles = client(&bus);
    wait_available(&profiles).await;
    assert_eq!(profiles.current().active_id(), Some("performance"));

    // ...and it is settable through the old name too.
    profiles
        .handle()
        .set_profile("balanced".into())
        .await
        .expect("the daemon accepts it");
    wait_for("the write to land", || state.writes() == ["balanced"]).await;
}

#[tokio::test]
async fn a_machine_with_no_daemon_reports_no_power_profiles() {
    let bus = private_bus!();
    let profiles = client(&bus);

    // Long enough for a daemon that was going to answer to have answered.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        !profiles.current().available,
        "the Power Mode toggle must not be drawn without a daemon"
    );

    let error = profiles
        .handle()
        .set_profile("balanced".into())
        .await
        .expect_err("there is nothing to set");
    assert_eq!(error.user_message(), "Could not change the power mode");
}

#[tokio::test]
async fn a_daemon_that_starts_late_is_still_found() {
    let bus = private_bus!();
    let profiles = client(&bus);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(!profiles.current().available);

    let state = Shared::new("power-saver", &["power-saver", "balanced"]);
    let _daemon = fake::serve(bus.address(), Names::Both, &state)
        .await
        .expect("the fake daemon starts");

    wait_available(&profiles).await;
    assert_eq!(profiles.current().active_id(), Some("power-saver"));
}

#[tokio::test]
async fn setting_a_profile_reaches_the_daemon_and_comes_back() {
    let bus = private_bus!();
    let state = Shared::new("balanced", &["power-saver", "balanced", "performance"]);
    let _daemon = fake::serve(bus.address(), Names::Both, &state)
        .await
        .expect("the fake daemon starts");

    let profiles = client(&bus);
    wait_available(&profiles).await;

    profiles
        .handle()
        .set_profile("performance".into())
        .await
        .expect("the daemon accepts it");

    assert_eq!(state.writes(), ["performance"], "the daemon was asked once");
    wait_active(&profiles, "performance").await;
}

#[tokio::test]
async fn a_profile_changed_elsewhere_moves_the_panel_with_it() {
    let bus = private_bus!();
    let state = Shared::new("balanced", &["balanced", "performance"]);
    let daemon = fake::serve(bus.address(), Names::Modern, &state)
        .await
        .expect("the fake daemon starts");

    let profiles = client(&bus);
    wait_available(&profiles).await;

    // Someone else — `powerprofilesctl`, a laptop's own key — moves it.
    set_from_outside(&daemon[0], "performance").await;
    wait_active(&profiles, "performance").await;
}

#[tokio::test]
async fn a_refused_write_reverts_the_optimistic_flip() {
    let bus = private_bus!();
    let state = Shared::refusing("balanced", &["balanced", "performance"]);
    let _daemon = fake::serve(bus.address(), Names::Modern, &state)
        .await
        .expect("the fake daemon starts");

    let profiles = client(&bus);
    wait_available(&profiles).await;

    let error = profiles
        .handle()
        .set_profile("performance".into())
        .await
        .expect_err("this daemon refuses everything");
    assert_eq!(error.user_message(), "Could not change the power mode");
    assert_eq!(
        profiles.current().active_id(),
        Some("balanced"),
        "an optimistic flip that failed has to be visibly undone"
    );
}

#[tokio::test]
async fn a_daemon_that_stops_takes_the_toggle_with_it() {
    let bus = private_bus!();
    let state = Shared::new("balanced", &["balanced", "performance"]);
    let daemon = fake::serve(bus.address(), Names::Modern, &state)
        .await
        .expect("the fake daemon starts");

    let profiles = client(&bus);
    wait_available(&profiles).await;

    drop(daemon);
    wait_for("the daemon to go away", || !profiles.current().available).await;
}

/// Write `ActiveProfile` the way `powerprofilesctl` would.
async fn set_from_outside(connection: &zbus::Connection, profile: &str) {
    let endpoint = super::ENDPOINTS[0];
    let properties = zbus::fdo::PropertiesProxy::builder(connection)
        .destination(endpoint.name)
        .expect("a well-formed name")
        .path(endpoint.path)
        .expect("a well-formed path")
        .build()
        .await
        .expect("the daemon is there");
    properties
        .set(
            endpoint.name.try_into().expect("a well-formed interface"),
            "ActiveProfile",
            zbus::zvariant::Value::from(profile),
        )
        .await
        .expect("the daemon takes the write");
}

/// A shared state is `Send + Sync`, which is what lets a test read it while the
/// daemon is serving from another task.
#[allow(dead_code)]
fn assert_shared_is_shareable(state: Arc<Shared>) -> impl Send + Sync {
    state
}
