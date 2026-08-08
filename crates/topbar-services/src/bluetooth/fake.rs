//! A BlueZ that exists to be talked to.
//!
//! Test support only: behind `cfg(test)` for the bus tests and behind the
//! `fake-bluez` feature for `topbar-fake-bluez`, the sidecar the nested-niri
//! smoke run puts on its private bus. The packaged panel contains none of it.
//!
//! It has to be a fairly complete BlueZ, because the interesting things here
//! are conversations rather than calls. An external pairing is BlueZ calling
//! *back* into the panel's own agent and waiting on a delayed reply; a fake
//! that could not make that call would leave the whole agent path untested.
//!
//! Nothing here ever touches the machine's real adapter. That is the point: the
//! real BlueZ is on the system bus, it is the developer's headphones, and no
//! screenshot is worth disconnecting somebody's music to take.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use tracing::debug;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

/// The well-known name.
pub const BLUEZ_NAME: &str = "org.bluez";
/// Where the object manager lives.
pub const ROOT_PATH: &str = "/";
/// Where the agent manager lives.
pub const AGENT_MANAGER_PATH: &str = "/org/bluez";
/// The one adapter the fake has.
pub const ADAPTER_PATH: &str = "/org/bluez/hci0";
/// Where the fake's own control interface lives.
pub const CONTROL_PATH: &str = "/io/github/trevarj/topbar/FakeBluez1";

/// The adapter interface.
const ADAPTER_IFACE: &str = "org.bluez.Adapter1";
/// The device interface.
const DEVICE_IFACE: &str = "org.bluez.Device1";
/// The battery interface.
const BATTERY_IFACE: &str = "org.bluez.Battery1";
/// The interface a registered agent serves.
const AGENT_IFACE: &str = "org.bluez.Agent1";

/// How long a "slow" connect takes before it settles.
const SLOW_CONNECT: std::time::Duration = std::time::Duration::from_secs(30);

/// One device the fake knows about.
#[derive(Debug, Clone)]
pub struct FakeDevice {
    /// The name to draw.
    pub alias: String,
    /// Its address.
    pub address: String,
    /// BlueZ's own icon name for what kind of thing it is.
    pub icon: String,
    /// Whether it is paired.
    pub paired: bool,
    /// Whether it may connect itself without asking again.
    pub trusted: bool,
    /// Whether it is connected.
    pub connected: bool,
    /// Its battery, when it publishes one.
    pub battery: Option<u8>,
}

impl FakeDevice {
    /// A paired device of a given kind.
    pub fn paired(alias: &str, address: &str, icon: &str) -> Self {
        Self {
            alias: alias.to_string(),
            address: address.to_string(),
            icon: icon.to_string(),
            paired: true,
            trusted: true,
            connected: false,
            battery: None,
        }
    }

    /// The same, connected.
    #[must_use]
    pub fn connected(mut self) -> Self {
        self.connected = true;
        self
    }

    /// The same, publishing a battery level.
    #[must_use]
    pub fn with_battery(mut self, percent: u8) -> Self {
        self.battery = Some(percent);
        self
    }

    /// The same, never paired — so the panel lists it only while it is looking.
    #[must_use]
    pub fn unpaired(mut self) -> Self {
        self.paired = false;
        self.trusted = false;
        self
    }

    /// The same, with nothing but its address to call itself.
    ///
    /// What BlueZ publishes for a device that has not answered a name request:
    /// an alias built out of the address, with dashes.
    #[must_use]
    pub fn nameless(mut self) -> Self {
        self.alias = self.address.replace(':', "-");
        self
    }
}

/// What the next `Connect` should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Connect straight away.
    Success,
    /// Refuse, the way a device that is switched off does.
    Fail,
    /// Take [`SLOW_CONNECT`] about it, so the row's spinner is photographable.
    Slow,
}

impl Outcome {
    /// Parse the word a driver spells it with.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "success" => Some(Self::Success),
            "fail" => Some(Self::Fail),
            "slow" => Some(Self::Slow),
            _ => None,
        }
    }
}

