//! The one owner of the battery reading.
//!
//! It joins two sources that disagree. UPower knows the charge and what the
//! battery is doing; the kernel's sysfs files know what charge limit is
//! actually in force, and they know it *first* — UPower can be seconds behind
//! a write. So sysfs wins on thresholds, always, and UPower fills in the rest.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::{mpsc, oneshot, watch};
use tracing::{debug, info, warn};

use super::model::{BatteryState, BatteryStatus, Thresholds, validate};
use super::proxy::{BUS_NAME, DISPLAY_DEVICE, DeviceProxy, device_path};
use super::sysfs;
use crate::error::SvcError;

/// How often the threshold files are re-read when nothing else happens.
///
/// A limit changed by `tlp` or by the firmware's own setup utility should show
/// up on the card without the user having to restart the panel; twice a minute
/// is enough for something a person changes twice a year.
const SYSFS_POLL: Duration = Duration::from_secs(20);
/// How long after a write the files are read again.
///
/// Some firmware applies a limit a beat after the write returns, and a card
/// that showed the old numbers until the next poll would look like the write
/// had failed.
const SETTLE: Duration = Duration::from_millis(400);

/// What the panel may ask.
#[derive(Debug)]
pub(crate) enum Action {
    /// Put the charge limit at `start`–`end`.
    SetThresholds { start: u8, end: u8 },
    /// Read everything again.
    Refresh,
}

/// A command and where to answer it.
#[derive(Debug)]
pub(crate) struct Command {
    pub(crate) action: Action,
    pub(crate) reply: oneshot::Sender<Result<(), SvcError>>,
}

/// Everything the task needs to answer one question.
struct Sources {
    /// The composite battery, when UPower is there.
    display: Option<DeviceProxy<'static>>,
    /// The real battery, for the charge-threshold interface.
    battery: Option<DeviceProxy<'static>>,
    /// Where the kernel's power supplies are, a temporary tree under test.
    root: PathBuf,
}

/// Follow the battery until every handle is dropped.
pub(crate) async fn run(
    mut commands: mpsc::Receiver<Command>,
    publisher: watch::Sender<Arc<BatteryState>>,
    address: Option<String>,
    root: PathBuf,
) {
    let connection = match crate::logind::connect(address.as_deref()).await {
        Ok(connection) => Some(connection),
        Err(error) => {
            info!("no system bus ({error}); the battery is read from sysfs alone");
            None
        }
    };

    let mut sources = Sources {
        display: None,
        battery: None,
        root: root.clone(),
    };
    let mut changes: Box<dyn futures_util::Stream<Item = ()> + Send + Unpin> =
        Box::new(futures_util::stream::pending());

    if let Some(connection) = &connection {
        sources.display = display_device(connection).await;
        sources.battery = battery_device(connection, &root).await;
        changes = property_changes(connection).await;
    }

    publish(&publisher, read(&sources).await);

    let mut poll = tokio::time::interval(SYSFS_POLL);
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // The first tick fires immediately, and the state has just been published.
    poll.tick().await;

    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { break };
                let answer = match command.action {
                    Action::SetThresholds { start, end } => {
                        apply_thresholds(&sources, start, end).await
                    }
                    Action::Refresh => Ok(()),
                };
                publish(&publisher, read(&sources).await);
                let _ = command.reply.send(answer);
            }
            Some(()) = changes.next() => {
                publish(&publisher, read(&sources).await);
            }
            _ = poll.tick() => {
                publish(&publisher, read(&sources).await);
            }
        }
    }
}

