//! A NetworkManager that exists to be talked to.
//!
//! Test support only: behind `cfg(test)` for the bus tests and behind the
//! `fake-nm` feature for `topbar-fake-nm`, the sidecar the nested-niri smoke
//! run puts on its private bus. The packaged panel contains none of it.
//!
//! It has to be a fairly complete NetworkManager, because the things worth
//! testing here are conversations rather than calls: joining a secured network
//! is an `AddAndActivateConnection`, then a `GetSecrets` *back* at the panel's
//! own agent, then a state change — and a fake that could not make that call
//! back would leave the whole secret-agent path untested.
//!
//! Nothing here ever touches the machine's real network. That is the point: the
//! real NetworkManager is on the system bus and is the developer's live
//! connection, and no screenshot is worth joining a network on somebody's
//! laptop to take.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use tracing::debug;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

/// Where the manager lives.
pub const NM_PATH: &str = "/org/freedesktop/NetworkManager";
/// The well-known name.
pub const NM_NAME: &str = "org.freedesktop.NetworkManager";
/// Where the settings store lives.
pub const SETTINGS_PATH: &str = "/org/freedesktop/NetworkManager/Settings";
/// Where agents sign up.
pub const AGENT_MANAGER_PATH: &str = "/org/freedesktop/NetworkManager/AgentManager";
/// The wireless card.
pub const WIFI_PATH: &str = "/org/freedesktop/NetworkManager/Devices/1";
/// The Ethernet port.
pub const WIRED_PATH: &str = "/org/freedesktop/NetworkManager/Devices/2";
/// Where the fake's own control interface lives.
pub const CONTROL_PATH: &str = "/io/github/trevarj/topbar/FakeNm1";
/// The panel's secret agent, as this fake calls it.
const AGENT_PATH: &str = "/org/freedesktop/NetworkManager/SecretAgent";
/// The interface it serves.
const AGENT_IFACE: &str = "org.freedesktop.NetworkManager.SecretAgent";

/// How long a "slow" activation stays on screen before it settles.
const SLOW_ACTIVATION: std::time::Duration = std::time::Duration::from_secs(30);

/// One access point the fake advertises.
#[derive(Debug, Clone)]
pub struct Ap {
    /// The SSID, as bytes — which is what it is on the air.
    pub ssid: Vec<u8>,
    /// Signal strength, 0–100.
    pub strength: u8,
    /// `NM_802_11_AP_FLAGS_*`.
    pub flags: u32,
    /// `NM_802_11_AP_SEC_*` for WPA.
    pub wpa: u32,
    /// The same for RSN.
    pub rsn: u32,
}

impl Ap {
    /// An open access point.
    pub fn open(ssid: &str, strength: u8) -> Self {
        Self {
            ssid: ssid.as_bytes().to_vec(),
            strength,
            flags: 0,
            wpa: 0,
            rsn: 0,
        }
    }

    /// A WPA2-PSK one, with the flags a real router advertises.
    pub fn secured(ssid: &str, strength: u8) -> Self {
        Self {
            ssid: ssid.as_bytes().to_vec(),
            strength,
            flags: 0x1,
            wpa: 0,
            rsn: 0x188,
        }
    }
}

/// One saved profile.
#[derive(Debug, Clone)]
pub struct Profile {
    /// What the user called it.
    pub id: String,
    /// Its stable identifier.
    pub uuid: String,
    /// `802-11-wireless`, `vpn`, `wireguard`.
    pub kind: String,
    /// The SSID, for a Wi-Fi profile.
    pub ssid: Option<Vec<u8>>,
    /// `vpn.service-type`, for a plugin profile.
    pub service: Option<String>,
}

impl Profile {
    /// A saved Wi-Fi network.
    pub fn wifi(id: &str, ssid: &str) -> Self {
        Self {
            id: id.to_string(),
            uuid: format!("uuid-{id}"),
            kind: "802-11-wireless".to_string(),
            ssid: Some(ssid.as_bytes().to_vec()),
            service: None,
        }
    }

    /// A VPN profile of one kind or another.
    pub fn vpn(id: &str, uuid: &str, kind: &str, service: Option<&str>) -> Self {
        Self {
            id: id.to_string(),
            uuid: uuid.to_string(),
            kind: kind.to_string(),
            ssid: None,
            service: service.map(str::to_string),
        }
    }
}

/// One connection that is up, or coming up.
#[derive(Debug, Clone)]
struct Active {
    id: String,
    uuid: String,
    kind: String,
    state: u32,
    connection: String,
}

/// What the next activation should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Ask for a secret if the network needs one, then activate.
    Success,
    /// Ask for a secret, refuse it, and deactivate with `NO_SECRETS`.
    AuthFail,
    /// Stay in `ACTIVATING` for half a minute, which is long enough to
    /// photograph a spinner.
    Slow,
    /// Never answer at all.
    Timeout,
}

impl Outcome {
    /// Parse a control-interface argument.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "success" => Some(Self::Success),
            "auth_fail" | "auth-fail" => Some(Self::AuthFail),
            "slow" => Some(Self::Slow),
            "timeout" => Some(Self::Timeout),
            _ => None,
        }
    }
}

/// Everything the fake knows.
#[derive(Debug)]
struct Inner {
    state: u32,
    wireless_enabled: bool,
    has_wifi: bool,
    has_wired: bool,
    wifi_state: u32,
    wired_state: u32,
    wired_carrier: bool,
    wired_speed: u32,
    active_ap: Option<String>,
    aps: BTreeMap<String, Ap>,
    profiles: BTreeMap<String, Profile>,
    actives: BTreeMap<String, Active>,
    /// Every method call the panel made, in order.
    calls: Vec<String>,
    /// Every secret the panel handed back, so a test can prove it arrived
    /// through the agent reply and nowhere else.
    secrets: Vec<String>,
    /// Every identifier an agent registered under.
    agents: Vec<String>,
    /// The unique bus name of the agent to call back.
    agent_name: Option<String>,
    /// How many times the card was asked to look around.
    scans: u32,
    /// What the queued activations should do, in order.
    outcomes: VecDeque<Outcome>,
    next_id: u32,
}