/// Everything the fake is, behind one lock.
#[derive(Debug, Default)]
struct Inner {
    has_adapter: bool,
    powered: bool,
    /// Whether a discovery session is open.
    discovering: bool,
    /// Devices by object path.
    devices: BTreeMap<String, FakeDevice>,
    /// Every method the panel called, in order.
    calls: Vec<String>,
    /// Every agent path and capability that registered.
    agents: Vec<String>,
    /// The unique bus name the agent registered from, and its object path.
    ///
    /// Recorded from the registration's own message header, which is exactly
    /// how BlueZ knows whom to call back: an agent lives on the *registering
    /// peer's* connection, not on a well-known name of its own.
    agent_owner: Option<(String, String)>,
    /// Every answer an agent gave a pairing question.
    replies: Vec<String>,
    /// What the next connects should do.
    outcomes: VecDeque<Outcome>,
}

/// A BlueZ of a test's own.
#[derive(Debug)]
pub struct Bluez(Mutex<Inner>);

/// Take the lock, surviving a poisoning.
fn lock(bluez: &Bluez) -> MutexGuard<'_, Inner> {
    bluez.0.lock().unwrap_or_else(|poison| poison.into_inner())
}

impl Bluez {
    /// A fake with one powered adapter and nothing paired to it.
    pub fn new() -> Arc<Self> {
        Arc::new(Self(Mutex::new(Inner {
            has_adapter: true,
            powered: true,
            ..Inner::default()
        })))
    }

    /// Whether there is an adapter at all.
    pub fn set_has_adapter(&self, present: bool) {
        lock(self).has_adapter = present;
    }

    /// Whether the radio is on, without recording a call.
    pub fn set_powered(&self, powered: bool) {
        lock(self).powered = powered;
    }

    /// Whether the radio is on.
    pub fn powered(&self) -> bool {
        lock(self).powered
    }

    /// Whether a discovery session is open right now.
    pub fn discovering(&self) -> bool {
        lock(self).discovering
    }

    /// Whether a device is paired, as the fake sees it.
    pub fn is_paired(&self, name: &str) -> bool {
        lock(self)
            .devices
            .get(&device_path(name))
            .is_some_and(|device| device.paired)
    }

    /// Whether a device is trusted.
    pub fn is_trusted(&self, name: &str) -> bool {
        lock(self)
            .devices
            .get(&device_path(name))
            .is_some_and(|device| device.trusted)
    }

    /// Put a device in the tree before anything is serving it.
    pub fn seed_device(&self, name: &str, device: FakeDevice) {
        lock(self).devices.insert(device_path(name), device);
    }

    /// Queue what the next `Connect` should do.
    pub fn queue(&self, outcome: Outcome) {
        lock(self).outcomes.push_back(outcome);
    }

    /// Every method the panel called, in order.
    pub fn calls(&self) -> Vec<String> {
        lock(self).calls.clone()
    }

    /// Whether the panel called something matching `needle`.
    pub fn called(&self, needle: &str) -> bool {
        lock(self).calls.iter().any(|call| call.contains(needle))
    }

    /// Every agent that registered, as `path capability`.
    pub fn agents(&self) -> Vec<String> {
        lock(self).agents.clone()
    }

    /// Every answer an agent gave, as `method result`.
    pub fn replies(&self) -> Vec<String> {
        lock(self).replies.clone()
    }

    /// Record that the panel called something.
    fn record(&self, method: &str) {
        debug!("fake bluez: {method}");
        lock(self).calls.push(method.to_string());
    }
}

/// The object path of a device the driver names `name`.
fn device_path(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("{ADAPTER_PATH}/dev_{safe}")
}

/// A property dictionary as it travels on the bus.
type Properties = HashMap<String, OwnedValue>;

/// One object's interfaces.
type Interfaces = HashMap<String, Properties>;

/// Everything under the root.
type Managed = HashMap<OwnedObjectPath, Interfaces>;

/// Own a value, or drop the entry.
fn own(value: Value<'_>) -> Option<OwnedValue> {
    value.try_to_owned().ok()
}

