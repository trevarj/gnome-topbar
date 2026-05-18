//! UpdatesService - shared, event-driven package update state.
//!
//! This service provides:
//! - Auto-detection of GNU Guix
//! - Periodic checking for available updates
//! - Background thread execution to avoid blocking the UI
//! - Grouped updates by repository

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::process::Command;
use std::rc::Rc;
use std::time::SystemTime;

use gnome_topbar_core::Config;
use gtk4::glib::{self, SourceId};
use tracing::{debug, info, warn};

use super::callbacks::{CallbackId, Callbacks};
use super::network::NetworkService;

/// Default check interval in seconds (1 hour).
const DEFAULT_CHECK_INTERVAL: u64 = 3600;

/// Minimum check interval to prevent abuse (5 minutes).
const MIN_CHECK_INTERVAL: u64 = 300;

/// Supported package managers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageManager {
    /// GNU Guix profile package manager.
    Guix,
}

impl PackageManager {
    /// Get the upgrade command for this package manager.
    pub fn upgrade_command(&self) -> &'static str {
        match self {
            Self::Guix => "guix pull && guix package --upgrade",
        }
    }
}

/// Information about a single package update.
#[derive(Debug, Clone)]
pub struct UpdateInfo {
    /// Package name.
    pub name: String,
}

/// Canonical snapshot of update state.
#[derive(Debug, Clone)]
pub struct UpdatesSnapshot {
    /// Whether a package manager was detected.
    pub available: bool,
    /// Whether the initial check has completed.
    pub is_ready: bool,
    /// Whether a check is currently in progress.
    pub checking: bool,
    /// Human-readable status during an active check (e.g. "Checking Guix...").
    pub check_status: Option<String>,
    /// Last error message, if any.
    pub error: Option<String>,
    /// Total number of available updates.
    pub update_count: usize,
    /// Updates grouped by repository name.
    pub updates_by_repo: HashMap<String, Vec<UpdateInfo>>,
    /// Time of the last successful check.
    pub last_check: Option<SystemTime>,
    /// Detected package manager.
    pub package_manager: Option<PackageManager>,
}

impl UpdatesSnapshot {
    /// Create an initial "unknown" snapshot.
    pub fn unknown() -> Self {
        Self {
            available: false,
            is_ready: false,
            checking: false,
            check_status: None,
            error: None,
            update_count: 0,
            updates_by_repo: HashMap::new(),
            last_check: None,
            package_manager: None,
        }
    }
}

/// Result of a background update check.
#[derive(Debug)]
struct CheckResult {
    update_count: usize,
    updates_by_repo: HashMap<String, Vec<UpdateInfo>>,
    error: Option<String>,
}

/// Shared, process-wide updates service.
pub struct UpdatesService {
    snapshot: RefCell<UpdatesSnapshot>,
    callbacks: Callbacks<UpdatesSnapshot>,
    check_interval: Cell<u64>,
    count_command: RefCell<Option<String>>,
    timer_source: RefCell<Option<SourceId>>,
    /// Prevent concurrent checks.
    check_in_progress: Cell<bool>,
}

impl UpdatesService {
    fn new() -> Rc<Self> {
        let service = Rc::new(Self {
            snapshot: RefCell::new(UpdatesSnapshot::unknown()),
            callbacks: Callbacks::new(),
            check_interval: Cell::new(DEFAULT_CHECK_INTERVAL),
            count_command: RefCell::new(None),
            timer_source: RefCell::new(None),
            check_in_progress: Cell::new(false),
        });

        // Detect package manager
        let pm = detect_package_manager();
        {
            let mut snapshot = service.snapshot.borrow_mut();
            snapshot.package_manager = pm;
            snapshot.available = pm.is_some();
        }

        if pm.is_some() {
            info!("UpdatesService: detected package manager {:?}", pm);
        } else {
            info!("UpdatesService: no supported package manager detected");
        }

        service.snapshot.borrow_mut().is_ready = true;
        service
    }

    /// Get the global UpdatesService singleton.
    pub fn global() -> Rc<Self> {
        thread_local! {
            static INSTANCE: Rc<UpdatesService> = UpdatesService::new();
        }

        INSTANCE.with(|s| s.clone())
    }

    /// Register a callback to be invoked whenever the snapshot changes.
    pub fn connect<F>(&self, callback: F) -> CallbackId
    where
        F: Fn(&UpdatesSnapshot) + 'static,
    {
        let id = self.callbacks.register(callback);
        // Immediately notify with current snapshot
        self.callbacks.notify_single(id, &self.snapshot.borrow());
        id
    }

