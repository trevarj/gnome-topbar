//! The one owner of the BlueZ connection.
//!
//! BlueZ's whole world is one `ObjectManager` tree, and every change to it —
//! a device appearing, a battery interface arriving, a `Connected` flag
//! flipping — arrives as one of three signals under `org.bluez`. So the task
//! keeps **one** match rule and does **one** thing with it: re-read the tree.
//!
//! That looks wasteful and is not. `GetManagedObjects` on a machine with six
//! paired devices is a single round trip returning a few kilobytes, and it
//! happens when something actually changed rather than on a timer. The
//! alternative — a proxy per device, a property stream per proxy, and a second
//! set for the battery interfaces that come and go independently — is a dozen
//! subscriptions to maintain and a set of races to get wrong, in exchange for
//! saving a round trip nobody is waiting on.
//!
//! Signals are **coalesced**: connecting a pair of earbuds emits a burst of
//! property changes, and re-reading the tree once per change would be a dozen
//! round trips where one will do.

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::{mpsc, oneshot, watch};
use tracing::{debug, info, warn};
use zbus::proxy::CacheProperties;
use zbus::zvariant::{ObjectPath, OwnedObjectPath};

use super::agent::{AGENT_PATH, AgentMessage, CAPABILITY, PairingAgent, Question};
use super::model::{BtDevice, BtState, IconKind, PairingPrompt, PromptKind, listed, order};
use super::proxy::{
    AdapterProxy, AgentManagerProxy, DeviceProxy, Interfaces, ObjectManagerProxy, names, property,
};
use crate::error::SvcError;
use crate::network::Access;

/// How long a burst of signals is allowed to gather before the tree is re-read.
///
/// Connecting a headset emits `Connected`, `ServicesResolved`, a `Battery1`
/// interface and two or three more inside a few milliseconds. One read after
/// the burst is one round trip; one per signal is six.
const COALESCE: Duration = Duration::from_millis(80);

/// How many pairing-agent messages may be queued before the agent waits.
const AGENT_QUEUE: usize = 32;

/// How long one discovery burst runs before the radio stops looking.
///
/// A scan that ran for as long as the list was open would be a radio
/// transmitting at a popover somebody walked away from, which is the thing this
/// feature was not allowed to become. It would also never let the list settle:
/// devices come and go while a scan is running, and rows that reorder
/// themselves under the pointer are the one thing this panel refuses to do.
///
/// Twenty seconds is several times what a device in pairing mode needs to
/// announce itself. When it runs out the list keeps everything it found and
/// only the spinner goes; closing and reopening the list looks again.
const DISCOVERY_LIMIT: Duration = Duration::from_secs(20);

/// The well-known name.
///
/// The rule filters on the *sender* and nothing else. A path namespace would
/// be tighter, and would be wrong: `InterfacesAdded` and `InterfacesRemoved`
/// are emitted from `/`, which is not under `/org/bluez`, so a namespace
/// filter would quietly drop exactly the two signals that say a device
/// appeared.
const BLUEZ_NAME: &str = "org.bluez";

/// What the panel may ask of Bluetooth.
pub(crate) enum Command {
    /// Switch the adapter's radio.
    SetPowered {
        /// The state wanted.
        powered: bool,
        /// Where to answer.
        reply: oneshot::Sender<Result<(), SvcError>>,
    },
    /// Connect or disconnect one device.
    SetConnected {
        /// Its object path.
        path: String,
        /// Up or down.
        connected: bool,
        /// Answered when BlueZ has finished, which may be seconds later.
        reply: oneshot::Sender<Result<(), SvcError>>,
    },
    /// Answer the pairing row.
    AnswerPrompt {
        /// Whether the user confirmed.
        confirm: bool,
        /// Where to answer.
        reply: oneshot::Sender<Result<(), SvcError>>,
    },
    /// Open or close the discovery session behind the device list.
    SetDiscovery {
        /// Whether the list is open.
        on: bool,
        /// Where to answer.
        reply: oneshot::Sender<Result<(), SvcError>>,
    },
    /// Pair with one of the devices a scan found, and connect it.
    Pair {
        /// Its object path.
        path: String,
        /// Answered when the whole pair-trust-connect chain has finished,
        /// which for a device that wants a code confirmed is however long the
        /// user takes to look at their phone.
        reply: oneshot::Sender<Result<(), SvcError>>,
    },
}

/// Where a command's answer goes.
fn reply_of(command: Command) -> oneshot::Sender<Result<(), SvcError>> {
    match command {
        Command::SetPowered { reply, .. }
        | Command::SetConnected { reply, .. }
        | Command::AnswerPrompt { reply, .. }
        | Command::SetDiscovery { reply, .. }
        | Command::Pair { reply, .. } => reply,
    }
}

/// A pairing that has finished, on its way back to the loop.
///
/// Pairing cannot be awaited on the task's own loop: BlueZ calls *back* into
/// the panel's agent partway through `Pair`, and a loop blocked on the call
/// would never poll the channel that question arrives on — so the row would
/// never appear, the user could never confirm, and every outgoing pairing would
/// time out. So the chain is spawned and reports here instead, carrying the
/// caller's `reply` with it so a failure still lands under the row that asked.
struct Paired {
    /// The device it was about.
    path: String,
    /// How it went.
    result: Result<(), SvcError>,
    /// The caller waiting on it.
    reply: oneshot::Sender<Result<(), SvcError>>,
}

