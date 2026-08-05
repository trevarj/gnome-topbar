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
use topbar_core::ipc::VisibilityAction;
use topbar_services::Services;
use tracing::{debug, info, warn};

use crate::bar::window::BarWindow;

/// How long to wait for a burst of monitor changes to settle.
const SYNC_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(300);

/// How long between retries while a monitor is still being set up.
const READY_RETRY: std::time::Duration = std::time::Duration::from_millis(250);

/// How long a monitor may take to become usable before its bar is built anyway.
///
/// A monitor arrives from GDK before the compositor has finished configuring
/// it: no connector name yet, or a geometry of zero. Building against that
/// gives a bar with a made-up key and no width. Waiting forever is worse — a
/// monitor that never reports a geometry would mean a session with no panel at
/// all — so the wait is bounded, the bar is built with whatever is known, and
/// the next monitor signal corrects it.
const READY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

thread_local! {
    /// Signal handlers this process has connected to the monitor list and not
    /// disconnected.
    ///
    /// A hotplug loop that leaks one handler per cycle ends up reconfiguring
    /// the bars once per past hotplug, which looks like a slow panel rather
    /// than like a leak. Counting them makes it visible in `panel.log`, which
    /// is what the hotplug smoke run asserts on.
    static HANDLERS: std::cell::Cell<i64> = const { std::cell::Cell::new(0) };
}