/// The fake, as everything that serves it shares it.
#[derive(Debug)]
pub struct Nm {
    inner: Mutex<Inner>,
}

/// Lock through poisoning: the state is plain data.
fn lock(nm: &Nm) -> MutexGuard<'_, Inner> {
    nm.inner
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Nm {
    /// A NetworkManager with a wireless card, an Ethernet port and nothing on
    /// either.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner {
                state: 70,
                wireless_enabled: true,
                has_wifi: true,
                has_wired: true,
                wifi_state: 30,
                wired_state: 20,
                wired_carrier: false,
                wired_speed: 0,
                active_ap: None,
                aps: BTreeMap::new(),
                profiles: BTreeMap::new(),
                actives: BTreeMap::new(),
                calls: Vec::new(),
                secrets: Vec::new(),
                agents: Vec::new(),
                agent_name: None,
                scans: 0,
                outcomes: VecDeque::new(),
                next_id: 100,
            }),
        })
    }

    /// Whether this machine has a wireless card.
    pub fn set_has_wifi(&self, has: bool) {
        lock(self).has_wifi = has;
    }

    /// Whether it has an Ethernet port.
    pub fn set_has_wired(&self, has: bool) {
        lock(self).has_wired = has;
    }

    /// Put an access point in range, before the bus is up.
    pub fn seed_ap(&self, name: &str, ap: Ap) {
        lock(self).aps.insert(ap_path(name), ap);
    }

    /// Put a saved profile in the store, before the bus is up.
    pub fn seed_profile(&self, profile: Profile) {
        let path = profile_path(&profile.uuid);
        lock(self).profiles.insert(path, profile);
    }

    /// Say what the card is associated with, before the bus is up.
    pub fn seed_active_ap(&self, name: &str) {
        let mut inner = lock(self);
        inner.active_ap = Some(ap_path(name));
        inner.wifi_state = 100;
    }

    /// Put a VPN up, before the bus is up.
    pub fn seed_vpn_active(&self, uuid: &str) {
        let mut inner = lock(self);
        let Some(profile) = inner
            .profiles
            .values()
            .find(|profile| profile.uuid == uuid)
            .cloned()
        else {
            return;
        };
        let id = inner.next_id;
        inner.next_id += 1;
        inner.actives.insert(
            active_path(id),
            Active {
                id: profile.id.clone(),
                uuid: profile.uuid.clone(),
                kind: profile.kind.clone(),
                state: 2,
                connection: profile_path(&profile.uuid),
            },
        );
    }

    /// Queue what the next activation should do.
    pub fn queue(&self, outcome: Outcome) {
        lock(self).outcomes.push_back(outcome);
    }

    /// Set the machine's overall state, before the bus is up.
    pub fn set_state(&self, state: u32) {
        lock(self).state = state;
    }

    /// Plug a cable in, before the bus is up.
    pub fn set_carrier(&self, carrier: bool, speed: u32) {
        let mut inner = lock(self);
        inner.wired_carrier = carrier;
        inner.wired_speed = speed;
        inner.wired_state = if carrier { 100 } else { 20 };
    }

    /// Every method the panel called, in order.
    pub fn calls(&self) -> Vec<String> {
        lock(self).calls.clone()
    }

    /// How many times one method was called.
    pub fn count(&self, method: &str) -> usize {
        lock(self)
            .calls
            .iter()
            .filter(|call| call.as_str() == method)
            .count()
    }

    /// Every secret the panel handed back.
    pub fn secrets(&self) -> Vec<String> {
        lock(self).secrets.clone()
    }

    /// Every identifier an agent registered under.
    pub fn agents(&self) -> Vec<String> {
        lock(self).agents.clone()
    }

    /// How many scans were asked for.
    pub fn scans(&self) -> u32 {
        lock(self).scans
    }

    /// Whether a profile is still in the store.
    pub fn has_profile(&self, uuid: &str) -> bool {
        lock(self)
            .profiles
            .values()
            .any(|profile| profile.uuid == uuid)
    }

    /// How many profiles are in the store.
    pub fn profile_count(&self) -> usize {
        lock(self).profiles.len()
    }

    /// Record one call.
    fn record(&self, method: &str) {
        lock(self).calls.push(method.to_string());
    }

    /// The next object identifier.
    fn take_id(&self) -> u32 {
        let mut inner = lock(self);
        let id = inner.next_id;
        inner.next_id += 1;
        id
    }
}

/// The object path of one access point.
fn ap_path(name: &str) -> String {
    format!("/org/freedesktop/NetworkManager/AccessPoint/{name}")
}

/// The object path of one saved profile.
fn profile_path(uuid: &str) -> String {
    format!(
        "/org/freedesktop/NetworkManager/Settings/{}",
        uuid.replace(['-', '.'], "_")
    )
}

/// The object path of one active connection.
fn active_path(id: u32) -> String {
    format!("/org/freedesktop/NetworkManager/ActiveConnection/{id}")
}

/// Emit a `PropertiesChanged` the way NetworkManager does.
async fn changed(
    connection: &zbus::Connection,
    path: &str,
    interface: &str,
    properties: HashMap<&str, Value<'_>>,
) {
    let _ = connection
        .emit_signal(
            None::<&str>,
            path,
            "org.freedesktop.DBus.Properties",
            "PropertiesChanged",
            &(interface, properties, Vec::<&str>::new()),
        )
        .await;
}

