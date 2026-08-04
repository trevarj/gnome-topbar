//! A tray application that exists only to be tested against.
//!
//! It serves the two interfaces a real one does — `org.kde.StatusNotifierItem`
//! and `com.canonical.dbusmenu` — plus a control interface so a test (or the
//! visual smoke driver) can make it do things no button on the panel can:
//! change its icon, start shouting for attention, or re-register five times in
//! a fifth of a second the way a chat client reconnecting does.
//!
//! It is used two ways, and deliberately only written once:
//!
//! - the bus tests drive it in-process over a private `dbus-daemon`;
//! - `topbar-fake-sni` (`--features fake-sni`) runs it as a program, so the
//!   nested-niri smoke run can put a dozen of them on its private bus.
//!
//! ```text
//! org.example.FakeSni.<name>             the well-known name
//!   /StatusNotifierItem
//!     org.kde.StatusNotifierItem         the item
//!     io.github.trevarj.topbar.FakeSni1  SetStatus, SetIconName, …
//!   /MenuBar
//!     com.canonical.dbusmenu             the menu
//! ```
//!
//! Menus are built from a small JSON grammar so a scenario can describe one in
//! a shell script:
//!
//! ```json
//! [{"label": "Open"},
//!  {"label": "Notify", "toggle": "checkmark", "state": 1},
//!  {"separator": true},
//!  {"label": "Away", "disabled": true},
//!  {"label": "More", "submenu": [{"label": "About"}]}]
//! ```

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedValue, Structure, Value};
use zbus::{Connection, interface};

/// Where the fake serves its item.
pub const ITEM_PATH: &str = "/StatusNotifierItem";
/// Where it serves its menu.
pub const MENU_PATH: &str = "/MenuBar";
/// The prefix every fake takes its well-known name under.
pub const NAME_PREFIX: &str = "org.example.FakeSni.";
/// The control interface, so a test can move the fake about.
pub const CONTROL_INTERFACE: &str = "io.github.trevarj.topbar.FakeSni1";

/// What the panel called on the fake, so a test can assert it arrived.
#[derive(Debug, Default, Clone)]
struct Calls(Arc<Mutex<Vec<String>>>);

impl Calls {
    fn note(&self, what: String) {
        self.0
            .lock()
            .expect("the call log is never held across an await")
            .push(what);
    }

    fn seen(&self) -> Vec<String> {
        self.0
            .lock()
            .expect("the call log is never held across an await")
            .clone()
    }
}

/// How a fake tray application starts out.
#[derive(Debug, Clone)]
pub struct Recipe {
    /// The tail of the bus name, e.g. `one`.
    pub name: String,
    /// The `Id` it publishes.
    pub id: String,
    /// The `Title` it publishes.
    pub title: String,
    /// `Passive`, `Active` or `NeedsAttention`.
    pub status: String,
    /// A Freedesktop icon name, or nothing.
    pub icon_name: Option<String>,
    /// `IconThemePath`, for an application shipping its own icons.
    pub theme_path: Option<String>,
    /// Pixmaps to publish, as `(width, height, fill)` triples.
    ///
    /// The fill is one ARGB pixel repeated across the whole picture, which is
    /// enough to tell a colourful icon from a near-black one in a screenshot.
    pub pixmaps: Vec<(i32, i32, u32)>,
    /// The tooltip's title.
    pub tooltip_title: String,
    /// The tooltip's description.
    pub tooltip_body: String,
    /// Whether a left click should open the menu.
    pub item_is_menu: bool,
    /// The menu, in the JSON grammar. `None` means it publishes no menu.
    pub menu: Option<String>,
}

impl Default for Recipe {
    fn default() -> Self {
        Self {
            name: "fake".to_string(),
            id: "fake-sni".to_string(),
            title: "Fake Tray Item".to_string(),
            status: "Active".to_string(),
            icon_name: Some("application-x-executable".to_string()),
            theme_path: None,
            pixmaps: Vec::new(),
            tooltip_title: "Fake Tray Item".to_string(),
            tooltip_body: String::new(),
            item_is_menu: false,
            menu: None,
        }
    }
}

impl Recipe {
    /// The well-known name this application takes.
    pub fn bus_name(&self) -> String {
        format!("{NAME_PREFIX}{}", self.name)
    }
}

// ---------------------------------------------------------------------------
// The menu grammar
// ---------------------------------------------------------------------------

