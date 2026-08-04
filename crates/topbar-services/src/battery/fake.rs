//! A UPower that exists to be read from.
//!
//! Test support only: behind `cfg(test)` for the bus tests and behind the
//! `fake-power` feature for `topbar-fake-power`, the sidecar the nested-niri
//! smoke run puts on its private bus. The packaged panel contains none of it.
//!
//! It serves the two objects the panel talks to — the composite
//! `DisplayDevice` and one real battery — plus a control interface a test (or
//! a `gdbus` line in a smoke driver) drives it through, because there is no
//! synthetic pointer in the smoke session and a battery that never moves
//! proves nothing.
//!
//! `EnableChargeThreshold` writes through to a sysfs tree when it is given
//! one, exactly as the real UPower writes through to the kernel. Without that
//! the fallback path would be a call that returns `Ok` and changes nothing,
//! which is a fake that behaves differently from the thing it stands in for.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use super::model::{FULL_PRESET, LIMIT_PRESET};
use super::proxy::{BUS_NAME, DEVICES, DISPLAY_DEVICE};

/// Where the control interface lives.
pub const CONTROL_PATH: &str = "/io/github/trevarj/topbar/FakePower1";
/// What the control interface is called.
pub const CONTROL_NAME: &str = "io.github.trevarj.topbar.FakePower1";

/// How a fake battery starts out.
#[derive(Debug, Clone)]
pub struct Recipe {
    /// Charge, 0–100.
    pub percent: f64,
    /// UPower's `State` code.
    pub state: u32,
    /// Whether there is a battery at all.
    pub present: bool,
    /// Seconds until flat.
    pub time_to_empty: i64,
    /// Seconds until full.
    pub time_to_full: i64,
    /// Whether UPower offers to drive the charge limit.
    pub threshold_supported: bool,
    /// Where charging resumes, as UPower reports it.
    pub start_threshold: u32,
    /// Where charging stops.
    pub end_threshold: u32,
    /// The sysfs tree `EnableChargeThreshold` writes through to.
    pub sysfs: Option<PathBuf>,
    /// The battery's sysfs directory name, which names its D-Bus object.
    pub battery: String,
}

impl Default for Recipe {
    fn default() -> Self {
        Self {
            percent: 62.0,
            state: 2,
            present: true,
            time_to_empty: 8100,
            time_to_full: 0,
            threshold_supported: false,
            start_threshold: 0,
            end_threshold: 0,
            sysfs: None,
            battery: "BAT0".to_string(),
        }
    }
}

/// Every `EnableChargeThreshold` the fake has been asked for.
pub type Calls = Arc<Mutex<Vec<bool>>>;

/// Lock through poisoning: the log is plain data.
fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// One UPower device object.
struct Device {
    percent: f64,
    state: u32,
    present: bool,
    time_to_empty: i64,
    time_to_full: i64,
    threshold_supported: bool,
    start_threshold: u32,
    end_threshold: u32,
    /// Set only on the real battery: where its threshold files live.
    sysfs: Option<PathBuf>,
    /// Set only on the real battery: what it has been asked to do.
    calls: Option<Calls>,
}

#[zbus::interface(name = "org.freedesktop.UPower.Device")]
impl Device {
    #[zbus(property)]
    fn percentage(&self) -> f64 {
        self.percent
    }

    #[zbus(property)]
    fn state(&self) -> u32 {
        self.state
    }

    #[zbus(property)]
    fn is_present(&self) -> bool {
        self.present
    }

    #[zbus(property)]
    fn time_to_empty(&self) -> i64 {
        self.time_to_empty
    }

    #[zbus(property)]
    fn time_to_full(&self) -> i64 {
        self.time_to_full
    }

    #[zbus(property)]
    fn charge_start_threshold(&self) -> u32 {
        self.start_threshold
    }

    #[zbus(property)]
    fn charge_end_threshold(&self) -> u32 {
        self.end_threshold
    }

    #[zbus(property)]
    fn charge_threshold_supported(&self) -> bool {
        self.threshold_supported
    }

    /// Turn the limit on or off, writing through to sysfs the way UPower does.
    async fn enable_charge_threshold(&mut self, enable: bool) -> zbus::fdo::Result<()> {
        if !self.threshold_supported {
            return Err(zbus::fdo::Error::NotSupported(
                "this battery has no charge limit".into(),
            ));
        }
        if let Some(calls) = &self.calls {
            lock(calls).push(enable);
        }

        let (start, end) = if enable { LIMIT_PRESET } else { FULL_PRESET };
        self.start_threshold = u32::from(start);
        self.end_threshold = u32::from(end);

        if let Some(root) = &self.sysfs {
            let battery = super::sysfs::battery_path(root).ok_or_else(|| {
                zbus::fdo::Error::Failed("the fake sysfs tree has no battery".into())
            })?;
            let paths = super::sysfs::threshold_paths(&battery).ok_or_else(|| {
                zbus::fdo::Error::Failed("the fake battery has no threshold files".into())
            })?;
            // End first, then start: the same order the real write path uses,
            // so the fake cannot succeed where the real one would not.
            for (path, value) in [(paths.end, end), (paths.start, start)] {
                write_as_root(&path, value)
                    .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
            }
        }
        Ok(())
    }
}

