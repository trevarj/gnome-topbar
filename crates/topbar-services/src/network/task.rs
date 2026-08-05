//! The one owner of the NetworkManager connection.
//!
//! Every proxy, every subscription and every piece of network state the panel
//! has lives in this task. Nothing else in the crate holds a NetworkManager
//! proxy, which is what makes "one client, not two" true rather than intended
//! — the connectivity watcher that used to have a connection of its own is now
//! a subscriber to this one.
//!
//! Signals arrive through a **single** bus match rule covering everything
//! NetworkManager emits under its own object namespace. A device, an access
//! point, an active connection and the settings store each publish through
//! different interfaces, and subscribing to them object by object would mean a
//! stream per access point — fifty subscriptions in a coffee shop, each set up
//! and torn down as the radio comes and goes. One rule and one decoder is the
//! same information at a fixed cost.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use tokio::sync::{mpsc, oneshot, watch};
use tracing::{debug, info, warn};
use zbus::proxy::CacheProperties;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, Value};

use super::flow::{Attempt, Event as FlowEvent, Failure, Step};
use super::model::{
    ACTIVE_ACTIVATED, ACTIVE_ACTIVATING, ACTIVE_DEACTIVATED, Access, ApView, CONNECTION_SETTING,
    DEVICE_ACTIVATED, DEVICE_ETHERNET, DEVICE_WIFI, NetworkState, Pending, PendingPrompt,
    TUNNEL_TYPES, VPN_TYPES, VpnKind, VpnView, WIFI_SETTING, WifiState, WiredState, collapse,
    is_secured, online_from_state, order_vpn, ssid_text, vpn_kind,
};
use super::proxy::{
    AccessPointProxy, ActiveConnectionProxy, AgentManagerProxy, ConnectionRef, DeviceProxy,
    DeviceWiredProxy, DeviceWirelessProxy, NetworkManagerProxy, SettingsConnectionProxy,
    SettingsProxy,
};
use super::secret_agent::{
    AGENT_IDENTIFIER, AGENT_PATH, AgentMessage, Secret, SecretAgent, SecretRequest,
};
use crate::error::SvcError;
use crate::state_store::StateStore;

/// How long between scans the panel is willing to ask for.
///
/// Opening Quick Settings asks for one; opening it four times in a row must
/// not make the card transmit four times.
const SCAN_INTERVAL: Duration = Duration::from_secs(10);

/// How long a scan may be believed to be running.
///
/// The spinner stops when the access-point list changes, which is what
/// finishing looks like. A scan that finds *nothing new* changes no list, and a
/// card that has gone away answers nothing at all — so the spinner is bounded
/// as well, because one that never stops is a panel that looks stuck.
const SCAN_SETTLE: Duration = Duration::from_secs(6);

/// How many events may be queued before a forwarder waits.
const EVENTS: usize = 64;

/// The object namespace every NetworkManager signal is emitted under.
const NM_NAMESPACE: &str = "/org/freedesktop/NetworkManager";
/// The well-known name.
const NM_NAME: &str = "org.freedesktop.NetworkManager";

/// The empty object path NetworkManager uses for "no object".
const NO_OBJECT: &str = "/";

/// What the panel may ask of the network.
pub(crate) enum Command {
    /// Switch the Wi-Fi radio.
    SetWifiEnabled {
        /// The state wanted.
        enabled: bool,
        /// Where to answer.
        reply: oneshot::Sender<Result<(), SvcError>>,
    },
    /// Join a network by name.
    Connect {
        /// Which one.
        ssid: String,
        /// Answered when the attempt has finished, one way or the other.
        reply: oneshot::Sender<Result<(), SvcError>>,
    },
    /// Leave the network the card is on.
    DisconnectWifi {
        /// Where to answer.
        reply: oneshot::Sender<Result<(), SvcError>>,
    },
    /// Look for networks.
    Scan {
        /// Where to answer.
        reply: oneshot::Sender<Result<(), SvcError>>,
    },
    /// Answer the password row.
    SubmitSecret {
        /// What the user typed.
        secret: Secret,
        /// Where to answer.
        reply: oneshot::Sender<Result<(), SvcError>>,
    },
    /// Take the password row away.
    CancelPrompt {
        /// Where to answer.
        reply: oneshot::Sender<Result<(), SvcError>>,
    },
    /// Switch one VPN profile.
    SetVpn {
        /// Which profile.
        uuid: String,
        /// Up or down.
        active: bool,
        /// Answered when NetworkManager has finished.
        reply: oneshot::Sender<Result<(), SvcError>>,
    },
}

/// Something the task has to react to.
enum Event {
    /// A signal says the device list changed.
    Devices,
    /// The manager's own properties moved.
    Manager,
    /// The settings store gained or lost a profile.
    Profiles,
    /// The list of active connections changed.
    Actives,
    /// One access point's properties moved.
    AccessPoint(OwnedObjectPath),
    /// An access point came or went.
    AccessPoints,
    /// One device's properties moved.
    Device(OwnedObjectPath),
    /// One active connection moved.
    Active {
        /// Which one.
        path: OwnedObjectPath,
        /// `NM_ACTIVE_CONNECTION_STATE_*`.
        state: u32,
        /// `NM_ACTIVE_CONNECTION_STATE_REASON_*`.
        reason: u32,
    },
    /// A scan has had long enough.
    ScanSettled,
}

/// One access point, as the task tracks it.
struct Ap {
    ssid: Option<String>,
    strength: u8,
    secured: bool,
}

/// The wireless card.
struct Wifi {
    path: OwnedObjectPath,
    state: u32,
    aps: HashMap<OwnedObjectPath, Ap>,
    active_ap: Option<OwnedObjectPath>,
}

/// The Ethernet port.
struct Wired {
    path: OwnedObjectPath,
    state: u32,
    carrier: bool,
    speed: u32,
    id: Option<String>,
}

/// One saved VPN profile.
struct VpnProfile {
    id: String,
    uuid: String,
    kind: VpnKind,
    path: OwnedObjectPath,
}

/// One connection that is up, or coming up.
struct Active {
    path: OwnedObjectPath,
    uuid: String,
    id: String,
    kind: String,
    state: u32,
}

/// A VPN switch the panel is waiting on.
struct VpnWait {
    uuid: String,
    activating: bool,
    reply: oneshot::Sender<Result<(), SvcError>>,
}

/// Everything the task knows, and everything it talks to.
struct World {
    connection: zbus::Connection,
    manager: NetworkManagerProxy<'static>,
    settings: SettingsProxy<'static>,
    access: Access,
    publisher: watch::Sender<Arc<NetworkState>>,
    store: Option<StateStore>,
    /// Where the signal forwarders — and the scan timer — put their events.
    events: mpsc::Sender<Event>,

