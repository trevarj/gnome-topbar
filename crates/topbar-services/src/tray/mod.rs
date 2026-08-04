//! The system tray: `StatusNotifierItem` hosting and `com.canonical.dbusmenu`.
//!
//! ```text
//!   proxy.rs      the three interfaces, trimmed
//!   props.rs      the item property dictionary, read once   (pure)
//!   model.rs      the published snapshot and icon choice    (pure)
//!   menu.rs       the dbusmenu layout, parsed once          (pure)
//!   watcher.rs    org.kde.StatusNotifierWatcher, served
//!   task.rs       the one owner of all of it
//! ```
//!
//! The panel serves the watcher itself when nothing else does, so a bare niri
//! session with no desktop environment behind it still has a tray. When
//! something else already holds the name the panel joins as an ordinary host
//! instead — it never takes the name away from a running tray.
//!
//! **This is hand-written rather than the `system-tray` crate**, which was
//! spiked first against a fake application on a private bus and turned down on
//! four counts: it reads a pixmap's height out of the width field, so every
//! non-square icon arrives square with a mis-sized buffer; it exposes no way to
//! send `Scroll`; it sends `Activate` to `/StatusNotifierItem` whatever path
//! the item is actually served at; and its client can only ever connect to
//! `$DBUS_SESSION_BUS_ADDRESS`, which nothing in this crate is allowed to
//! touch. See the module tests, several of which name the behaviour they
//! exist to protect.

mod menu;
mod model;
mod props;
mod proxy;
mod task;
mod watcher;

#[cfg(any(test, feature = "fake-sni"))]
pub mod fake;

#[cfg(test)]
mod bus_tests;

use std::sync::Arc;

use tokio::sync::{mpsc, oneshot, watch};

use crate::error::SvcError;

pub use menu::{MenuEvent, MenuKind, MenuNode, ToggleKind, ToggleState};
pub use model::{FALLBACK_ICON, IconView, ItemView, Pixmap, ScrollAxis, Status, TrayState};

use task::Command;

/// How many commands may be in flight before a sender waits.
const QUEUE: usize = 32;

/// The size a pixmap icon is chosen for when the configuration says nothing.
///
/// The same 18px the rest of the panel's symbolic icons are drawn at.
pub const DEFAULT_ICON_SIZE: i32 = 18;

/// The tray service.
///
/// Cloning is cheap — a channel sender and a watch subscription — so every
/// widget that wants the tray can hold its own copy.
#[derive(Clone)]
pub struct Tray {
    handle: TrayHandle,
    state: watch::Receiver<Arc<TrayState>>,
}

impl Tray {
    /// Start hosting the tray.
    ///
    /// `target_size` is the pixel size pixmap icons are chosen for, from
    /// `widgets.tray.pixmap_icon_size`. `address` overrides the session bus;
    /// production passes `None` and the tests pass a private bus, which is what
    /// keeps a test run from taking the desktop's tray away from it.
    pub(crate) fn start(target_size: i32, address: Option<String>) -> Self {
        let (commands, queue) = mpsc::channel(QUEUE);
        let (publisher, state) = watch::channel(Arc::new(TrayState::default()));
        tokio::spawn(task::run(queue, publisher, target_size, address));
        Self {
            handle: TrayHandle { commands },
            state,
        }
    }

    /// The handle commands are sent through.
    pub fn handle(&self) -> &TrayHandle {
        &self.handle
    }

    /// Subscribe to the tray.
    pub fn state(&self) -> watch::Receiver<Arc<TrayState>> {
        self.state.clone()
    }
}

/// What the panel may ask of a tray item.
///
/// Every call returns a `Result` so a failure is reported rather than dropped
/// in a click handler — see `bridge::act` on the GTK side.
#[derive(Clone)]
pub struct TrayHandle {
    commands: mpsc::Sender<Command>,
}

impl TrayHandle {
    /// Primary activation: what a left click means.
    pub async fn activate(&self, id: &str) -> Result<(), SvcError> {
        self.ask(|reply| Command::Activate(id.to_string(), reply))
            .await
    }

    /// Secondary activation: what a middle click means.
    pub async fn secondary_activate(&self, id: &str) -> Result<(), SvcError> {
        self.ask(|reply| Command::SecondaryActivate(id.to_string(), reply))
            .await
    }

    /// A scroll over the icon, in notches.
    pub async fn scroll(&self, id: &str, delta: i32, axis: ScrollAxis) -> Result<(), SvcError> {
        self.ask(|reply| Command::Scroll(id.to_string(), delta, axis, reply))
            .await
    }

    /// The item's menu, ready to draw.
    ///
    /// Fetched fresh every time: a tray menu is built by the application when
    /// it is asked for, and a remembered one would show the state of the last
    /// time the user looked.
    pub async fn menu(&self, id: &str) -> Result<MenuNode, SvcError> {
        self.ask(|reply| Command::Menu(id.to_string(), reply)).await
    }

    /// Tell the application something happened to one of its menu rows.
    pub async fn menu_event(
        &self,
        id: &str,
        item_id: i32,
        event: MenuEvent,
    ) -> Result<(), SvcError> {
        self.ask(|reply| Command::MenuEvent(id.to_string(), item_id, event, reply))
            .await
    }

    /// Post a command and wait for the task's answer.
    async fn ask<T>(
        &self,
        build: impl FnOnce(oneshot::Sender<Result<T, SvcError>>) -> Command,
    ) -> Result<T, SvcError> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(build(reply))
            .await
            .map_err(|_| SvcError::ServiceStopped("tray"))?;
        answer.await.map_err(|_| SvcError::ServiceStopped("tray"))?
    }
}
