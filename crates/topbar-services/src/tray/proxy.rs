//! The three tray interfaces, trimmed to what the panel uses.
//!
//! Hand-written rather than taken from a crate. The `system-tray` crate was
//! spiked against a fake application on a private bus first and turned down:
//! it reads a pixmap's height out of the width field (a 22x16 icon arrives as
//! 22x22), it offers no way to send `Scroll`, it sends `Activate` to
//! `/StatusNotifierItem` whatever path the item actually lives at, and its
//! client can only ever connect to `$DBUS_SESSION_BUS_ADDRESS`, which no test
//! in this crate is allowed to touch.
//!
//! Signals are read with [`zbus::Proxy::receive_all_signals`] rather than as
//! seven typed streams: every `New*` signal means the same thing to the panel —
//! *ask again* — and applications emit ones that are not in the specification.
//! One stream per item, one re-read per burst.

use std::collections::HashMap;

use serde::Deserialize;
use zbus::zvariant::{OwnedValue, Type, Value};

/// The item interface: what an application shows, and what may be done to it.
#[zbus::proxy(interface = "org.kde.StatusNotifierItem", assume_defaults = false)]
pub(crate) trait StatusNotifierItem {
    /// Primary activation, at a screen position the item may use as a hint.
    fn activate(&self, x: i32, y: i32) -> zbus::Result<()>;

    /// Secondary activation — the middle button.
    fn secondary_activate(&self, x: i32, y: i32) -> zbus::Result<()>;

    /// Ask the application to show its own menu.
    ///
    /// The fallback for an item that declares no dbusmenu object but does
    /// declare `ItemIsMenu`; the application then puts up a menu of its own.
    fn context_menu(&self, x: i32, y: i32) -> zbus::Result<()>;

    /// A scroll over the icon. `orientation` is `vertical` or `horizontal`.
    fn scroll(&self, delta: i32, orientation: &str) -> zbus::Result<()>;
}

/// The watcher, as a client sees it.
#[zbus::proxy(
    default_service = "org.kde.StatusNotifierWatcher",
    default_path = "/StatusNotifierWatcher",
    interface = "org.kde.StatusNotifierWatcher"
)]
pub(crate) trait StatusNotifierWatcher {
    /// Announce an item. `service` is a bus name, an object path, or both.
    fn register_status_notifier_item(&self, service: &str) -> zbus::Result<()>;

    /// Announce that something wants to draw the items.
    fn register_status_notifier_host(&self, service: &str) -> zbus::Result<()>;

    /// Every item registered so far, as `bus_name` or `bus_name/path`.
    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> zbus::Result<Vec<String>>;

    /// An item arrived.
    #[zbus(signal)]
    fn status_notifier_item_registered(&self, service: &str) -> zbus::Result<()>;

    /// An item went away.
    #[zbus(signal)]
    fn status_notifier_item_unregistered(&self, service: &str) -> zbus::Result<()>;
}

/// One node of a dbusmenu layout, exactly as it arrives: `(ia{sv}av)`.
///
/// `children` stays untyped because the structure is recursive and D-Bus has
/// no way to say so; [`super::menu`] walks it once, at the edge.
#[derive(Debug, Deserialize, Type)]
pub(crate) struct RawNode {
    /// The item id `Event` and `AboutToShow` are addressed by.
    pub id: i32,
    /// `label`, `enabled`, `toggle-type` and the rest.
    pub properties: HashMap<String, OwnedValue>,
    /// Child nodes, each a `RawNode` inside a variant.
    pub children: Vec<OwnedValue>,
}

/// The menu interface an item points at through its `Menu` property.
#[zbus::proxy(interface = "com.canonical.dbusmenu", assume_defaults = false)]
pub(crate) trait DBusMenu {
    /// Tell the application a menu is about to open, so it can refresh it.
    ///
    /// Returns whether the layout changed. Plenty of applications do not
    /// implement it at all, which is why its failure is never fatal.
    fn about_to_show(&self, id: i32) -> zbus::Result<bool>;

    /// Report that something happened to an item: `clicked`, `hovered`.
    fn event(&self, id: i32, event_id: &str, data: &Value<'_>, timestamp: u32) -> zbus::Result<()>;

    /// The layout under `parent_id`, to `recursion_depth` (-1 for all of it).
    ///
    /// An empty `property_names` asks for every property the application has,
    /// which is one round trip instead of one per property the panel forgot.
    fn get_layout(
        &self,
        parent_id: i32,
        recursion_depth: i32,
        property_names: &[&str],
    ) -> zbus::Result<(u32, RawNode)>;
}
