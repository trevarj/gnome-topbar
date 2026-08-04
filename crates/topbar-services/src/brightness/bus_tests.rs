//! The brightness service against a logind of the test's own.
//!
//! What is being checked here is the *throttle*, from the outside: a burst of
//! values must not become a burst of D-Bus calls, and whatever the burst ended
//! on must be the value logind was last given. The pure state machine in
//! `throttle.rs` proves the coalescing rule; this proves the service is wired
//! to it, over a real socket, with a real round trip in the way.
//!
//! The sysfs tree is the test's too. The developer's screen is not a fixture.

use std::path::PathBuf;
use std::time::Duration;

use super::Brightness;
use crate::change::ChangeSource;
use crate::logind::bus_tests::{Log, journal, serve_logind, wait_for};
use crate::private_bus::private_bus;

/// How long the fake takes to answer one `SetBrightness`.
///
/// Longer than a burst of commands takes to arrive, which is what makes the
/// coalescing deterministic rather than a race: everything sent while the first
/// call is outstanding collapses into the one that follows it.
const CALL_DELAY: Duration = Duration::from_millis(200);

/// Build a sysfs tree with one backlight in it.
fn sysfs(label: &str, max: u32) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "topbar-brightness-bus-{}-{label}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let device = root.join("intel_backlight");
    std::fs::create_dir_all(&device).expect("a writable temp dir");
    std::fs::write(device.join("brightness"), (max / 2).to_string()).expect("write");
    std::fs::write(device.join("max_brightness"), max.to_string()).expect("write");
    root
}

#[tokio::test]
async fn a_burst_of_values_becomes_two_calls_ending_on_the_last_one() {
    let bus = private_bus!();
    let log = Log::default();
    let root = sysfs("burst", 100);
    let _logind = serve_logind(&bus, &log, CALL_DELAY, Some(root.clone())).await;

    let brightness = Brightness::start_at(Some(bus.address().to_string()), Some(root.clone()));
    wait_for("the backlight to be found", || {
        brightness.current().available
    })
    .await;

    // A drag: twenty values, none of them awaited on logind.
    for percent in 1..=20 {
        brightness
            .handle()
            .set(percent, ChangeSource::Ui)
            .await
            .expect("the service takes the value");
    }

    wait_for("both calls to land", || journal(&log).brightness.len() >= 2).await;
    // Long enough for a third to have arrived if the throttle were not there.
    tokio::time::sleep(CALL_DELAY * 2).await;

    let calls = journal(&log).brightness.clone();
    assert_eq!(
        calls.len(),
        2,
        "twenty values should cost two calls, not twenty: {calls:?}"
    );
    assert_eq!(
        calls[0],
        ("backlight".to_string(), "intel_backlight".to_string(), 1)
    );
    assert_eq!(
        calls[1],
        ("backlight".to_string(), "intel_backlight".to_string(), 20),
        "the value the user let go on is the one that has to land"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn a_single_change_goes_out_immediately_and_is_attributed_to_its_source() {
    let bus = private_bus!();
    let log = Log::default();
    let root = sysfs("single", 200);
    let _logind = serve_logind(&bus, &log, Duration::ZERO, Some(root.clone())).await;

    let brightness = Brightness::start_at(Some(bus.address().to_string()), Some(root.clone()));
    wait_for("the backlight to be found", || {
        brightness.current().available
    })
    .await;
    assert_eq!(
        brightness.current().device.as_deref(),
        Some("intel_backlight")
    );
    assert_eq!(
        brightness.current().change,
        None,
        "reading the backlight at start-up is not a change anybody made"
    );

    brightness
        .handle()
        .set(30, ChangeSource::Cli)
        .await
        .expect("the service takes the value");

    wait_for("the call to land", || !journal(&log).brightness.is_empty()).await;
    // 30% of a 200-step device is 60 raw steps.
    assert_eq!(journal(&log).brightness[0].2, 60);

    let state = brightness.current();
    assert_eq!(state.percent, 30);
    let change = state.change.expect("a change the panel made");
    assert_eq!(change.source, ChangeSource::Cli);

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn a_machine_with_no_backlight_answers_rather_than_hanging() {
    let bus = private_bus!();
    let log = Log::default();
    let root = std::env::temp_dir().join(format!("topbar-brightness-none-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("a writable temp dir");
    let _logind = serve_logind(&bus, &log, Duration::ZERO, Some(root.clone())).await;

    let brightness = Brightness::start_at(Some(bus.address().to_string()), Some(root.clone()));
    let error = brightness
        .handle()
        .set(30, ChangeSource::Cli)
        .await
        .expect_err("there is no backlight to set");
    assert!(matches!(error, crate::error::SvcError::NoBacklight));
    assert!(!brightness.current().available);
    assert!(journal(&log).brightness.is_empty());

    let _ = std::fs::remove_dir_all(&root);
}