/// The interfaces one device publishes.
fn device_interfaces(device: &FakeDevice) -> Interfaces {
    let mut properties = Properties::new();
    for (key, value) in [
        ("Alias", Value::from(device.alias.clone())),
        ("Address", Value::from(device.address.clone())),
        ("Icon", Value::from(device.icon.clone())),
        ("Paired", Value::from(device.paired)),
        ("Connected", Value::from(device.connected)),
        ("Trusted", Value::from(device.trusted)),
    ] {
        if let Some(value) = own(value) {
            properties.insert(key.to_string(), value);
        }
    }

    let mut interfaces = Interfaces::new();
    interfaces.insert(DEVICE_IFACE.to_string(), properties);
    if let Some(percent) = device.battery
        && let Some(value) = own(Value::from(percent))
    {
        interfaces.insert(
            BATTERY_IFACE.to_string(),
            HashMap::from([("Percentage".to_string(), value)]),
        );
    }
    interfaces
}

/// The object manager: the whole tree, in one call.
struct Manager {
    bluez: Arc<Bluez>,
}

#[zbus::interface(name = "org.freedesktop.DBus.ObjectManager")]
impl Manager {
    /// Every adapter and every device.
    async fn get_managed_objects(&self) -> Managed {
        self.bluez.record("GetManagedObjects");
        let inner = lock(&self.bluez);
        let mut managed = Managed::new();
        if !inner.has_adapter {
            return managed;
        }

        let mut adapter = Properties::new();
        if let Some(value) = own(Value::from(inner.powered)) {
            adapter.insert("Powered".to_string(), value);
        }
        if let Some(value) = own(Value::from(inner.discovering)) {
            adapter.insert("Discovering".to_string(), value);
        }
        if let Ok(path) = OwnedObjectPath::try_from(ADAPTER_PATH) {
            managed.insert(path, HashMap::from([(ADAPTER_IFACE.to_string(), adapter)]));
        }

        for (path, device) in &inner.devices {
            if let Ok(path) = OwnedObjectPath::try_from(path.as_str()) {
                managed.insert(path, device_interfaces(device));
            }
        }
        managed
    }
}

/// The adapter.
struct Adapter {
    bluez: Arc<Bluez>,
}

#[zbus::interface(name = "org.bluez.Adapter1")]
impl Adapter {
    /// Whether the radio is on.
    #[zbus(property)]
    fn powered(&self) -> bool {
        lock(&self.bluez).powered
    }

    /// Switch it. Recorded, because "the panel did not write this" is the
    /// assertion the read-only policy test makes.
    #[zbus(property)]
    fn set_powered(&self, powered: bool) {
        self.bluez.record(&format!("Adapter1.Powered={powered}"));
        lock(&self.bluez).powered = powered;
    }

    /// Whether a scan is running.
    #[zbus(property)]
    fn discovering(&self) -> bool {
        lock(&self.bluez).discovering
    }

    /// Start looking. Recorded, because "a read-only panel did not make this
    /// adapter transmit" is an assertion the policy test makes.
    async fn start_discovery(&self, #[zbus(connection)] bus: &zbus::Connection) {
        self.bluez.record("StartDiscovery");
        lock(&self.bluez).discovering = true;
        changed(
            bus,
            ADAPTER_PATH,
            ADAPTER_IFACE,
            [("Discovering", Value::from(true))],
        )
        .await;
    }

    /// Stop looking.
    async fn stop_discovery(&self, #[zbus(connection)] bus: &zbus::Connection) {
        self.bluez.record("StopDiscovery");
        lock(&self.bluez).discovering = false;
        changed(
            bus,
            ADAPTER_PATH,
            ADAPTER_IFACE,
            [("Discovering", Value::from(false))],
        )
        .await;
    }
}

/// One device.
struct Device {
    bluez: Arc<Bluez>,
    path: String,
}

#[zbus::interface(name = "org.bluez.Device1")]
impl Device {
    /// Connect it, doing whatever the next queued outcome says.
    async fn connect(&self, #[zbus(connection)] bus: &zbus::Connection) -> zbus::fdo::Result<()> {
        self.bluez.record(&format!("Connect {}", self.path));
        let outcome = lock(&self.bluez)
            .outcomes
            .pop_front()
            .unwrap_or(Outcome::Success);

        if outcome == Outcome::Slow {
            // Long enough for the row's spinner to be photographed, and the
            // call stays outstanding the whole time — which is what a real
            // device in a drawer does.
            tokio::time::sleep(SLOW_CONNECT).await;
        }
        if outcome == Outcome::Fail {
            return Err(zbus::fdo::Error::Failed(
                "br-connection-canceled".to_string(),
            ));
        }

        if let Some(device) = lock(&self.bluez).devices.get_mut(&self.path) {
            device.connected = true;
        }
        changed(
            bus,
            &self.path,
            DEVICE_IFACE,
            [("Connected", Value::from(true))],
        )
        .await;
        Ok(())
    }