/// Write a threshold file the way a process that owns it would.
///
/// The real UPower runs as root and the permission bits simply do not apply to
/// it. A test cannot be root, so the read-only bit is lifted for the duration
/// of the write and put straight back — which leaves the *panel's* view of the
/// file exactly as it was: not ours to write.
fn write_as_root(path: &std::path::Path, value: u8) -> std::io::Result<()> {
    let permissions = std::fs::metadata(path)?.permissions();
    let was_readonly = permissions.readonly();
    if was_readonly {
        let mut writable = permissions.clone();
        #[allow(clippy::permissions_set_readonly_false)]
        writable.set_readonly(false);
        std::fs::set_permissions(path, writable)?;
    }
    let written = std::fs::write(path, format!("{value}\n"));
    if was_readonly {
        std::fs::set_permissions(path, permissions)?;
    }
    written
}

/// What a test may do that a user could not.
struct Control;

#[zbus::interface(name = "io.github.trevarj.topbar.FakePower1")]
impl Control {
    /// Move the charge, announcing it the way UPower would.
    async fn set_percentage(
        &self,
        percent: f64,
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> zbus::fdo::Result<()> {
        let interface = server.interface::<_, Device>(DISPLAY_DEVICE).await?;
        let mut device = interface.get_mut().await;
        device.percent = percent;
        device
            .percentage_changed(interface.signal_emitter())
            .await?;
        Ok(())
    }

    /// Set what the battery is doing.
    async fn set_state(
        &self,
        state: u32,
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> zbus::fdo::Result<()> {
        let interface = server.interface::<_, Device>(DISPLAY_DEVICE).await?;
        let mut device = interface.get_mut().await;
        device.state = state;
        device.state_changed(interface.signal_emitter()).await?;
        Ok(())
    }

    /// Take the battery out, or put it back.
    async fn set_present(
        &self,
        present: bool,
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> zbus::fdo::Result<()> {
        let interface = server.interface::<_, Device>(DISPLAY_DEVICE).await?;
        let mut device = interface.get_mut().await;
        device.present = present;
        device
            .is_present_changed(interface.signal_emitter())
            .await?;
        Ok(())
    }

    /// Set how long UPower thinks is left.
    async fn set_time_to_empty(
        &self,
        seconds: i64,
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> zbus::fdo::Result<()> {
        let interface = server.interface::<_, Device>(DISPLAY_DEVICE).await?;
        let mut device = interface.get_mut().await;
        device.time_to_empty = seconds;
        device
            .time_to_empty_changed(interface.signal_emitter())
            .await?;
        Ok(())
    }
}

/// A running fake UPower.
pub struct FakeUpower {
    /// Kept alive: dropping it drops the bus name.
    _connection: zbus::Connection,
    /// Every `EnableChargeThreshold` the battery has been asked for.
    calls: Calls,
}

impl FakeUpower {
    /// Every `EnableChargeThreshold` call, in order.
    pub fn calls(&self) -> Vec<bool> {
        lock(&self.calls).clone()
    }

    /// The connection, so a test can reach the object server directly.
    pub fn connection(&self) -> &zbus::Connection {
        &self._connection
    }
}

/// Serve a UPower on `address`, built from `recipe`.
pub async fn serve(address: &str, recipe: Recipe) -> zbus::Result<FakeUpower> {
    let calls: Calls = Arc::new(Mutex::new(Vec::new()));
    let battery_path = format!("{DEVICES}/battery_{}", recipe.battery);

    let display = Device {
        percent: recipe.percent,
        state: recipe.state,
        present: recipe.present,
        time_to_empty: recipe.time_to_empty,
        time_to_full: recipe.time_to_full,
        threshold_supported: false,
        start_threshold: 0,
        end_threshold: 0,
        sysfs: None,
        calls: None,
    };
    let battery = Device {
        percent: recipe.percent,
        state: recipe.state,
        present: recipe.present,
        time_to_empty: recipe.time_to_empty,
        time_to_full: recipe.time_to_full,
        threshold_supported: recipe.threshold_supported,
        start_threshold: recipe.start_threshold,
        end_threshold: recipe.end_threshold,
        sysfs: recipe.sysfs.clone(),
        calls: Some(Arc::clone(&calls)),
    };

    let connection = zbus::connection::Builder::address(address)?
        .name(BUS_NAME)?
        .serve_at(DISPLAY_DEVICE, display)?
        .serve_at(battery_path, battery)?
        .serve_at(CONTROL_PATH, Control)?
        .build()
        .await?;

    Ok(FakeUpower {
        _connection: connection,
        calls,
    })
}

/// Announce a property change on the composite device, from outside.
///
/// The smoke driver reaches this through `gdbus`; the bus tests call the
/// control interface directly. Both exist because a battery that never moves
/// would prove only that the first frame renders.
pub async fn emit_percentage(connection: &zbus::Connection, percent: f64) -> zbus::Result<()> {
    let interface = connection
        .object_server()
        .interface::<_, Device>(DISPLAY_DEVICE)
        .await?;
    let mut device = interface.get_mut().await;
    device.percent = percent;
    device.percentage_changed(interface.signal_emitter()).await
}

/// The same, for the state.
pub async fn emit_state(connection: &zbus::Connection, state: u32) -> zbus::Result<()> {
    let interface = connection
        .object_server()
        .interface::<_, Device>(DISPLAY_DEVICE)
        .await?;
    let mut device = interface.get_mut().await;
    device.state = state;
    device.state_changed(interface.signal_emitter()).await
}