/// One row, as a scenario describes it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MenuSpec {
    /// The row's text. Underscores are mnemonic markers, as in a real menu.
    pub label: String,
    /// Whether the row is a horizontal rule instead.
    pub separator: bool,
    /// Whether the row is greyed out.
    pub disabled: bool,
    /// Whether the row is left out of the layout's visible rows.
    pub hidden: bool,
    /// `checkmark` or `radio`.
    pub toggle: Option<String>,
    /// 0 off, 1 on, anything else indeterminate.
    pub state: i32,
    /// A Freedesktop icon name for the row.
    pub icon: Option<String>,
    /// Rows underneath this one.
    pub submenu: Vec<MenuSpec>,
}

/// The menu every scenario gets when it does not ask for another.
pub const DEFAULT_MENU: &str = r#"[
  {"label": "_Open Window"},
  {"label": "Show __Notifications", "toggle": "checkmark", "state": 1},
  {"separator": true},
  {"label": "Online", "toggle": "radio", "state": 1},
  {"label": "Away", "toggle": "radio", "state": 0},
  {"separator": true},
  {"label": "Not Available", "disabled": true},
  {"label": "Hidden Row", "hidden": true},
  {"label": "Preferences", "icon": "preferences-system-symbolic"},
  {"label": "More", "submenu": [
      {"label": "About"},
      {"label": "Report a Bug"}
  ]},
  {"label": "Quit"}
]"#;

/// Parse a menu description, or explain why it is not one.
pub fn parse_menu(json: &str) -> Result<Vec<MenuSpec>, String> {
    serde_json::from_str(json).map_err(|error| error.to_string())
}

/// Turn one row into the `(ia{sv}av)` a dbusmenu layout is made of.
fn layout_node(id: &mut i32, spec: &MenuSpec) -> Structure<'static> {
    let own = *id;
    *id += 1;

    let mut properties: HashMap<String, Value<'static>> = HashMap::new();
    if spec.separator {
        properties.insert("type".into(), "separator".into());
    } else {
        properties.insert("label".into(), spec.label.clone().into());
    }
    if spec.disabled {
        properties.insert("enabled".into(), false.into());
    }
    if spec.hidden {
        properties.insert("visible".into(), false.into());
    }
    if let Some(toggle) = &spec.toggle {
        properties.insert("toggle-type".into(), toggle.clone().into());
        properties.insert("toggle-state".into(), spec.state.into());
    }
    if let Some(icon) = &spec.icon {
        properties.insert("icon-name".into(), icon.clone().into());
    }

    let children: Vec<Value<'static>> = spec
        .submenu
        .iter()
        .map(|child| Value::from(layout_node(id, child)))
        .collect();
    if !children.is_empty() {
        properties.insert("children-display".into(), "submenu".into());
    }

    Structure::from((own, properties, children))
}

/// The root of a layout: `(ia{sv}av)`, spelled out so it has a static
/// signature. [`Structure`] has a dynamic one, which an interface method may
/// not return.
#[derive(Debug, Serialize, zbus::zvariant::Type)]
pub(super) struct LayoutNode {
    /// The row's id.
    pub id: i32,
    /// Its properties.
    pub properties: HashMap<String, OwnedValue>,
    /// Its children, each a node inside a variant.
    pub children: Vec<OwnedValue>,
}

/// Build a whole layout: the root, and every row under it.
fn layout(specs: &[MenuSpec]) -> LayoutNode {
    let mut next = 1;
    let children = specs
        .iter()
        .map(|spec| {
            OwnedValue::try_from(Value::from(layout_node(&mut next, spec)))
                .expect("a generated layout node can be owned")
        })
        .collect();

    let mut properties = HashMap::new();
    properties.insert(
        "children-display".to_string(),
        OwnedValue::try_from(Value::from("submenu")).expect("a constant can be owned"),
    );

    LayoutNode {
        id: 0,
        properties,
        children,
    }
}

// ---------------------------------------------------------------------------
// The interfaces
// ---------------------------------------------------------------------------

/// The three interfaces, and the state behind them.
///
/// A private module, and deliberately: zbus drops the doc comment off the
/// emitter it generates for a signal, and `missing_docs` is on for this crate.
/// Nothing in here is reachable from outside `fake`, so nothing in here is
/// public API and the lint has nothing to say about it.
mod imp {
    use super::*;

    /// The `ToolTip` property: an icon name, icon data, a title, a body.
    pub(super) type ToolTip = (String, Vec<(i32, i32, Vec<u8>)>, String, String);