/// The manager object.
struct Manager {
    nm: Arc<Nm>,
}

#[zbus::interface(name = "org.freedesktop.NetworkManager")]
impl Manager {
    async fn get_devices(&self) -> Vec<OwnedObjectPath> {
        self.nm.record("GetDevices");
        devices(&self.nm)
    }

    async fn activate_connection(
        &self,
        connection: OwnedObjectPath,
        device: OwnedObjectPath,
        specific_object: OwnedObjectPath,
        #[zbus(connection)] bus: &zbus::Connection,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        self.nm.record("ActivateConnection");
        let profile = lock(&self.nm)
            .profiles
            .get(connection.as_str())
            .cloned()
            .ok_or_else(|| zbus::fdo::Error::UnknownObject(connection.as_str().to_string()))?;
        let active = start_activation(
            &self.nm,
            bus,
            &profile,
            connection.as_str(),
            specific_object.as_str(),
            device.as_str().contains("Devices/1"),
            false,
        )
        .await?;
        Ok(active)
    }

    async fn add_and_activate_connection(
        &self,
        connection: HashMap<String, HashMap<String, OwnedValue>>,
        device: OwnedObjectPath,
        specific_object: OwnedObjectPath,
        #[zbus(connection)] bus: &zbus::Connection,
    ) -> zbus::fdo::Result<(OwnedObjectPath, OwnedObjectPath)> {
        self.nm.record("AddAndActivateConnection");
        // The panel deliberately sends an empty dictionary and lets
        // NetworkManager build the profile from the access point, so this is
        // what the real one does with it.
        if !connection.is_empty() {
            self.nm.record("AddAndActivateConnection:with-settings");
        }
        let ap = lock(&self.nm)
            .aps
            .get(specific_object.as_str())
            .cloned()
            .ok_or_else(|| zbus::fdo::Error::UnknownObject(specific_object.as_str().to_string()))?;
        let name = String::from_utf8_lossy(&ap.ssid).to_string();
        let id = self.nm.take_id();
        let profile = Profile {
            id: name.clone(),
            uuid: format!("added-{id}"),
            kind: "802-11-wireless".to_string(),
            ssid: Some(ap.ssid.clone()),
            service: None,
        };
        let path = profile_path(&profile.uuid);
        lock(&self.nm)
            .profiles
            .insert(path.clone(), profile.clone());
        serve_profile(bus, &self.nm, &path).await;
        let _ = bus
            .emit_signal(
                None::<&str>,
                SETTINGS_PATH,
                "org.freedesktop.NetworkManager.Settings",
                "NewConnection",
                &(ObjectPath::try_from(path.as_str())
                    .unwrap_or_else(|_| ObjectPath::from_static_str_unchecked("/")),),
            )
            .await;

        let active = start_activation(
            &self.nm,
            bus,
            &profile,
            &path,
            specific_object.as_str(),
            device.as_str().contains("Devices/1"),
            true,
        )
        .await?;
        let settings = OwnedObjectPath::try_from(path.as_str())
            .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
        Ok((settings, active))
    }

    async fn deactivate_connection(
        &self,
        active_connection: OwnedObjectPath,
        #[zbus(connection)] bus: &zbus::Connection,
    ) -> zbus::fdo::Result<()> {
        self.nm.record("DeactivateConnection");
        let existed = lock(&self.nm)
            .actives
            .remove(active_connection.as_str())
            .is_some();
        if !existed {
            return Err(zbus::fdo::Error::UnknownObject(
                active_connection.as_str().to_string(),
            ));
        }
        let _ = bus
            .emit_signal(
                None::<&str>,
                active_connection.as_str(),
                "org.freedesktop.NetworkManager.Connection.Active",
                "StateChanged",
                &(4_u32, 2_u32),
            )
            .await;
        publish_actives(bus, &self.nm).await;
        Ok(())
    }

    #[zbus(property, name = "State")]
    fn nm_state(&self) -> u32 {
        lock(&self.nm).state
    }

    #[zbus(property)]
    fn wireless_enabled(&self) -> bool {
        lock(&self.nm).wireless_enabled
    }

    #[zbus(property)]
    fn set_wireless_enabled(&self, enabled: bool) {
        self.nm.record("SetWirelessEnabled");
        lock(&self.nm).wireless_enabled = enabled;
    }

    #[zbus(property)]
    fn active_connections(&self) -> Vec<OwnedObjectPath> {
        paths(lock(&self.nm).actives.keys())
    }

    #[zbus(property)]
    fn devices(&self) -> Vec<OwnedObjectPath> {
        devices(&self.nm)
    }
}

/// Every device that exists right now.
fn devices(nm: &Arc<Nm>) -> Vec<OwnedObjectPath> {
    let inner = lock(nm);
    let mut list = Vec::new();
    if inner.has_wifi {
        list.push(WIFI_PATH);
    }
    if inner.has_wired {
        list.push(WIRED_PATH);
    }
    paths(list.into_iter())
}

/// Turn a run of path strings into object paths, skipping anything malformed.
fn paths<'a>(source: impl Iterator<Item = impl AsRef<str> + 'a>) -> Vec<OwnedObjectPath> {
    source
        .filter_map(|path| OwnedObjectPath::try_from(path.as_ref()).ok())
        .collect()
}

