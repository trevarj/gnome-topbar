//! The one task that owns the tray.
//!
//! ```text
//!   an application ──RegisterStatusNotifierItem──▶ watcher.rs ─┐
//!                                                              ├─▶ task.rs
//!   the bus ────────NameOwnerChanged────────────▶ discover() ──┘      │ ▲
//!   an item ────────New*───────────────────────▶ follow() ────────────┘ │
//!                                                                       ▼
//!   the tray widget ◀───────────────────── watch<Arc<TrayState>>
//! ```
//!
//! One task per item reads that item and nothing else; every answer arrives
//! here as a message and is applied in order. Nothing is awaited from inside
//! the loop, so an application that has stopped answering delays its own
//! watcher rather than the panel.
//!
//! Publishing is debounced. Applications re-register in bursts — a chat client
//! reconnecting will announce itself five times in a fifth of a second — and a
//! tray that rebuilt on each of those would flicker every icon beside it.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};
use zbus::names::InterfaceName;
use zbus::proxy::CacheProperties;
use zbus::zvariant::Value;
use zbus::{Connection, fdo};

use crate::error::SvcError;

use super::menu::{MenuEvent, MenuNode};
use super::model::{ItemView, ScrollAxis, TrayState, make_id, resolve, split_id};
use super::props::{ITEM_INTERFACE, ItemProps};
use super::proxy::{DBusMenuProxy, StatusNotifierItemProxy, StatusNotifierWatcherProxy};
use super::watcher::{Registration, Registry, WATCHER_NAME, WATCHER_PATH, Watcher};

/// How long a burst of changes must go quiet before the tray is republished.
const QUIET: Duration = Duration::from_millis(50);
/// How long a *continuous* burst may hold the tray back.
///
/// Without it an application that changes something every 40ms — an animated
/// icon — would keep resetting the quiet period and the panel would never draw
/// it at all.
const BURST_LIMIT: Duration = Duration::from_millis(250);
/// How long any single call to an application may take before it is given up
/// on. A tray icon that does not answer must never leave a menu waiting.
const CALL_TIMEOUT: Duration = Duration::from_secs(5);

/// Everything the panel asks of the tray.
#[derive(Debug)]
pub(super) enum Command {
    /// Left click.
    Activate(String, Reply<()>),
    /// Middle click.
    SecondaryActivate(String, Reply<()>),
    /// A scroll over the icon.
    Scroll(String, i32, ScrollAxis, Reply<()>),
    /// Ask the application to put up a menu of its own.
    ContextMenu(String, Reply<()>),
    /// Fetch the item's menu, ready to draw.
    Menu(String, Reply<MenuNode>),
    /// Report that something happened to a menu row.
    MenuEvent(String, i32, MenuEvent, Reply<()>),
}

/// Where a command's answer goes.
type Reply<T> = oneshot::Sender<Result<T, SvcError>>;

/// Everything the bus tells the tray task.
enum Event {
    /// An item was announced, by `bus_name` + object path.
    Appeared(String),
    /// A bus name lost its owner.
    NameGone(String),
    /// An item answered its questions.
    Read(Box<Ready>),
    /// The external watcher went away; the panel may take the name now.
    WatcherGone,
}

/// What a follower learned about its item.
struct Ready {
    id: String,
    props: ItemProps,
}

/// One item the task is following.
struct Item {
    view: ItemView,
    bus_name: String,
    path: String,
    /// The dbusmenu object, when the item declares one.
    menu_path: Option<String>,
    follower: JoinHandle<()>,
}

/// Publish-debounce timing, with no clock of its own.
///
/// Two deadlines: one that slides with every change, and one that does not
/// move at all. A burst that keeps arriving is published when the second is
/// reached, so continuous churn is drawn at a steady rate rather than never.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Debounce {
    /// When the burst started.
    opened: Option<Instant>,
    /// When the most recent change arrived.
    latest: Option<Instant>,
}

impl Debounce {
    /// Note that something changed.
    fn touch(&mut self, now: Instant) {
        self.opened.get_or_insert(now);
        self.latest = Some(now);
    }