    /// The tray item itself, and the state behind it.
    pub(super) struct Item {
        pub(super) id: String,
        pub(super) title: String,
        pub(super) status: String,
        pub(super) icon_name: Option<String>,
        pub(super) theme_path: Option<String>,
        pub(super) pixmaps: Vec<(i32, i32, u32)>,
        pub(super) tooltip_title: String,
        pub(super) tooltip_body: String,
        pub(super) item_is_menu: bool,
        pub(super) has_menu: bool,
        pub(super) calls: Calls,
        pub(super) acted: Arc<Notify>,
    }

    impl Item {
        fn note(&self, what: String) {
            self.calls.note(what);
            self.acted.notify_waiters();
        }
    }

    /// The ARGB bytes of a solid rectangle, in the network byte order the
    /// specification asks for.
    pub(super) fn solid(width: i32, height: i32, argb: u32) -> Vec<u8> {
        let pixel = argb.to_be_bytes();
        pixel
            .iter()
            .copied()
            .cycle()
            .take((width.max(0) as usize) * (height.max(0) as usize) * 4)
            .collect()
    }

    #[interface(name = "org.kde.StatusNotifierItem")]
    impl Item {
        /// A left click reached the application.
        fn activate(&self, x: i32, y: i32) {
            self.note(format!("Activate({x},{y})"));
        }

        /// A middle click did.
        fn secondary_activate(&self, x: i32, y: i32) {
            self.note(format!("SecondaryActivate({x},{y})"));
        }

        /// The panel asked the application to show its own menu.
        fn context_menu(&self, x: i32, y: i32) {
            self.note(format!("ContextMenu({x},{y})"));
        }

        /// A scroll over the icon.
        fn scroll(&self, delta: i32, orientation: &str) {
            self.note(format!("Scroll({delta},{orientation})"));
        }

        #[zbus(property)]
        fn id(&self) -> &str {
            &self.id
        }

        #[zbus(property)]
        fn category(&self) -> &str {
            "ApplicationStatus"
        }

        #[zbus(property)]
        fn title(&self) -> &str {
            &self.title
        }

        #[zbus(property)]
        fn status(&self) -> &str {
            &self.status
        }

        #[zbus(property)]
        fn window_id(&self) -> i32 {
            0
        }

        #[zbus(property)]
        fn item_is_menu(&self) -> bool {
            self.item_is_menu
        }

        #[zbus(property)]
        fn icon_name(&self) -> &str {
            self.icon_name.as_deref().unwrap_or_default()
        }

        #[zbus(property)]
        fn icon_theme_path(&self) -> &str {
            self.theme_path.as_deref().unwrap_or_default()
        }

        #[zbus(property)]
        fn icon_pixmap(&self) -> Vec<(i32, i32, Vec<u8>)> {
            self.pixmaps
                .iter()
                .map(|&(width, height, argb)| (width, height, solid(width, height, argb)))
                .collect()
        }

        #[zbus(property)]
        fn attention_icon_name(&self) -> &str {
            ""
        }

        #[zbus(property)]
        fn tool_tip(&self) -> ToolTip {
            (
                String::new(),
                Vec::new(),
                self.tooltip_title.clone(),
                self.tooltip_body.clone(),
            )
        }

        #[zbus(property)]
        fn menu(&self) -> ObjectPath<'_> {
            let path = if self.has_menu { MENU_PATH } else { "/" };
            ObjectPath::try_from(path).expect("a constant path is well formed")
        }

