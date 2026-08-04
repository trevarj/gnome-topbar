//! `org.kde.StatusNotifierWatcher`, served by the panel.
//!
//! There is supposed to be exactly one watcher on a session and it is supposed
//! to be whoever draws the tray. The panel therefore serves the interface and
//! asks for the name; if something else already holds it — another panel, a
//! desktop environment — the request is refused and the panel becomes an
//! ordinary host instead. It never *replaces* an existing watcher: taking the
//! name from a running tray would strand every item registered with it.
//!
//! The registry behind the interface is shared with [`super::task`], which is
//! what lets an item that quits be struck off and announced without the
//! interface having to know how items are followed.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use tracing::debug;
use zbus::message::Header;
use zbus::object_server::SignalEmitter;

/// The well-known name a watcher owns.
pub(super) const WATCHER_NAME: &str = "org.kde.StatusNotifierWatcher";
/// Where it serves its interface.
pub(super) const WATCHER_PATH: &str = "/StatusNotifierWatcher";
/// The protocol revision the panel implements.
const PROTOCOL_VERSION: i32 = 0;

/// What the watcher interface tells the task.
#[derive(Debug)]
pub(super) enum Registration {
    /// An application announced an item, by `bus_name` + object path.
    Item(String),
}

/// The item and host sets, shared between the interface and the task.
#[derive(Debug, Clone, Default)]
pub(super) struct Registry {
    items: Arc<Mutex<BTreeSet<String>>>,
    hosts: Arc<Mutex<BTreeSet<String>>>,
}

impl Registry {
    /// Add an item, reporting whether it was new.
    pub(super) fn add_item(&self, id: &str) -> bool {
        self.items
            .lock()
            .expect("the registry lock is never held across an await")
            .insert(id.to_string())
    }

    /// Strike an item off, reporting whether it was there.
    pub(super) fn remove_item(&self, id: &str) -> bool {
        self.items
            .lock()
            .expect("the registry lock is never held across an await")
            .remove(id)
    }

    /// Add a host, reporting whether it was new.
    fn add_host(&self, name: &str) -> bool {
        self.hosts
            .lock()
            .expect("the registry lock is never held across an await")
            .insert(name.to_string())
    }

    fn items(&self) -> Vec<String> {
        self.items
            .lock()
            .expect("the registry lock is never held across an await")
            .iter()
            .cloned()
            .collect()
    }

    fn has_host(&self) -> bool {
        !self
            .hosts
            .lock()
            .expect("the registry lock is never held across an await")
            .is_empty()
    }
}

/// The watcher interface itself.
pub(super) struct Watcher {
    registry: Registry,
    registrations: mpsc::Sender<Registration>,
}

impl Watcher {
    /// Build the interface over a shared registry.
    pub(super) fn new(registry: Registry, registrations: mpsc::Sender<Registration>) -> Self {
        Self {
            registry,
            registrations,
        }
    }
}

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl Watcher {
    /// An application announces an item.
    ///
    /// The argument may be a bus name, an object path, or the two run
    /// together, and all three are in the wild. Whatever arrives, the item is
    /// recorded under the *sender's unique name* plus the path: a well-known
    /// name can be released while the connection lives on, and an item the
    /// panel could no longer address would sit on the bar doing nothing.
    async fn register_status_notifier_item(
        &mut self,
        service: &str,
        #[zbus(header)] header: Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        let Some(sender) = header.sender() else {
            return Err(zbus::fdo::Error::InvalidArgs(
                "RegisterStatusNotifierItem needs a sender".into(),
            ));
        };
        let id = format!("{sender}{}", object_path(service));

        if !self.registry.add_item(&id) {
            // Applications re-register on reconnect, and some do it in bursts.
            // Saying so once is enough; saying so again would rebuild the bar.
            debug!("tray item {id} registered again");
            return Ok(());
        }

        debug!("tray item {id} registered");
        Self::status_notifier_item_registered(&emitter, &id).await?;
        let _ = self.registrations.send(Registration::Item(id)).await;
        Ok(())
    }

    /// Something announces that it wants to draw the items.
    async fn register_status_notifier_host(
        &mut self,
        service: &str,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        if !self.registry.add_host(service) {
            return Ok(());
        }
        debug!("tray host {service} registered");
        self.is_status_notifier_host_registered_changed(&emitter)
            .await?;
        Self::status_notifier_host_registered(&emitter).await?;
        Ok(())
    }

    /// Every item announced so far.
    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        self.registry.items()
    }

    /// Whether anything is drawing them.
    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        self.registry.has_host()
    }

    /// The protocol revision.
    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        PROTOCOL_VERSION
    }

    /// An item arrived.
    #[zbus(signal)]
    pub(super) async fn status_notifier_item_registered(
        emitter: &SignalEmitter<'_>,
        service: &str,
    ) -> zbus::Result<()>;

    /// An item went away.
    #[zbus(signal)]
    pub(super) async fn status_notifier_item_unregistered(
        emitter: &SignalEmitter<'_>,
        service: &str,
    ) -> zbus::Result<()>;

    /// Something started drawing the items.
    #[zbus(signal)]
    pub(super) async fn status_notifier_host_registered(
        emitter: &SignalEmitter<'_>,
    ) -> zbus::Result<()>;
}

/// The object path hidden in a `RegisterStatusNotifierItem` argument.
///
/// An application may send `/StatusNotifierItem`, `org.example.App`, or
/// `:1.42/org/ayatana/NotificationItem/thing`. Only the path is taken from it;
/// the bus name always comes from the sender.
fn object_path(service: &str) -> String {
    let service = service.trim();
    match service.find('/') {
        Some(index) => service[index..].to_string(),
        None => "/StatusNotifierItem".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_path_is_taken_from_whatever_shape_the_argument_has() {
        assert_eq!(object_path("/StatusNotifierItem"), "/StatusNotifierItem");
        assert_eq!(object_path("org.example.App"), "/StatusNotifierItem");
        assert_eq!(
            object_path(":1.72/org/ayatana/NotificationItem/dropbox_1"),
            "/org/ayatana/NotificationItem/dropbox_1"
        );
        assert_eq!(object_path(""), "/StatusNotifierItem");
        assert_eq!(object_path("  /Custom  "), "/Custom");
    }

    #[test]
    fn the_registry_only_reports_something_new_once() {
        let registry = Registry::default();
        assert!(registry.add_item(":1.2/StatusNotifierItem"));
        assert!(
            !registry.add_item(":1.2/StatusNotifierItem"),
            "a re-registration is not a new item"
        );
        assert_eq!(registry.items(), vec![":1.2/StatusNotifierItem"]);

        assert!(registry.remove_item(":1.2/StatusNotifierItem"));
        assert!(!registry.remove_item(":1.2/StatusNotifierItem"));
        assert!(registry.items().is_empty());
    }

    #[test]
    fn a_watcher_with_no_host_says_so() {
        let registry = Registry::default();
        assert!(!registry.has_host());
        assert!(registry.add_host("org.kde.StatusNotifierHost-1"));
        assert!(!registry.add_host("org.kde.StatusNotifierHost-1"));
        assert!(registry.has_host());
    }
}