    /// Unregister a callback by its ID.
    pub fn disconnect(&self, id: CallbackId) -> bool {
        self.callbacks.unregister(id)
    }

    /// Return the current snapshot.
    pub fn snapshot(&self) -> UpdatesSnapshot {
        self.snapshot.borrow().clone()
    }

    /// Trigger an immediate update check.
    pub fn refresh(&self) {
        if !self.snapshot.borrow().available {
            return;
        }
        let net = NetworkService::global().snapshot();
        if !net.connected() && !net.wired_connected() && !net.mobile_active() {
            let mut snapshot = self.snapshot.borrow_mut();
            snapshot.error = Some("Offline — check skipped".to_string());
            let clone = snapshot.clone();
            drop(snapshot);
            self.callbacks.notify(&clone);
            return;
        }
        self.check_updates_async();
    }

    /// Update the human-readable check status and notify listeners.
    ///
    /// Called from the main thread (via `glib::idle_add_once`) while a
    /// background check is in progress.
    fn set_check_status(&self, status: &str) {
        let mut snapshot = self.snapshot.borrow_mut();
        snapshot.check_status = Some(status.to_string());
        let snapshot_clone = snapshot.clone();
        drop(snapshot);
        self.callbacks.notify(&snapshot_clone);
    }

    /// Set the check interval in seconds.
    ///
    /// Takes effect on the next timer cycle.
    pub fn set_check_interval(&self, seconds: u64) {
        let seconds = seconds.max(MIN_CHECK_INTERVAL);
        self.check_interval.set(seconds);
        debug!("UpdatesService: check interval set to {}s", seconds);
    }

    /// Configure update checks from the loaded application config.
    ///
    /// GNU Guix does not expose a stable, cheap "available updates" command
    /// across releases. Keep this disabled until the user supplies a command.
    pub fn configure_from_config(self: &Rc<Self>, config: &Config) {
        let check_interval = if config.updates.check_interval == 0 {
            DEFAULT_CHECK_INTERVAL
        } else {
            config.updates.check_interval
        };
        let count_command = config
            .updates
            .update_count_command
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from);

        self.configure(check_interval, count_command);
    }

    /// Configure the update count command and restart periodic checks.
    pub fn configure(self: &Rc<Self>, seconds: u64, count_command: Option<String>) {
        self.set_check_interval(seconds);
        *self.count_command.borrow_mut() = count_command.clone();

        if let Some(source_id) = self.timer_source.borrow_mut().take() {
            source_id.remove();
        }

        let mut snapshot = self.snapshot.borrow_mut();
        snapshot.available = snapshot.package_manager.is_some() && count_command.is_some();
        snapshot.is_ready = true;
        snapshot.checking = false;
        snapshot.check_status = None;
        snapshot.error = None;

        if !snapshot.available {
            snapshot.update_count = 0;
            snapshot.updates_by_repo.clear();
            snapshot.last_check = None;
        }

        let snapshot_clone = snapshot.clone();
        drop(snapshot);
        self.callbacks.notify(&snapshot_clone);

        if count_command.is_some() && self.snapshot.borrow().package_manager.is_some() {
            Self::start_periodic_checks(self);
        }
    }

    /// Start periodic update checks.
    fn start_periodic_checks(this: &Rc<Self>) {
        // Do an initial check
        this.check_updates_async();

        // Schedule periodic checks
        let this_weak = Rc::downgrade(this);
        let interval = this.check_interval.get();

        let source_id = glib::timeout_add_seconds_local(interval as u32, move || {
            if let Some(this) = this_weak.upgrade() {
                this.check_updates_async();
                glib::ControlFlow::Continue
            } else {
                glib::ControlFlow::Break
            }
        });

        *this.timer_source.borrow_mut() = Some(source_id);
    }

    /// Perform an async update check in a background thread.
    fn check_updates_async(&self) {
        // Prevent concurrent checks
        if self.check_in_progress.get() {
            debug!("UpdatesService: check already in progress, skipping");
            return;
        }

        // Skip if offline
        let net = NetworkService::global().snapshot();
        if !net.connected() && !net.wired_connected() && !net.mobile_active() {
            debug!("UpdatesService: offline, skipping update check");
            return;
        }

        let pm = match self.snapshot.borrow().package_manager {
            Some(pm) => pm,
            None => return,
        };
        let count_command = match self.count_command.borrow().clone() {
            Some(command) => command,
            None => return,
        };

        self.check_in_progress.set(true);

        // Mark as checking
        {
            let mut snapshot = self.snapshot.borrow_mut();
            snapshot.checking = true;
            let snapshot_clone = snapshot.clone();
            drop(snapshot);
            self.callbacks.notify(&snapshot_clone);
        }

        debug!("UpdatesService: starting update check with {:?}", pm);

        // Spawn background thread
        std::thread::spawn(move || {
            let report_status = |status: String| {
                glib::idle_add_once(move || {
                    UpdatesService::global().set_check_status(&status);
                });
            };

            let result = run_update_check(pm, &count_command, &report_status);

            // Send result back to main thread
            glib::idle_add_once(move || {
                UpdatesService::global().apply_check_result(result);
            });
        });
    }

    /// Apply the result of a background check.
    fn apply_check_result(&self, result: CheckResult) {
        self.check_in_progress.set(false);

        let mut snapshot = self.snapshot.borrow_mut();
        snapshot.checking = false;
        snapshot.check_status = None;
        snapshot.is_ready = true;

        if let Some(err) = result.error {
            warn!("UpdatesService: check failed: {}", err);
            snapshot.error = Some(err);
            // Keep previous update data on error
        } else {
            snapshot.error = None;
            snapshot.updates_by_repo = result.updates_by_repo;
            snapshot.update_count = result.update_count;
            snapshot.last_check = Some(SystemTime::now());

            debug!(
                "UpdatesService: found {} updates across {} repos",
                snapshot.update_count,
                snapshot.updates_by_repo.len()
            );
        }

        let snapshot_clone = snapshot.clone();
        drop(snapshot);
        self.callbacks.notify(&snapshot_clone);
    }
}