/// Everything the task knows, and everything it talks to.
struct World {
    connection: zbus::Connection,
    objects: ObjectManagerProxy<'static>,
    access: Access,
    publisher: watch::Sender<Arc<BtState>>,

    /// The adapter in use, when there is one.
    adapter: Option<OwnedObjectPath>,
    available: bool,
    powered: bool,
    devices: Vec<BtDevice>,

    /// Whether the radio switch is being waited on.
    powering: bool,
    /// The device a connect, disconnect or pairing is in flight for.
    pending: Option<String>,

    /// Whether the device list is open, and unpaired devices belong in it.
    browsing: bool,
    /// Whether `StartDiscovery` is outstanding against this adapter.
    scanning: bool,
    /// When the burst runs out. See [`DISCOVERY_LIMIT`].
    scan_deadline: Option<tokio::time::Instant>,
    /// Where a spawned pairing reports back.
    paired: mpsc::Sender<Paired>,

    /// The question on screen, and where its answer goes.
    prompt: Option<PairingPrompt>,
    answer: Option<oneshot::Sender<bool>>,
    prompt_deadline: Option<tokio::time::Instant>,
    /// The agent's registration, so it can be withdrawn on the way out.
    registered: bool,
}

/// Sleep until `deadline`, or for ever when there is none.
async fn wait_until(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

/// Follow BlueZ until every handle is dropped.
pub(crate) async fn run(
    mut commands: mpsc::Receiver<Command>,
    publisher: watch::Sender<Arc<BtState>>,
    address: Option<String>,
    access: Access,
) {
    if access == Access::ReadOnly {
        info!(
            "bluetooth: read-only against this BlueZ; no pairing agent, no radio switch, no connect"
        );
    }

    let connection = match connect(address).await {
        Ok(connection) => connection,
        Err(error) => {
            info!("no system bus ({error}); Bluetooth is unknown");
            publish_unavailable(&publisher, access);
            return drain(commands).await;
        }
    };

    // Uncached: the object tree is read in full whenever anything moves, and a
    // proxy that fetched every property at build time would fail outright
    // against a BlueZ with no adapter rather than simply having none to report.
    let objects = match ObjectManagerProxy::builder(&connection)
        .cache_properties(CacheProperties::No)
        .build()
        .await
    {
        Ok(objects) => objects,
        Err(error) => {
            info!("no BlueZ ({error}); Bluetooth is unknown");
            publish_unavailable(&publisher, access);
            return drain(commands).await;
        }
    };

    let (events, mut queue) = mpsc::unbounded_channel();
    let (agent_out, mut agent_in) = mpsc::channel(AGENT_QUEUE);
    // One pairing at a time — `pending` sees to that — so one slot is enough.
    let (paired_out, mut paired_in) = mpsc::channel(1);

    // Subscribed before the first read, so a change that lands between the two
    // is queued rather than lost.
    spawn_signals(&connection, events);

    let mut world = World {
        connection,
        objects,
        access,
        publisher,
        adapter: None,
        available: false,
        powered: false,
        devices: Vec::new(),
        powering: false,
        pending: None,
        browsing: false,
        scanning: false,
        scan_deadline: None,
        paired: paired_out,
        prompt: None,
        answer: None,
        prompt_deadline: None,
        registered: false,
    };

    world.read_tree().await;
    world.register_agent(agent_out).await;
    world.publish();

    loop {
        let deadline = world.prompt_deadline;
        let scan_deadline = world.scan_deadline;
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { break };
                world.command(command).await;
            }
            Some(()) = queue.recv() => {
                // Everything else that arrived in the same breath is taken off
                // the queue before the tree is read, so a burst is one round
                // trip rather than one each.
                tokio::time::sleep(COALESCE).await;
                while queue.try_recv().is_ok() {}
                world.read_tree().await;
                world.settle_prompt();
                world.publish();
            }
            Some(message) = agent_in.recv() => world.agent(message).await,
            Some(finished) = paired_in.recv() => world.settle_pair(finished).await,
            () = wait_until(deadline) => {
                info!("bluetooth: nobody answered the pairing prompt");
                world.clear_prompt();
                world.publish();
            }
            () = wait_until(scan_deadline) => {
                debug!("bluetooth: the discovery burst ran out; the radio stops looking");
                world.stop_scanning().await;
                world.publish();
            }
        }
    }

    // The radio stops looking even if the panel is being torn down mid-scan:
    // a discovery session outliving the process that opened it is a radio
    // transmitting with nobody left to look at what it finds.
    let _ = world.set_discovery(false).await;
    world.unregister_agent().await;
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

/// Publish "there is no adapter".
fn publish_unavailable(publisher: &watch::Sender<Arc<BtState>>, access: Access) {
    let _ = publisher.send(Arc::new(BtState {
        access,
        ..BtState::default()
    }));
}

/// Answer every command with "unavailable" rather than blocking a caller.
async fn drain(mut commands: mpsc::Receiver<Command>) {
    while let Some(command) = commands.recv().await {
        let _ =
            reply_of(command).send(Err(SvcError::Bluetooth("BlueZ is not running".to_string())));
    }
}

