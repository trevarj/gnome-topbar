//! BatteryService - shared, event-driven battery state via UPower.
//!
//! - Asynchronously connects to the system DBus and UPower DisplayDevice
//! - Reads cached properties for initial state
//! - Listens for `PropertiesChanged` ("g-properties-changed") updates
//! - Notifies listeners on the GLib main loop with a canonical snapshot.

use std::cell::RefCell;
use std::fs;
use std::fs::OpenOptions;
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;

use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use tracing::{debug, error, warn};

use super::callbacks::{CallbackId, Callbacks};

/// Path to the kernel's power supply sysfs directory.
const POWER_SUPPLY_PATH: &str = "/sys/class/power_supply";

/// DBus constants for the UPower DisplayDevice.
const UPOWER_NAME: &str = "org.freedesktop.UPower";
const DISPLAY_PATH: &str = "/org/freedesktop/UPower/devices/DisplayDevice";
const DEVICES_PATH: &str = "/org/freedesktop/UPower/devices";
const DEVICE_IFACE: &str = "org.freedesktop.UPower.Device";

/// UPower state codes of interest.
/// See: https://upower.freedesktop.org/docs/Device.html#Device:state
/// Note: UPower returns State as u32, TimeToEmpty/TimeToFull as i64.
pub const STATE_CHARGING: u32 = 1;
pub const STATE_DISCHARGING: u32 = 2;
pub const STATE_FULLY_CHARGED: u32 = 4;
pub const STATE_PENDING_CHARGE: u32 = 5;
pub const STATE_PENDING_DISCHARGE: u32 = 6;

/// Default battery-health preset used by the Quick Settings controls.
pub const HEALTH_CHARGE_START_THRESHOLD: u8 = 75;
pub const HEALTH_CHARGE_STOP_THRESHOLD: u8 = 80;
/// Full-charge preset. A high start threshold avoids needless trickle cycling.
pub const FULL_CHARGE_START_THRESHOLD: u8 = 96;
pub const FULL_CHARGE_STOP_THRESHOLD: u8 = 100;

/// Kernel charge behavior state from `charge_behaviour`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChargeBehaviourSnapshot {
    /// Currently selected behavior, if the kernel marks one with brackets.
    pub current: Option<String>,
    /// All advertised behavior modes.
    pub options: Vec<String>,
}

/// Canonical snapshot of battery state.
#[derive(Debug, Clone, PartialEq)]
pub struct BatterySnapshot {
    /// Whether the UPower service is available.
    pub available: bool,
    /// Percentage in range 0.0-100.0 if known.
    pub percent: Option<f64>,
    /// Raw UPower state code, if known (u32 from DBus).
    pub state: Option<u32>,
    /// Power draw in Watts, if known.
    pub energy_rate: Option<f64>,
    /// Current full capacity in Wh, if known.
    pub energy_full: Option<f64>,
    /// Design full capacity in Wh, if known.
    pub energy_full_design: Option<f64>,
    /// Seconds until empty, if known (i64 from DBus).
    pub time_to_empty: Option<i64>,
    /// Seconds until full, if known (i64 from DBus).
    pub time_to_full: Option<i64>,
    /// Lower charge threshold, if configured by the kernel/firmware.
    pub charge_start_threshold: Option<u8>,
    /// Upper charge threshold, if configured by the kernel/firmware.
    pub charge_stop_threshold: Option<u8>,
    /// Kernel charge behavior modes, if exposed.
    pub charge_behaviour: Option<ChargeBehaviourSnapshot>,
    /// Battery charge cycle count, if exposed.
    pub cycle_count: Option<u32>,
    /// Whether any external power supply currently reports online.
    pub ac_online: Option<bool>,
    /// Whether charge thresholds are exposed by the kernel/firmware.
    pub charge_control_available: bool,
    /// Whether this process can write charge thresholds directly.
    pub charge_control_writable: bool,
    /// Whether UPower can manage charge thresholds through its D-Bus API.
    pub charge_control_upower_available: bool,
}