/// Bring one connection up, in the background.
///
/// Returns as soon as the object exists, because the panel is *awaiting this
/// method* — and the next thing the fake does is call back into the panel's
/// secret agent, which the panel cannot answer until this reply has landed.
async fn start_activation(
    nm: &Arc<Nm>,
    bus: &zbus::Connection,
    profile: &Profile,
    connection: &str,
    access_point: &str,
    on_wifi: bool,
    added: bool,
) -> zbus::fdo::Result<OwnedObjectPath> {
    let id = nm.take_id();
    let path = active_path(id);
    lock(nm).actives.insert(
        path.clone(),
        Active {
            id: profile.id.clone(),
            uuid: profile.uuid.clone(),
            kind: profile.kind.clone(),
            state: 1,
            connection: connection.to_string(),
        },
    );
    serve_active(bus, nm, &path).await;
    publish_actives(bus, nm).await;

    let outcome = lock(nm).outcomes.pop_front().unwrap_or(Outcome::Success);
    let secured = lock(nm)
        .aps
        .get(access_point)
        .is_some_and(|ap| ap.flags & 0x1 != 0 || ap.wpa != 0 || ap.rsn != 0);

    tokio::spawn(drive(
        Arc::clone(nm),
        bus.clone(),
        path.clone(),
        connection.to_string(),
        profile.clone(),
        access_point.to_string(),
        outcome,
        // A profile that already existed has its password in it, which is why
        // joining a saved network does not put a prompt on screen. One that was
        // added a moment ago has nothing, so NetworkManager has to ask — and so
        // does a saved one whose stored key has stopped working, which is what
        // a queued auth failure stands in for.
        secured && (added || outcome == Outcome::AuthFail),
        on_wifi,
    ));

    OwnedObjectPath::try_from(path.as_str())
        .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))
}

/// Run one activation to whatever end it was queued for.
#[allow(clippy::too_many_arguments)]
async fn drive(
    nm: Arc<Nm>,
    bus: zbus::Connection,
    active: String,
    connection: String,
    profile: Profile,
    access_point: String,
    outcome: Outcome,
    ask: bool,
    on_wifi: bool,
) {
    if outcome == Outcome::Timeout {
        debug!("fake-nm: {} is going nowhere, as asked", profile.id);
        return;
    }
    if outcome == Outcome::Slow {
        // Long enough for a screenshot of the spinner, and then it works.
        tokio::time::sleep(SLOW_ACTIVATION).await;
    }

    if ask {
        let answered = ask_for_secrets(&nm, &bus, &connection, &profile, 0x1).await;
        if !answered {
            fail(&nm, &bus, &active, 9).await;
            return;
        }
    }

    if outcome == Outcome::AuthFail {
        // A real NetworkManager tries the key, is deauthenticated, and gives
        // up with NO_SECRETS. The profile it added is still in the store,
        // which is exactly what the panel has to clean up.
        fail(&nm, &bus, &active, 9).await;
        return;
    }

    if let Some(slot) = lock(&nm).actives.get_mut(&active) {
        slot.state = 2;
    }
    if on_wifi {
        let mut inner = lock(&nm);
        inner.active_ap = Some(access_point.clone());
        inner.wifi_state = 100;
    }
    let _ = bus
        .emit_signal(
            None::<&str>,
            active.as_str(),
            "org.freedesktop.NetworkManager.Connection.Active",
            "StateChanged",
            &(2_u32, 1_u32),
        )
        .await;
    if on_wifi {
        changed(
            &bus,
            WIFI_PATH,
            "org.freedesktop.NetworkManager.Device.Wireless",
            HashMap::from([(
                "ActiveAccessPoint",
                Value::from(
                    ObjectPath::try_from(access_point.as_str())
                        .unwrap_or_else(|_| ObjectPath::from_static_str_unchecked("/")),
                ),
            )]),
        )
        .await;
        changed(
            &bus,
            WIFI_PATH,
            "org.freedesktop.NetworkManager.Device",
            HashMap::from([("State", Value::from(100_u32))]),
        )
        .await;
    }
}

/// Deactivate one connection with a reason.
async fn fail(nm: &Arc<Nm>, bus: &zbus::Connection, active: &str, reason: u32) {
    lock(nm).actives.remove(active);
    let _ = bus
        .emit_signal(
            None::<&str>,
            active,
            "org.freedesktop.NetworkManager.Connection.Active",
            "StateChanged",
            &(4_u32, reason),
        )
        .await;
    publish_actives(bus, nm).await;
}

/// Ask the registered agent for a Wi-Fi password.
///
/// This is the call that makes the whole secret-agent path real: it goes out to
/// the panel's own unique bus name, at the object NetworkManager's protocol
/// says an agent lives at, and the reply is recorded so a test can prove the
/// password travelled here and nowhere else.
async fn ask_for_secrets(
    nm: &Arc<Nm>,
    bus: &zbus::Connection,
    connection: &str,
    profile: &Profile,
    flags: u32,
) -> bool {
    let Some(agent) = lock(nm).agent_name.clone() else {
        debug!("fake-nm: no agent is registered, so nobody can be asked");
        return false;
    };
    nm.record("GetSecrets");

    let mut wifi: HashMap<&str, Value<'_>> = HashMap::new();
    if let Some(ssid) = &profile.ssid {
        wifi.insert("ssid", Value::from(ssid.clone()));
    }
    let mut settings: HashMap<&str, HashMap<&str, Value<'_>>> = HashMap::new();
    settings.insert(
        "connection",
        HashMap::from([
            ("id", Value::from(profile.id.as_str())),
            ("uuid", Value::from(profile.uuid.as_str())),
            ("type", Value::from(profile.kind.as_str())),
        ]),
    );
    settings.insert("802-11-wireless", wifi);

    let path = ObjectPath::try_from(connection)
        .unwrap_or_else(|_| ObjectPath::from_static_str_unchecked("/"));
    let reply: zbus::Result<HashMap<String, HashMap<String, OwnedValue>>> = async {
        let message = bus
            .call_method(
                Some(agent.as_str()),
                AGENT_PATH,
                Some(AGENT_IFACE),
                "GetSecrets",
                &(
                    settings,
                    &path,
                    "802-11-wireless-security",
                    Vec::<String>::new(),
                    flags,
                ),
            )
            .await?;
        message.body().deserialize()
    }
    .await;

    match reply {
        Ok(secrets) => {
            let psk = secrets
                .get("802-11-wireless-security")
                .and_then(|group| group.get("psk"))
                .and_then(|value| String::try_from(value.try_clone().ok()?).ok());
            match psk {
                Some(psk) => {
                    lock(nm).secrets.push(psk);
                    true
                }
                None => false,
            }
        }
        Err(error) => {
            debug!("fake-nm: the agent refused ({error})");
            false
        }
    }
}