    /// When the tray should next be published, if anything is pending.
    fn due(&self) -> Option<Instant> {
        let opened = self.opened?;
        let latest = self.latest?;
        Some((latest + QUIET).min(opened + BURST_LIMIT))
    }

    /// Forget the burst, having published it.
    fn clear(&mut self) {
        *self = Self::default();
    }
}

/// The tray task's whole state.
struct Tray {
    connection: Connection,
    /// Every item, keyed by the identifier it is addressed with. A `BTreeMap`
    /// because iterating it in key order *is* the stable icon order.
    items: BTreeMap<String, Item>,
    /// The pixmap size the panel would like, from `widgets.tray`.
    target_size: i32,
    registry: Registry,
    debounce: Debounce,
    events: mpsc::Sender<Event>,
    publisher: watch::Sender<Arc<TrayState>>,
    /// Whether the panel holds `org.kde.StatusNotifierWatcher`.
    is_watcher: bool,
}

/// Connect to the bus and run the tray until every handle is dropped.
pub(super) async fn run(
    commands: mpsc::Receiver<Command>,
    publisher: watch::Sender<Arc<TrayState>>,
    target_size: i32,
    address: Option<String>,
) {
    let (events_tx, events_rx) = mpsc::channel(64);
    let (registrations_tx, mut registrations_rx) = mpsc::channel(32);
    let registry = Registry::default();

    let built = match address.as_deref() {
        Some(address) => zbus::connection::Builder::address(address),
        None => zbus::connection::Builder::session(),
    };
    let connection = match serve(built, registry.clone(), registrations_tx).await {
        Ok(connection) => connection,
        Err(error) => {
            // A session with no bus has no tray icons either, and the widget
            // simply never appears.
            warn!("no tray items will be found: {error}");
            return;
        }
    };

    // An item announced to our own watcher interface reaches the task the same
    // way one announced to somebody else's does.
    {
        let events = events_tx.clone();
        tokio::spawn(async move {
            while let Some(Registration::Item(id)) = registrations_rx.recv().await {
                if events.send(Event::Appeared(id)).await.is_err() {
                    return;
                }
            }
        });
    }

    let is_watcher = take_the_watcher_name(&connection).await;
    if let Err(error) = become_a_host(&connection, is_watcher, &registry, &events_tx).await {
        warn!("the tray could not register as a host: {error}");
    }
    tokio::spawn(discover(connection.clone(), events_tx.clone()));

    Tray {
        connection,
        items: BTreeMap::new(),
        target_size,
        registry,
        debounce: Debounce::default(),
        events: events_tx,
        publisher,
        is_watcher,
    }
    .serve(commands, events_rx)
    .await;
}

/// Build the connection and put the watcher interface on it.
///
/// Serving before asking for the name is deliberate: an application that finds
/// the name the instant it appears must find the interface behind it too.
async fn serve(
    builder: zbus::Result<zbus::connection::Builder<'_>>,
    registry: Registry,
    registrations: mpsc::Sender<Registration>,
) -> zbus::Result<Connection> {
    builder?
        .serve_at(WATCHER_PATH, Watcher::new(registry, registrations))?
        .build()
        .await
}

/// Ask for the watcher name, reporting whether the panel got it.
///
/// `DoNotQueue`, and never `ReplaceExisting`: taking the name from a running
/// tray would strand every item already registered with it.
async fn take_the_watcher_name(connection: &Connection) -> bool {
    let flags = [fdo::RequestNameFlags::DoNotQueue].into_iter().collect();
    match connection
        .request_name_with_flags(WATCHER_NAME, flags)
        .await
    {
        Ok(fdo::RequestNameReply::PrimaryOwner) => {
            info!("the panel is the StatusNotifierWatcher for this session");
            true
        }
        Ok(reply) => {
            info!("another StatusNotifierWatcher is running ({reply:?}); joining it as a host");
            false
        }
        Err(error) => {
            warn!("could not ask for {WATCHER_NAME}: {error}");
            false
        }
    }
}