    /// Disconnect it.
    async fn disconnect(&self, #[zbus(connection)] bus: &zbus::Connection) {
        self.bluez.record(&format!("Disconnect {}", self.path));
        if let Some(device) = lock(&self.bluez).devices.get_mut(&self.path) {
            device.connected = false;
        }
        changed(
            bus,
            &self.path,
            DEVICE_IFACE,
            [("Connected", Value::from(false))],
        )
        .await;
    }

    /// Whether it is connected.
    #[zbus(property)]
    fn connected(&self) -> bool {
        lock(&self.bluez)
            .devices
            .get(&self.path)
            .is_some_and(|device| device.connected)
    }

    /// Whether it is paired.
    #[zbus(property)]
    fn paired(&self) -> bool {
        lock(&self.bluez)
            .devices
            .get(&self.path)
            .is_some_and(|device| device.paired)
    }

    /// Pair with it, asking the registered agent first.
    ///
    /// This is the shape that matters, and the reason it is worth a fake at
    /// all: BlueZ calls *back* into the panel's own agent while `Pair` is
    /// outstanding. A panel that awaited `Pair` on the task loop that has to
    /// deliver that question would deadlock, and only a fake that makes the
    /// call can show it does not.
    async fn pair(&self, #[zbus(connection)] bus: &zbus::Connection) -> zbus::fdo::Result<()> {
        self.bluez.record(&format!("Pair {}", self.path));
        let agent = lock(&self.bluez).agent_owner.clone();
        if let Some((owner, agent_path)) = agent {
            ask_agent(bus, &owner, &agent_path, &self.path, PAIR_PASSKEY)
                .await
                .map_err(|error| {
                    zbus::fdo::Error::Failed(format!(
                        "org.bluez.Error.AuthenticationCanceled: {error}"
                    ))
                })?;
        }
        if let Some(device) = lock(&self.bluez).devices.get_mut(&self.path) {
            device.paired = true;
        }
        changed(
            bus,
            &self.path,
            DEVICE_IFACE,
            [("Paired", Value::from(true))],
        )
        .await;
        Ok(())
    }

    /// Whether it may connect itself without asking again.
    #[zbus(property)]
    fn trusted(&self) -> bool {
        lock(&self.bluez)
            .devices
            .get(&self.path)
            .is_some_and(|device| device.trusted)
    }

    /// Trust it. Recorded, so a test can say the pairing finished the job.
    #[zbus(property)]
    fn set_trusted(&self, trusted: bool) {
        self.bluez
            .record(&format!("Device1.Trusted={trusted} {}", self.path));
        if let Some(device) = lock(&self.bluez).devices.get_mut(&self.path) {
            device.trusted = trusted;
        }
    }
}

/// The passkey the fake's own pairings show.
///
/// Distinctive rather than round: a test asserting on "000000" would pass
/// against a default nobody set.
pub const PAIR_PASSKEY: u32 = 731_509;

/// Ask an agent to confirm a passkey, and wait for the answer.
async fn ask_agent(
    bus: &zbus::Connection,
    owner: &str,
    agent_path: &str,
    device: &str,
    passkey: u32,
) -> zbus::Result<()> {
    let object = OwnedObjectPath::try_from(device)?;
    let agent = OwnedObjectPath::try_from(agent_path)?;
    let proxy = zbus::Proxy::new(
        bus,
        zbus::names::BusName::try_from(owner.to_string())?,
        agent,
        AGENT_IFACE,
    )
    .await?;
    proxy
        .call_method("RequestConfirmation", &(&object, passkey))
        .await?;
    Ok(())
}

/// Where agents sign up.
struct AgentManager {
    bluez: Arc<Bluez>,
}