/// Read everything, from both sources, and reconcile them.
async fn read(sources: &Sources) -> BatteryState {
    let limits = read_sysfs(sources.root.clone()).await;
    let sysfs_battery = has_sysfs_battery(sources.root.clone()).await;

    let mut state = BatteryState {
        available: sysfs_battery,
        thresholds: limits,
        ..BatteryState::default()
    };

    if let Some(display) = &sources.display {
        // `IsPresent` on the composite device is false on a desktop, which is
        // exactly the machine whose battery pill must not be drawn.
        let present = display.is_present().await.unwrap_or(false);
        if present {
            state.available = true;
            state.percent = display.percentage().await.ok();
            state.status = display
                .state()
                .await
                .map_or(BatteryStatus::Unknown, BatteryStatus::from_upower);
            state.time_to_empty = display.time_to_empty().await.ok().filter(|left| *left > 0);
            state.time_to_full = display.time_to_full().await.ok().filter(|left| *left > 0);
        } else {
            state.available = false;
        }
    }

    if let Some(battery) = &sources.battery {
        state.upower_thresholds = battery.charge_threshold_supported().await.unwrap_or(false);
        // Only where the kernel files said nothing: they are the source of
        // truth and UPower's copy of them lags behind a write.
        if state.thresholds.is_none() {
            let start = battery
                .charge_start_threshold()
                .await
                .ok()
                .and_then(percent);
            let end = battery.charge_end_threshold().await.ok().and_then(percent);
            if let (Some(start), Some(end)) = (start, end) {
                state.thresholds = Some(Thresholds {
                    start,
                    end,
                    // Not ours to write: whatever happens goes through UPower.
                    writable: false,
                });
            }
        }
    }

    state
}

/// A UPower threshold percentage, or `None` for the zero that means "unset".
fn percent(value: u32) -> Option<u8> {
    (1..=100).contains(&value).then_some(value as u8)
}

/// Put the charge limit where it was asked for.
///
/// The files first, because they are what the firmware reads and because the
/// result is visible immediately; UPower second, for the machines where the
/// files are root-owned and a udev rule has not been written.
async fn apply_thresholds(sources: &Sources, start: u8, end: u8) -> Result<(), SvcError> {
    validate(start, end).map_err(|error| SvcError::Battery(error.to_string()))?;

    let root = sources.root.clone();
    let direct = tokio::task::spawn_blocking(move || sysfs::write_thresholds(&root, start, end))
        .await
        .unwrap_or_else(|error| Err(format!("the write task failed: {error}")));

    match direct {
        Ok(()) => {
            debug!("charge limit set to {start}–{end} through sysfs");
            tokio::time::sleep(SETTLE).await;
            return Ok(());
        }
        Err(error) => debug!("sysfs would not take the charge limit ({error}); trying UPower"),
    }

    let Some(battery) = &sources.battery else {
        return Err(SvcError::Battery(
            "this battery's charge limit is not writable".into(),
        ));
    };
    if !battery.charge_threshold_supported().await.unwrap_or(false) {
        return Err(SvcError::Battery(
            "this battery's charge limit is not writable".into(),
        ));
    }

    // UPower owns the numbers; the panel's two presets are "limited" and
    // "charge to full", which is exactly what the flag means.
    battery
        .enable_charge_threshold(end < 100)
        .await
        .map_err(|error| SvcError::Battery(error.to_string()))?;
    tokio::time::sleep(SETTLE).await;
    Ok(())
}

/// The composite battery, if UPower is running.
async fn display_device(connection: &zbus::Connection) -> Option<DeviceProxy<'static>> {
    match DeviceProxy::builder(connection)
        .path(DISPLAY_DEVICE)
        .ok()?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
    {
        Ok(device) => Some(device),
        Err(error) => {
            info!("no UPower ({error}); the battery is read from sysfs alone");
            None
        }
    }
}

/// The real battery's own object, for the charge-threshold interface.
async fn battery_device(
    connection: &zbus::Connection,
    root: &std::path::Path,
) -> Option<DeviceProxy<'static>> {
    let battery = sysfs::battery_path(root)?;
    let path = device_path(&battery)?;
    DeviceProxy::builder(connection)
        .path(path)
        .ok()?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
        .map_err(|error| warn!("no UPower object for {}: {error}", battery.display()))
        .ok()
}