/// Follow every signal BlueZ emits, through one match rule.
///
/// The payloads are not decoded. All three signals mean the same thing to this
/// task — "the tree moved" — and the tree is the thing that gets read.
///
/// The queue is unbounded for the reason [`crate::network::task`] spells out:
/// zbus reads the socket in one task and waits to hand each message to the
/// streams that match it, so a forwarder that can be made to wait stops every
/// method reply on the connection — including the ones the read below is
/// blocked on. Unbounded is safe here because the events are collapsed rather
/// than answered: a burst is drained and costs one read of the tree.
fn spawn_signals(connection: &zbus::Connection, events: mpsc::UnboundedSender<()>) {
    let connection = connection.clone();
    tokio::spawn(async move {
        let rule = match zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .sender(BLUEZ_NAME)
        {
            Ok(builder) => builder.build(),
            Err(error) => return warn!("cannot build the BlueZ match rule: {error}"),
        };
        let mut stream = match zbus::MessageStream::for_match_rule(rule, &connection, None).await {
            Ok(stream) => stream,
            Err(error) => return warn!("cannot follow BlueZ's signals: {error}"),
        };

        while let Some(message) = stream.next().await {
            // An error is the connection itself: zbus only puts one on this
            // stream when the socket has failed, and nothing after it.
            let message = match message {
                Ok(message) => message,
                Err(error) => {
                    warn!("BlueZ's signal stream failed: {error}");
                    break;
                }
            };
            if !interesting(&message) {
                continue;
            }
            if events.send(()).is_err() {
                debug!("the Bluetooth service has stopped; nothing to forward to");
                return;
            }
        }
        warn!("BlueZ's signals have stopped; the panel will not see further changes");
    });
}

/// Whether one signal is one of the three the task acts on.
fn interesting(message: &zbus::Message) -> bool {
    let header = message.header();
    let Some(member) = header.member() else {
        return false;
    };
    matches!(
        member.as_str(),
        "InterfacesAdded" | "InterfacesRemoved" | "PropertiesChanged"
    )
}

impl World {
    /// Read the whole object tree and rebuild the device list from it.
    async fn read_tree(&mut self) {
        let objects = match self.objects.get_managed_objects().await {
            Ok(objects) => objects,
            Err(error) => {
                if self.available {
                    info!("BlueZ stopped answering ({error})");
                }
                self.available = false;
                self.powered = false;
                self.scanning = false;
                self.scan_deadline = None;
                self.adapter = None;
                self.devices.clear();
                return;
            }
        };

        // The lowest-numbered adapter, which on every machine with one is
        // `hci0`. A laptop with a second dongle in it has two, and picking the
        // built-in one every time beats picking whichever answered first.
        let mut adapters: Vec<&OwnedObjectPath> = objects
            .iter()
            .filter(|(_, interfaces)| interfaces.contains_key(names::ADAPTER))
            .map(|(path, _)| path)
            .collect();
        adapters.sort_by_key(|path| path.as_str());
        let adapter = adapters.first().map(|path| (*path).clone());

        self.available = adapter.is_some();
        self.powered = adapter
            .as_ref()
            .and_then(|path| objects.get(path))
            .and_then(|interfaces| interfaces.get(names::ADAPTER))
            .and_then(|adapter| property::<bool>(adapter, "Powered"))
            .unwrap_or(false);
        // A radio switched off from anywhere takes the discovery session with
        // it, so the spinner goes rather than turning over a dead adapter.
        if !self.powered {
            self.scanning = false;
            self.scan_deadline = None;
        }

        let mut devices = Vec::new();
        if let Some(adapter) = &adapter {
            // Only this adapter's devices: a second dongle's paired headset is
            // not something the row under this radio can connect.
            let prefix = format!("{}/", adapter.as_str());
            for (path, interfaces) in &objects {
                if !path.as_str().starts_with(&prefix) {
                    continue;
                }
                // The device being paired stays in the list even once the scan
                // that found it has stopped: it is the row with the spinner on
                // it, and a row that vanished the moment it was clicked would
                // be the panel losing the thing the user was doing.
                let held = self.pending.as_deref() == Some(path.as_str());
                if let Some(device) = read_device(path.as_str(), interfaces, self.browsing || held)
                {
                    devices.push(device);
                }
            }
        }

        for device in &mut devices {
            device.pending = self.pending.as_deref() == Some(device.path.as_str());
        }
        order(&mut devices);

        self.adapter = adapter;
        self.devices = devices;
    }

    /// Put the panel's pairing agent on the bus, if this panel may.
    async fn register_agent(&mut self, questions: mpsc::Sender<AgentMessage>) {
        if !self.access.writable() {
            info!("bluetooth: not registering a pairing agent (read-only)");
            return;
        }
        if let Err(error) = self
            .connection
            .object_server()
            .at(AGENT_PATH, PairingAgent::new(questions))
            .await
        {
            warn!("cannot serve the pairing agent: {error}");
            return;
        }
        let manager = match AgentManagerProxy::new(&self.connection).await {
            Ok(manager) => manager,
            Err(error) => return warn!("no BlueZ agent manager: {error}"),
        };
        let Ok(path) = ObjectPath::try_from(AGENT_PATH) else {
            return;
        };
        match manager.register_agent(&path, CAPABILITY).await {
            Ok(()) => {
                self.registered = true;
                info!("bluetooth: pairing agent registered as {CAPABILITY}");
            }
            Err(error) => return warn!("could not register the pairing agent: {error}"),
        }
        // Without this an *incoming* pairing never reaches the panel at all —
        // BlueZ refuses a request it has no default agent for. A failure is
        // somebody else already holding the slot, which is their pairing to
        // answer, not a reason to stop.
        match manager.request_default_agent(&path).await {
            Ok(()) => info!("bluetooth: answering pairing requests for this session"),
            Err(error) => {
                info!("bluetooth: another agent is answering pairing requests ({error})");
            }
        }
    }