/// Tell the bus the active-connection list changed.
async fn publish_actives(bus: &zbus::Connection, nm: &Arc<Nm>) {
    let list = paths(lock(nm).actives.keys());
    changed(
        bus,
        NM_PATH,
        "org.freedesktop.NetworkManager",
        HashMap::from([("ActiveConnections", Value::from(list))]),
    )
    .await;
}

/// Where agents sign up.
struct AgentManager {
    nm: Arc<Nm>,
}

#[zbus::interface(name = "org.freedesktop.NetworkManager.AgentManager")]
impl AgentManager {
    fn register(&self, identifier: String, #[zbus(header)] header: zbus::message::Header<'_>) {
        self.nm.record("Register");
        remember_agent(&self.nm, identifier, &header);
    }

    fn register_with_capabilities(
        &self,
        identifier: String,
        capabilities: u32,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) {
        self.nm.record("RegisterWithCapabilities");
        self.nm
            .record(&format!("RegisterWithCapabilities:{capabilities}"));
        remember_agent(&self.nm, identifier, &header);
    }

    fn unregister(&self) {
        self.nm.record("Unregister");
        lock(&self.nm).agent_name = None;
    }
}

/// Note who registered, and where to call them back.
fn remember_agent(nm: &Arc<Nm>, identifier: String, header: &zbus::message::Header<'_>) {
    let mut inner = lock(nm);
    inner.agents.push(identifier);
    inner.agent_name = header.sender().map(std::string::ToString::to_string);
}

/// The settings store.
struct Settings {
    nm: Arc<Nm>,
}

#[zbus::interface(name = "org.freedesktop.NetworkManager.Settings")]
impl Settings {
    fn list_connections(&self) -> Vec<OwnedObjectPath> {
        self.nm.record("ListConnections");
        paths(lock(&self.nm).profiles.keys())
    }

    #[zbus(property)]
    fn connections(&self) -> Vec<OwnedObjectPath> {
        paths(lock(&self.nm).profiles.keys())
    }
}

/// One saved profile.
struct Connection {
    nm: Arc<Nm>,
    path: String,
}

#[zbus::interface(name = "org.freedesktop.NetworkManager.Settings.Connection")]
impl Connection {
    fn get_settings(&self) -> zbus::fdo::Result<HashMap<String, HashMap<String, OwnedValue>>> {
        self.nm.record("GetSettings");
        let profile = lock(&self.nm)
            .profiles
            .get(&self.path)
            .cloned()
            .ok_or_else(|| zbus::fdo::Error::UnknownObject(self.path.clone()))?;

        let mut settings = HashMap::new();
        let mut connection = HashMap::new();
        insert(&mut connection, "id", Value::from(profile.id.as_str()));
        insert(&mut connection, "uuid", Value::from(profile.uuid.as_str()));
        insert(&mut connection, "type", Value::from(profile.kind.as_str()));
        settings.insert("connection".to_string(), connection);

        if let Some(ssid) = &profile.ssid {
            let mut wifi = HashMap::new();
            insert(&mut wifi, "ssid", Value::from(ssid.clone()));
            settings.insert("802-11-wireless".to_string(), wifi);
        }
        if let Some(service) = &profile.service {
            let mut vpn = HashMap::new();
            insert(&mut vpn, "service-type", Value::from(service.as_str()));
            settings.insert("vpn".to_string(), vpn);
        }
        Ok(settings)
    }

    async fn delete(&self, #[zbus(connection)] bus: &zbus::Connection) -> zbus::fdo::Result<()> {
        self.nm.record("Delete");
        lock(&self.nm).profiles.remove(&self.path);
        let path = ObjectPath::try_from(self.path.as_str())
            .unwrap_or_else(|_| ObjectPath::from_static_str_unchecked("/"));
        let _ = bus
            .emit_signal(
                None::<&str>,
                SETTINGS_PATH,
                "org.freedesktop.NetworkManager.Settings",
                "ConnectionRemoved",
                &(&path,),
            )
            .await;
        Ok(())
    }
}

/// Put one value in a settings group, dropping anything that will not travel.
fn insert(group: &mut HashMap<String, OwnedValue>, key: &str, value: Value<'_>) {
    if let Ok(value) = value.try_to_owned() {
        group.insert(key.to_string(), value);
    }
}

/// One active connection.
struct ActiveConnection {
    nm: Arc<Nm>,
    path: String,
}

impl ActiveConnection {
    /// This connection's record, or a blank one once it has gone.
    fn spec(&self) -> Active {
        lock(&self.nm)
            .actives
            .get(&self.path)
            .cloned()
            .unwrap_or(Active {
                id: String::new(),
                uuid: String::new(),
                kind: String::new(),
                state: 4,
                connection: "/".to_string(),
            })
    }
}

#[zbus::interface(name = "org.freedesktop.NetworkManager.Connection.Active")]
impl ActiveConnection {
    #[zbus(property)]
    fn id(&self) -> String {
        self.spec().id
    }

    #[zbus(property, name = "Type")]
    fn connection_type(&self) -> String {
        self.spec().kind
    }

    #[zbus(property)]
    fn uuid(&self) -> String {
        self.spec().uuid
    }

    #[zbus(property)]
    fn state(&self) -> u32 {
        self.spec().state
    }