/// A stream that yields whenever the composite battery changes.
async fn property_changes(
    connection: &zbus::Connection,
) -> Box<dyn futures_util::Stream<Item = ()> + Send + Unpin> {
    let properties = zbus::fdo::PropertiesProxy::builder(connection)
        .destination(BUS_NAME)
        .and_then(|builder| builder.path(DISPLAY_DEVICE));
    let Ok(builder) = properties else {
        return Box::new(futures_util::stream::pending());
    };
    match builder.build().await {
        Ok(properties) => match properties.receive_properties_changed().await {
            Ok(stream) => Box::new(stream.map(|_| ())),
            Err(error) => {
                debug!("cannot watch the battery ({error}); polling instead");
                Box::new(futures_util::stream::pending())
            }
        },
        Err(error) => {
            debug!("cannot watch the battery ({error}); polling instead");
            Box::new(futures_util::stream::pending())
        }
    }
}

/// Read the charge limit off a blocking thread.
async fn read_sysfs(root: PathBuf) -> Option<Thresholds> {
    tokio::task::spawn_blocking(move || sysfs::read_thresholds(&root))
        .await
        .unwrap_or(None)
}

/// Whether the kernel has a system battery, likewise.
async fn has_sysfs_battery(root: PathBuf) -> bool {
    tokio::task::spawn_blocking(move || sysfs::battery_path(&root).is_some())
        .await
        .unwrap_or(false)
}

/// Publish a state, if it is not the one already published.
fn publish(publisher: &watch::Sender<Arc<BatteryState>>, next: BatteryState) {
    publisher.send_if_modified(|current| {
        if **current == next {
            false
        } else {
            *current = Arc::new(next);
            true
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zero_threshold_from_upower_means_unset() {
        assert_eq!(percent(0), None);
        assert_eq!(percent(80), Some(80));
        assert_eq!(percent(100), Some(100));
        assert_eq!(percent(200), None);
    }

    #[test]
    fn publishing_the_same_state_twice_does_not_wake_a_subscriber() {
        let (publisher, mut receiver) = watch::channel(Arc::new(BatteryState::default()));
        receiver.mark_unchanged();

        publish(
            &publisher,
            BatteryState {
                available: true,
                percent: Some(50.0),
                ..BatteryState::default()
            },
        );
        assert!(receiver.has_changed().expect("the channel is alive"));
        receiver.mark_unchanged();

        publish(
            &publisher,
            BatteryState {
                available: true,
                percent: Some(50.0),
                ..BatteryState::default()
            },
        );
        assert!(!receiver.has_changed().expect("the channel is alive"));
    }

    #[tokio::test]
    async fn a_sysfs_only_machine_still_reports_its_charge_limit() {
        let root = sysfs::tests::TempRoot::new("task-sysfs");
        sysfs::tests::battery_with_thresholds(&root, 75, 80);

        let sources = Sources {
            display: None,
            battery: None,
            root: root.path().to_path_buf(),
        };
        let state = read(&sources).await;
        assert!(state.available, "the kernel says there is a battery");
        assert_eq!(
            state.thresholds.map(|limits| (limits.start, limits.end)),
            Some((75, 80))
        );
        assert!(state.can_set_thresholds());
    }

    #[tokio::test]
    async fn a_desktop_reports_no_battery_at_all() {
        let root = sysfs::tests::TempRoot::new("task-desktop");
        root.supply("AC", &[("type", "Mains\n")]);

        let sources = Sources {
            display: None,
            battery: None,
            root: root.path().to_path_buf(),
        };
        let state = read(&sources).await;
        assert!(!state.available);
        assert_eq!(state.thresholds, None);
    }

    #[tokio::test]
    async fn a_crossed_limit_is_refused_before_anything_is_written() {
        let root = sysfs::tests::TempRoot::new("task-crossed");
        sysfs::tests::battery_with_thresholds(&root, 75, 80);

        let sources = Sources {
            display: None,
            battery: None,
            root: root.path().to_path_buf(),
        };
        let error = apply_thresholds(&sources, 90, 80)
            .await
            .expect_err("start above stop");
        assert_eq!(error.user_message(), "Could not change the charge limit");
        assert_eq!(
            sysfs::read_thresholds(root.path()).map(|limits| (limits.start, limits.end)),
            Some((75, 80)),
            "a refused limit changes nothing"
        );
    }
}