    /// Take it off the bus again.
    async fn unregister_agent(&mut self) {
        if !self.registered {
            return;
        }
        if let (Ok(manager), Ok(path)) = (
            AgentManagerProxy::new(&self.connection).await,
            ObjectPath::try_from(AGENT_PATH),
        ) {
            let _ = manager.unregister_agent(&path).await;
        }
        let _ = self
            .connection
            .object_server()
            .remove::<PairingAgent, _>(AGENT_PATH)
            .await;
        self.registered = false;
    }

    /// Deal with a message from the agent.
    async fn agent(&mut self, message: AgentMessage) {
        match message {
            AgentMessage::Ask(question) => self.ask(*question),
            AgentMessage::Cancel => self.clear_prompt(),
        }
        self.publish();
    }

    /// Put a pairing question on screen.
    fn ask(&mut self, question: Question) {
        let Question {
            path,
            code,
            kind,
            reply,
        } = question;
        let alias = self.alias_of(path.as_str());
        self.prompt = Some(PairingPrompt {
            path: path.as_str().to_string(),
            alias,
            code,
            kind,
        });
        // Dropping the previous sender is what makes a superseded question
        // answer `Canceled` — one pairing at a time, and the newest is the one
        // the user is looking at.
        self.answer = reply;
        self.prompt_deadline = Some(tokio::time::Instant::now() + super::agent::PROMPT_TIMEOUT);
    }

    /// Take the question away, refusing it if it is still waiting.
    fn clear_prompt(&mut self) {
        // Dropping the sender rather than sending `false`: the agent reads a
        // dropped channel as "the prompt went away", which is `Canceled` — and
        // a timeout is not the user saying no.
        self.answer = None;
        self.prompt = None;
        self.prompt_deadline = None;
    }

    /// Clear a display-only prompt once the pairing it belonged to is over.
    ///
    /// There is no reply to wait on for one of those, so nothing else would
    /// ever take it off the screen.
    fn settle_prompt(&mut self) {
        let Some(prompt) = &self.prompt else { return };
        if prompt.kind != PromptKind::Display {
            return;
        }
        let paired = self
            .devices
            .iter()
            .any(|device| device.path == prompt.path && device.paired);
        if paired {
            debug!("bluetooth: the pairing finished; taking the code away");
            self.clear_prompt();
        }
    }

    /// One device's name, as far as the tree knows it.
    fn alias_of(&self, path: &str) -> String {
        self.devices
            .iter()
            .find(|device| device.path == path)
            .map_or_else(|| "a Bluetooth device".to_string(), |d| d.alias.clone())
    }

    /// Run one command.
    ///
    /// The snapshot is published **before** the caller is answered, and the
    /// order is load-bearing: `bridge::act` renders the failure the instant its
    /// future resolves, and a panel that answered first would spend a frame
    /// showing an error under a row whose spinner was still turning.
    async fn command(&mut self, command: Command) {
        let (answer, reply) = match command {
            Command::SetPowered { powered, reply } => (self.set_powered(powered).await, reply),
            Command::SetConnected {
                path,
                connected,
                reply,
            } => (self.set_connected(&path, connected).await, reply),
            Command::AnswerPrompt { confirm, reply } => (self.answer_prompt(confirm), reply),
            Command::SetDiscovery { on, reply } => (self.set_discovery(on).await, reply),
            // The one command that does not answer here: the chain runs off
            // this loop and carries the caller's reply with it.
            Command::Pair { path, reply } => return self.begin_pair(path, reply).await,
        };
        self.publish();
        let _ = reply.send(answer);
    }

    /// Refuse a change this panel is not allowed to make.
    fn read_only(&self) -> SvcError {
        SvcError::Bluetooth("this build only reads Bluetooth".to_string())
    }

    /// Switch the radio.
    ///
    /// Pessimistic: the pill does not move until BlueZ says the adapter did.
    async fn set_powered(&mut self, powered: bool) -> Result<(), SvcError> {
        if !self.access.writable() {
            return Err(self.read_only());
        }
        let Some(path) = self.adapter.clone() else {
            return Err(SvcError::Bluetooth("there is no adapter".to_string()));
        };
        let adapter = self
            .adapter_proxy(&path)
            .await
            .map_err(|error| SvcError::Bluetooth(error.to_string()))?;

        self.powering = true;
        self.publish();
        let answer = adapter
            .set_powered(powered)
            .await
            .map_err(|error| SvcError::Bluetooth(error.to_string()));
        self.powering = false;
        if answer.is_ok() {
            // The property write has returned, so the tree is already current;
            // reading it here is what makes the pill move in the same frame
            // rather than on whichever signal happens to arrive next.
            self.read_tree().await;
        }
        answer
    }