/// Take a host name and announce it, then read whatever is already registered.
async fn become_a_host(
    connection: &Connection,
    is_watcher: bool,
    registry: &Registry,
    events: &mpsc::Sender<Event>,
) -> zbus::Result<()> {
    // The specification names the host after the process that runs it, and
    // watchers written to it look for exactly this prefix.
    let host = format!("org.kde.StatusNotifierHost-{}", std::process::id());
    let flags = [fdo::RequestNameFlags::DoNotQueue].into_iter().collect();
    let _ = connection
        .request_name_with_flags(host.as_str(), flags)
        .await;

    let watcher = StatusNotifierWatcherProxy::new(connection).await?;
    watcher.register_status_notifier_host(&host).await?;

    match watcher.registered_status_notifier_items().await {
        Ok(items) => {
            for id in items {
                // Ours or somebody else's, the registry is what stops the same
                // item being followed twice.
                registry.add_item(&id);
                if events.send(Event::Appeared(id)).await.is_err() {
                    return Ok(());
                }
            }
        }
        Err(error) => warn!("could not list the registered tray items: {error}"),
    }

    if !is_watcher {
        tokio::spawn(follow_watcher(connection.clone(), events.clone()));
    }
    Ok(())
}

/// Follow somebody else's watcher: its items, and its departure.
async fn follow_watcher(connection: Connection, events: mpsc::Sender<Event>) {
    let Ok(watcher) = StatusNotifierWatcherProxy::new(&connection).await else {
        return;
    };
    let (Ok(mut registered), Ok(mut unregistered)) = (
        watcher.receive_status_notifier_item_registered().await,
        watcher.receive_status_notifier_item_unregistered().await,
    ) else {
        warn!("cannot follow the external watcher's items");
        return;
    };

    loop {
        let event = tokio::select! {
            Some(signal) = registered.next() => match signal.args() {
                Ok(args) => Event::Appeared(args.service().to_string()),
                Err(_) => continue,
            },
            Some(signal) = unregistered.next() => match signal.args() {
                // The item's *name* is what the task tracks, so an
                // unregistration is handled exactly like a departure.
                Ok(args) => match split_id(args.service()) {
                    Some((bus_name, _)) => Event::NameGone(bus_name),
                    None => continue,
                },
                Err(_) => continue,
            },
            else => {
                // Both streams ended: the watcher is gone.
                let _ = events.send(Event::WatcherGone).await;
                return;
            }
        };
        if events.send(event).await.is_err() {
            return;
        }
    }
}

impl Tray {
    /// Apply messages until the panel drops its last handle.
    async fn serve(
        mut self,
        mut commands: mpsc::Receiver<Command>,
        mut events: mpsc::Receiver<Event>,
    ) {
        self.publish();
        loop {
            tokio::select! {
                command = commands.recv() => match command {
                    Some(command) => self.apply_command(command),
                    None => break,
                },
                event = events.recv() => match event {
                    Some(event) => self.apply_event(event).await,
                    // Only possible if this task dropped its own sender.
                    None => break,
                },
                () = sleep_until(self.debounce.due()) => {
                    self.debounce.clear();
                    self.publish();
                }
            }
        }

        for item in self.items.values() {
            item.follower.abort();
        }
        debug!("the tray service is shutting down");
    }

    // -----------------------------------------------------------------------
    // Commands
    // -----------------------------------------------------------------------

    /// Apply one command from the panel.
    ///
    /// Every one of them ends in a spawned call: an application that has
    /// stopped answering must delay its own click, not the whole tray.
    fn apply_command(&mut self, command: Command) {
        match command {
            Command::Activate(id, reply) => {
                self.act(
                    &id,
                    reply,
                    |proxy| async move { proxy.activate(0, 0).await },
                )
            }
            Command::SecondaryActivate(id, reply) => self.act(&id, reply, |proxy| async move {
                proxy.secondary_activate(0, 0).await
            }),
            Command::Scroll(id, delta, axis, reply) => {
                self.act(&id, reply, move |proxy| async move {
                    proxy.scroll(delta, axis.as_str()).await
                })
            }
            Command::ContextMenu(id, reply) => {
                self.act(
                    &id,
                    reply,
                    |proxy| async move { proxy.context_menu(0, 0).await },
                )
            }
            Command::Menu(id, reply) => self.fetch_menu(&id, reply),
            Command::MenuEvent(id, item_id, event, reply) => {
                self.send_menu_event(&id, item_id, event, reply);
            }
        }
    }