/// How many monitor-list handlers are connected right now.
pub fn live_handlers() -> i64 {
    HANDLERS.with(std::cell::Cell::get)
}

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

    /// Install a freshly loaded configuration.
    ///
    /// Everything read at rebuild time picks the new values up; widgets built
    /// against the old ones keep them until they are rebuilt, which is what
    /// [`crate::reload`] does with the sections that changed.
    pub fn replace(&self, config: Config) {
        *self.0.borrow_mut() = Rc::new(config);
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
    /// When the panel started waiting for a monitor to finish arriving.
    ///
    /// Cleared the moment every monitor is usable, so the timeout is per wait
    /// rather than per session.
    waiting_since: std::cell::Cell<Option<std::time::Instant>>,
    /// The handlers on the monitor list, disconnected when the manager goes.
    handlers: RefCell<Vec<(gio::ListModel, glib::SignalHandlerId)>>,
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
            waiting_since: std::cell::Cell::new(None),
            handlers: RefCell::new(Vec::new()),
        })
    }

    /// Bring the set of bars in line with the monitors GDK reports.
    ///
    /// Bars for monitors that are still present are left untouched: rebuilding
    /// them would restart every widget's timers for no reason.
    pub fn sync(self: &Rc<Self>) {
        let config = self.config.current();
        let monitors = self.display.monitors();
        let mut present: Vec<String> = Vec::new();
        let mut waiting = false;

        for index in 0..monitors.n_items() {
            let Some(monitor) = monitors.item(index).and_downcast::<gdk::Monitor>() else {
                continue;
            };

            // A monitor GDK has told us about but the compositor has not
            // finished configuring yet. Building now would key the bar on a
            // made-up name and size it against a geometry of zero, and the
            // second bar built when the real name arrives would be a duplicate.
            if !is_ready(&monitor) && !self.waited_long_enough() {
                debug!("monitor {index} is not ready yet; waiting");
                waiting = true;
                continue;
            }

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

        if waiting {
            // Retried rather than watched: a `notify::geometry` handler per
            // half-arrived monitor is a handler to disconnect on every path a
            // monitor can leave by, and this loop is bounded by
            // `READY_TIMEOUT`. The retry goes through the same slot as a
            // hotplug's debounce, so a monitor arriving meanwhile cancels it.
            self.schedule_sync_in(READY_RETRY);
        } else {
            self.waiting_since.set(None);
        }

        info!(
            "{} bar(s) active, {} monitor handler(s)",
            self.bars.borrow().len(),
            live_handlers()
        );
    }

    /// Whether the wait for a half-arrived monitor has gone on long enough to
    /// build its bar regardless. Starts the clock on the first call of a wait.
    fn waited_long_enough(&self) -> bool {
        let started = self.waiting_since.get().unwrap_or_else(|| {
            let now = std::time::Instant::now();
            self.waiting_since.set(Some(now));
            now
        });
        if started.elapsed() < READY_TIMEOUT {
            return false;
        }
        warn!(
            "a monitor has not reported a connector and geometry in {}s; \
             building its bar anyway",
            READY_TIMEOUT.as_secs()
        );
        true
    }

    /// Watch the display for monitors coming and going.
    ///
    /// `items-changed` covers the list itself; `notify::n-items` catches the
    /// backends that only bump the count. Both land on the same debounced
    /// sync, so a duplicate notification costs nothing.
    pub fn watch_monitors(self: &Rc<Self>) {
        let monitors = self.display.monitors();

        let items_changed = monitors.connect_items_changed({
            let manager = Rc::clone(self);
            move |_, position, removed, added| {
                debug!("monitors changed at {position}: -{removed} +{added}");
                manager.schedule_sync();
            }
        });

        let n_items = monitors.connect_notify_local(Some("n-items"), {
            let manager = Rc::clone(self);
            move |monitors: &gio::ListModel, _| {
                debug!("monitor count is now {}", monitors.n_items());
                manager.schedule_sync();
            }
        });

        let mut handlers = self.handlers.borrow_mut();
        for id in [items_changed, n_items] {
            handlers.push((monitors.clone().upcast(), id));
            HANDLERS.with(|count| count.set(count.get() + 1));
        }
    }

    /// Throw every bar away and build them again from the current config.
    ///
    /// What a changed `[bar]` section or a changed widget list needs: the
    /// window height is the exclusive zone and the order of a section is the
    /// order its widgets were appended in, so neither can be edited in place.
    /// Also what a `theme.blur` flip needs — an attachment is made once,
    /// against the blur manager as it was at the time, and is never mutated.
    pub fn rebuild(self: &Rc<Self>) {
        let visible = self.bars.borrow().values().any(BarWindow::is_visible);
        self.bars.borrow_mut().clear();
        self.sync();
        if !visible {
            // A reload must not put a hidden bar back on screen.
            self.set_bars_visible(VisibilityAction::Hide);
        }
    }

    /// Build the widgets named in `names` again, on every bar.
    ///
    /// Returns how many were replaced across all monitors.
    pub fn rebuild_widgets(&self, names: &std::collections::BTreeSet<String>) -> usize {
        let config = self.config.current();
        let mut rebuilt = 0;
        for bar in self.bars.borrow_mut().values_mut() {
            rebuilt += bar.rebuild_widgets(names, &config);
        }
        rebuilt
    }

    /// Build every bar's OSD again from the current `[osd]` section.
    pub fn reconfigure_osd(&self) {
        let config = self.config.current();
        for bar in self.bars.borrow_mut().values_mut() {
            bar.reconfigure_osd(&config);
        }
    }

    /// Show, hide, or flip every bar, returning whether they are now visible.
    ///
    /// All monitors together: `topbar bar toggle` is bound to a key, and a key
    /// that hid the bar on one screen and not the others would be a bug rather
    /// than a feature. A new monitor arriving while the bars are hidden builds
    /// its bar visible, which is [`Self::sync`]'s doing and is the safer of the
    /// two mistakes — a bar nobody can find is worse than one nobody asked for.
    pub fn set_bars_visible(&self, action: VisibilityAction) -> bool {
        let bars = self.bars.borrow();
        let visible = match action {
            VisibilityAction::Show => true,
            VisibilityAction::Hide => false,
            // Any bar still up means "hide"; the answer is the same on every
            // monitor afterwards either way.
            VisibilityAction::Toggle => !bars.values().any(BarWindow::is_visible),
        };
        for bar in bars.values() {
            bar.set_visible(visible);
        }
        info!(
            "{} bar(s) {}",
            bars.len(),
            if visible { "shown" } else { "hidden" }
        );
        visible
    }

    /// Queue a sync, replacing any sync already queued.
    ///
    /// The cancellation the hotplug path needs: a mode set emits several
    /// signals in a few milliseconds, and a second one arriving while the first
    /// is still pending must reconfigure the bars *once*, from the state the
    /// display settled in — never twice, and never from the state half way
    /// through. Taking the pending source is what makes that structural.
    fn schedule_sync(self: &Rc<Self>) {
        self.schedule_sync_in(SYNC_DEBOUNCE);
    }

    /// The same, at a delay of the caller's choosing.
    fn schedule_sync_in(self: &Rc<Self>, delay: std::time::Duration) {
        if let Some(pending) = self.pending_sync.borrow_mut().take() {
            pending.remove();
        }

        let manager = Rc::clone(self);
        let source = glib::timeout_add_local_once(delay, move || {
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
        for (model, id) in self.handlers.borrow_mut().drain(..) {
            model.disconnect(id);
            HANDLERS.with(|count| count.set(count.get() - 1));
        }
    }
}

/// Whether the compositor has finished telling GDK about this monitor.
///
/// Both halves matter: the connector name is the bar's identity across a
/// hotplug, and a geometry of zero means the mode has not been set yet.
fn is_ready(monitor: &gdk::Monitor) -> bool {
    monitor.connector().is_some() && monitor.geometry().width() > 0
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