    /// Connect or disconnect one device.
    ///
    /// Pessimistic and *slow on purpose*: `Connect` on a headset that is in a
    /// drawer takes BlueZ the better part of ten seconds to give up on, and the
    /// row spins for exactly that long rather than flipping and flipping back.
    async fn set_connected(&mut self, path: &str, connected: bool) -> Result<(), SvcError> {
        if !self.access.writable() {
            return Err(self.read_only());
        }
        if self.pending.is_some() {
            return Err(SvcError::Bluetooth(
                "another device is connecting".to_string(),
            ));
        }
        let object = OwnedObjectPath::try_from(path)
            .map_err(|error| SvcError::Bluetooth(error.to_string()))?;
        let device = self
            .device_proxy(&object)
            .await
            .map_err(|error| SvcError::Bluetooth(error.to_string()))?;

        self.pending = Some(path.to_string());
        self.mark_pending();
        self.publish();

        let answer = if connected {
            device.connect().await
        } else {
            device.disconnect().await
        };

        self.pending = None;
        self.read_tree().await;
        answer.map_err(|error| SvcError::Bluetooth(describe(&error, connected)))
    }

    /// Open or close the discovery session behind the device list.
    ///
    /// The radio looks around for at most [`DISCOVERY_LIMIT`], and not one
    /// moment of it happens because a panel was merely looked at: the caller is
    /// the *chevron*, not the popover. Every other way the list can leave the
    /// screen — the section collapsing, the panel closing, the radio going off,
    /// the process going away — closes it again.
    ///
    /// Closing it is more than stopping the scan: it also takes what the scan
    /// found back out of the list, which is why `browsing` and `scanning` are
    /// two flags rather than one.
    async fn set_discovery(&mut self, on: bool) -> Result<(), SvcError> {
        if on && !self.access.writable() {
            // Not an error the user needs to see. A build being worked on
            // simply lists what BlueZ already knows about, exactly as it does
            // for the Wi-Fi scan.
            debug!("bluetooth: not scanning (read-only)");
            return Ok(());
        }
        let wanted = on && self.powered;
        if self.browsing == on && self.scanning == wanted {
            return Ok(());
        }
        self.browsing = on;
        if !wanted {
            self.stop_scanning().await;
            // The list still has to lose whatever the scan found.
            self.read_tree().await;
            return Ok(());
        }

        let Some(path) = self.adapter.clone() else {
            return Ok(());
        };
        let adapter = self
            .adapter_proxy(&path)
            .await
            .map_err(|error| SvcError::Bluetooth(error.to_string()))?;
        // `scanning` is set from the answer rather than from the intent: a
        // spinner beside a discovery BlueZ refused is a control lying about a
        // radio that is not on.
        adapter
            .start_discovery()
            .await
            .map_err(|error| SvcError::Bluetooth(error.to_string()))?;
        self.scanning = true;
        self.scan_deadline = Some(tokio::time::Instant::now() + DISCOVERY_LIMIT);
        self.read_tree().await;
        Ok(())
    }

    /// Stop the radio looking, leaving the list as it stands.
    ///
    /// The burst running out, a pairing starting, and the list closing all end
    /// here. Only the last of them also clears `browsing` — a pairing has to
    /// keep the row it is happening under, and a list that has finished looking
    /// still shows what it found.
    async fn stop_scanning(&mut self) {
        self.scan_deadline = None;
        if !self.scanning {
            return;
        }
        self.scanning = false;
        let Some(path) = self.adapter.clone() else {
            return;
        };
        let Ok(adapter) = self.adapter_proxy(&path).await else {
            return;
        };
        // Refused is the normal answer when somebody else's session is the one
        // still running, or when BlueZ has already dropped ours.
        if let Err(error) = adapter.stop_discovery().await {
            debug!("bluetooth: the adapter would not stop looking ({error})");
        }
    }

    /// Start pairing with one of the devices a scan found.
    ///
    /// Spawned rather than awaited: `Pair` is the call BlueZ answers the
    /// panel's own agent through, and a task loop blocked on it would never
    /// deliver the question to the row that has to answer it. See [`Paired`].
    async fn begin_pair(&mut self, path: String, reply: oneshot::Sender<Result<(), SvcError>>) {
        if let Err(error) = self.pair_precondition(&path) {
            let _ = reply.send(Err(error));
            return;
        }
        let object = match OwnedObjectPath::try_from(path.as_str()) {
            Ok(object) => object,
            Err(error) => {
                let _ = reply.send(Err(SvcError::Bluetooth(error.to_string())));
                return;
            }
        };
        let device = match self.device_proxy(&object).await {
            Ok(device) => device,
            Err(error) => {
                let _ = reply.send(Err(SvcError::Bluetooth(error.to_string())));
                return;
            }
        };

        // The radio stops looking first. BlueZ pairs a great deal more
        // reliably when it is not also scanning, and the row does not go
        // anywhere: `browsing` is what keeps it listed, and that stays on.
        self.stop_scanning().await;

        self.pending = Some(path.clone());
        self.mark_pending();
        self.publish();

        let done = self.paired.clone();
        tokio::spawn(async move {
            let result = pair_and_connect(&device).await;
            let _ = done
                .send(Paired {
                    path,
                    result,
                    reply,
                })
                .await;
        });
    }

    /// Whether a pairing may start at all.
    fn pair_precondition(&self, path: &str) -> Result<(), SvcError> {
        if !self.access.writable() {
            return Err(self.read_only());
        }
        if !self.powered {
            return Err(SvcError::Bluetooth("the radio is off".to_string()));
        }
        if self.pending.is_some() {
            return Err(SvcError::Bluetooth(
                "another device is connecting".to_string(),
            ));
        }
        if self
            .devices
            .iter()
            .any(|device| device.path == path && device.paired)
        {
            return Err(SvcError::Bluetooth(
                "this device is already paired".to_string(),
            ));
        }
        Ok(())
    }