    /// Call something on an item, off the loop.
    fn act<F, R>(&self, id: &str, reply: Reply<()>, call: F)
    where
        F: FnOnce(StatusNotifierItemProxy<'static>) -> R + Send + 'static,
        R: std::future::Future<Output = zbus::Result<()>> + Send,
    {
        let Some(item) = self.items.get(id) else {
            let _ = reply.send(Err(SvcError::NoTrayItem(id.to_string())));
            return;
        };
        let (connection, bus_name, path) = (
            self.connection.clone(),
            item.bus_name.clone(),
            item.path.clone(),
        );
        let id = id.to_string();

        tokio::spawn(async move {
            let answer = async {
                let proxy = item_proxy(&connection, &bus_name, &path).await?;
                call(proxy).await
            };
            let outcome = match tokio::time::timeout(CALL_TIMEOUT, answer).await {
                Ok(Ok(())) => Ok(()),
                Ok(Err(error)) => Err(SvcError::Bus(format!("{id}: {error}"))),
                Err(_) => Err(SvcError::Bus(format!("{id} did not answer"))),
            };
            let _ = reply.send(outcome);
        });
    }

    /// Fetch an item's menu: `AboutToShow`, then `GetLayout`.
    ///
    /// The order is the protocol's: an application is entitled to build its
    /// menu only when told one is about to open, and several only populate it
    /// there. Its answer — and its failure — are both ignored, because just as
    /// many never implement it at all.
    fn fetch_menu(&self, id: &str, reply: Reply<MenuNode>) {
        let Some(item) = self.items.get(id) else {
            let _ = reply.send(Err(SvcError::NoTrayItem(id.to_string())));
            return;
        };
        let Some(menu_path) = item.menu_path.clone() else {
            let _ = reply.send(Err(SvcError::NoTrayMenu(id.to_string())));
            return;
        };
        let (connection, bus_name) = (self.connection.clone(), item.bus_name.clone());
        let id = id.to_string();

        tokio::spawn(async move {
            let answer = async {
                let menu = menu_proxy(&connection, &bus_name, &menu_path).await?;
                if let Err(error) = menu.about_to_show(0).await {
                    debug!("{id} does not implement AboutToShow: {error}");
                }
                let (_revision, layout) = menu.get_layout(0, -1, &[]).await?;
                zbus::Result::Ok(MenuNode::parse(&layout))
            };
            let outcome = match tokio::time::timeout(CALL_TIMEOUT, answer).await {
                Ok(Ok(menu)) => Ok(menu),
                Ok(Err(error)) => Err(SvcError::Bus(format!("{id} menu: {error}"))),
                Err(_) => Err(SvcError::Bus(format!("{id} did not send its menu"))),
            };
            let _ = reply.send(outcome);
        });
    }

    /// Tell an application that one of its menu rows was chosen.
    fn send_menu_event(&self, id: &str, item_id: i32, event: MenuEvent, reply: Reply<()>) {
        let Some(item) = self.items.get(id) else {
            let _ = reply.send(Err(SvcError::NoTrayItem(id.to_string())));
            return;
        };
        let Some(menu_path) = item.menu_path.clone() else {
            let _ = reply.send(Err(SvcError::NoTrayMenu(id.to_string())));
            return;
        };
        let (connection, bus_name) = (self.connection.clone(), item.bus_name.clone());
        let id = id.to_string();

        tokio::spawn(async move {
            let answer = async {
                let menu = menu_proxy(&connection, &bus_name, &menu_path).await?;
                menu.event(item_id, event.as_str(), &Value::I32(0), timestamp())
                    .await
            };
            let outcome = match tokio::time::timeout(CALL_TIMEOUT, answer).await {
                Ok(Ok(())) => Ok(()),
                Ok(Err(error)) => Err(SvcError::Bus(format!("{id} menu event: {error}"))),
                Err(_) => Err(SvcError::Bus(format!("{id} did not take the menu event"))),
            };
            let _ = reply.send(outcome);
        });
    }

    // -----------------------------------------------------------------------
    // Events
    // -----------------------------------------------------------------------

    /// Apply one message from the bus.
    async fn apply_event(&mut self, event: Event) {
        match event {
            Event::Appeared(id) => self.add(&id),
            Event::NameGone(bus_name) => self.drop_name(&bus_name).await,
            Event::Read(ready) => self.read(*ready),
            Event::WatcherGone => self.take_over().await,
        }
    }

    /// Start following an item.
    fn add(&mut self, id: &str) {
        let Some((bus_name, path)) = split_id(id) else {
            warn!("ignoring a tray item with no bus name: {id:?}");
            return;
        };
        let id = make_id(&bus_name, &path);
        if self.items.contains_key(&id) {
            // Applications re-register in bursts. The item is already on the
            // bar; announcing it again must not take it off and put it back.
            return;
        }

        debug!("tray item {id} appeared");
        let follower = tokio::spawn(follow(
            id.clone(),
            bus_name.clone(),
            path.clone(),
            self.connection.clone(),
            self.events.clone(),
        ));

        self.items.insert(
            id.clone(),
            Item {
                // Nothing is published until the item has answered: an icon
                // that flashed a placeholder before its real one would be
                // worse than one that appears a moment later.
                view: ItemView {
                    id,
                    title: String::new(),
                    status: super::model::Status::Passive,
                    tooltip: None,
                    icon: super::model::IconView::Fallback,
                    has_menu: false,
                    item_is_menu: false,
                },
                bus_name,
                path,
                menu_path: None,
                follower,
            },
        );
    }

    /// An item answered its questions.
    fn read(&mut self, ready: Ready) {
        let target = self.target_size;
        let Some(item) = self.items.get_mut(&ready.id) else {
            return;
        };
        let props = ready.props;

        item.menu_path = props.menu_path.clone();
        item.view = ItemView {
            id: ready.id.clone(),
            title: props.title,
            status: props.status,
            tooltip: props.tooltip,
            icon: resolve(&props.icon, props.status, target),
            has_menu: props.menu_path.is_some(),
            item_is_menu: props.item_is_menu,
        };
        self.debounce.touch(Instant::now());
    }

    /// A bus name lost its owner: everything it served is gone.
    async fn drop_name(&mut self, bus_name: &str) {
        let gone: Vec<String> = self
            .items
            .iter()
            .filter(|(_, item)| item.bus_name == bus_name)
            .map(|(id, _)| id.clone())
            .collect();

        for id in gone {
            if let Some(item) = self.items.remove(&id) {
                debug!("tray item {id} went away");
                item.follower.abort();
            }
            if self.registry.remove_item(&id) && self.is_watcher {
                self.announce_departure(&id).await;
            }
            self.debounce.touch(Instant::now());
        }
    }

    /// Tell everyone else on the bus that an item has gone.
    async fn announce_departure(&self, id: &str) {
        let server = self.connection.object_server();
        let Ok(watcher) = server.interface::<_, Watcher>(WATCHER_PATH).await else {
            return;
        };
        if let Err(error) =
            Watcher::status_notifier_item_unregistered(watcher.signal_emitter(), id).await
        {
            warn!("could not announce that {id} has gone: {error}");
        }
    }

    /// The watcher that held the name has quit; take it over.
    ///
    /// Every item the panel knew was registered with that watcher, so the
    /// slate is wiped and the applications are given the chance to announce
    /// themselves again — which is what they do when the name reappears.
    async fn take_over(&mut self) {
        if self.is_watcher {
            return;
        }
        self.is_watcher = take_the_watcher_name(&self.connection).await;
        if !self.is_watcher {
            return;
        }
        info!("the previous StatusNotifierWatcher quit; the panel has taken over");
        for (id, item) in std::mem::take(&mut self.items) {
            item.follower.abort();
            self.registry.remove_item(&id);
        }
        self.debounce.touch(Instant::now());
    }

    // -----------------------------------------------------------------------
    // Publishing
    // -----------------------------------------------------------------------

    /// Publish the tray, unless nothing the widget draws has changed.
    ///
    /// Passive items are left out entirely: the specification says a
    /// visualization is likely to hide them, and a panel that says nothing
    /// when there is nothing to say is the whole design.
    fn publish(&self) {
        let state = TrayState {
            items: self
                .items
                .values()
                .map(|item| &item.view)
                .filter(|view| view.status.is_visible())
                .cloned()
                .collect(),
        };
        self.publisher.send_if_modified(|current| {
            if **current == state {
                return false;
            }
            *current = Arc::new(state);
            true
        });
    }
}

/// The timestamp a dbusmenu event carries.
fn timestamp() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs() as u32)
        .unwrap_or_default()
}