    /// Whether NetworkManager answered at all.
    available: bool,
    nm_state: u32,
    wireless_enabled: bool,
    wifi: Option<Wifi>,
    wired: Option<Wired>,
    /// SSIDs with a saved profile, and where that profile lives.
    wifi_profiles: HashMap<String, OwnedObjectPath>,
    vpn_profiles: Vec<VpnProfile>,
    actives: Vec<Active>,

    attempt: Option<Attempt>,
    attempt_reply: Option<oneshot::Sender<Result<(), SvcError>>>,
    /// The active connection the current attempt is riding on.
    attempt_active: Option<OwnedObjectPath>,
    /// The agent's pending question, waiting on an answer from the panel, and
    /// the connection object it was asked about — so a cancel for some other
    /// connection cannot take this prompt away.
    pending_secret: Option<(OwnedObjectPath, oneshot::Sender<Option<Secret>>)>,
    /// A password typed while no question was outstanding, for the restart.
    stashed_secret: Option<Secret>,
    prompt: Option<PendingPrompt>,

    radio_pending: bool,
    vpn_wait: Option<VpnWait>,
    last_vpn_uuid: Option<String>,

    scanning: bool,
    last_scan: Option<Instant>,
    /// When the password row on screen stops being worth waiting for.
    ///
    /// The agent has a timeout of its own, but it only ends the *call*; this
    /// one ends the attempt, so a panel the user walked away from does not sit
    /// there with a password row and a spinner for ever.
    prompt_deadline: Option<tokio::time::Instant>,
}

/// Sleep until `deadline`, or for ever when there is none.
async fn wait_until(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

/// Follow NetworkManager until every handle is dropped.
pub(crate) async fn run(
    mut commands: mpsc::Receiver<Command>,
    publisher: watch::Sender<Arc<NetworkState>>,
    address: Option<String>,
    packaged: bool,
    last_vpn_uuid: Option<String>,
    store: Option<StateStore>,
) {
    let access = Access::decide(address.as_deref(), packaged);
    if access == Access::ReadOnly {
        info!(
            "network: read-only against this NetworkManager; no scan, no activation, no secret agent"
        );
    }

    let connection = match connect(address).await {
        Ok(connection) => connection,
        Err(error) => {
            info!("no system bus ({error}); the network is unknown");
            publish_unavailable(&publisher, access);
            return drain(commands).await;
        }
    };

    // Deliberately uncached. NetworkManager announces its overall state through
    // a signal of its own as well as through `PropertiesChanged`, and a proxy
    // that trusted its cache would answer with the value from before a
    // `StateChanged` that carried no property notification with it.
    let manager = match NetworkManagerProxy::builder(&connection)
        .cache_properties(CacheProperties::No)
        .build()
        .await
    {
        Ok(manager) => manager,
        Err(error) => {
            info!("no NetworkManager ({error}); the network is unknown");
            publish_unavailable(&publisher, access);
            return drain(commands).await;
        }
    };
    // Also uncached, and for a second reason: caching asks for every property
    // at build time, so a NetworkManager without a settings store — which is
    // what a minimal stand-in is — would fail here rather than simply having no
    // saved profiles to report.
    let settings = match SettingsProxy::builder(&connection)
        .cache_properties(CacheProperties::No)
        .build()
        .await
    {
        Ok(settings) => settings,
        Err(error) => {
            debug!("no NetworkManager settings store ({error})");
            publish_unavailable(&publisher, access);
            return drain(commands).await;
        }
    };

    let (events, mut queue) = mpsc::channel(EVENTS);
    let (agent_out, mut agent_in) = mpsc::channel(EVENTS);

    // Subscribed before the first read, so a change that lands between the two
    // is queued rather than lost.
    spawn_signals(&connection, events.clone());

    let mut world = World {
        connection: connection.clone(),
        manager,
        settings,
        access,
        publisher,
        store,
        events,
        available: false,
        nm_state: 0,
        wireless_enabled: false,
        wifi: None,
        wired: None,
        wifi_profiles: HashMap::new(),
        vpn_profiles: Vec::new(),
        actives: Vec::new(),
        attempt: None,
        attempt_reply: None,
        attempt_active: None,
        pending_secret: None,
        stashed_secret: None,
        prompt: None,
        radio_pending: false,
        vpn_wait: None,
        last_vpn_uuid,
        scanning: false,
        last_scan: None,
        prompt_deadline: None,
    };

    world.register_agent(agent_out).await;
    world.read_manager().await;
    world.read_devices().await;
    world.read_profiles().await;
    world.read_actives().await;
    world.publish();

    loop {
        let deadline = world.prompt_deadline;
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { break };
                world.command(command).await;
            }
            Some(event) = queue.recv() => world.event(event).await,
            Some(message) = agent_in.recv() => world.agent(message).await,
            () = wait_until(deadline) => {
                info!("network: nobody answered the password prompt");
                world.pending_secret = None;
                world.step(FlowEvent::TimedOut).await;
            }
        }
    }
}

/// The system bus, or the address a test handed us.
async fn connect(address: Option<String>) -> zbus::Result<zbus::Connection> {
    match address {
        Some(address) => {
            zbus::connection::Builder::address(address.as_str())?
                .build()
                .await
        }
        None => zbus::Connection::system().await,
    }
}

/// Publish "nothing is answering" and keep the optimistic online guess.
///
/// A machine with no NetworkManager runs systemd-networkd, connman, or nothing
/// at all; guessing "offline" there would leave the weather widget permanently
/// blank, and a failed fetch costs one timeout while a wrong guess costs the
/// whole feature.
fn publish_unavailable(publisher: &watch::Sender<Arc<NetworkState>>, access: Access) {
    let _ = publisher.send(Arc::new(NetworkState {
        access,
        ..NetworkState::default()
    }));
}

/// Answer every command with "unavailable" rather than blocking a caller.
async fn drain(mut commands: mpsc::Receiver<Command>) {
    while let Some(command) = commands.recv().await {
        let _ = reply_of(command).send(Err(SvcError::Network(
            "NetworkManager is not running".into(),
        )));
    }
}

/// Where a command's answer goes.
fn reply_of(command: Command) -> oneshot::Sender<Result<(), SvcError>> {
    match command {
        Command::SetWifiEnabled { reply, .. }
        | Command::Connect { reply, .. }
        | Command::DisconnectWifi { reply }
        | Command::Scan { reply }
        | Command::SubmitSecret { reply, .. }
        | Command::CancelPrompt { reply }
        | Command::SetVpn { reply, .. } => reply,
    }
}