    /// Take a finished pairing off the row and answer whoever asked for it.
    ///
    /// The snapshot goes out before the caller is answered, for the reason
    /// [`World::command`] spells out: the inline failure is rendered the
    /// instant the future resolves, and it must not land beside a spinner that
    /// is still turning.
    async fn settle_pair(&mut self, finished: Paired) {
        let Paired {
            path,
            result,
            reply,
        } = finished;
        if self.pending.as_deref() == Some(path.as_str()) {
            self.pending = None;
        }
        self.read_tree().await;
        self.publish();
        let _ = reply.send(result);
    }

    /// Answer the pairing row.
    fn answer_prompt(&mut self, confirm: bool) -> Result<(), SvcError> {
        let Some(reply) = self.answer.take() else {
            return Err(SvcError::Bluetooth(
                "there is nothing to answer".to_string(),
            ));
        };
        let _ = reply.send(confirm);
        self.prompt = None;
        self.prompt_deadline = None;
        Ok(())
    }

    /// Flag whichever row is waiting.
    fn mark_pending(&mut self) {
        for device in &mut self.devices {
            device.pending = self.pending.as_deref() == Some(device.path.as_str());
        }
    }

    /// Build the snapshot and publish it if anything moved.
    fn publish(&self) {
        let mut devices = self.devices.clone();
        order(&mut devices);
        let next = BtState {
            available: self.available,
            powered: self.powered,
            powering: self.powering,
            browsing: self.browsing,
            scanning: self.scanning,
            devices,
            prompt: self.prompt.clone(),
            access: self.access,
        };
        self.publisher.send_if_modified(|current| {
            if **current == next {
                false
            } else {
                *current = Arc::new(next);
                true
            }
        });
    }

    /// One adapter proxy.
    async fn adapter_proxy(&self, path: &OwnedObjectPath) -> zbus::Result<AdapterProxy<'static>> {
        AdapterProxy::builder(&self.connection)
            .path(path.clone())?
            .cache_properties(CacheProperties::No)
            .build()
            .await
    }

    /// One device proxy.
    async fn device_proxy(&self, path: &OwnedObjectPath) -> zbus::Result<DeviceProxy<'static>> {
        DeviceProxy::builder(&self.connection)
            .path(path.clone())?
            .cache_properties(CacheProperties::No)
            .build()
            .await
    }
}

/// Pair with a device, trust it, and connect it.
///
/// The three calls GNOME's own pairing makes, in that order and for the same
/// reasons. `Trusted` is what stops the machine asking again every time the
/// headset comes out of its case, and the `Connect` is what makes a pairing the
/// user just performed do something — BlueZ connects some profiles by itself
/// after a pair and not others, so a panel that stopped at `Pair` would leave
/// half of them silently unpaired-looking.
///
/// A `Connect` that fails is **not** a failed pairing. The device is paired,
/// trusted and in the list; it just did not come up, which is what the row's own
/// switch is for. Saying "pairing failed" there would be the panel reporting a
/// success as a failure.
async fn pair_and_connect(device: &DeviceProxy<'static>) -> Result<(), SvcError> {
    device
        .pair()
        .await
        .map_err(|error| SvcError::Bluetooth(describe_pairing(&error)))?;
    if let Err(error) = device.set_trusted(true).await {
        // Worth a line and not worth a caption: the pairing worked, and the
        // cost of this is being asked again next time.
        warn!("bluetooth: paired, but could not mark the device trusted ({error})");
    }
    if let Err(error) = device.connect().await {
        info!("bluetooth: paired, but the device did not connect ({error})");
    }
    Ok(())
}

/// What went wrong with a pairing, in words a row can show.
fn describe_pairing(error: &zbus::Error) -> String {
    let text = error.to_string();
    // BlueZ's own names, which are the useful part. `AuthenticationCanceled`
    // is the user pressing Cancel in the panel's own confirmation row, and
    // repeating the interface name back at somebody who just did that is the
    // panel explaining their own click to them.
    if text.contains("AuthenticationCanceled") || text.contains("AuthenticationRejected") {
        return "the pairing was cancelled".to_string();
    }
    if text.contains("AuthenticationTimeout") {
        return "the device stopped waiting; try again".to_string();
    }
    if text.contains("AlreadyExists") {
        return "this device is already paired".to_string();
    }
    if text.contains("ConnectionAttemptFailed") || text.contains("Failed") {
        return "the device did not answer; is it in pairing mode?".to_string();
    }
    text
}

/// What went wrong, in words a row can show.
///
/// BlueZ's own error names are the useful part: `org.bluez.Error.Failed` on a
/// connect is nearly always "the device is off or out of range", and saying so
/// is worth more than repeating the name at somebody.
fn describe(error: &zbus::Error, connecting: bool) -> String {
    let text = error.to_string();
    if connecting && (text.contains("Failed") || text.contains("timed out")) {
        return "the device did not answer; is it switched on and in range?".to_string();
    }
    text
}