    #[zbus(property)]
    fn connection(&self) -> OwnedObjectPath {
        OwnedObjectPath::try_from(self.spec().connection.as_str())
            .unwrap_or_else(|_| OwnedObjectPath::try_from("/").expect("the root path"))
    }

    #[zbus(property)]
    fn devices(&self) -> Vec<OwnedObjectPath> {
        paths([WIFI_PATH].into_iter())
    }
}

/// The wireless card.
struct WifiDevice {
    nm: Arc<Nm>,
}

#[zbus::interface(name = "org.freedesktop.NetworkManager.Device")]
impl WifiDevice {
    fn disconnect(&self) {
        self.nm.record("Disconnect");
        let mut inner = lock(&self.nm);
        inner.active_ap = None;
        inner.wifi_state = 30;
    }

    #[zbus(property)]
    fn device_type(&self) -> u32 {
        2
    }

    #[zbus(property)]
    fn interface(&self) -> String {
        "wlan0".to_string()
    }

    #[zbus(property)]
    fn state(&self) -> u32 {
        // The device's state, not the machine's: the two are different numbers
        // in NetworkManager and clippy cannot know which `state` this is.
        let inner = lock(&self.nm);
        inner.wifi_state
    }

    #[zbus(property)]
    fn active_connection(&self) -> OwnedObjectPath {
        OwnedObjectPath::try_from("/").expect("the root path")
    }
}

/// The Wi-Fi half of the same object.
struct WifiRadio {
    nm: Arc<Nm>,
}

#[zbus::interface(name = "org.freedesktop.NetworkManager.Device.Wireless")]
impl WifiRadio {
    fn request_scan(&self, _options: HashMap<String, OwnedValue>) {
        self.nm.record("RequestScan");
        lock(&self.nm).scans += 1;
    }

    #[zbus(property)]
    fn access_points(&self) -> Vec<OwnedObjectPath> {
        paths(lock(&self.nm).aps.keys())
    }

    #[zbus(property)]
    fn active_access_point(&self) -> OwnedObjectPath {
        let active = lock(&self.nm).active_ap.clone();
        OwnedObjectPath::try_from(active.as_deref().unwrap_or("/"))
            .unwrap_or_else(|_| OwnedObjectPath::try_from("/").expect("the root path"))
    }
}

/// The Ethernet port.
struct WiredDevice {
    nm: Arc<Nm>,
}

#[zbus::interface(name = "org.freedesktop.NetworkManager.Device")]
impl WiredDevice {
    fn disconnect(&self) {
        self.nm.record("Disconnect");
    }

    #[zbus(property)]
    fn device_type(&self) -> u32 {
        1
    }

    #[zbus(property)]
    fn interface(&self) -> String {
        "eth0".to_string()
    }

    #[zbus(property)]
    fn state(&self) -> u32 {
        let inner = lock(&self.nm);
        inner.wired_state
    }

    #[zbus(property)]
    fn active_connection(&self) -> OwnedObjectPath {
        OwnedObjectPath::try_from("/").expect("the root path")
    }
}

/// The wired half of the same object.
struct WiredLink {
    nm: Arc<Nm>,
}

#[zbus::interface(name = "org.freedesktop.NetworkManager.Device.Wired")]
impl WiredLink {
    #[zbus(property)]
    fn carrier(&self) -> bool {
        lock(&self.nm).wired_carrier
    }

    #[zbus(property)]
    fn speed(&self) -> u32 {
        lock(&self.nm).wired_speed
    }
}

/// One access point.
struct AccessPoint {
    nm: Arc<Nm>,
    path: String,
}

impl AccessPoint {
    /// This access point's record, or a blank one once it has gone.
    fn spec(&self) -> Ap {
        lock(&self.nm)
            .aps
            .get(&self.path)
            .cloned()
            .unwrap_or_else(|| Ap::open("", 0))
    }
}

#[zbus::interface(name = "org.freedesktop.NetworkManager.AccessPoint")]
impl AccessPoint {
    #[zbus(property)]
    fn ssid(&self) -> Vec<u8> {
        self.spec().ssid
    }

    #[zbus(property)]
    fn strength(&self) -> u8 {
        self.spec().strength
    }

    #[zbus(property)]
    fn flags(&self) -> u32 {
        self.spec().flags
    }

    #[zbus(property)]
    fn wpa_flags(&self) -> u32 {
        self.spec().wpa
    }

    #[zbus(property)]
    fn rsn_flags(&self) -> u32 {
        self.spec().rsn
    }
}

/// Put one saved profile's object on the bus.
async fn serve_profile(bus: &zbus::Connection, nm: &Arc<Nm>, path: &str) {
    let _ = bus
        .object_server()
        .at(
            path,
            Connection {
                nm: Arc::clone(nm),
                path: path.to_string(),
            },
        )
        .await;
}

/// Put one active connection's object on the bus.
async fn serve_active(bus: &zbus::Connection, nm: &Arc<Nm>, path: &str) {
    let _ = bus
        .object_server()
        .at(
            path,
            ActiveConnection {
                nm: Arc::clone(nm),
                path: path.to_string(),
            },
        )
        .await;
}

/// Put one access point's object on the bus.
async fn serve_ap(bus: &zbus::Connection, nm: &Arc<Nm>, path: &str) {
    let _ = bus
        .object_server()
        .at(
            path,
            AccessPoint {
                nm: Arc::clone(nm),
                path: path.to_string(),
            },
        )
        .await;
}

/// The fake's own control interface, for a driver with no pointer.
struct Control {
    nm: Arc<Nm>,
    quit: tokio::sync::mpsc::Sender<()>,
}