/// Sleep until `deadline`, or forever when there is nothing to wait for.
async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline.into()).await,
        None => std::future::pending().await,
    }
}

// ---------------------------------------------------------------------------
// The bus side
// ---------------------------------------------------------------------------

/// Watch the bus for names going away.
///
/// This, rather than the item's own signals, is how a departure is noticed: an
/// application that is killed emits nothing at all on its way out.
async fn discover(connection: Connection, events: mpsc::Sender<Event>) {
    let dbus = match fdo::DBusProxy::new(&connection).await {
        Ok(proxy) => proxy,
        Err(error) => {
            warn!("cannot watch the bus for tray items: {error}");
            return;
        }
    };
    let mut changes = match dbus.receive_name_owner_changed().await {
        Ok(changes) => changes,
        Err(error) => {
            warn!("cannot watch the bus for tray items: {error}");
            return;
        }
    };

    while let Some(signal) = changes.next().await {
        let Ok(args) = signal.args() else {
            continue;
        };
        if args.new_owner().is_some() {
            continue;
        }
        let name = args.name().to_string();
        // The watcher going away is worth knowing about too: the panel can
        // then take the name and serve the session itself.
        let event = if name == WATCHER_NAME {
            Event::WatcherGone
        } else {
            Event::NameGone(name)
        };
        if events.send(event).await.is_err() {
            return;
        }
    }
}