#[zbus::interface(name = "org.bluez.AgentManager1")]
impl AgentManager {
    /// Take an agent's registration, and remember what it promised.
    ///
    /// The sender's unique name comes off the message header, because that is
    /// the only way to call the agent back: it is served on the registering
    /// peer's own connection.
    fn register_agent(
        &self,
        agent: OwnedObjectPath,
        capability: String,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) {
        self.bluez
            .record(&format!("RegisterAgent {} {capability}", agent.as_str()));
        let mut inner = lock(&self.bluez);
        inner
            .agents
            .push(format!("{} {capability}", agent.as_str()));
        if let Some(sender) = header.sender() {
            inner.agent_owner = Some((sender.to_string(), agent.as_str().to_string()));
        }
    }

    /// Make it the one that answers unattributed pairings.
    fn request_default_agent(&self, agent: OwnedObjectPath) {
        self.bluez
            .record(&format!("RequestDefaultAgent {}", agent.as_str()));
    }

    /// Withdraw it.
    fn unregister_agent(&self, agent: OwnedObjectPath) {
        self.bluez
            .record(&format!("UnregisterAgent {}", agent.as_str()));
    }
}

/// Emit `PropertiesChanged` for one object.
async fn changed<'a, I>(bus: &zbus::Connection, path: &str, interface: &str, properties: I)
where
    I: IntoIterator<Item = (&'a str, Value<'a>)>,
{
    let changed: HashMap<&str, Value<'_>> = properties.into_iter().collect();
    let empty: Vec<&str> = Vec::new();
    let _ = bus
        .emit_signal(
            None::<&str>,
            path,
            "org.freedesktop.DBus.Properties",
            "PropertiesChanged",
            &(interface, changed, empty),
        )
        .await;
}

/// Announce a new object.
async fn interfaces_added(bus: &zbus::Connection, path: &str, interfaces: Interfaces) {
    let Ok(object) = ObjectPath::try_from(path) else {
        return;
    };
    let _ = bus
        .emit_signal(
            None::<&str>,
            ROOT_PATH,
            "org.freedesktop.DBus.ObjectManager",
            "InterfacesAdded",
            &(&object, interfaces),
        )
        .await;
}

/// Announce that one went away.
async fn interfaces_removed(bus: &zbus::Connection, path: &str, interfaces: Vec<String>) {
    let Ok(object) = ObjectPath::try_from(path) else {
        return;
    };
    let _ = bus
        .emit_signal(
            None::<&str>,
            ROOT_PATH,
            "org.freedesktop.DBus.ObjectManager",
            "InterfacesRemoved",
            &(&object, interfaces),
        )
        .await;
}

/// Serve one device object.
async fn serve_device(bus: &zbus::Connection, bluez: &Arc<Bluez>, path: &str) {
    let _ = bus
        .object_server()
        .at(
            path,
            Device {
                bluez: Arc::clone(bluez),
                path: path.to_string(),
            },
        )
        .await;
}

/// The fake's own control interface, for a driver with no pointer.
struct Control {
    bluez: Arc<Bluez>,
    quit: tokio::sync::mpsc::Sender<()>,
}

#[zbus::interface(name = "io.github.trevarj.topbar.FakeBluez1")]
impl Control {
    /// Pair a new device to the adapter.
    async fn add_device(
        &self,
        name: String,
        alias: String,
        address: String,
        icon: String,
        #[zbus(connection)] bus: &zbus::Connection,
    ) {
        let device = FakeDevice::paired(&alias, &address, &icon);
        let path = device_path(&name);
        let interfaces = device_interfaces(&device);
        lock(&self.bluez).devices.insert(path.clone(), device);
        serve_device(bus, &self.bluez, &path).await;
        interfaces_added(bus, &path, interfaces).await;
    }

    /// Put a device in range that nobody has paired, as a scan would find it.
    ///
    /// Announced through `InterfacesAdded` exactly like a paired one: from
    /// BlueZ's side a discovered device *is* a new object in the tree, and
    /// whether the panel draws it is the panel's own filter to apply.
    async fn add_nearby_device(
        &self,
        name: String,
        alias: String,
        address: String,
        icon: String,
        #[zbus(connection)] bus: &zbus::Connection,
    ) {
        let device = FakeDevice::paired(&alias, &address, &icon).unpaired();
        let path = device_path(&name);
        let interfaces = device_interfaces(&device);
        lock(&self.bluez).devices.insert(path.clone(), device);
        serve_device(bus, &self.bluez, &path).await;
        interfaces_added(bus, &path, interfaces).await;
    }