        /// The icon changed.
        #[zbus(signal)]
        async fn new_icon(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

        /// The status changed. The argument is what real applications send.
        #[zbus(signal)]
        async fn new_status(emitter: &SignalEmitter<'_>, status: &str) -> zbus::Result<()>;

        /// The tooltip changed.
        #[zbus(signal)]
        async fn new_tool_tip(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

        /// The title changed.
        #[zbus(signal)]
        async fn new_title(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
    }

    // ---------------------------------------------------------------------------
    // The menu
    // ---------------------------------------------------------------------------

    /// The dbusmenu object, and the rows it serves.
    pub(super) struct Menu {
        pub(super) rows: Vec<MenuSpec>,
        pub(super) revision: u32,
        pub(super) calls: Calls,
        pub(super) acted: Arc<Notify>,
    }

    #[interface(name = "com.canonical.dbusmenu")]
    impl Menu {
        /// The panel is about to open the menu.
        fn about_to_show(&self, id: i32) -> bool {
            self.calls.note(format!("AboutToShow({id})"));
            self.acted.notify_waiters();
            false
        }

        /// Something happened to a row.
        fn event(&self, id: i32, event_id: &str, _data: Value<'_>, _timestamp: u32) {
            self.calls.note(format!("Event({id},{event_id})"));
            self.acted.notify_waiters();
        }

        /// The layout, whole.
        fn get_layout(
            &self,
            _parent_id: i32,
            _recursion_depth: i32,
            _property_names: Vec<String>,
        ) -> (u32, LayoutNode) {
            (self.revision, layout(&self.rows))
        }

        /// Properties for a set of ids. Nothing the panel asks for.
        fn get_group_properties(
            &self,
            _ids: Vec<i32>,
            _property_names: Vec<String>,
        ) -> Vec<(i32, HashMap<String, OwnedValue>)> {
            Vec::new()
        }

        /// One property of one row. Nothing the panel asks for either.
        fn get_property(&self, _id: i32, name: &str) -> zbus::fdo::Result<OwnedValue> {
            Err(zbus::fdo::Error::InvalidArgs(format!("no property {name}")))
        }

        #[zbus(property)]
        fn version(&self) -> u32 {
            3
        }

        #[zbus(property)]
        fn status(&self) -> &str {
            "normal"
        }

        /// The layout changed and should be fetched again.
        #[zbus(signal)]
        async fn layout_updated(
            emitter: &SignalEmitter<'_>,
            revision: u32,
            parent: i32,
        ) -> zbus::Result<()>;
    }

    // ---------------------------------------------------------------------------
    // The control interface
    // ---------------------------------------------------------------------------

    /// What a test may do to the fake that a user could not.
    pub(super) struct Control {
        pub(super) bus_name: String,
        pub(super) stopped: Arc<Notify>,
    }

    /// Announce an item to whatever watcher is on the bus.
    pub(super) async fn register(connection: &Connection, bus_name: &str) -> zbus::Result<()> {
        crate::tray::proxy::StatusNotifierWatcherProxy::new(connection)
            .await?
            .register_status_notifier_item(bus_name)
            .await
    }

    #[interface(name = "io.github.trevarj.topbar.FakeSni1")]
    impl Control {
        /// Set the status, announcing it the way a real application would.
        async fn set_status(
            &self,
            status: String,
            #[zbus(object_server)] server: &zbus::ObjectServer,
        ) -> zbus::fdo::Result<()> {
            let iface = server.interface::<_, Item>(ITEM_PATH).await?;
            {
                let mut item = iface.get_mut().await;
                item.status = status.clone();
            }
            Item::new_status(iface.signal_emitter(), &status).await?;
            Ok(())
        }

        /// Set the icon name, announcing it.
        async fn set_icon_name(
            &self,
            name: String,
            #[zbus(object_server)] server: &zbus::ObjectServer,
        ) -> zbus::fdo::Result<()> {
            let iface = server.interface::<_, Item>(ITEM_PATH).await?;
            {
                let mut item = iface.get_mut().await;
                item.icon_name = (!name.is_empty()).then_some(name);
            }
            Item::new_icon(iface.signal_emitter()).await?;
            Ok(())
        }

        /// Replace the pixmaps with one solid `width`x`height` picture.
        ///
        /// A non-square size is the point of the width and height being separate:
        /// this is what catches a host that reads one out of the other.
        async fn set_icon_pixmap(
            &self,
            width: i32,
            height: i32,
            argb: u32,
            #[zbus(object_server)] server: &zbus::ObjectServer,
        ) -> zbus::fdo::Result<()> {
            let iface = server.interface::<_, Item>(ITEM_PATH).await?;
            {
                let mut item = iface.get_mut().await;
                item.icon_name = None;
                item.pixmaps = vec![(width, height, argb)];
            }
            Item::new_icon(iface.signal_emitter()).await?;
            Ok(())
        }

        /// Set the tooltip, announcing it.
        async fn set_tool_tip(
            &self,
            title: String,
            body: String,
            #[zbus(object_server)] server: &zbus::ObjectServer,
        ) -> zbus::fdo::Result<()> {
            let iface = server.interface::<_, Item>(ITEM_PATH).await?;
            {
                let mut item = iface.get_mut().await;
                item.tooltip_title = title;
                item.tooltip_body = body;
            }
            Item::new_tool_tip(iface.signal_emitter()).await?;
            Ok(())
        }

        /// Replace the menu from the JSON grammar, announcing the new layout.
        async fn set_menu(
            &self,
            json: String,
            #[zbus(object_server)] server: &zbus::ObjectServer,
        ) -> zbus::fdo::Result<()> {
            let rows = parse_menu(&json).map_err(zbus::fdo::Error::InvalidArgs)?;
            let iface = server.interface::<_, Menu>(MENU_PATH).await?;
            let revision = {
                let mut menu = iface.get_mut().await;
                menu.rows = rows;
                menu.revision = menu.revision.wrapping_add(1);
                menu.revision
            };
            Menu::layout_updated(iface.signal_emitter(), revision, 0).await?;
            Ok(())
        }

        /// Emit `NewIcon` without changing anything, the way a spinner does.
        async fn trigger_new_icon(
            &self,
            #[zbus(object_server)] server: &zbus::ObjectServer,
        ) -> zbus::fdo::Result<()> {
            let iface = server.interface::<_, Item>(ITEM_PATH).await?;
            Item::new_icon(iface.signal_emitter()).await?;
            Ok(())
        }

        /// Announce the item to the watcher again, as a reconnecting client does.
        ///
        /// The connection is borrowed from the call rather than held in the
        /// struct: an interface that owns the connection it is served on is a
        /// cycle, and the fake would never leave the bus when it was told to.
        async fn reregister(
            &self,
            #[zbus(connection)] connection: &Connection,
        ) -> zbus::fdo::Result<()> {
            register(connection, &self.bus_name)
                .await
                .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))
        }

        /// Leave the bus.
        fn quit(&self) {
            self.stopped.notify_waiters();
        }
    }
}

// ---------------------------------------------------------------------------
// The application
// ---------------------------------------------------------------------------

use imp::{Control, Item, Menu};

/// A fake tray application that is on the bus for as long as this value lives.
pub struct FakeSni {
    connection: Connection,
    bus_name: String,
    calls: Calls,
    acted: Arc<Notify>,
    stopped: Arc<Notify>,
}

impl FakeSni {
    /// Put an application on the bus at `address`, or on the session bus.
    ///
    /// It takes its name and serves its interfaces, but does *not* announce
    /// itself: [`FakeSni::register`] is separate so a test can start the panel
    /// first and watch the item arrive.
    pub async fn start(recipe: &Recipe, address: Option<&str>) -> zbus::Result<Self> {
        let calls = Calls::default();
        let acted = Arc::new(Notify::new());
        let stopped = Arc::new(Notify::new());

        let rows = match recipe.menu.as_deref() {
            Some(json) => parse_menu(json)
                .map_err(|error| zbus::Error::Failure(format!("bad menu: {error}")))?,
            None => Vec::new(),
        };

        let builder = match address {
            Some(address) => zbus::connection::Builder::address(address)?,
            None => zbus::connection::Builder::session()?,
        };

        let bus_name = recipe.bus_name();
        let connection = builder
            .name(bus_name.clone())?
            .serve_at(
                ITEM_PATH,
                Item {
                    id: recipe.id.clone(),
                    title: recipe.title.clone(),
                    status: recipe.status.clone(),
                    icon_name: recipe.icon_name.clone(),
                    theme_path: recipe.theme_path.clone(),
                    pixmaps: recipe.pixmaps.clone(),
                    tooltip_title: recipe.tooltip_title.clone(),
                    tooltip_body: recipe.tooltip_body.clone(),
                    item_is_menu: recipe.item_is_menu,
                    has_menu: recipe.menu.is_some(),
                    calls: calls.clone(),
                    acted: Arc::clone(&acted),
                },
            )?
            .serve_at(
                MENU_PATH,
                Menu {
                    rows,
                    revision: 1,
                    calls: calls.clone(),
                    acted: Arc::clone(&acted),
                },
            )?
            .build()
            .await?;

        connection
            .object_server()
            .at(
                ITEM_PATH,
                Control {
                    bus_name: bus_name.clone(),
                    stopped: Arc::clone(&stopped),
                },
            )
            .await?;

        Ok(Self {
            connection,
            bus_name,
            calls,
            acted,
            stopped,
        })
    }