#[zbus::interface(name = "io.github.trevarj.topbar.FakeNm1")]
impl Control {
    /// Put an access point in range.
    async fn add_ap(
        &self,
        name: String,
        ssid: String,
        strength: u8,
        secured: bool,
        #[zbus(connection)] bus: &zbus::Connection,
    ) {
        let ap = if secured {
            Ap::secured(&ssid, strength)
        } else {
            Ap::open(&ssid, strength)
        };
        let path = ap_path(&name);
        lock(&self.nm).aps.insert(path.clone(), ap);
        serve_ap(bus, &self.nm, &path).await;
        let object = ObjectPath::try_from(path.as_str())
            .unwrap_or_else(|_| ObjectPath::from_static_str_unchecked("/"));
        let _ = bus
            .emit_signal(
                None::<&str>,
                WIFI_PATH,
                "org.freedesktop.NetworkManager.Device.Wireless",
                "AccessPointAdded",
                &(&object,),
            )
            .await;
    }

    /// Take one out of range.
    async fn remove_ap(&self, name: String, #[zbus(connection)] bus: &zbus::Connection) {
        let path = ap_path(&name);
        lock(&self.nm).aps.remove(&path);
        let object = ObjectPath::try_from(path.as_str())
            .unwrap_or_else(|_| ObjectPath::from_static_str_unchecked("/"));
        let _ = bus
            .emit_signal(
                None::<&str>,
                WIFI_PATH,
                "org.freedesktop.NetworkManager.Device.Wireless",
                "AccessPointRemoved",
                &(&object,),
            )
            .await;
    }

    /// Move one access point's signal.
    async fn set_strength(
        &self,
        name: String,
        strength: u8,
        #[zbus(connection)] bus: &zbus::Connection,
    ) {
        let path = ap_path(&name);
        if let Some(ap) = lock(&self.nm).aps.get_mut(&path) {
            ap.strength = strength;
        }
        changed(
            bus,
            &path,
            "org.freedesktop.NetworkManager.AccessPoint",
            HashMap::from([("Strength", Value::from(strength))]),
        )
        .await;
    }

    /// Plug a cable in, or pull it out.
    async fn set_carrier(
        &self,
        carrier: bool,
        speed: u32,
        #[zbus(connection)] bus: &zbus::Connection,
    ) {
        {
            let mut inner = lock(&self.nm);
            inner.wired_carrier = carrier;
            inner.wired_speed = speed;
            inner.wired_state = if carrier { 100 } else { 20 };
        }
        changed(
            bus,
            WIRED_PATH,
            "org.freedesktop.NetworkManager.Device.Wired",
            HashMap::from([
                ("Carrier", Value::from(carrier)),
                ("Speed", Value::from(speed)),
            ]),
        )
        .await;
        changed(
            bus,
            WIRED_PATH,
            "org.freedesktop.NetworkManager.Device",
            HashMap::from([("State", Value::from(if carrier { 100_u32 } else { 20 }))]),
        )
        .await;
    }

    /// Switch the radio from outside, as `nmcli radio wifi off` would.
    async fn set_wireless_enabled(
        &self,
        enabled: bool,
        #[zbus(connection)] bus: &zbus::Connection,
    ) {
        lock(&self.nm).wireless_enabled = enabled;
        changed(
            bus,
            NM_PATH,
            "org.freedesktop.NetworkManager",
            HashMap::from([("WirelessEnabled", Value::from(enabled))]),
        )
        .await;
    }

    /// Move the machine's overall state.
    async fn set_state(&self, state: u32, #[zbus(connection)] bus: &zbus::Connection) {
        lock(&self.nm).state = state;
        let _ = bus
            .emit_signal(
                None::<&str>,
                NM_PATH,
                "org.freedesktop.NetworkManager",
                "StateChanged",
                &(state,),
            )
            .await;
        // The real one emits both, and a client that trusted only the property
        // notification would still be right.
        changed(
            bus,
            NM_PATH,
            "org.freedesktop.NetworkManager",
            HashMap::from([("State", Value::from(state))]),
        )
        .await;
    }

    /// Say what the next activation should do.
    fn queue_activation_outcome(&self, outcome: String) -> zbus::fdo::Result<()> {
        let outcome = Outcome::parse(&outcome)
            .ok_or_else(|| zbus::fdo::Error::InvalidArgs(format!("no such outcome: {outcome}")))?;
        self.nm.queue(outcome);
        Ok(())
    }

    /// Ask the panel's agent for a password out of the blue.
    ///
    /// What a network reconnecting on its own looks like: NetworkManager wants
    /// a secret and there is no activation the panel started.
    async fn trigger_get_secrets(
        &self,
        id: String,
        ssid: String,
        flags: u32,
        #[zbus(connection)] bus: &zbus::Connection,
    ) -> bool {
        let profile = Profile {
            id,
            uuid: "triggered".to_string(),
            kind: "802-11-wireless".to_string(),
            ssid: Some(ssid.into_bytes()),
            service: None,
        };
        ask_for_secrets(&self.nm, bus, "/triggered", &profile, flags).await
    }

    /// Add a VPN profile.
    async fn add_vpn_profile(
        &self,
        id: String,
        uuid: String,
        kind: String,
        service: String,
        #[zbus(connection)] bus: &zbus::Connection,
    ) {
        let service = (!service.is_empty()).then_some(service);
        let profile = Profile {
            id,
            uuid,
            kind,
            ssid: None,
            service,
        };
        let path = profile_path(&profile.uuid);
        lock(&self.nm).profiles.insert(path.clone(), profile);
        serve_profile(bus, &self.nm, &path).await;
        let object = ObjectPath::try_from(path.as_str())
            .unwrap_or_else(|_| ObjectPath::from_static_str_unchecked("/"));
        let _ = bus
            .emit_signal(
                None::<&str>,
                SETTINGS_PATH,
                "org.freedesktop.NetworkManager.Settings",
                "NewConnection",
                &(&object,),
            )
            .await;
    }