    /// Take one away.
    async fn remove_device(&self, name: String, #[zbus(connection)] bus: &zbus::Connection) {
        let path = device_path(&name);
        lock(&self.bluez).devices.remove(&path);
        let _ = bus.object_server().remove::<Device, _>(path.as_str()).await;
        interfaces_removed(bus, &path, vec![DEVICE_IFACE.to_string()]).await;
    }

    /// Connect or disconnect one from outside, as a device's own button would.
    async fn set_connected(
        &self,
        name: String,
        connected: bool,
        #[zbus(connection)] bus: &zbus::Connection,
    ) {
        let path = device_path(&name);
        if let Some(device) = lock(&self.bluez).devices.get_mut(&path) {
            device.connected = connected;
        }
        changed(
            bus,
            &path,
            DEVICE_IFACE,
            [("Connected", Value::from(connected))],
        )
        .await;
    }

    /// Give a device a battery, or move the one it has.
    ///
    /// The interface *arrives* rather than changing, which is what a headset
    /// does a second or two after connecting — and the reason the panel reads
    /// the whole tree instead of watching a fixed set of properties.
    async fn set_battery(
        &self,
        name: String,
        percent: u8,
        #[zbus(connection)] bus: &zbus::Connection,
    ) {
        let path = device_path(&name);
        let interfaces = {
            let mut inner = lock(&self.bluez);
            let Some(device) = inner.devices.get_mut(&path) else {
                return;
            };
            device.battery = Some(percent);
            device_interfaces(device)
        };
        interfaces_added(bus, &path, interfaces).await;
    }

    /// Switch the radio from outside, as `bluetoothctl power off` would.
    async fn set_powered(&self, powered: bool, #[zbus(connection)] bus: &zbus::Connection) {
        lock(&self.bluez).powered = powered;
        changed(
            bus,
            ADAPTER_PATH,
            ADAPTER_IFACE,
            [("Powered", Value::from(powered))],
        )
        .await;
    }

    /// Whether there is an adapter at all.
    async fn set_has_adapter(&self, present: bool, #[zbus(connection)] bus: &zbus::Connection) {
        lock(&self.bluez).has_adapter = present;
        if present {
            interfaces_added(bus, ADAPTER_PATH, HashMap::new()).await;
        } else {
            interfaces_removed(bus, ADAPTER_PATH, vec![ADAPTER_IFACE.to_string()]).await;
        }
    }

    /// Say what the next `Connect` should do.
    fn queue_connect_outcome(&self, outcome: String) -> zbus::fdo::Result<()> {
        let outcome = Outcome::parse(&outcome)
            .ok_or_else(|| zbus::fdo::Error::InvalidArgs(format!("no such outcome: {outcome}")))?;
        self.bluez.queue(outcome);
        Ok(())
    }

    /// Start a pairing from the *other* side, the way a phone does.
    ///
    /// Calls `RequestConfirmation` on whichever agent registered, waits for the
    /// answer, and records it. This is the whole reason the fake exists: it is
    /// the one path that cannot be exercised without something calling back
    /// into the panel.
    async fn trigger_confirmation(
        &self,
        name: String,
        passkey: u32,
        #[zbus(connection)] bus: &zbus::Connection,
    ) -> zbus::fdo::Result<()> {
        let Some((owner, path)) = lock(&self.bluez).agent_owner.clone() else {
            return Err(zbus::fdo::Error::Failed("no agent registered".to_string()));
        };
        let device = device_path(&name);
        let bluez = Arc::clone(&self.bluez);
        let bus = bus.clone();

        // Spawned, because the agent's reply is delayed for as long as the row
        // is on screen and this call must return so the driver can carry on.
        tokio::spawn(async move {
            let Ok(object) = OwnedObjectPath::try_from(device.as_str()) else {
                return;
            };
            let Ok(agent_path) = OwnedObjectPath::try_from(path.as_str()) else {
                return;
            };
            let answer: zbus::Result<()> = async {
                let proxy = zbus::Proxy::new(
                    &bus,
                    zbus::names::BusName::try_from(owner)?,
                    agent_path,
                    AGENT_IFACE,
                )
                .await?;
                proxy
                    .call_method("RequestConfirmation", &(&object, passkey))
                    .await?;
                Ok(())
            }
            .await;

            let recorded = match answer {
                Ok(()) => "RequestConfirmation confirmed".to_string(),
                Err(error) => format!("RequestConfirmation refused: {error}"),
            };
            debug!("fake bluez: {recorded}");
            lock(&bluez).replies.push(recorded);
        });
        Ok(())
    }

