//! Bar lifecycle across monitors.
//!
//! One [`BarWindow`] per monitor, keyed by **connector name** (`eDP-1`,
//! `DP-2`). GDK hands out fresh `GdkMonitor` objects across a hotplug, so
//! object identity says nothing about which physical output a bar belongs to;
//! the connector name is the only stable key.
//!
//! Monitor changes arrive as a burst — a mode set can emit several signals in
//! a few milliseconds — so both signals feed one debounced sync. The
//! configuration is read from the shared cell at sync time, never captured up
//! front, so a reload landing mid-debounce rebuilds with the new values.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Application, gdk, gio, glib};
use topbar_core::Config;
use topbar_services::Services;
use tracing::{debug, info, warn};

use crate::bar::window::BarWindow;

/// How long to wait for a burst of monitor changes to settle.
const SYNC_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(300);

/// The live configuration, shared with whatever reloads it.
///
/// Holding the config behind a cell (rather than cloning it into the manager)
/// is what makes "read the config fresh at rebuild time" structural instead of
/// a rule to remember.
#[derive(Clone)]
pub struct SharedConfig(Rc<RefCell<Rc<Config>>>);

impl SharedConfig {
    /// Wrap a freshly loaded configuration.
    pub fn new(config: Config) -> Self {
        Self(Rc::new(RefCell::new(Rc::new(config))))
    }

    /// The configuration as of right now.
    pub fn current(&self) -> Rc<Config> {
        Rc::clone(&self.0.borrow())
    }
}

/// Owns every bar window and keeps the set in step with the display.
pub struct BarManager {
    app: Application,
    display: gdk::Display,
    config: SharedConfig,
    services: Services,
    bars: RefCell<BTreeMap<String, BarWindow>>,
    pending_sync: RefCell<Option<glib::SourceId>>,
}

impl BarManager {
    /// Create a manager for `display`. No bars exist until [`Self::sync`].
    pub fn new(
        app: &Application,
        display: &gdk::Display,
        config: SharedConfig,
        services: Services,
    ) -> Rc<Self> {
        Rc::new(Self {
            app: app.clone(),
            display: display.clone(),
            config,
            services,
            bars: RefCell::new(BTreeMap::new()),
            pending_sync: RefCell::new(None),
        })
    }

    /// Bring the set of bars in line with the monitors GDK reports.
    ///
    /// Bars for monitors that are still present are left untouched: rebuilding
    /// them would restart every widget's timers for no reason.
    pub fn sync(&self) {
        let config = self.config.current();
        let monitors = self.display.monitors();
        let mut present: Vec<String> = Vec::new();

        for index in 0..monitors.n_items() {
            let Some(monitor) = monitors.item(index).and_downcast::<gdk::Monitor>() else {
                continue;
            };
            let connector = connector_key(&monitor, index);
            present.push(connector.clone());

            if self.bars.borrow().contains_key(&connector) {
                continue;
            }
            let bar = BarWindow::build(&self.app, &config, &monitor, &connector, &self.services);
            self.bars.borrow_mut().insert(connector, bar);
        }

        let removed: Vec<String> = self
            .bars
            .borrow()
            .keys()
            .filter(|connector| !present.contains(connector))
            .cloned()
            .collect();
        for connector in removed {
            info!("monitor {connector} disconnected; removing its bar");
            self.bars.borrow_mut().remove(&connector);
        }

        info!("{} bar(s) active", self.bars.borrow().len());
    }

    /// Watch the display for monitors coming and going.
    ///
    /// `items-changed` covers the list itself; `notify::n-items` catches the
    /// backends that only bump the count. Both land on the same debounced
    /// sync, so a duplicate notification costs nothing.
    pub fn watch_monitors(self: &Rc<Self>) {
        let monitors = self.display.monitors();

        monitors.connect_items_changed({
            let manager = Rc::clone(self);
            move |_, position, removed, added| {
                debug!("monitors changed at {position}: -{removed} +{added}");
                manager.schedule_sync();
            }
        });

        monitors.connect_notify_local(Some("n-items"), {
            let manager = Rc::clone(self);
            move |monitors: &gio::ListModel, _| {
                debug!("monitor count is now {}", monitors.n_items());
                manager.schedule_sync();
            }
        });
    }

    /// Queue a sync, replacing any sync already queued.
    fn schedule_sync(self: &Rc<Self>) {
        if let Some(pending) = self.pending_sync.borrow_mut().take() {
            pending.remove();
        }

        let manager = Rc::clone(self);
        let source = glib::timeout_add_local_once(SYNC_DEBOUNCE, move || {
            *manager.pending_sync.borrow_mut() = None;
            manager.sync();
        });
        *self.pending_sync.borrow_mut() = Some(source);
    }
}

impl Drop for BarManager {
    fn drop(&mut self) {
        if let Some(pending) = self.pending_sync.borrow_mut().take() {
            pending.remove();
        }
    }
}

/// The stable key for a monitor.
///
/// Connector names come from the compositor and survive hotplug; the indexed
/// fallback only covers a monitor GDK has not finished setting up, which the
/// next sync replaces with the real name.
fn connector_key(monitor: &gdk::Monitor, index: u32) -> String {
    match monitor.connector() {
        Some(connector) => connector.to_string(),
        None => {
            warn!("monitor {index} has no connector name yet");
            format!("unknown-{index}")
        }
    }
}