impl Drop for UpdatesService {
    fn drop(&mut self) {
        if let Some(source_id) = self.timer_source.borrow_mut().take() {
            source_id.remove();
        }
    }
}

/// Detect the available package manager.
fn detect_package_manager() -> Option<PackageManager> {
    Command::new("guix")
        .arg("--version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|_| PackageManager::Guix)
}

/// Run the update check for the given package manager.
///
/// This runs in a background thread and should not touch any GTK state.
/// `report_status` is called before each step to update the UI with progress.
fn run_update_check(
    pm: PackageManager,
    count_command: &str,
    report_status: &dyn Fn(String),
) -> CheckResult {
    match pm {
        PackageManager::Guix => check_guix_updates(count_command, report_status),
    }
}

/// Check for profile updates using GNU Guix.
fn check_guix_updates(count_command: &str, report_status: &dyn Fn(String)) -> CheckResult {
    report_status("Checking updates...".to_string());
    let output = Command::new("sh").args(["-c", count_command]).output();

    match output {
        Ok(output) => {
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return CheckResult {
                    update_count: 0,
                    updates_by_repo: HashMap::new(),
                    error: Some(format!("Failed to check updates: {}", stderr.trim())),
                };
            }

            let stdout = String::from_utf8_lossy(&output.stdout);
            let (update_count, updates) = parse_count_command_output(&stdout);
            let mut updates_by_repo = HashMap::new();
            if !updates.is_empty() {
                updates_by_repo.insert("guix profile".to_string(), updates);
            }

            CheckResult {
                update_count,
                updates_by_repo,
                error: None,
            }
        }
        Err(e) => CheckResult {
            update_count: 0,
            updates_by_repo: HashMap::new(),
            error: Some(format!("Failed to run update count command: {}", e)),
        },
    }
}

/// Parse custom update count command output.
///
/// A plain integer is treated as the total count. Otherwise each non-empty
/// output line is treated as one update and shown in the details list.
fn parse_count_command_output(output: &str) -> (usize, Vec<UpdateInfo>) {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return (0, Vec::new());
    }

    if let Ok(count) = trimmed.parse::<usize>() {
        return (count, Vec::new());
    }

    let updates: Vec<UpdateInfo> = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| UpdateInfo {
            name: line.to_string(),
        })
        .collect();

    (updates.len(), updates)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_count_command_output_as_lines() {
        let output = r#"
linux-libre     6.15.1          6.15.2
icecat          128.10.0        128.11.0
"#;

        let (count, result) = parse_count_command_output(output);

        assert_eq!(count, 2);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].name, "linux-libre     6.15.1          6.15.2");
        assert_eq!(result[1].name, "icecat          128.10.0        128.11.0");
    }

    #[test]
    fn test_parse_count_command_output_as_number() {
        let (count, result) = parse_count_command_output("12\n");
        assert_eq!(count, 12);
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_count_command_output_empty() {
        let (count, result) = parse_count_command_output("");
        assert_eq!(count, 0);
        assert!(result.is_empty());
    }

    #[test]
    fn test_package_manager_upgrade_command() {
        assert_eq!(
            PackageManager::Guix.upgrade_command(),
            "guix pull && guix package --upgrade"
        );
    }
}