    /// Every method the panel called, in order.
    fn calls(&self) -> Vec<String> {
        self.bluez.calls()
    }

    /// Every agent that registered, as `path capability`.
    fn agents(&self) -> Vec<String> {
        self.bluez.agents()
    }

    /// Every answer an agent gave a pairing question.
    fn replies(&self) -> Vec<String> {
        self.bluez.replies()
    }

    /// Stop.
    async fn quit(&self) {
        let _ = self.quit.send(()).await;
    }
}

/// Everything the fake serves, held for as long as it should exist.
pub struct Served {
    /// The bus connection the name and the objects live on.
    pub connection: zbus::Connection,
    /// The state, for a test that wants to look at it directly.
    pub bluez: Arc<Bluez>,
    /// Resolves when something calls `Quit`.
    quit: tokio::sync::mpsc::Receiver<()>,
}

impl Served {
    /// Wait until `Quit` is called.
    pub async fn until_quit(&mut self) {
        self.quit.recv().await;
    }
}

/// Serve `bluez` on `address`, under BlueZ's own name.
pub async fn serve(address: &str, bluez: &Arc<Bluez>) -> zbus::Result<Served> {
    let (quit, quit_rx) = tokio::sync::mpsc::channel(1);

    let connection = zbus::connection::Builder::address(address)?
        .name(BLUEZ_NAME)?
        .serve_at(
            ROOT_PATH,
            Manager {
                bluez: Arc::clone(bluez),
            },
        )?
        .serve_at(
            ADAPTER_PATH,
            Adapter {
                bluez: Arc::clone(bluez),
            },
        )?
        .serve_at(
            AGENT_MANAGER_PATH,
            AgentManager {
                bluez: Arc::clone(bluez),
            },
        )?
        .serve_at(
            CONTROL_PATH,
            Control {
                bluez: Arc::clone(bluez),
                quit,
            },
        )?
        .build()
        .await?;

    // The seeded devices go on afterwards, because their paths are not known
    // until the test has said what it wants paired.
    let paths: Vec<String> = lock(bluez).devices.keys().cloned().collect();
    for path in paths {
        serve_device(&connection, bluez, &path).await;
    }

    Ok(Served {
        connection,
        bluez: Arc::clone(bluez),
        quit: quit_rx,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_outcome_is_named_the_way_the_driver_spells_it() {
        assert_eq!(Outcome::parse("success"), Some(Outcome::Success));
        assert_eq!(Outcome::parse("fail"), Some(Outcome::Fail));
        assert_eq!(Outcome::parse("slow"), Some(Outcome::Slow));
        assert_eq!(Outcome::parse("nonsense"), None);
    }

    #[test]
    fn object_paths_are_built_the_way_bluez_builds_them() {
        assert_eq!(device_path("headset"), "/org/bluez/hci0/dev_headset");
        // A path may not have a colon or a dash in it, so an address-shaped
        // name is flattened rather than rejected.
        assert_eq!(device_path("AA:BB:CC"), "/org/bluez/hci0/dev_AA_BB_CC");
    }

    #[test]
    fn a_device_with_no_name_calls_itself_by_its_address_the_way_bluez_does() {
        let bare = FakeDevice::paired("Ignored", "AA:BB:CC:DD:EE:FF", "").nameless();
        assert_eq!(bare.alias, "AA-BB-CC-DD-EE-FF");
    }

    #[test]
    fn a_device_publishes_a_battery_interface_only_when_it_has_one() {
        let plain = device_interfaces(&FakeDevice::paired("Mouse", "AA", "input-mouse"));
        assert!(plain.contains_key(DEVICE_IFACE));
        assert!(!plain.contains_key(BATTERY_IFACE));

        let powered =
            device_interfaces(&FakeDevice::paired("Buds", "BB", "audio-headset").with_battery(85));
        assert!(powered.contains_key(BATTERY_IFACE));
    }
}