    /// Announce the item to whatever watcher is on the bus.
    pub async fn register(&self) -> zbus::Result<()> {
        imp::register(&self.connection, &self.bus_name).await
    }

    /// The well-known name this application took.
    pub fn bus_name(&self) -> &str {
        &self.bus_name
    }

    /// The identifier the panel knows this item by.
    ///
    /// The panel records an item under the *unique* name of the connection
    /// that registered it, which is why this asks the bus rather than guessing.
    pub async fn item_id(&self) -> String {
        let unique = self
            .connection
            .unique_name()
            .map(ToString::to_string)
            .unwrap_or_else(|| self.bus_name.clone());
        format!("{unique}{ITEM_PATH}")
    }

    /// The connection it is serving on, for a test that wants to poke it.
    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// Everything the panel has called on it, in order.
    pub fn calls(&self) -> Vec<String> {
        self.calls.seen()
    }

    /// Wait until the panel calls something, then report everything so far.
    pub async fn acted(&self) -> Vec<String> {
        loop {
            let waiter = self.acted.notified();
            let seen = self.calls.seen();
            if !seen.is_empty() {
                return seen;
            }
            waiter.await;
        }
    }

    /// Wait until `Quit` is called on the control interface.
    pub async fn stopped(&self) {
        self.stopped.notified().await;
    }