/// One device out of the tree, or nothing if it does not belong in the list.
///
/// `browsing` is whether unpaired devices belong in it at all — see
/// [`listed`].
fn read_device(path: &str, interfaces: &Interfaces, browsing: bool) -> Option<BtDevice> {
    let device = interfaces.get(names::DEVICE)?;
    let paired = property::<bool>(device, "Paired").unwrap_or(false);

    let address = property::<String>(device, "Address").unwrap_or_default();
    let name = property::<String>(device, "Alias")
        .filter(|alias| !alias.trim().is_empty())
        .or_else(|| property::<String>(device, "Name"))
        .filter(|name| !name.trim().is_empty())
        .filter(|name| !is_address(name, &address));
    if !listed(paired, name.is_some(), browsing) {
        return None;
    }
    let alias = name.unwrap_or_else(|| {
        if address.is_empty() {
            "Unknown device".to_string()
        } else {
            address.clone()
        }
    });

    let battery_pct = interfaces
        .get(names::BATTERY)
        .and_then(|battery| property::<u8>(battery, "Percentage"))
        .filter(|percent| *percent <= 100);

    Some(BtDevice {
        path: path.to_string(),
        alias,
        icon: IconKind::from_bluez(property::<String>(device, "Icon").as_deref()),
        connected: property::<bool>(device, "Connected").unwrap_or(false),
        paired,
        battery_pct,
        pending: false,
    })
}