impl BatterySnapshot {
    pub fn unknown() -> Self {
        Self {
            available: false,
            percent: None,
            state: None,
            energy_rate: None,
            energy_full: None,
            energy_full_design: None,
            time_to_empty: None,
            time_to_full: None,
            charge_start_threshold: None,
            charge_stop_threshold: None,
            charge_behaviour: None,
            cycle_count: None,
            ac_online: None,
            charge_control_available: false,
            charge_control_writable: false,
            charge_control_upower_available: false,
        }
    }

    /// Whether the system appears to have a charge-limiting health mode active.
    pub fn health_limit_active(&self) -> bool {
        self.charge_stop_threshold.is_some_and(|stop| stop < 100)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
struct BatterySysfsSnapshot {
    charge_start_threshold: Option<u8>,
    charge_stop_threshold: Option<u8>,
    charge_behaviour: Option<ChargeBehaviourSnapshot>,
    cycle_count: Option<u32>,
    energy_full: Option<f64>,
    energy_full_design: Option<f64>,
    ac_online: Option<bool>,
    charge_control_available: bool,
    charge_control_writable: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
struct BatteryUpowerChargeSnapshot {
    charge_start_threshold: Option<u8>,
    charge_stop_threshold: Option<u8>,
    charge_control_supported: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ChargeThresholdPaths {
    start: PathBuf,
    stop: PathBuf,
}

#[derive(Debug, Clone)]
struct ThresholdWrite {
    path: PathBuf,
    value: u8,
}

/// Shared, process-wide battery service.
pub struct BatteryService {
    proxy: RefCell<Option<gio::DBusProxy>>,
    charge_proxy: RefCell<Option<gio::DBusProxy>>,
    snapshot: RefCell<BatterySnapshot>,
    callbacks: Callbacks<BatterySnapshot>,
}

impl BatteryService {
    fn new() -> Rc<Self> {
        let has_battery = Self::has_battery_device();

        // Set available = true immediately if we detected a battery device, so
        // that synchronous checks (e.g., widget factory) see the correct state
        // before the async D-Bus initialization completes.
        let initial_snapshot = if has_battery {
            let mut snapshot = BatterySnapshot {
                available: true,
                ..BatterySnapshot::unknown()
            };
            Self::apply_sysfs_snapshot(&mut snapshot, Self::read_sysfs_snapshot());
            snapshot
        } else {
            BatterySnapshot::unknown()
        };

        let service = Rc::new(Self {
            proxy: RefCell::new(None),
            charge_proxy: RefCell::new(None),
            snapshot: RefCell::new(initial_snapshot),
            callbacks: Callbacks::new(),
        });

        if has_battery {
            Self::init_dbus(&service);
        } else {
            warn!("BatteryService: no battery device found; service disabled");
        }

        service
    }

    /// Check if any battery device exists under /sys/class/power_supply.
    fn has_battery_device() -> bool {
        Self::system_battery_path().is_some()
    }

    /// Return the first system battery under /sys/class/power_supply.
    fn system_battery_path() -> Option<PathBuf> {
        let path = Path::new(POWER_SUPPLY_PATH);
        if !path.exists() {
            debug!("BatteryService: {} does not exist", POWER_SUPPLY_PATH);
            return None;
        }

        let entries = match fs::read_dir(path) {
            Ok(it) => it,
            Err(err) => {
                debug!(
                    "BatteryService: failed to read {}: {err}",
                    POWER_SUPPLY_PATH
                );
                return None;
            }
        };

        for entry in entries.flatten() {
            let entry_path = entry.path();
            let type_path = entry_path.join("type");

            // Check if this is a battery device
            let is_battery = fs::read_to_string(&type_path)
                .is_ok_and(|content| content.trim().eq_ignore_ascii_case("battery"));

            if !is_battery {
                continue;
            }

            // Exclude peripheral batteries (e.g., Logitech mice) by checking scope.
            // System batteries either have scope=System or no scope attribute at all.
            // Peripheral batteries have scope=Device.
            let scope_path = entry_path.join("scope");
            let is_peripheral = fs::read_to_string(&scope_path)
                .is_ok_and(|content| content.trim().eq_ignore_ascii_case("device"));

            if !is_peripheral {
                return Some(entry_path);
            }
        }

        debug!(
            "BatteryService: no battery type device found in {}",
            POWER_SUPPLY_PATH
        );
        None
    }

    fn upower_battery_device_path() -> Option<String> {
        let battery_path = Self::system_battery_path()?;
        Self::upower_device_path_for_battery(&battery_path)
    }

    fn upower_device_path_for_battery(battery_path: &Path) -> Option<String> {
        let native_path = battery_path.file_name()?.to_string_lossy();
        let escaped = native_path
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || ch == '_' {
                    ch
                } else {
                    '_'
                }
            })
            .collect::<String>();

        Some(format!("{DEVICES_PATH}/battery_{escaped}"))
    }

    fn read_trimmed(path: &Path) -> Option<String> {
        fs::read_to_string(path)
            .ok()
            .map(|content| content.trim().to_string())
            .filter(|content| !content.is_empty())
    }

    fn read_u8(path: &Path) -> Option<u8> {
        Self::read_trimmed(path).and_then(|content| content.parse::<u8>().ok())
    }

    fn read_u32(path: &Path) -> Option<u32> {
        Self::read_trimmed(path).and_then(|content| content.parse::<u32>().ok())
    }

    fn read_energy_wh(path: &Path) -> Option<f64> {
        Self::read_trimmed(path)
            .and_then(|content| content.parse::<f64>().ok())
            .map(|microwatt_hours| microwatt_hours / 1_000_000.0)
    }

    fn parse_charge_behaviour(raw: &str) -> ChargeBehaviourSnapshot {
        let mut current = None;
        let mut options = Vec::new();

        for token in raw.split_whitespace() {
            let selected = token
                .strip_prefix('[')
                .and_then(|value| value.strip_suffix(']'));
            let option = selected.unwrap_or(token).to_string();

            if selected.is_some() {
                current = Some(option.clone());
            }
            options.push(option);
        }

        if current.is_none() && options.len() == 1 {
            current = options.first().cloned();
        }

        ChargeBehaviourSnapshot { current, options }
    }

    fn read_charge_behaviour(battery_path: &Path) -> Option<ChargeBehaviourSnapshot> {
        Self::read_trimmed(&battery_path.join("charge_behaviour"))
            .map(|raw| Self::parse_charge_behaviour(&raw))
    }

    fn charge_threshold_paths(battery_path: &Path) -> Option<ChargeThresholdPaths> {
        let control_start = battery_path.join("charge_control_start_threshold");
        let control_stop = battery_path.join("charge_control_end_threshold");
        if control_start.exists() && control_stop.exists() {
            return Some(ChargeThresholdPaths {
                start: control_start,
                stop: control_stop,
            });
        }

        let start = battery_path.join("charge_start_threshold");
        let stop = battery_path.join("charge_stop_threshold");
        if start.exists() && stop.exists() {
            return Some(ChargeThresholdPaths { start, stop });
        }

        None
    }

    fn can_write_path(path: &Path) -> bool {
        OpenOptions::new().write(true).open(path).is_ok()
    }

    fn threshold_paths_writable(paths: &ChargeThresholdPaths) -> bool {
        Self::can_write_path(&paths.start) && Self::can_write_path(&paths.stop)
    }

    fn read_external_power_online() -> Option<bool> {
        let path = Path::new(POWER_SUPPLY_PATH);
        let entries = fs::read_dir(path).ok()?;
        let mut saw_external_supply = false;

        for entry in entries.flatten() {
            let entry_path = entry.path();
            let supply_type = Self::read_trimmed(&entry_path.join("type"))
                .map(|value| value.to_ascii_lowercase());
            let is_external = matches!(
                supply_type.as_deref(),
                Some("mains" | "usb" | "usb_c" | "usb_pd" | "wireless")
            );

            if !is_external {
                continue;
            }

            saw_external_supply = true;
            if Self::read_trimmed(&entry_path.join("online")).as_deref() == Some("1") {
                return Some(true);
            }
        }

        saw_external_supply.then_some(false)
    }

    fn read_sysfs_snapshot() -> BatterySysfsSnapshot {
        let Some(battery_path) = Self::system_battery_path() else {
            return BatterySysfsSnapshot {
                ac_online: Self::read_external_power_online(),
                ..BatterySysfsSnapshot::default()
            };
        };

        let charge_start_threshold =
            Self::read_u8(&battery_path.join("charge_control_start_threshold"))
                .or_else(|| Self::read_u8(&battery_path.join("charge_start_threshold")));
        let charge_stop_threshold =
            Self::read_u8(&battery_path.join("charge_control_end_threshold"))
                .or_else(|| Self::read_u8(&battery_path.join("charge_stop_threshold")));
        let threshold_paths = Self::charge_threshold_paths(&battery_path);
        let charge_control_available = threshold_paths.is_some();
        let charge_control_writable = threshold_paths
            .as_ref()
            .is_some_and(Self::threshold_paths_writable);

        BatterySysfsSnapshot {
            charge_start_threshold,
            charge_stop_threshold,
            charge_behaviour: Self::read_charge_behaviour(&battery_path),
            cycle_count: Self::read_u32(&battery_path.join("cycle_count")),
            energy_full: Self::read_energy_wh(&battery_path.join("energy_full")),
            energy_full_design: Self::read_energy_wh(&battery_path.join("energy_full_design")),
            ac_online: Self::read_external_power_online(),
            charge_control_available,
            charge_control_writable,
        }
    }

    fn apply_sysfs_snapshot(snapshot: &mut BatterySnapshot, sysfs: BatterySysfsSnapshot) {
        snapshot.charge_start_threshold = sysfs.charge_start_threshold;
        snapshot.charge_stop_threshold = sysfs.charge_stop_threshold;
        snapshot.charge_behaviour = sysfs.charge_behaviour;
        snapshot.cycle_count = sysfs.cycle_count;
        snapshot.energy_full = snapshot.energy_full.or(sysfs.energy_full);
        snapshot.energy_full_design = snapshot.energy_full_design.or(sysfs.energy_full_design);
        snapshot.ac_online = sysfs.ac_online;
        snapshot.charge_control_available = sysfs.charge_control_available;
        snapshot.charge_control_writable = sysfs.charge_control_writable;
    }

    fn apply_upower_charge_snapshot(
        snapshot: &mut BatterySnapshot,
        upower: BatteryUpowerChargeSnapshot,
    ) {
        if let Some(start) = upower.charge_start_threshold {
            snapshot.charge_start_threshold = Some(start);
        }
        if let Some(stop) = upower.charge_stop_threshold {
            snapshot.charge_stop_threshold = Some(stop);
        }

        if upower.charge_control_supported {
            snapshot.charge_control_available = true;
            snapshot.charge_control_upower_available = true;
        } else {
            snapshot.charge_control_upower_available = false;
        }
    }

    /// Get the global BatteryService singleton.
    pub fn global() -> Rc<Self> {
        thread_local! {
            static INSTANCE: Rc<BatteryService> = BatteryService::new();
        }

        INSTANCE.with(|s| s.clone())
    }

    /// Register a callback to be invoked whenever the battery snapshot changes.
    /// The callback is always executed on the GLib main loop.
    pub fn connect<F>(&self, callback: F) -> CallbackId
    where
        F: Fn(&BatterySnapshot) + 'static,
    {
        let id = self.callbacks.register(callback);

        // Immediately send current snapshot so widgets can render without
        // waiting for the next change.
        self.callbacks.notify_single(id, &self.snapshot.borrow());
        id
    }

    /// Unregister a callback by its ID.
    pub fn disconnect(&self, id: CallbackId) -> bool {
        self.callbacks.unregister(id)
    }

    /// Return the current battery snapshot.
    pub fn snapshot(&self) -> BatterySnapshot {
        self.snapshot.borrow().clone()
    }

    /// Refresh cached battery state from UPower/sysfs and notify listeners if it changed.
    pub fn refresh(&self) {
        if self.proxy.borrow().is_some() || self.charge_proxy.borrow().is_some() {
            self.update_from_proxy();
            return;
        }

        let mut new_snapshot = self.snapshot.borrow().clone();
        Self::apply_sysfs_snapshot(&mut new_snapshot, Self::read_sysfs_snapshot());
        let mut snapshot = self.snapshot.borrow_mut();
        if *snapshot == new_snapshot {
            return;
        }
        *snapshot = new_snapshot;
        drop(snapshot);
        self.callbacks.notify(&self.snapshot.borrow());
    }

    /// Enable or disable the default battery-health charge limit.
    pub fn set_health_limit_enabled(&self, enabled: bool) {
        let (start, stop) = if enabled {
            (HEALTH_CHARGE_START_THRESHOLD, HEALTH_CHARGE_STOP_THRESHOLD)
        } else {
            (FULL_CHARGE_START_THRESHOLD, FULL_CHARGE_STOP_THRESHOLD)
        };

        self.set_charge_thresholds(start, stop);
    }

    /// Set start/stop charge thresholds.
    ///
    /// UPower is preferred because it handles policy and vendor-specific
    /// charge-limit details. Direct sysfs writes are only used when already
    /// writable by this process.
    pub fn set_charge_thresholds(&self, start: u8, stop: u8) {
        if let Err(err) = Self::validate_charge_thresholds(start, stop) {
            warn!("BatteryService: refusing invalid charge thresholds: {err}");
            return;
        }

        if Self::charge_thresholds_direct_writable() {
            if self.set_charge_thresholds_direct(start, stop, "threshold files are writable") {
                return;
            }
            warn!("BatteryService: direct charge threshold write failed; trying UPower");
        }

        if let Some(enabled) = Self::upower_charge_threshold_enabled_for_thresholds(start, stop)
            && self.set_charge_threshold_enabled_via_upower(enabled, start, stop)
        {
            return;
        }

        self.set_charge_thresholds_direct(start, stop, "UPower is unavailable");
    }

    fn charge_thresholds_direct_writable() -> bool {
        let Some(battery_path) = Self::system_battery_path() else {
            return false;
        };
        let Some(paths) = Self::charge_threshold_paths(&battery_path) else {
            return false;
        };

        Self::threshold_paths_writable(&paths)
    }

    fn set_charge_thresholds_direct(&self, start: u8, stop: u8, reason: &str) -> bool {
        let Some(battery_path) = Self::system_battery_path() else {
            warn!("BatteryService: cannot set charge thresholds; no system battery found");
            return false;
        };
        let Some(paths) = Self::charge_threshold_paths(&battery_path) else {
            warn!(
                "BatteryService: cannot set charge thresholds; no threshold files under {}",
                battery_path.display()
            );
            return false;
        };

        let current_stop = self.snapshot.borrow().charge_stop_threshold;
        let writes = Self::ordered_threshold_writes(&paths, start, stop, current_stop);

        if !Self::threshold_paths_writable(&paths) {
            warn!(
                "BatteryService: cannot set charge thresholds directly after {reason}; threshold files are not writable"
            );
            return false;
        }

        match Self::write_thresholds_direct(&writes) {
            Ok(()) => {
                debug!(
                    "BatteryService: set charge thresholds directly to start={} stop={}",
                    start, stop
                );
                self.refresh();
                true
            }
            Err(err) => {
                warn!("BatteryService: direct charge threshold write failed: {err}");
                false
            }
        }
    }

    fn validate_charge_thresholds(start: u8, stop: u8) -> Result<(), String> {
        if start >= stop {
            return Err(format!("start {start}% must be below stop {stop}%"));
        }
        if stop > 100 {
            return Err(format!("stop {stop}% must be at most 100%"));
        }
        Ok(())
    }

    fn ordered_threshold_writes(
        paths: &ChargeThresholdPaths,
        start: u8,
        stop: u8,
        current_stop: Option<u8>,
    ) -> Vec<ThresholdWrite> {
        let start_write = ThresholdWrite {
            path: paths.start.clone(),
            value: start,
        };
        let stop_write = ThresholdWrite {
            path: paths.stop.clone(),
            value: stop,
        };

        // When raising the start threshold above the current stop threshold,
        // set stop first so the kernel never sees an invalid start >= stop pair.
        if current_stop.is_some_and(|current| start >= current) {
            vec![stop_write, start_write]
        } else {
            vec![start_write, stop_write]
        }
    }

    fn write_thresholds_direct(writes: &[ThresholdWrite]) -> Result<(), String> {
        for write in writes {
            fs::write(&write.path, format!("{}\n", write.value))
                .map_err(|err| format!("failed to write {}: {err}", write.path.display()))?;
        }
        Ok(())
    }

    fn init_dbus(this: &Rc<Self>) {
        Self::init_display_proxy(this);
        Self::init_charge_proxy(this);
    }

    fn init_display_proxy(this: &Rc<Self>) {
        let this_weak = Rc::downgrade(this);

        // Asynchronously create proxy on the system bus.
        gio::DBusProxy::for_bus(
            gio::BusType::System,
            gio::DBusProxyFlags::NONE,
            None::<&gio::DBusInterfaceInfo>,
            UPOWER_NAME,
            DISPLAY_PATH,
            DEVICE_IFACE,
            None::<&gio::Cancellable>,
            move |res| {
                let this = match this_weak.upgrade() {
                    Some(this) => this,
                    None => return,
                };

                let proxy = match res {
                    Ok(p) => p,
                    Err(e) => {
                        error!("Failed to create UPower DBusProxy: {}", e);
                        // Leave snapshot as unknown; widgets will show fallback.
                        return;
                    }
                };

                this.proxy.replace(Some(proxy.clone()));

                // Initial snapshot.
                this.update_from_proxy();

                // Subscribe to property changes.
                let this_weak = Rc::downgrade(&this);
                proxy.connect_local("g-properties-changed", false, move |_values| {
                    if let Some(this) = this_weak.upgrade() {
                        this.update_from_proxy();
                    }
                    None
                });

                // Monitor for service appearing/disappearing (e.g., UPower restart).
                let this_weak = Rc::downgrade(&this);
                proxy.connect_local("notify::g-name-owner", false, move |values| {
                    let this = this_weak.upgrade()?;
                    let proxy = values[0].get::<gio::DBusProxy>().ok();
                    let has_owner = proxy.and_then(|p| p.name_owner()).is_some();
                    if has_owner {
                        // Service reappeared - refresh state.
                        this.update_from_proxy();
                    } else {
                        // Service disappeared - mark unavailable.
                        this.set_unavailable();
                    }
                    None
                });
            },
        );
    }

    fn init_charge_proxy(this: &Rc<Self>) {
        let Some(device_path) = Self::upower_battery_device_path() else {
            debug!("BatteryService: no UPower battery device path available");
            return;
        };
        let path_for_error = device_path.clone();
        let this_weak = Rc::downgrade(this);

        gio::DBusProxy::for_bus(
            gio::BusType::System,
            gio::DBusProxyFlags::NONE,
            None::<&gio::DBusInterfaceInfo>,
            UPOWER_NAME,
            &device_path,
            DEVICE_IFACE,
            None::<&gio::Cancellable>,
            move |res| {
                let this = match this_weak.upgrade() {
                    Some(this) => this,
                    None => return,
                };

                let proxy = match res {
                    Ok(p) => p,
                    Err(e) => {
                        warn!(
                            "BatteryService: failed to create UPower battery proxy at {}: {}",
                            path_for_error, e
                        );
                        return;
                    }
                };

                this.charge_proxy.replace(Some(proxy.clone()));
                this.update_from_proxy();

                let this_weak = Rc::downgrade(&this);
                proxy.connect_local("g-properties-changed", false, move |_values| {
                    if let Some(this) = this_weak.upgrade() {
                        this.update_from_proxy();
                    }
                    None
                });

                let this_weak = Rc::downgrade(&this);
                proxy.connect_local("notify::g-name-owner", false, move |_values| {
                    if let Some(this) = this_weak.upgrade() {
                        this.update_from_proxy();
                    }
                    None
                });
            },
        );
    }

    fn set_unavailable(&self) {
        let mut snapshot = self.snapshot.borrow_mut();
        if !snapshot.available {
            return; // Already unavailable
        }
        *snapshot = BatterySnapshot::unknown();
        let snapshot_clone = snapshot.clone();
        drop(snapshot);
        self.callbacks.notify(&snapshot_clone);
    }

    fn update_from_proxy(&self) {
        fn variant_f64(v: Option<glib::Variant>) -> Option<f64> {
            v.and_then(|v| v.get::<f64>())
        }

        fn variant_u32(v: Option<glib::Variant>) -> Option<u32> {
            v.and_then(|v| v.get::<u32>())
        }

        fn variant_i64(v: Option<glib::Variant>) -> Option<i64> {
            v.and_then(|v| v.get::<i64>())
        }

        let mut new_snapshot = if let Some(ref proxy) = *self.proxy.borrow() {
            let energy = variant_f64(proxy.cached_property("Energy"));
            let full = variant_f64(proxy.cached_property("EnergyFull"));
            let full_design = variant_f64(proxy.cached_property("EnergyFullDesign"));
            let percentage_prop = variant_f64(proxy.cached_property("Percentage"));
            let state = variant_u32(proxy.cached_property("State"));
            let energy_rate = variant_f64(proxy.cached_property("EnergyRate"));
            let time_to_empty = variant_i64(proxy.cached_property("TimeToEmpty"));
            let time_to_full = variant_i64(proxy.cached_property("TimeToFull"));

            let percent = match (energy, full) {
                (Some(e), Some(f)) if f > 0.0 => Some(((e / f) * 100.0).clamp(0.0, 100.0)),
                _ => percentage_prop,
            };

            BatterySnapshot {
                available: true,
                percent,
                state,
                energy_rate,
                energy_full: full,
                energy_full_design: full_design,
                time_to_empty,
                time_to_full,
                charge_start_threshold: None,
                charge_stop_threshold: None,
                charge_behaviour: None,
                cycle_count: None,
                ac_online: None,
                charge_control_available: false,
                charge_control_writable: false,
                charge_control_upower_available: false,
            }
        } else {
            let mut snapshot = self.snapshot.borrow().clone();
            snapshot.charge_control_available = false;
            snapshot.charge_control_writable = false;
            snapshot.charge_control_upower_available = false;
            snapshot
        };
        Self::apply_sysfs_snapshot(&mut new_snapshot, Self::read_sysfs_snapshot());
        Self::apply_upower_charge_snapshot(&mut new_snapshot, self.read_upower_charge_snapshot());

        let mut snapshot = self.snapshot.borrow_mut();
        if *snapshot == new_snapshot {
            return;
        }

        *snapshot = new_snapshot;
        drop(snapshot); // Release borrow before notify
        self.callbacks.notify(&self.snapshot.borrow());
    }

    fn read_upower_charge_snapshot(&self) -> BatteryUpowerChargeSnapshot {
        let Some(ref proxy) = *self.charge_proxy.borrow() else {
            return BatteryUpowerChargeSnapshot::default();
        };
        if proxy.name_owner().is_none() {
            return BatteryUpowerChargeSnapshot::default();
        }

        let start = proxy
            .cached_property("ChargeStartThreshold")
            .and_then(|value| value.get::<u32>())
            .and_then(Self::nonzero_threshold_u32_to_u8);
        let stop = proxy
            .cached_property("ChargeEndThreshold")
            .and_then(|value| value.get::<u32>())
            .and_then(Self::nonzero_threshold_u32_to_u8);
        let supported = proxy
            .cached_property("ChargeThresholdSupported")
            .and_then(|value| value.get::<bool>())
            .unwrap_or(false);
        let settings_supported = proxy
            .cached_property("ChargeThresholdSettingsSupported")
            .and_then(|value| value.get::<u32>())
            .unwrap_or(0);

        BatteryUpowerChargeSnapshot {
            charge_start_threshold: start,
            charge_stop_threshold: stop,
            charge_control_supported: supported || settings_supported > 0,
        }
    }

    fn nonzero_threshold_u32_to_u8(value: u32) -> Option<u8> {
        if (1..=100).contains(&value) {
            Some(value as u8)
        } else {
            None
        }
    }

    fn upower_charge_threshold_enabled_for_thresholds(start: u8, stop: u8) -> Option<bool> {
        if start == HEALTH_CHARGE_START_THRESHOLD && stop == HEALTH_CHARGE_STOP_THRESHOLD {
            Some(true)
        } else if start == FULL_CHARGE_START_THRESHOLD && stop == FULL_CHARGE_STOP_THRESHOLD {
            Some(false)
        } else {
            None
        }
    }

    fn set_charge_threshold_enabled_via_upower(&self, enabled: bool, start: u8, stop: u8) -> bool {
        let Some(proxy) = self.charge_proxy.borrow().clone() else {
            return false;
        };
        let snapshot = self.snapshot.borrow().clone();
        if !snapshot.charge_control_upower_available {
            return false;
        }

        debug!(
            "BatteryService: requesting UPower charge threshold enabled={} for start={} stop={} (current start={:?} stop={:?})",
            enabled, start, stop, snapshot.charge_start_threshold, snapshot.charge_stop_threshold
        );

        proxy.call(
            "EnableChargeThreshold",
            Some(&(enabled,).to_variant()),
            gio::DBusCallFlags::NONE,
            5000,
            None::<&gio::Cancellable>,
            move |res| {
                match res {
                    Ok(_) => debug!(
                        "BatteryService: set UPower charge threshold enabled={}",
                        enabled
                    ),
                    Err(err) => {
                        error!(
                            "BatteryService: failed to set UPower charge threshold enabled={} for start={} stop={}: {}",
                            enabled, start, stop, err
                        );

                        BatteryService::global().set_charge_thresholds_direct(
                            start,
                            stop,
                            "UPower call failed",
                        );
                    }
                }

                BatteryService::global().refresh();
            },
        );

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_charge_behaviour_marks_selected_option() {
        let parsed =
            BatteryService::parse_charge_behaviour("[auto] inhibit-charge force-discharge");

        assert_eq!(parsed.current.as_deref(), Some("auto"));
        assert_eq!(
            parsed.options,
            vec!["auto", "inhibit-charge", "force-discharge"]
        );
    }

    #[test]
    fn parse_charge_behaviour_handles_single_plain_value() {
        let parsed = BatteryService::parse_charge_behaviour("auto");

        assert_eq!(parsed.current.as_deref(), Some("auto"));
        assert_eq!(parsed.options, vec!["auto"]);
    }

    #[test]
    fn health_limit_active_requires_stop_below_full() {
        let mut snapshot = BatterySnapshot::unknown();
        snapshot.charge_stop_threshold = Some(80);
        assert!(snapshot.health_limit_active());

        snapshot.charge_stop_threshold = Some(100);
        assert!(!snapshot.health_limit_active());
    }

    #[test]
    fn validate_charge_thresholds_rejects_crossed_values() {
        assert!(BatteryService::validate_charge_thresholds(75, 80).is_ok());
        assert!(BatteryService::validate_charge_thresholds(80, 80).is_err());
        assert!(BatteryService::validate_charge_thresholds(90, 80).is_err());
    }

    #[test]
    fn ordered_threshold_writes_raises_stop_before_start_when_needed() {
        let paths = ChargeThresholdPaths {
            start: PathBuf::from("start"),
            stop: PathBuf::from("stop"),
        };

        let writes = BatteryService::ordered_threshold_writes(&paths, 96, 100, Some(80));

        assert_eq!(writes[0].path, PathBuf::from("stop"));
        assert_eq!(writes[0].value, 100);
        assert_eq!(writes[1].path, PathBuf::from("start"));
        assert_eq!(writes[1].value, 96);
    }

    #[test]
    fn ordered_threshold_writes_lowers_start_before_stop_when_safe() {
        let paths = ChargeThresholdPaths {
            start: PathBuf::from("start"),
            stop: PathBuf::from("stop"),
        };

        let writes = BatteryService::ordered_threshold_writes(&paths, 75, 80, Some(100));

        assert_eq!(writes[0].path, PathBuf::from("start"));
        assert_eq!(writes[0].value, 75);
        assert_eq!(writes[1].path, PathBuf::from("stop"));
        assert_eq!(writes[1].value, 80);
    }

    #[test]
    fn upower_threshold_mapping_matches_quick_settings_presets() {
        assert_eq!(
            BatteryService::upower_charge_threshold_enabled_for_thresholds(75, 80),
            Some(true)
        );
        assert_eq!(
            BatteryService::upower_charge_threshold_enabled_for_thresholds(96, 100),
            Some(false)
        );
        assert_eq!(
            BatteryService::upower_charge_threshold_enabled_for_thresholds(70, 85),
            None
        );
    }

    #[test]
    fn upower_device_path_escapes_battery_name() {
        let path = PathBuf::from("/sys/class/power_supply/BAT-0");

        assert_eq!(
            BatteryService::upower_device_path_for_battery(&path).as_deref(),
            Some("/org/freedesktop/UPower/devices/battery_BAT_0")
        );
    }
}