/// Follow one item until it stops answering.
///
/// Subscribed *before* the first read, deliberately: an item that changed its
/// icon in the moment between the read and the subscription would sit on the
/// bar wearing the wrong one until it next did something.
async fn follow(
    id: String,
    bus_name: String,
    path: String,
    connection: Connection,
    events: mpsc::Sender<Event>,
) {
    let subscription = async {
        let properties = properties_proxy(&connection, &bus_name, &path).await?;
        let item = item_proxy(&connection, &bus_name, &path).await?;
        let signals = item.inner().receive_all_signals().await?;
        zbus::Result::Ok((properties, signals))
    };
    let (properties, mut signals) = match subscription.await {
        Ok(subscription) => subscription,
        Err(error) => {
            debug!("cannot follow tray item {id}: {error}");
            let _ = events.send(Event::NameGone(bus_name)).await;
            return;
        }
    };

    let interface = match InterfaceName::try_from(ITEM_INTERFACE) {
        Ok(interface) => interface,
        Err(_) => return,
    };

    // Every `New*` signal means the same thing — ask again — so the read is
    // shared rather than written seven times.
    loop {
        let read = tokio::time::timeout(CALL_TIMEOUT, properties.get_all(interface.clone())).await;
        match read {
            Ok(Ok(properties)) => {
                let props = super::props::parse(&properties, &id);
                let ready = Ready {
                    id: id.clone(),
                    props,
                };
                if events.send(Event::Read(Box::new(ready))).await.is_err() {
                    return;
                }
            }
            Ok(Err(error)) => {
                debug!("tray item {id} would not answer: {error}");
                let _ = events.send(Event::NameGone(bus_name)).await;
                return;
            }
            Err(_) => {
                debug!("tray item {id} did not answer in time");
                let _ = events.send(Event::NameGone(bus_name)).await;
                return;
            }
        }

        // Wait for the item to say something changed, then read it all again.
        let Some(signal) = signals.next().await else {
            // The stream ended: the connection behind it is gone.
            let _ = events.send(Event::NameGone(bus_name)).await;
            return;
        };
        let member = signal
            .header()
            .member()
            .map(|member| member.to_string())
            .unwrap_or_default();
        debug!("tray item {id}: {member}");
    }
}

