//! Exactly the BlueZ surface the panel touches, and nothing else.
//!
//! BlueZ publishes its whole world through `ObjectManager`: one call returns
//! every adapter, every device and every extra interface hanging off them, and
//! two signals say when that tree changes. So there is no "list devices"
//! method here — [`ObjectManagerProxy::get_managed_objects`] *is* the listing,
//! and the per-object interfaces below are only for the properties and the two
//! methods the panel actually uses.
//!
//! **No signals are declared.** `InterfacesAdded`, `InterfacesRemoved` and
//! `PropertiesChanged` all arrive through a single bus match rule in
//! [`super::task`], for the same reason the network service uses one: a laptop
//! in a café sees a dozen devices come and go, and a subscription per object
//! would mean setting up and tearing down a stream for each.

use std::collections::HashMap;

use zbus::zvariant::{OwnedObjectPath, OwnedValue};

/// One object's interfaces, and each interface's properties.
///
/// `a{sa{sv}}` in BlueZ's signatures.
pub(crate) type Interfaces = HashMap<String, HashMap<String, OwnedValue>>;

/// Every object BlueZ manages.
///
/// `a{oa{sa{sv}}}`.
pub(crate) type ManagedObjects = HashMap<OwnedObjectPath, Interfaces>;

/// The root of BlueZ's object tree.
#[zbus::proxy(
    interface = "org.freedesktop.DBus.ObjectManager",
    default_service = "org.bluez",
    default_path = "/"
)]
pub(crate) trait ObjectManager {
    /// Every adapter, every device, and every interface on them.
    fn get_managed_objects(&self) -> zbus::Result<ManagedObjects>;
}

/// One Bluetooth adapter — the radio in the machine.
///
/// Only the writes are here. Every *reading* — `Powered`, and every device
/// property below it — comes out of the object tree, which the task already
/// has to fetch to know what devices exist; asking for the same values a
/// second time, one property per round trip, would be the N+1 the plan names
/// as a defect elsewhere.
#[zbus::proxy(interface = "org.bluez.Adapter1", default_service = "org.bluez")]
pub(crate) trait Adapter {
    /// Switch the radio. This is the toggle.
    #[zbus(property)]
    fn set_powered(&self, powered: bool) -> zbus::Result<()>;

    /// Start looking for devices in range.
    ///
    /// Scoped to the *session*, not to the adapter: BlueZ counts discovery
    /// clients, so this only makes the radio transmit for as long as the panel
    /// keeps its own session open, and stopping it does not stop somebody
    /// else's. See [`super::task::World::set_discovery`] for what bounds the
    /// panel's.
    fn start_discovery(&self) -> zbus::Result<()>;

    /// Stop looking.
    fn stop_discovery(&self) -> zbus::Result<()>;
}

/// One remote device: the four things the panel does to one.
#[zbus::proxy(interface = "org.bluez.Device1", default_service = "org.bluez")]
pub(crate) trait Device {
    /// Connect every profile this device supports.
    fn connect(&self) -> zbus::Result<()>;

    /// Disconnect it.
    fn disconnect(&self) -> zbus::Result<()>;

    /// Pair with it.
    ///
    /// The call BlueZ answers the *agent* through: it stays outstanding for as
    /// long as the pairing takes, and somewhere in the middle of it BlueZ calls
    /// back into [`super::agent`] with the question the panel puts on screen.
    /// That is why the task never awaits this on its own loop — see
    /// [`super::task::World::begin_pair`].
    fn pair(&self) -> zbus::Result<()>;

    /// Let it connect itself from now on, without asking again.
    ///
    /// What GNOME's own pairing does at the end, and what makes a headset that
    /// was just paired reconnect when it comes out of its case rather than
    /// putting an authorization prompt up every time.
    #[zbus(property)]
    fn set_trusted(&self, trusted: bool) -> zbus::Result<()>;
}

/// Where a pairing agent signs up.
#[zbus::proxy(
    interface = "org.bluez.AgentManager1",
    default_service = "org.bluez",
    default_path = "/org/bluez"
)]
pub(crate) trait AgentManager {
    /// Register an agent object with a capability string.
    fn register_agent(
        &self,
        agent: &zbus::zvariant::ObjectPath<'_>,
        capability: &str,
    ) -> zbus::Result<()>;

    /// Ask to be the agent BlueZ uses for pairings nobody else claimed.
    ///
    /// This is what makes an *incoming* pairing — a phone asking to pair with
    /// this machine — reach the panel at all: BlueZ hands unattributed
    /// requests to the default agent and refuses the pairing when there is
    /// none. See [`super::agent`] for why the panel asks for it and when it
    /// does not.
    fn request_default_agent(&self, agent: &zbus::zvariant::ObjectPath<'_>) -> zbus::Result<()>;

    /// Stand down.
    fn unregister_agent(&self, agent: &zbus::zvariant::ObjectPath<'_>) -> zbus::Result<()>;
}

/// Interface names, spelled once.
pub(crate) mod names {
    /// The adapter interface.
    pub(crate) const ADAPTER: &str = "org.bluez.Adapter1";
    /// The device interface.
    pub(crate) const DEVICE: &str = "org.bluez.Device1";
    /// A device's battery, when it has one.
    ///
    /// Its own interface rather than a property on `Device1`, and it *arrives
    /// and departs* through `InterfacesAdded`/`InterfacesRemoved` rather than
    /// through a property change — a headset publishes it a second or two
    /// after connecting. That is the reason the task re-reads the whole tree
    /// on every signal instead of watching a fixed set of properties: the set
    /// is not fixed.
    pub(crate) const BATTERY: &str = "org.bluez.Battery1";
}

/// One property out of an interface dictionary, as whatever it is.
pub(crate) fn property<T>(interface: &HashMap<String, OwnedValue>, key: &str) -> Option<T>
where
    T: TryFrom<OwnedValue>,
{
    let value = interface.get(key)?;
    T::try_from(value.try_clone().ok()?).ok()
}