    /// Leave the bus, as if the application had been killed.
    pub async fn quit(self) {
        let _ = self.connection.release_name(self.bus_name.as_str()).await;
        drop(self.connection);
    }
}

#[cfg(test)]
mod tests {
    use super::imp::solid;
    use super::*;
    use crate::tray::menu::{MenuKind, MenuNode, ToggleKind};
    use crate::tray::proxy::RawNode;

    /// Parse a layout the fake would serve, the way the panel does.
    fn parsed(json: &str) -> MenuNode {
        let rows = parse_menu(json).expect("the fixture menu parses");
        let root = layout(&rows);
        MenuNode::parse(&RawNode {
            id: root.id,
            properties: root.properties,
            children: root.children,
        })
    }

    #[test]
    fn the_default_menu_round_trips_through_the_panels_own_parser() {
        let menu = parsed(DEFAULT_MENU);
        let rows: Vec<&str> = menu.rows().map(|row| row.label.as_str()).collect();
        assert_eq!(
            rows,
            [
                "Open Window",
                "Show _Notifications",
                "",
                "Online",
                "Away",
                "",
                "Not Available",
                "Preferences",
                "More",
                "Quit",
            ],
            "mnemonics are stripped and the hidden row is not offered"
        );

        assert_eq!(menu.children[1].toggle, ToggleKind::Checkmark);
        assert!(menu.children[1].toggle_state.is_on());
        assert_eq!(menu.children[3].toggle, ToggleKind::Radio);
        assert!(menu.children[3].toggle_state.is_on());
        assert!(!menu.children[4].toggle_state.is_on());
        assert_eq!(menu.children[2].kind, MenuKind::Separator);
        assert!(!menu.children[6].enabled);
        assert!(!menu.children[7].visible);
        assert_eq!(
            menu.children[8].icon_name.as_deref(),
            Some("preferences-system-symbolic")
        );

        let more = &menu.children[9];
        assert!(more.has_submenu);
        assert_eq!(more.children.len(), 2);
        assert_eq!(more.children[0].label, "About");
    }

    #[test]
    fn menu_ids_are_unique_across_the_whole_tree() {
        let menu = parsed(DEFAULT_MENU);
        let mut ids = Vec::new();
        fn walk(node: &MenuNode, ids: &mut Vec<i32>) {
            ids.push(node.id);
            for child in &node.children {
                walk(child, ids);
            }
        }
        walk(&menu, &mut ids);

        let unique: std::collections::BTreeSet<i32> = ids.iter().copied().collect();
        assert_eq!(unique.len(), ids.len(), "a row id must address one row");
    }

    #[test]
    fn a_bad_menu_description_is_refused_with_a_reason() {
        assert!(parse_menu("not json").is_err());
        assert!(
            parse_menu(r#"[{"labell": "typo"}]"#).is_err(),
            "an unknown field is a mistake worth reporting"
        );
        assert!(
            parse_menu("[]")
                .expect("an empty menu is a menu")
                .is_empty()
        );
    }

    #[test]
    fn a_solid_pixmap_is_the_size_it_says_it_is() {
        let bytes = solid(4, 2, 0xff_11_22_33);
        assert_eq!(bytes.len(), 4 * 2 * 4);
        assert_eq!(&bytes[..4], &[0xff, 0x11, 0x22, 0x33]);
        assert_eq!(&bytes[4..8], &[0xff, 0x11, 0x22, 0x33]);
        assert!(solid(0, 8, 0).is_empty());
    }
}