/// Whether a "name" is really the device's own address wearing a label.
///
/// BlueZ gives a device that has not answered a name request an `Alias` built
/// out of its address with the colons turned into dashes. It is a string, it is
/// not empty, and it is not a name — so a scan filtered on "has a name" would
/// otherwise fill the list with MAC addresses.
fn is_address(name: &str, address: &str) -> bool {
    !address.is_empty() && name.replace('-', ":").eq_ignore_ascii_case(address)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use zbus::zvariant::{OwnedValue, Value};

    fn interfaces(entries: &[(&str, &[(&str, Value<'static>)])]) -> Interfaces {
        entries
            .iter()
            .map(|(name, properties)| {
                let map: HashMap<String, OwnedValue> = properties
                    .iter()
                    .map(|(key, value)| {
                        (
                            (*key).to_string(),
                            value.try_clone().expect("clone").try_into().expect("own"),
                        )
                    })
                    .collect();
                ((*name).to_string(), map)
            })
            .collect()
    }

    #[test]
    fn a_paired_device_is_read_out_of_the_tree_whole() {
        let tree = interfaces(&[
            (
                names::DEVICE,
                &[
                    ("Paired", Value::from(true)),
                    ("Connected", Value::from(true)),
                    ("Alias", Value::from("WH-1000XM4")),
                    ("Address", Value::from("AA:BB:CC:DD:EE:FF")),
                    ("Icon", Value::from("audio-headset")),
                ],
            ),
            (names::BATTERY, &[("Percentage", Value::from(85_u8))]),
        ]);

        let device = read_device("/org/bluez/hci0/dev_AA", &tree, false).expect("a device");
        assert_eq!(device.alias, "WH-1000XM4");
        assert_eq!(device.icon, IconKind::Headset);
        assert!(device.connected);
        assert!(device.paired);
        assert_eq!(device.battery_pct, Some(85));
    }

    #[test]
    fn a_device_that_has_never_been_paired_is_a_row_only_while_the_list_is_open() {
        let tree = interfaces(&[(
            names::DEVICE,
            &[
                ("Paired", Value::from(false)),
                ("Alias", Value::from("Somebody's Pixel")),
            ],
        )]);
        assert!(
            read_device("/org/bluez/hci0/dev_BB", &tree, false).is_none(),
            "a phone walking past a closed list is not a row in Quick Settings"
        );
        let found = read_device("/org/bluez/hci0/dev_BB", &tree, true).expect("a found device");
        assert!(!found.paired, "and it is drawn as something to pair with");
    }

    #[test]
    fn a_scan_does_not_put_bare_addresses_in_the_list() {
        // BlueZ hands out an address-shaped alias for a device that has not
        // answered a name request. Nobody can pick their own headset out of a
        // column of MAC addresses.
        let tree = interfaces(&[(
            names::DEVICE,
            &[
                ("Paired", Value::from(false)),
                ("Address", Value::from("AA:BB:CC:DD:EE:FF")),
                ("Alias", Value::from("AA-BB-CC-DD-EE-FF")),
            ],
        )]);
        assert!(read_device("/org/bluez/hci0/dev_AA", &tree, true).is_none());

        // The same device once it *is* paired stays a row, address or not:
        // something got it into the list, and taking it away again would be
        // the panel losing a device the user owns.
        let paired = interfaces(&[(
            names::DEVICE,
            &[
                ("Paired", Value::from(true)),
                ("Address", Value::from("AA:BB:CC:DD:EE:FF")),
                ("Alias", Value::from("AA-BB-CC-DD-EE-FF")),
            ],
        )]);
        let device = read_device("/org/bluez/hci0/dev_AA", &paired, true).expect("a device");
        assert_eq!(device.alias, "AA:BB:CC:DD:EE:FF");
    }

    #[test]
    fn an_object_with_no_device_interface_is_skipped() {
        let tree = interfaces(&[(names::ADAPTER, &[("Powered", Value::from(true))])]);
        assert!(read_device("/org/bluez/hci0", &tree, true).is_none());
    }

    #[test]
    fn a_device_with_no_name_falls_back_to_its_address_and_then_to_a_phrase() {
        let addressed = interfaces(&[(
            names::DEVICE,
            &[
                ("Paired", Value::from(true)),
                ("Address", Value::from("AA:BB:CC:DD:EE:FF")),
            ],
        )]);
        let device = read_device("/org/bluez/hci0/dev_AA", &addressed, false).expect("a device");
        assert_eq!(device.alias, "AA:BB:CC:DD:EE:FF");
        assert_eq!(device.icon, IconKind::Generic);
        assert_eq!(device.battery_pct, None);

        // An empty alias is not a name, and neither is an empty address.
        let nameless = interfaces(&[(
            names::DEVICE,
            &[("Paired", Value::from(true)), ("Alias", Value::from("  "))],
        )]);
        let device = read_device("/org/bluez/hci0/dev_CC", &nameless, false).expect("a device");
        assert_eq!(device.alias, "Unknown device");
    }

    #[test]
    fn a_battery_reading_past_full_is_not_shown() {
        // Some devices report 255 for "no idea".
        let tree = interfaces(&[
            (names::DEVICE, &[("Paired", Value::from(true))]),
            (names::BATTERY, &[("Percentage", Value::from(255_u8))]),
        ]);
        let device = read_device("/org/bluez/hci0/dev_AA", &tree, false).expect("a device");
        assert_eq!(device.battery_pct, None);
    }

    #[test]
    fn an_alias_bluez_built_out_of_an_address_is_not_a_name() {
        assert!(is_address("AA-BB-CC-DD-EE-FF", "AA:BB:CC:DD:EE:FF"));
        assert!(is_address("aa-bb-cc-dd-ee-ff", "AA:BB:CC:DD:EE:FF"));
        assert!(is_address("AA:BB:CC:DD:EE:FF", "AA:BB:CC:DD:EE:FF"));
        assert!(!is_address("WH-1000XM4", "AA:BB:CC:DD:EE:FF"));
        // A device with no address at all: whatever it called itself stands.
        assert!(!is_address("WH-1000XM4", ""));
    }

    #[test]
    fn a_pairing_says_what_to_do_about_it_rather_than_naming_an_interface() {
        let cancelled = zbus::Error::Failure("org.bluez.Error.AuthenticationCanceled".into());
        assert_eq!(describe_pairing(&cancelled), "the pairing was cancelled");

        let timeout = zbus::Error::Failure("org.bluez.Error.AuthenticationTimeout".into());
        assert!(describe_pairing(&timeout).contains("try again"));

        let unreachable = zbus::Error::Failure("org.bluez.Error.ConnectionAttemptFailed".into());
        assert!(describe_pairing(&unreachable).contains("pairing mode"));

        // Anything BlueZ says that the panel has no better words for is
        // repeated rather than swallowed.
        let odd = zbus::Error::Failure("org.bluez.Error.NotSupported".into());
        assert_eq!(describe_pairing(&odd), odd.to_string());
    }

    #[test]
    fn a_connect_failure_says_what_to_do_about_it() {
        let failed = zbus::Error::Failure("org.bluez.Error.Failed: br-connection-canceled".into());
        assert!(describe(&failed, true).contains("in range"));
        // The same error on the way down is not the same problem.
        assert_eq!(
            describe(&failed, false),
            failed.to_string(),
            "a disconnect that failed is not a device out of range"
        );
    }

    /// One signal, as it would arrive on the bus.
    fn signal(interface: &str, member: &str) -> zbus::Message {
        zbus::Message::signal("/org/bluez/hci0", interface, member)
            .expect("a well-formed signal header")
            .build(&())
            .expect("a signal with an empty body")
    }

    #[test]
    fn the_signal_filter_takes_the_three_that_mean_the_tree_moved() {
        // Against real messages rather than against the list itself, so a typo
        // in the filter is a failing test rather than a panel that quietly
        // stops noticing devices.
        for (interface, member) in [
            ("org.freedesktop.DBus.ObjectManager", "InterfacesAdded"),
            ("org.freedesktop.DBus.ObjectManager", "InterfacesRemoved"),
            ("org.freedesktop.DBus.Properties", "PropertiesChanged"),
        ] {
            assert!(
                interesting(&signal(interface, member)),
                "{member} says the tree moved"
            );
        }
    }

    #[test]
    fn everything_else_bluez_emits_is_ignored() {
        // BlueZ is chatty on its own namespace, and re-reading the whole object
        // tree for a signal that cannot have changed it would be a round trip
        // for nothing.
        for (interface, member) in [
            ("org.bluez.AgentManager1", "Release"),
            ("org.freedesktop.DBus", "NameOwnerChanged"),
            ("org.bluez.Device1", "Disconnected"),
        ] {
            assert!(!interesting(&signal(interface, member)), "{member}");
        }
    }

    #[test]
    fn a_discovery_burst_is_bounded_and_long_enough_to_find_something() {
        assert!(
            DISCOVERY_LIMIT >= Duration::from_secs(10),
            "a device in pairing mode has to have time to announce itself"
        );
        assert!(
            DISCOVERY_LIMIT <= Duration::from_secs(60),
            "past this it is a radio transmitting at a panel nobody is watching"
        );
    }

    #[test]
    fn signals_are_coalesced_rather_than_read_one_at_a_time() {
        assert!(
            COALESCE >= Duration::from_millis(50),
            "a burst has to have time to gather"
        );
        assert!(
            COALESCE <= Duration::from_millis(150),
            "a row that took longer than this to move would read as stuck"
        );
    }
}