/// Follow every signal NetworkManager emits, through one match rule.
fn spawn_signals(connection: &zbus::Connection, events: mpsc::Sender<Event>) {
    let connection = connection.clone();
    tokio::spawn(async move {
        let rule = match zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .sender(NM_NAME)
            .and_then(|builder| builder.path_namespace(NM_NAMESPACE))
        {
            Ok(builder) => builder.build(),
            Err(error) => return warn!("cannot build the NetworkManager match rule: {error}"),
        };
        let mut stream = match zbus::MessageStream::for_match_rule(rule, &connection, None).await {
            Ok(stream) => stream,
            Err(error) => return warn!("cannot follow NetworkManager's signals: {error}"),
        };

        while let Some(Ok(message)) = stream.next().await {
            let Some(event) = decode(&message) else {
                continue;
            };
            if events.send(event).await.is_err() {
                break;
            }
        }
        debug!("NetworkManager's signal stream ended");
    });
}

/// Turn one signal into something the task cares about, or nothing.
fn decode(message: &zbus::Message) -> Option<Event> {
    let header = message.header();
    let interface = header.interface()?.to_string();
    let member = header.member()?.to_string();
    let path = header.path()?.to_owned();

    match (interface.as_str(), member.as_str()) {
        ("org.freedesktop.NetworkManager", "DeviceAdded" | "DeviceRemoved") => Some(Event::Devices),
        ("org.freedesktop.NetworkManager", "StateChanged") => Some(Event::Manager),
        (
            "org.freedesktop.NetworkManager.Device.Wireless",
            "AccessPointAdded" | "AccessPointRemoved",
        ) => Some(Event::AccessPoints),
        ("org.freedesktop.NetworkManager.Settings", "NewConnection" | "ConnectionRemoved") => {
            Some(Event::Profiles)
        }
        ("org.freedesktop.NetworkManager.Connection.Active", "StateChanged") => {
            let (state, reason) = message.body().deserialize::<(u32, u32)>().ok()?;
            Some(Event::Active {
                path: path.into(),
                state,
                reason,
            })
        }
        ("org.freedesktop.DBus.Properties", "PropertiesChanged") => {
            let body = message.body();
            let (interface, changed, _invalidated) = body
                .deserialize::<(String, HashMap<String, Value<'_>>, Vec<String>)>()
                .ok()?;
            properties_changed(&interface, &changed, path.into())
        }
        _ => None,
    }
}

/// Which event a `PropertiesChanged` on one of NetworkManager's objects is.
fn properties_changed(
    interface: &str,
    changed: &HashMap<String, Value<'_>>,
    path: OwnedObjectPath,
) -> Option<Event> {
    match interface {
        "org.freedesktop.NetworkManager" => {
            if changed.contains_key("ActiveConnections") {
                Some(Event::Actives)
            } else if changed.contains_key("WirelessEnabled")
                || changed.contains_key("State")
                || changed.contains_key("Devices")
            {
                Some(Event::Manager)
            } else {
                None
            }
        }
        "org.freedesktop.NetworkManager.AccessPoint" => Some(Event::AccessPoint(path)),
        "org.freedesktop.NetworkManager.Device"
        | "org.freedesktop.NetworkManager.Device.Wireless"
        | "org.freedesktop.NetworkManager.Device.Wired" => Some(Event::Device(path)),
        "org.freedesktop.NetworkManager.Connection.Active" => {
            let state = changed.get("State")?;
            let state = u32::try_from(state.try_clone().ok()?).ok()?;
            Some(Event::Active {
                path,
                state,
                reason: 0,
            })
        }
        _ => None,
    }
}

/// Whether an object path means "nothing".
fn is_object(path: &OwnedObjectPath) -> bool {
    path.as_str() != NO_OBJECT && !path.as_str().is_empty()
}

impl World {
    /// Put the panel's secret agent on the bus, if this panel may.
    ///
    /// Gated exactly like every other mutation. Registering a second agent on
    /// the machine's real bus would put this process in the queue for the
    /// prompts the session's actual panel is waiting for, which is the one
    /// failure here that a user would experience as their desktop breaking.
    async fn register_agent(&mut self, requests: mpsc::Sender<AgentMessage>) {
        if !self.access.writable() {
            info!("network: not registering a secret agent (read-only)");
            return;
        }
        if let Err(error) = self
            .connection
            .object_server()
            .at(AGENT_PATH, SecretAgent::new(requests))
            .await
        {
            warn!("cannot serve the secret agent: {error}");
            return;
        }
        let manager = match AgentManagerProxy::new(&self.connection).await {
            Ok(manager) => manager,
            Err(error) => return warn!("no NetworkManager agent manager: {error}"),
        };
        // Capabilities 0: the panel does not do VPN hints, because it does not
        // answer VPN secret requests at all. Claiming the capability and then
        // refusing would make NetworkManager wait on an agent that was never
        // going to help.
        match manager
            .register_with_capabilities(AGENT_IDENTIFIER, 0)
            .await
        {
            Ok(()) => info!("network: secret agent registered as {AGENT_IDENTIFIER}"),
            Err(error) => warn!("could not register the secret agent: {error}"),
        }
    }

    /// Read the manager's own properties.
    ///
    /// Whether the first one answers *is* whether NetworkManager is there: the
    /// proxies build without a round trip, so nothing before this point can
    /// tell a running daemon from an absent one.
    async fn read_manager(&mut self) {
        match self.manager.nm_state().await {
            Ok(state) => {
                self.available = true;
                self.nm_state = state;
            }
            Err(error) => {
                if self.available {
                    info!("NetworkManager stopped answering ({error})");
                }
                self.available = false;
                self.nm_state = 0;
            }
        }
        self.wireless_enabled = self.manager.wireless_enabled().await.unwrap_or(false);
    }

    /// Find the wireless card and the Ethernet port, and read them.
    async fn read_devices(&mut self) {
        let devices = self.manager.get_devices().await.unwrap_or_default();
        let mut wifi = None;
        let mut wired = None;

        for path in devices {
            let Ok(device) = self.device(&path).await else {
                continue;
            };
            let kind = device.device_type().await.unwrap_or(0);
            let state = device.state().await.unwrap_or(0);
            match kind {
                DEVICE_WIFI if wifi.is_none() => {
                    wifi = Some(Wifi {
                        path: path.clone(),
                        state,
                        aps: HashMap::new(),
                        active_ap: None,
                    });
                }
                DEVICE_ETHERNET if wired.is_none() => {
                    let (carrier, speed) = match self.wired_proxy(&path).await {
                        Ok(proxy) => (
                            proxy.carrier().await.unwrap_or(false),
                            proxy.speed().await.unwrap_or(0),
                        ),
                        Err(_) => (false, 0),
                    };
                    let id = match device.active_connection().await {
                        Ok(active) if is_object(&active) => self.active_id(&active).await,
                        _ => None,
                    };
                    wired = Some(Wired {
                        path: path.clone(),
                        state,
                        carrier,
                        speed,
                        id,
                    });
                }
                _ => {}
            }
        }

        self.wifi = wifi;
        self.wired = wired;
        self.read_access_points().await;
    }

    /// Read every access point the card can hear.
    async fn read_access_points(&mut self) {
        let Some(path) = self.wifi.as_ref().map(|wifi| wifi.path.clone()) else {
            return;
        };
        let Ok(wireless) = self.wireless_proxy(&path).await else {
            return;
        };

        let active = wireless.active_access_point().await.ok().filter(is_object);
        let paths = wireless.access_points().await.unwrap_or_default();

        let mut aps = HashMap::new();
        for ap_path in paths {
            if let Some(ap) = self.read_access_point(&ap_path).await {
                aps.insert(ap_path, ap);
            }
        }
        if let Some(wifi) = self.wifi.as_mut() {
            wifi.aps = aps;
            wifi.active_ap = active;
        }
        // The list arriving is what a scan finishing looks like from here.
        self.scanning = false;
    }

    /// Read one access point.
    async fn read_access_point(&self, path: &OwnedObjectPath) -> Option<Ap> {
        let proxy = self.ap_proxy(path).await.ok()?;
        let ssid = proxy.ssid().await.ok().as_deref().and_then(ssid_text);
        let strength = proxy.strength().await.unwrap_or(0);
        let secured = is_secured(
            proxy.flags().await.unwrap_or(0),
            proxy.wpa_flags().await.unwrap_or(0),
            proxy.rsn_flags().await.unwrap_or(0),
        );
        Some(Ap {
            ssid,
            strength,
            secured,
        })
    }

    /// Read every saved profile: one `ListConnections`, one `GetSettings` each.
    ///
    /// This is the whole of the "known networks" question, and it runs when a
    /// profile is added or removed rather than on every repaint. v1 answered it
    /// by running `nmcli` once for the list and then **once more per profile**,
    /// every thirty seconds, for the SSID of each — which is the N+1 the plan
    /// names as a defect.
    async fn read_profiles(&mut self) {
        let paths = self.settings.list_connections().await.unwrap_or_default();
        let mut wifi_profiles = HashMap::new();
        let mut vpn_profiles = Vec::new();

        for path in paths {
            let Ok(proxy) = self.settings_connection(&path).await else {
                continue;
            };
            let Ok(settings) = proxy.get_settings().await else {
                continue;
            };
            let Some(connection) = settings.get(CONNECTION_SETTING) else {
                continue;
            };
            let kind = string_of(connection, "type").unwrap_or_default();

            if kind == WIFI_SETTING {
                let ssid = settings
                    .get(WIFI_SETTING)
                    .and_then(|wifi| wifi.get("ssid"))
                    .and_then(|value| Vec::<u8>::try_from(value.try_clone().ok()?).ok())
                    .as_deref()
                    .and_then(ssid_text);
                if let Some(ssid) = ssid {
                    wifi_profiles.insert(ssid, path.clone());
                }
                continue;
            }

            if VPN_TYPES.contains(&kind.as_str()) {
                let Some(uuid) = string_of(connection, "uuid") else {
                    continue;
                };
                let service = settings
                    .get("vpn")
                    .and_then(|vpn| string_of(vpn, "service-type"));
                vpn_profiles.push(VpnProfile {
                    id: string_of(connection, "id").unwrap_or_else(|| uuid.clone()),
                    uuid,
                    kind: vpn_kind(&kind, service.as_deref()),
                    path: path.clone(),
                });
            }
        }

        self.wifi_profiles = wifi_profiles;
        self.vpn_profiles = vpn_profiles;
    }

    /// Read every active connection.
    async fn read_actives(&mut self) {
        let paths = self.manager.active_connections().await.unwrap_or_default();
        let mut actives = Vec::new();
        for path in paths {
            let Ok(proxy) = self.active_proxy(&path).await else {
                continue;
            };
            actives.push(Active {
                uuid: proxy.uuid().await.unwrap_or_default(),
                id: proxy.id().await.unwrap_or_default(),
                kind: proxy.connection_type().await.unwrap_or_default(),
                state: proxy.state().await.unwrap_or(0),
                path,
            });
        }
        self.actives = actives;
        self.remember_vpn();
    }

    /// Remember the last VPN the user actually used.
    fn remember_vpn(&mut self) {
        let known: HashSet<&str> = self
            .vpn_profiles
            .iter()
            .map(|profile| profile.uuid.as_str())
            .collect();
        let current = self.actives.iter().find(|active| {
            active.state == ACTIVE_ACTIVATED
                && VPN_TYPES.contains(&active.kind.as_str())
                && known.contains(active.uuid.as_str())
        });
        let Some(active) = current else { return };
        if self.last_vpn_uuid.as_deref() == Some(active.uuid.as_str()) {
            return;
        }
        self.last_vpn_uuid = Some(active.uuid.clone());
        if let Some(store) = &self.store {
            let uuid = active.uuid.clone();
            store.update(move |state| state.network.last_vpn_uuid = Some(uuid));
        }
    }

    /// React to one signal.
    async fn event(&mut self, event: Event) {
        match event {
            Event::Devices => {
                self.read_devices().await;
            }
            Event::Manager => {
                self.read_manager().await;
                // A radio switch is only ever waited on until NetworkManager
                // says it happened, which is exactly this.
                self.finish_radio();
            }
            Event::Profiles => self.read_profiles().await,
            Event::Actives => self.read_actives().await,
            Event::AccessPoints => self.read_access_points().await,
            Event::AccessPoint(path) => {
                if let Some(ap) = self.read_access_point(&path).await
                    && let Some(wifi) = self.wifi.as_mut()
                    && let Some(slot) = wifi.aps.get_mut(&path)
                {
                    *slot = ap;
                }
            }
            Event::Device(path) => self.read_device(&path).await,
            Event::Active {
                path,
                state,
                reason,
            } => self.active_changed(&path, state, reason).await,
            Event::ScanSettled => self.scanning = false,
        }
        self.publish();
    }

    /// Re-read one device that said something changed.
    async fn read_device(&mut self, path: &OwnedObjectPath) {
        if self.wifi.as_ref().is_some_and(|wifi| wifi.path == *path) {
            let state = match self.device(path).await {
                Ok(device) => device.state().await.unwrap_or(0),
                Err(_) => return,
            };
            let active_ap = match self.wireless_proxy(path).await {
                Ok(wireless) => wireless.active_access_point().await.ok().filter(is_object),
                Err(_) => None,
            };
            if let Some(wifi) = self.wifi.as_mut() {
                wifi.state = state;
                wifi.active_ap = active_ap;
            }
            return;
        }

        if self.wired.as_ref().is_some_and(|wired| wired.path == *path) {
            let (state, id) = match self.device(path).await {
                Ok(device) => {
                    let state = device.state().await.unwrap_or(0);
                    let id = match device.active_connection().await {
                        Ok(active) if is_object(&active) => self.active_id(&active).await,
                        _ => None,
                    };
                    (state, id)
                }
                Err(_) => return,
            };
            let (carrier, speed) = match self.wired_proxy(path).await {
                Ok(proxy) => (
                    proxy.carrier().await.unwrap_or(false),
                    proxy.speed().await.unwrap_or(0),
                ),
                Err(_) => (false, 0),
            };
            if let Some(wired) = self.wired.as_mut() {
                wired.state = state;
                wired.carrier = carrier;
                wired.speed = speed;
                wired.id = id;
            }
        }
    }

    /// React to an active connection moving.
    async fn active_changed(&mut self, path: &OwnedObjectPath, state: u32, reason: u32) {
        if let Some(slot) = self
            .actives
            .iter_mut()
            .find(|active| active.path.as_str() == path.as_str())
        {
            slot.state = state;
        } else if state == ACTIVE_ACTIVATING || state == ACTIVE_ACTIVATED {
            // Something came up that was not in the list when it was last read.
            self.read_actives().await;
        }

        self.vpn_settled(path, state, reason);

        if self
            .attempt_active
            .as_ref()
            .is_some_and(|watched| watched.as_str() == path.as_str())
        {
            self.step(FlowEvent::ActiveChanged { state, reason }).await;
        }

        if state == ACTIVE_DEACTIVATED {
            self.actives
                .retain(|active| active.path.as_str() != path.as_str());
        }
        self.remember_vpn();
    }

    /// Whether a VPN the panel was switching has finished.
    fn vpn_settled(&mut self, path: &OwnedObjectPath, state: u32, reason: u32) {
        let Some(wait) = self.vpn_wait.as_ref() else {
            return;
        };
        let matches = self
            .actives
            .iter()
            .any(|active| active.path.as_str() == path.as_str() && active.uuid == wait.uuid);
        if !matches {
            return;
        }
        let answer = match (wait.activating, state) {
            (true, ACTIVE_ACTIVATED) | (false, ACTIVE_DEACTIVATED) => Some(Ok(())),
            (true, ACTIVE_DEACTIVATED) => Some(Err(SvcError::Network(format!(
                "the VPN did not come up (reason {reason})"
            )))),
            _ => None,
        };
        if let Some(answer) = answer
            && let Some(wait) = self.vpn_wait.take()
        {
            let _ = wait.reply.send(answer);
        }
    }

    /// Deal with a message from the secret agent.
    async fn agent(&mut self, message: AgentMessage) {
        match message {
            AgentMessage::Secrets(request) => self.secrets_requested(*request).await,
            AgentMessage::Cancel(cancel) => {
                let ours = self
                    .pending_secret
                    .as_ref()
                    .is_some_and(|(path, _)| *path == cancel.path);
                if !ours {
                    debug!(
                        "network: a cancel for {} is not about the prompt on screen",
                        cancel.path.as_str()
                    );
                    return;
                }
                debug!("network: NetworkManager cancelled {}", cancel.setting);
                // Dropping the sender is the cancel: the agent's `GetSecrets`
                // is awaiting it and answers `UserCanceled` the moment it goes.
                self.pending_secret = None;
                self.clear_prompt();
                self.publish();
            }
        }
    }

    /// NetworkManager wants a password.
    async fn secrets_requested(&mut self, request: SecretRequest) {
        let SecretRequest {
            ssid,
            flags,
            reply,
            path,
        } = request;

        // No attempt in flight means something else asked — an autoconnect at
        // boot, say. The panel has nothing stored and no row to put up.
        let Some(attempt) = self.attempt.as_ref() else {
            debug!("network: a secret was asked for with no attempt in flight");
            let _ = reply.send(None);
            return;
        };
        if let Some(ssid) = &ssid
            && ssid != attempt.ssid()
        {
            debug!(
                "network: a secret was asked for {ssid}, not {}",
                attempt.ssid()
            );
            let _ = reply.send(None);
            return;
        }

        // A password typed before the question arrived — which is what a retry
        // after a failed attempt looks like — is answered straight away.
        if let Some(secret) = self.stashed_secret.take() {
            let _ = reply.send(Some(secret));
            self.step(FlowEvent::SecretSubmitted).await;
            return;
        }

        self.pending_secret = Some((path, reply));
        self.step(FlowEvent::SecretsRequested {
            request_new: flags & super::model::SECRET_REQUEST_NEW != 0,
        })
        .await;
    }

    /// Feed the connect state machine and act on what it says.
    async fn step(&mut self, event: FlowEvent) {
        let Some(attempt) = self.attempt.as_mut() else {
            return;
        };
        let ssid = attempt.ssid().to_string();
        match attempt.apply(event) {
            Step::Wait => {}
            Step::Prompt { attempt } => self.ask(PendingPrompt { ssid, attempt }),
            Step::Reprompt { attempt, delete } => {
                let restart = delete.is_some();
                self.delete_profile(delete).await;
                self.ask(PendingPrompt { ssid, attempt });
                if restart {
                    // The activation died with the profile; the next password
                    // starts a fresh one.
                    self.attempt_active = None;
                }
            }
            Step::Connected => {
                info!("network: joined {ssid}");
                self.finish_attempt(Ok(()));
            }
            Step::Failed { reason, delete } => {
                self.delete_profile(delete).await;
                self.finish_attempt(match reason {
                    // The user pressing Cancel is not an error to report back
                    // at them under the row they just dismissed.
                    Failure::Cancelled => Ok(()),
                    Failure::TimedOut => Err(SvcError::Network(format!(
                        "nothing answered the password prompt for {ssid}"
                    ))),
                    Failure::Refused(reason) => Err(SvcError::Network(format!(
                        "NetworkManager refused {ssid} (reason {reason})"
                    ))),
                });
            }
        }
        self.publish();
    }

    /// Put a password row on screen, and start the clock on it.
    fn ask(&mut self, prompt: PendingPrompt) {
        self.prompt = Some(prompt);
        self.prompt_deadline =
            Some(tokio::time::Instant::now() + super::secret_agent::PROMPT_TIMEOUT);
    }

    /// Take it away.
    fn clear_prompt(&mut self) {
        self.prompt = None;
        self.prompt_deadline = None;
    }

    /// Bin a profile the panel added and could not use.
    async fn delete_profile(&mut self, path: Option<String>) {
        let Some(path) = path else { return };
        let Ok(path) = OwnedObjectPath::try_from(path.as_str()) else {
            return;
        };
        match self.settings_connection(&path).await {
            Ok(proxy) => match proxy.delete().await {
                Ok(()) => info!("network: removed the profile a failed attempt created"),
                Err(error) => warn!("could not remove {}: {error}", path.as_str()),
            },
            Err(error) => warn!("could not reach {}: {error}", path.as_str()),
        }
    }

    /// End the attempt, answering whoever asked for it.
    fn finish_attempt(&mut self, answer: Result<(), SvcError>) {
        self.attempt = None;
        self.attempt_active = None;
        self.clear_prompt();
        self.pending_secret = None;
        self.stashed_secret = None;
        if let Some(reply) = self.attempt_reply.take() {
            let _ = reply.send(answer);
        }
    }

    /// A radio switch has been observed; stop waiting on it.
    fn finish_radio(&mut self) {
        self.radio_pending = false;
    }

    /// Run one command.
    async fn command(&mut self, command: Command) {
        match command {
            Command::SetWifiEnabled { enabled, reply } => {
                let answer = self.set_wifi_enabled(enabled).await;
                let _ = reply.send(answer);
            }
            Command::Connect { ssid, reply } => {
                if let Err(error) = self.start_connect(&ssid).await {
                    let _ = reply.send(Err(error));
                } else {
                    // Answered when the attempt finishes, however it does.
                    self.attempt_reply = Some(reply);
                }
            }
            Command::DisconnectWifi { reply } => {
                let answer = self.disconnect_wifi().await;
                let _ = reply.send(answer);
            }
            Command::Scan { reply } => {
                let answer = self.scan().await;
                let _ = reply.send(answer);
            }
            Command::SubmitSecret { secret, reply } => {
                let answer = self.submit_secret(secret).await;
                let _ = reply.send(answer);
            }
            Command::CancelPrompt { reply } => {
                self.pending_secret = None;
                self.step(FlowEvent::Cancelled).await;
                let _ = reply.send(Ok(()));
            }
            Command::SetVpn {
                uuid,
                active,
                reply,
            } => self.set_vpn(uuid, active, reply).await,
        }
        self.publish();
    }

    /// Refuse a change this panel is not allowed to make.
    fn read_only(&self) -> SvcError {
        SvcError::Network("this build only reads the network".into())
    }

    /// Switch the radio.
    async fn set_wifi_enabled(&mut self, enabled: bool) -> Result<(), SvcError> {
        if !self.access.writable() {
            return Err(self.read_only());
        }
        self.radio_pending = true;
        self.publish();
        let answer = self
            .manager
            .set_wireless_enabled(enabled)
            .await
            .map_err(|error| SvcError::Network(error.to_string()));
        if answer.is_err() {
            self.radio_pending = false;
        }
        answer
    }

    /// Ask the card to look around, at most once every [`SCAN_INTERVAL`].
    async fn scan(&mut self) -> Result<(), SvcError> {
        if !self.access.writable() {
            // Not an error the user needs to see: the panel asks for a scan
            // every time it opens, and a development build simply lists what
            // NetworkManager already knows about.
            debug!("network: not scanning (read-only)");
            return Ok(());
        }
        if !self.wireless_enabled {
            return Ok(());
        }
        if self
            .last_scan
            .is_some_and(|last| last.elapsed() < SCAN_INTERVAL)
        {
            return Ok(());
        }
        let Some(path) = self.wifi.as_ref().map(|wifi| wifi.path.clone()) else {
            return Ok(());
        };
        let wireless = self
            .wireless_proxy(&path)
            .await
            .map_err(|error| SvcError::Network(error.to_string()))?;

        self.last_scan = Some(Instant::now());
        self.scanning = true;
        self.publish();
        // The backstop, so a card that answers nothing does not leave a
        // spinner turning for the rest of the session.
        let events = self.events.clone();
        tokio::spawn(async move {
            tokio::time::sleep(SCAN_SETTLE).await;
            let _ = events.send(Event::ScanSettled).await;
        });
        match wireless.request_scan(HashMap::new()).await {
            Ok(()) => Ok(()),
            Err(error) => {
                self.scanning = false;
                // A scan refused while the card is busy associating is normal
                // and not worth a caption under the list.
                debug!("network: the card would not scan ({error})");
                Ok(())
            }
        }
    }

    /// Leave the network the card is on.
    async fn disconnect_wifi(&mut self) -> Result<(), SvcError> {
        if !self.access.writable() {
            return Err(self.read_only());
        }
        let Some(path) = self.wifi.as_ref().map(|wifi| wifi.path.clone()) else {
            return Err(SvcError::Network("there is no wireless card".into()));
        };
        let device = self
            .device(&path)
            .await
            .map_err(|error| SvcError::Network(error.to_string()))?;
        device
            .disconnect()
            .await
            .map_err(|error| SvcError::Network(error.to_string()))
    }

    /// Begin joining `ssid`.
    ///
    /// A saved network is activated from its own profile. An unsaved one goes
    /// through `AddAndActivateConnection` with an **empty** connection
    /// dictionary: NetworkManager then builds the profile from the access point
    /// it was pointed at, which is how the right key management is chosen for
    /// WPA2, WPA3 and OWE without the panel guessing — and it is also why no
    /// password is ever part of this call. The password arrives later, in the
    /// reply to NetworkManager's own `GetSecrets`.
    async fn start_connect(&mut self, ssid: &str) -> Result<(), SvcError> {
        if !self.access.writable() {
            return Err(self.read_only());
        }
        if self.attempt.is_some() {
            return Err(SvcError::Network("another network is connecting".into()));
        }
        let Some(device) = self.wifi.as_ref().map(|wifi| wifi.path.clone()) else {
            return Err(SvcError::Network("there is no wireless card".into()));
        };
        let ap = self
            .best_ap(ssid)
            .ok_or_else(|| SvcError::Network(format!("{ssid} is not in range")))?;
        let saved = self.wifi_profiles.get(ssid).cloned();

        let device_path = ObjectPath::try_from(device.as_str())
            .map_err(|error| SvcError::Network(error.to_string()))?;
        let ap_path = ObjectPath::try_from(ap.as_str())
            .map_err(|error| SvcError::Network(error.to_string()))?;

        let (added, active) = match saved {
            Some(profile) => {
                let profile_path = ObjectPath::try_from(profile.as_str())
                    .map_err(|error| SvcError::Network(error.to_string()))?;
                let active = self
                    .manager
                    .activate_connection(&profile_path, &device_path, &ap_path)
                    .await
                    .map_err(|error| SvcError::Network(error.to_string()))?;
                (None, active)
            }
            None => {
                let blank: ConnectionRef<'_> = HashMap::new();
                let (settings, active) = self
                    .manager
                    .add_and_activate_connection(blank, &device_path, &ap_path)
                    .await
                    .map_err(|error| SvcError::Network(error.to_string()))?;
                (Some(settings.as_str().to_string()), active)
            }
        };

        info!("network: joining {ssid}");
        self.attempt = Some(Attempt::new(ssid.to_string(), added));
        self.attempt_active = Some(active);
        Ok(())
    }

    /// Answer the password row.
    ///
    /// Either there is a question outstanding, in which case the answer goes
    /// straight back down the agent's delayed reply — or the attempt already
    /// died and the answer is kept for the retry that is about to start.
    async fn submit_secret(&mut self, secret: Secret) -> Result<(), SvcError> {
        if self.attempt.is_none() {
            return Err(SvcError::Network("there is nothing to answer".into()));
        }
        if let Some((_, reply)) = self.pending_secret.take() {
            let _ = reply.send(Some(secret));
            self.clear_prompt();
            self.step(FlowEvent::SecretSubmitted).await;
            return Ok(());
        }

        self.stashed_secret = Some(secret);
        self.clear_prompt();
        let ssid = self
            .attempt
            .as_ref()
            .map(|attempt| attempt.ssid().to_string())
            .unwrap_or_default();
        self.restart_attempt(&ssid).await
    }

    /// Start the activation again, keeping the attempt and its password.
    async fn restart_attempt(&mut self, ssid: &str) -> Result<(), SvcError> {
        let Some(device) = self.wifi.as_ref().map(|wifi| wifi.path.clone()) else {
            return Err(SvcError::Network("there is no wireless card".into()));
        };
        let ap = self
            .best_ap(ssid)
            .ok_or_else(|| SvcError::Network(format!("{ssid} is not in range")))?;
        let device_path = ObjectPath::try_from(device.as_str())
            .map_err(|error| SvcError::Network(error.to_string()))?;
        let ap_path = ObjectPath::try_from(ap.as_str())
            .map_err(|error| SvcError::Network(error.to_string()))?;

        let blank: ConnectionRef<'_> = HashMap::new();
        let (settings, active) = self
            .manager
            .add_and_activate_connection(blank, &device_path, &ap_path)
            .await
            .map_err(|error| SvcError::Network(error.to_string()))?;

        if let Some(attempt) = self.attempt.as_mut() {
            attempt.set_added(Some(settings.as_str().to_string()));
        }
        self.attempt_active = Some(active);
        Ok(())
    }

    /// Switch one VPN profile.
    async fn set_vpn(
        &mut self,
        uuid: String,
        active: bool,
        reply: oneshot::Sender<Result<(), SvcError>>,
    ) {
        if !self.access.writable() {
            let _ = reply.send(Err(self.read_only()));
            return;
        }
        if self.vpn_wait.is_some() {
            let _ = reply.send(Err(SvcError::Network("another VPN is switching".into())));
            return;
        }

        let answer = if active {
            self.activate_vpn(&uuid).await
        } else {
            self.deactivate_vpn(&uuid).await
        };
        match answer {
            Ok(()) => {
                self.vpn_wait = Some(VpnWait {
                    uuid,
                    activating: active,
                    reply,
                });
            }
            Err(error) => {
                let _ = reply.send(Err(error));
            }
        }
    }

    /// Bring a VPN up.
    async fn activate_vpn(&self, uuid: &str) -> Result<(), SvcError> {
        let profile = self
            .vpn_profiles
            .iter()
            .find(|profile| profile.uuid == uuid)
            .ok_or_else(|| SvcError::Network("no such VPN profile".into()))?;
        let path = ObjectPath::try_from(profile.path.as_str())
            .map_err(|error| SvcError::Network(error.to_string()))?;
        let nothing = ObjectPath::try_from(NO_OBJECT)
            .map_err(|error| SvcError::Network(error.to_string()))?;
        self.manager
            .activate_connection(&path, &nothing, &nothing)
            .await
            .map(|_| ())
            .map_err(|error| SvcError::Network(error.to_string()))
    }

    /// Take a VPN down.
    async fn deactivate_vpn(&self, uuid: &str) -> Result<(), SvcError> {
        let active = self
            .actives
            .iter()
            .find(|active| active.uuid == uuid)
            .ok_or_else(|| SvcError::Network("that VPN is not up".into()))?;
        let path = ObjectPath::try_from(active.path.as_str())
            .map_err(|error| SvcError::Network(error.to_string()))?;
        self.manager
            .deactivate_connection(&path)
            .await
            .map_err(|error| SvcError::Network(error.to_string()))
    }

    /// The strongest access point advertising `ssid`.
    fn best_ap(&self, ssid: &str) -> Option<OwnedObjectPath> {
        let wifi = self.wifi.as_ref()?;
        wifi.aps
            .iter()
            .filter(|(_, ap)| ap.ssid.as_deref() == Some(ssid))
            .max_by_key(|(_, ap)| ap.strength)
            .map(|(path, _)| path.clone())
    }

    /// One active connection's name.
    async fn active_id(&self, path: &OwnedObjectPath) -> Option<String> {
        self.active_proxy(path).await.ok()?.id().await.ok()
    }

    /// Build the snapshot and publish it if anything moved.
    fn publish(&self) {
        let next = self.snapshot();
        self.publisher.send_if_modified(|current| {
            if **current == next {
                false
            } else {
                *current = Arc::new(next);
                true
            }
        });
    }

    /// Everything the panel is shown, from everything the task knows.
    fn snapshot(&self) -> NetworkState {
        let connecting = self.attempt.as_ref().map(|attempt| attempt.ssid());
        let active_ssid = self.wifi.as_ref().and_then(|wifi| {
            let path = wifi.active_ap.as_ref()?;
            wifi.aps.get(path)?.ssid.clone()
        });

        let mut list: Vec<ApView> = Vec::new();
        if let Some(wifi) = &self.wifi
            && self.wireless_enabled
        {
            for ap in wifi.aps.values() {
                let Some(ssid) = ap.ssid.clone() else {
                    // A hidden network has no name to put on a row, and no way
                    // to be joined from a list.
                    continue;
                };
                let mut view = ApView::new(ssid.clone(), ap.strength, ap.secured);
                view.known = self.wifi_profiles.contains_key(&ssid);
                view.active =
                    active_ssid.as_deref() == Some(ssid.as_str()) && wifi.state == DEVICE_ACTIVATED;
                view.connecting = connecting == Some(ssid.as_str());
                list.push(view);
            }
        }
        let list = collapse(list);
        let active = list.iter().find(|ap| ap.active).cloned();

        let wifi = WifiState {
            present: self.wifi.is_some(),
            enabled: self.wireless_enabled,
            scanning: self.scanning,
            active,
            list,
        };

        let wired = self
            .wired
            .as_ref()
            .map_or_else(WiredState::default, |wired| WiredState {
                present: true,
                carrier: wired.carrier,
                connected: wired.state == DEVICE_ACTIVATED,
                id: wired.id.clone(),
                speed_mbps: wired.speed,
            });

        let mut vpn = self.vpn_views();
        order_vpn(&mut vpn, self.last_vpn_uuid.as_deref());

        NetworkState {
            available: self.available,
            // A machine whose NetworkManager is not answering is assumed to be
            // online: it may be running systemd-networkd, connman, or nothing
            // at all, and a failed fetch costs one timeout while a wrong guess
            // costs the weather widget entirely.
            online: !self.available || online_from_state(self.nm_state),
            wifi,
            wired,
            vpn,
            pending: self.pending(),
            prompt: self.prompt.clone(),
            access: self.access,
        }
    }

    /// Every VPN row: the saved profiles, and any tunnel somebody else raised.
    fn vpn_views(&self) -> Vec<VpnView> {
        let switching = self.vpn_wait.as_ref().map(|wait| wait.uuid.as_str());
        let mut views: Vec<VpnView> =
            self.vpn_profiles
                .iter()
                .map(|profile| VpnView {
                    id: profile.id.clone(),
                    uuid: profile.uuid.clone(),
                    kind: profile.kind,
                    active: self.actives.iter().any(|active| {
                        active.uuid == profile.uuid && active.state == ACTIVE_ACTIVATED
                    }),
                    pending: switching == Some(profile.uuid.as_str()),
                })
                .collect();

        let known: HashSet<&str> = self
            .vpn_profiles
            .iter()
            .map(|profile| profile.uuid.as_str())
            .collect();
        for active in &self.actives {
            if !TUNNEL_TYPES.contains(&active.kind.as_str())
                || known.contains(active.uuid.as_str())
                || active.state != ACTIVE_ACTIVATED
            {
                continue;
            }
            // A tunnel with no profile behind it: an `openvpn` somebody ran in
            // a terminal, or a corporate client of its own. Shown so the user
            // knows their traffic is going somewhere, never switched.
            views.push(VpnView {
                id: active.id.clone(),
                uuid: active.uuid.clone(),
                kind: VpnKind::External,
                active: true,
                pending: false,
            });
        }
        views
    }

    /// What the panel is waiting for, if anything.
    fn pending(&self) -> Option<Pending> {
        if self.radio_pending {
            return Some(Pending::Radio);
        }
        if let Some(attempt) = &self.attempt {
            return Some(Pending::Wifi {
                ssid: attempt.ssid().to_string(),
            });
        }
        self.vpn_wait.as_ref().map(|wait| Pending::Vpn {
            uuid: wait.uuid.clone(),
        })
    }

    /// A device proxy for one path.
    async fn device(&self, path: &OwnedObjectPath) -> zbus::Result<DeviceProxy<'static>> {
        DeviceProxy::builder(&self.connection)
            .path(path.clone())?
            .cache_properties(CacheProperties::No)
            .build()
            .await
    }

    /// The wireless half of one device.
    async fn wireless_proxy(
        &self,
        path: &OwnedObjectPath,
    ) -> zbus::Result<DeviceWirelessProxy<'static>> {
        DeviceWirelessProxy::builder(&self.connection)
            .path(path.clone())?
            .cache_properties(CacheProperties::No)
            .build()
            .await
    }

    /// The wired half of one device.
    async fn wired_proxy(&self, path: &OwnedObjectPath) -> zbus::Result<DeviceWiredProxy<'static>> {
        DeviceWiredProxy::builder(&self.connection)
            .path(path.clone())?
            .cache_properties(CacheProperties::No)
            .build()
            .await
    }

    /// One access point.
    async fn ap_proxy(&self, path: &OwnedObjectPath) -> zbus::Result<AccessPointProxy<'static>> {
        AccessPointProxy::builder(&self.connection)
            .path(path.clone())?
            .cache_properties(CacheProperties::No)
            .build()
            .await
    }

    /// One active connection.
    async fn active_proxy(
        &self,
        path: &OwnedObjectPath,
    ) -> zbus::Result<ActiveConnectionProxy<'static>> {
        ActiveConnectionProxy::builder(&self.connection)
            .path(path.clone())?
            .cache_properties(CacheProperties::No)
            .build()
            .await
    }

    /// One saved profile.
    async fn settings_connection(
        &self,
        path: &OwnedObjectPath,
    ) -> zbus::Result<SettingsConnectionProxy<'static>> {
        SettingsConnectionProxy::builder(&self.connection)
            .path(path.clone())?
            .cache_properties(CacheProperties::No)
            .build()
            .await
    }
}

/// One string out of a settings group.
fn string_of(setting: &super::proxy::Setting, key: &str) -> Option<String> {
    let value = setting.get(key)?;
    String::try_from(value.try_clone().ok()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_object_path_is_not_an_object() {
        assert!(!is_object(&OwnedObjectPath::try_from("/").expect("root")));
        assert!(is_object(
            &OwnedObjectPath::try_from("/org/freedesktop/NetworkManager/Devices/1")
                .expect("a device")
        ));
    }

    #[test]
    fn a_scan_interval_of_ten_seconds_is_what_the_plan_asks_for() {
        assert_eq!(SCAN_INTERVAL, Duration::from_secs(10));
    }
}