/// The standard properties interface of one item.
async fn properties_proxy(
    connection: &Connection,
    bus_name: &str,
    path: &str,
) -> zbus::Result<fdo::PropertiesProxy<'static>> {
    fdo::PropertiesProxy::builder(connection)
        .destination(bus_name.to_string())?
        .path(path.to_string())?
        .build()
        .await
}

/// A proxy for one item, at the path it actually lives at.
async fn item_proxy(
    connection: &Connection,
    bus_name: &str,
    path: &str,
) -> zbus::Result<StatusNotifierItemProxy<'static>> {
    StatusNotifierItemProxy::builder(connection)
        .destination(bus_name.to_string())?
        .path(path.to_string())?
        .cache_properties(CacheProperties::No)
        .build()
        .await
}

/// A proxy for one item's menu.
async fn menu_proxy(
    connection: &Connection,
    bus_name: &str,
    menu_path: &str,
) -> zbus::Result<DBusMenuProxy<'static>> {
    DBusMenuProxy::builder(connection)
        .destination(bus_name.to_string())?
        .path(menu_path.to_string())?
        .cache_properties(CacheProperties::No)
        .build()
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(base: Instant, millis: u64) -> Instant {
        base + Duration::from_millis(millis)
    }

    #[test]
    fn nothing_pending_means_nothing_to_wait_for() {
        let debounce = Debounce::default();
        assert_eq!(debounce.due(), None);
    }

    #[test]
    fn one_change_is_published_a_quiet_period_later() {
        let start = Instant::now();
        let mut debounce = Debounce::default();
        debounce.touch(start);
        assert_eq!(debounce.due(), Some(start + QUIET));
    }

    #[test]
    fn a_burst_is_published_once_it_goes_quiet() {
        // Five changes 40ms apart — an application re-registering — settle
        // into one publish 50ms after the last of them.
        let start = Instant::now();
        let mut debounce = Debounce::default();
        for step in 0..5 {
            debounce.touch(at(start, step * 40));
        }
        assert_eq!(
            debounce.due(),
            Some(at(start, 160) + QUIET),
            "the quiet period runs from the last change, not the first"
        );
        assert!(
            debounce.due().expect("pending") < at(start, 160) + BURST_LIMIT,
            "a burst that ends is published before the limit is reached"
        );
    }

    #[test]
    fn a_burst_that_never_ends_is_still_drawn() {
        // An animated icon changing every 40ms would reset the quiet period
        // forever; the hard limit is what stops the panel going blank.
        let start = Instant::now();
        let mut debounce = Debounce::default();
        for step in 0..20 {
            debounce.touch(at(start, step * 40));
        }
        assert_eq!(debounce.due(), Some(start + BURST_LIMIT));
    }

    #[test]
    fn publishing_closes_the_burst() {
        let start = Instant::now();
        let mut debounce = Debounce::default();
        debounce.touch(start);
        debounce.clear();
        assert_eq!(debounce.due(), None);

        debounce.touch(at(start, 500));
        assert_eq!(
            debounce.due(),
            Some(at(start, 500) + QUIET),
            "the next burst starts its own clock"
        );
    }
}