    /// Bring a VPN up or down from outside.
    async fn set_vpn_active(
        &self,
        uuid: String,
        active: bool,
        #[zbus(connection)] bus: &zbus::Connection,
    ) {
        if active {
            let profile = lock(&self.nm)
                .profiles
                .values()
                .find(|profile| profile.uuid == uuid)
                .cloned();
            let Some(profile) = profile else { return };
            let connection = profile_path(&profile.uuid);
            let _ = start_activation(&self.nm, bus, &profile, &connection, "/", false, false).await;
            return;
        }
        let path = lock(&self.nm)
            .actives
            .iter()
            .find(|(_, active)| active.uuid == uuid)
            .map(|(path, _)| path.clone());
        let Some(path) = path else { return };
        lock(&self.nm).actives.remove(&path);
        let _ = bus
            .emit_signal(
                None::<&str>,
                path.as_str(),
                "org.freedesktop.NetworkManager.Connection.Active",
                "StateChanged",
                &(4_u32, 2_u32),
            )
            .await;
        publish_actives(bus, &self.nm).await;
    }

    /// Every method the panel called, in order.
    fn calls(&self) -> Vec<String> {
        self.nm.calls()
    }

    /// Every secret the panel handed back.
    fn secrets(&self) -> Vec<String> {
        self.nm.secrets()
    }

    /// Every identifier an agent registered under.
    fn agents(&self) -> Vec<String> {
        self.nm.agents()
    }

    /// How many profiles are in the store.
    fn profile_count(&self) -> u32 {
        self.nm.profile_count() as u32
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
    pub nm: Arc<Nm>,
    /// Resolves when something calls `Quit`.
    quit: tokio::sync::mpsc::Receiver<()>,
}

impl Served {
    /// Wait until `Quit` is called.
    pub async fn until_quit(&mut self) {
        self.quit.recv().await;
    }
}

/// Serve `nm` on `address`, under NetworkManager's own name.
pub async fn serve(address: &str, nm: &Arc<Nm>) -> zbus::Result<Served> {
    let (quit, quit_rx) = tokio::sync::mpsc::channel(1);

    let connection = zbus::connection::Builder::address(address)?
        .name(NM_NAME)?
        .serve_at(NM_PATH, Manager { nm: Arc::clone(nm) })?
        .serve_at(AGENT_MANAGER_PATH, AgentManager { nm: Arc::clone(nm) })?
        .serve_at(SETTINGS_PATH, Settings { nm: Arc::clone(nm) })?
        .serve_at(WIFI_PATH, WifiDevice { nm: Arc::clone(nm) })?
        .serve_at(WIFI_PATH, WifiRadio { nm: Arc::clone(nm) })?
        .serve_at(WIRED_PATH, WiredDevice { nm: Arc::clone(nm) })?
        .serve_at(WIRED_PATH, WiredLink { nm: Arc::clone(nm) })?
        .serve_at(
            CONTROL_PATH,
            Control {
                nm: Arc::clone(nm),
                quit,
            },
        )?
        .build()
        .await?;

    // The seeded objects go on afterwards, because their paths are not known
    // until the test has said what it wants in range.
    let aps: Vec<String> = lock(nm).aps.keys().cloned().collect();
    for path in aps {
        serve_ap(&connection, nm, &path).await;
    }
    let profiles: Vec<String> = lock(nm).profiles.keys().cloned().collect();
    for path in profiles {
        serve_profile(&connection, nm, &path).await;
    }
    let actives: Vec<String> = lock(nm).actives.keys().cloned().collect();
    for path in actives {
        serve_active(&connection, nm, &path).await;
    }

    Ok(Served {
        connection,
        nm: Arc::clone(nm),
        quit: quit_rx,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_outcome_is_named_the_way_the_driver_spells_it() {
        assert_eq!(Outcome::parse("success"), Some(Outcome::Success));
        assert_eq!(Outcome::parse("auth_fail"), Some(Outcome::AuthFail));
        assert_eq!(Outcome::parse("auth-fail"), Some(Outcome::AuthFail));
        assert_eq!(Outcome::parse("slow"), Some(Outcome::Slow));
        assert_eq!(Outcome::parse("timeout"), Some(Outcome::Timeout));
        assert_eq!(Outcome::parse("nonsense"), None);
    }

    #[test]
    fn a_secured_access_point_carries_the_flags_a_real_router_does() {
        let ap = Ap::secured("Home", 70);
        // Privacy, and RSN with CCMP pairwise, CCMP group and PSK key
        // management — byte for byte what the developer's own router reports.
        assert_eq!(ap.flags, 0x1);
        assert_eq!(ap.rsn, 0x188);
        assert!(super::super::model::is_secured(ap.flags, ap.wpa, ap.rsn));

        let open = Ap::open("Cafe", 40);
        assert!(!super::super::model::is_secured(
            open.flags, open.wpa, open.rsn
        ));
    }

    #[test]
    fn object_paths_are_built_the_way_networkmanager_builds_them() {
        assert_eq!(
            ap_path("1"),
            "/org/freedesktop/NetworkManager/AccessPoint/1"
        );
        assert_eq!(
            active_path(7),
            "/org/freedesktop/NetworkManager/ActiveConnection/7"
        );
        // A uuid has dashes in it and an object path may not.
        assert_eq!(
            profile_path("a-b-c"),
            "/org/freedesktop/NetworkManager/Settings/a_b_c"
        );
    }

    #[test]
    fn a_fresh_fake_has_both_kinds_of_device_and_nothing_on_either() {
        let nm = Nm::new();
        assert_eq!(devices(&nm).len(), 2);
        nm.set_has_wired(false);
        assert_eq!(devices(&nm).len(), 1);
        nm.set_has_wifi(false);
        assert!(devices(&nm).is_empty());
    }
}
