//! The tray service against real applications on a real bus — a **private**
//! one.
//!
//! The applications are [`fake::FakeSni`]s, which serve the same two
//! interfaces Syncthing does plus a control interface no real one has, so a
//! test can change an icon or start shouting for attention. Everything runs on
//! a `dbus-daemon` that exists for the length of one test: `cargo test` never
//! takes the tray away from the desktop the developer is looking at, and never
//! meets the watcher that desktop is already running.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use super::fake::{DEFAULT_MENU, FakeSni, Recipe};
use super::watcher::WATCHER_NAME;
use super::*;
use crate::private_bus::{PrivateBus, private_bus};

/// How long a test waits for the panel to catch up before failing.
const PATIENCE: Duration = Duration::from_secs(10);
/// The size the tests ask for pixmaps at.
const TARGET: i32 = 18;

/// The fake application's control interface, as a test drives it.
#[zbus::proxy(
    interface = "io.github.trevarj.topbar.FakeSni1",
    default_path = "/StatusNotifierItem",
    assume_defaults = false
)]
trait FakeControl {
    fn set_status(&self, status: &str) -> zbus::Result<()>;
    fn set_icon_name(&self, name: &str) -> zbus::Result<()>;
    fn set_icon_pixmap(&self, width: i32, height: i32, argb: u32) -> zbus::Result<()>;
    fn set_tool_tip(&self, title: &str, body: &str) -> zbus::Result<()>;
    fn set_menu(&self, json: &str) -> zbus::Result<()>;
    fn trigger_new_icon(&self) -> zbus::Result<()>;
    fn reregister(&self) -> zbus::Result<()>;
}

/// A control proxy for one fake application.
async fn control(bus: &PrivateBus, item: &FakeSni) -> FakeControlProxy<'static> {
    FakeControlProxy::builder(&bus.connect().await)
        .destination(item.bus_name().to_string())
        .expect("a well-formed bus name")
        .path(super::fake::ITEM_PATH)
        .expect("a well-formed path")
        .build()
        .await
        .expect("the fake application's control interface")
}

/// Wait until a published snapshot satisfies `predicate`.
async fn wait_for(
    state: &mut watch::Receiver<Arc<TrayState>>,
    what: &str,
    predicate: impl Fn(&TrayState) -> bool,
) -> Arc<TrayState> {
    let wait = async {
        loop {
            // Cloned out before testing: holding a read guard across an await
            // deadlocks against the task trying to publish the next one.
            let snapshot = state.borrow_and_update().clone();
            if predicate(&snapshot) {
                return snapshot;
            }
            state.changed().await.expect("the tray service is alive");
        }
    };
    tokio::time::timeout(PATIENCE, wait)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

/// The recipe every test starts from, named after the test.
fn recipe(name: &str) -> Recipe {
    Recipe {
        name: name.to_string(),
        id: format!("fake-{name}"),
        title: format!("Fake {name}"),
        ..Recipe::default()
    }
}

/// Put an application on the bus and announce it.
async fn start(bus: &PrivateBus, recipe: &Recipe) -> FakeSni {
    let item = FakeSni::start(recipe, Some(bus.address()))
        .await
        .expect("the fake application takes its name");
    item.register().await.expect("the watcher takes the item");
    item
}

/// Whether a name is on the bus at all.
async fn owned(bus: &PrivateBus, name: &str) -> bool {
    zbus::fdo::DBusProxy::new(&bus.connect().await)
        .await
        .expect("the bus daemon answers")
        .list_names()
        .await
        .expect("the bus lists its names")
        .iter()
        .any(|owned| owned.as_str() == name)
}

/// Wait for a name to appear on the bus.
///
/// The tray publishes its first, empty snapshot before it has finished taking
/// its names, so "the tray is running" is not yet "the tray is the watcher".
async fn wait_owned(bus: &PrivateBus, name: &str) -> bool {
    tokio::time::timeout(PATIENCE, async {
        while !owned(bus, name).await {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .is_ok()
}

#[tokio::test]
async fn the_panel_becomes_the_watcher_on_a_bus_that_has_none() {
    let bus = private_bus!();
    assert!(
        !owned(&bus, WATCHER_NAME).await,
        "a fresh bus has no watcher"
    );

    let tray = Tray::start(TARGET, Some(bus.address().to_string()));
    let mut state = tray.state();
    wait_for(&mut state, "an empty tray", TrayState::is_empty).await;

    assert!(
        wait_owned(&bus, WATCHER_NAME).await,
        "the panel serves the watcher itself when nothing else does"
    );

    // And it registers itself as a host, which is what an application checks
    // before deciding a tray is worth talking to.
    let host = format!("org.kde.StatusNotifierHost-{}", std::process::id());
    assert!(wait_owned(&bus, &host).await, "the panel took a host name");

    let watcher = super::proxy::StatusNotifierWatcherProxy::new(&bus.connect().await)
        .await
        .expect("the watcher interface is reachable");
    assert!(
        watcher
            .registered_status_notifier_items()
            .await
            .expect("the watcher answers")
            .is_empty()
    );
}

#[tokio::test]
async fn an_application_that_registers_appears_on_the_bar() {
    let bus = private_bus!();
    let tray = Tray::start(TARGET, Some(bus.address().to_string()));
    let mut state = tray.state();
    wait_for(&mut state, "an empty tray", TrayState::is_empty).await;

    let mut wanted = recipe("appearing");
    wanted.icon_name = Some("folder-remote-symbolic".to_string());
    wanted.theme_path = Some("/opt/fake/icons".to_string());
    wanted.tooltip_title = "Fake appearing".to_string();
    wanted.tooltip_body = "Up to date".to_string();
    wanted.menu = Some(DEFAULT_MENU.to_string());
    let item = start(&bus, &wanted).await;

    let settled = wait_for(&mut state, "the item", |state| !state.is_empty()).await;
    assert_eq!(settled.items.len(), 1);

    let view = &settled.items[0];
    assert_eq!(view.id, item.item_id().await);
    assert_eq!(view.title, "Fake appearing");
    assert_eq!(view.status, Status::Active);
    assert_eq!(view.tooltip.as_deref(), Some("Fake appearing\nUp to date"));
    assert_eq!(
        view.icon,
        IconView::Themed {
            name: "folder-remote-symbolic".into(),
            theme_path: Some("/opt/fake/icons".into()),
        }
    );
    assert!(view.has_menu);
    assert!(!view.item_is_menu);
}

#[tokio::test]
async fn an_application_already_registered_is_found_at_startup() {
    let bus = private_bus!();
    // No watcher yet, so the fake takes the name itself to have somewhere to
    // register — which is what a desktop environment's own watcher does.
    let held = bus.connect().await;
    let flags = [zbus::fdo::RequestNameFlags::DoNotQueue]
        .into_iter()
        .collect();
    held.request_name_with_flags(WATCHER_NAME, flags)
        .await
        .expect("the stand-in watcher takes the name");
    held.object_server()
        .at(
            super::watcher::WATCHER_PATH,
            super::watcher::Watcher::new(
                super::watcher::Registry::default(),
                tokio::sync::mpsc::channel(4).0,
            ),
        )
        .await
        .expect("the stand-in watcher serves its interface");

    let item = start(&bus, &recipe("early")).await;

    let tray = Tray::start(TARGET, Some(bus.address().to_string()));
    let mut state = tray.state();
    let settled = wait_for(&mut state, "the item that was already there", |state| {
        !state.is_empty()
    })
    .await;

    assert_eq!(settled.items.len(), 1);
    assert_eq!(settled.items[0].title, "Fake early");
    assert_eq!(
        settled.items[0].id,
        item.item_id().await,
        "an item registered with somebody else's watcher is addressed the same way"
    );
}

#[tokio::test]
async fn an_application_that_quits_is_taken_off_the_bar() {
    let bus = private_bus!();
    let tray = Tray::start(TARGET, Some(bus.address().to_string()));
    let mut state = tray.state();
    wait_for(&mut state, "an empty tray", TrayState::is_empty).await;

    let staying = start(&bus, &recipe("staying")).await;
    let leaving = start(&bus, &recipe("leaving")).await;
    wait_for(&mut state, "both items", |state| state.items.len() == 2).await;

    let leaving_id = leaving.item_id().await;
    leaving.quit().await;

    let settled = wait_for(&mut state, "the item to go", |state| state.items.len() == 1).await;
    assert_eq!(settled.items[0].id, staying.item_id().await);
    assert!(settled.item(&leaving_id).is_none());

    // Acting on an item that has gone is reported, not silently dropped.
    let error = tray
        .handle()
        .activate(&leaving_id)
        .await
        .expect_err("there is nothing left to activate");
    assert!(matches!(error, SvcError::NoTrayItem(_)), "{error:?}");
    assert_eq!(error.user_message(), "That tray icon is no longer there");
}

#[tokio::test]
async fn a_passive_item_is_not_drawn_and_coming_back_puts_it_on_the_bar() {
    let bus = private_bus!();
    let mut quiet = recipe("quiet");
    quiet.status = "Passive".to_string();

    let tray = Tray::start(TARGET, Some(bus.address().to_string()));
    let mut state = tray.state();
    wait_for(&mut state, "an empty tray", TrayState::is_empty).await;

    let item = start(&bus, &quiet).await;
    let control = control(&bus, &item).await;

    // Nothing to draw. Waited on rather than asserted immediately, because
    // "still empty" and "not read yet" look the same on the first frame.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        state.borrow().is_empty(),
        "a Passive item is not worth drawing"
    );

    control
        .set_status("Active")
        .await
        .expect("the item wakes up");
    let settled = wait_for(&mut state, "the item to appear", |state| !state.is_empty()).await;
    assert_eq!(settled.items[0].status, Status::Active);

    control
        .set_status("Passive")
        .await
        .expect("the item goes quiet");
    wait_for(&mut state, "the item to go quiet", TrayState::is_empty).await;
}

#[tokio::test]
async fn a_new_icon_signal_reaches_the_panel() {
    let bus = private_bus!();
    let tray = Tray::start(TARGET, Some(bus.address().to_string()));
    let mut state = tray.state();
    wait_for(&mut state, "an empty tray", TrayState::is_empty).await;

    let item = start(&bus, &recipe("changing")).await;
    let control = control(&bus, &item).await;
    wait_for(&mut state, "the item", |state| !state.is_empty()).await;

    control
        .set_icon_name("weather-storm-symbolic")
        .await
        .expect("the item changes its icon");
    let settled = wait_for(&mut state, "the new icon", |state| {
        matches!(
            state.items.first().map(|item| &item.icon),
            Some(IconView::Themed { name, .. }) if name == "weather-storm-symbolic"
        )
    })
    .await;
    assert_eq!(settled.items.len(), 1);

    control
        .set_tool_tip("Storm", "Batten the hatches")
        .await
        .expect("the item changes its tooltip");
    wait_for(&mut state, "the new tooltip", |state| {
        state
            .items
            .first()
            .and_then(|item| item.tooltip.as_deref())
            .is_some_and(|tooltip| tooltip.contains("Batten"))
    })
    .await;
}

#[tokio::test]
async fn a_status_flip_to_needs_attention_reaches_the_panel() {
    let bus = private_bus!();
    let tray = Tray::start(TARGET, Some(bus.address().to_string()));
    let mut state = tray.state();
    wait_for(&mut state, "an empty tray", TrayState::is_empty).await;

    let item = start(&bus, &recipe("shouting")).await;
    let control = control(&bus, &item).await;
    let settled = wait_for(&mut state, "the item", |state| !state.is_empty()).await;
    assert_eq!(settled.items[0].status, Status::Active);

    control
        .set_status("NeedsAttention")
        .await
        .expect("the item starts shouting");
    let settled = wait_for(&mut state, "the flip", |state| {
        state.items.first().map(|item| item.status) == Some(Status::NeedsAttention)
    })
    .await;
    assert_eq!(settled.items.len(), 1, "it is still the same one item");
}

#[tokio::test]
async fn a_pixmap_keeps_the_shape_the_application_gave_it() {
    let bus = private_bus!();
    let mut drawn = recipe("pixels");
    drawn.icon_name = None;
    // Deliberately not square, and deliberately two sizes: the panel should
    // take the smallest that is big enough for the 18px it asked for.
    drawn.pixmaps = vec![(8, 4, 0xff_20_20_20), (24, 12, 0xff_35_84_e4)];

    let tray = Tray::start(TARGET, Some(bus.address().to_string()));
    let mut state = tray.state();
    wait_for(&mut state, "an empty tray", TrayState::is_empty).await;

    let _item = start(&bus, &drawn).await;
    let settled = wait_for(&mut state, "the item", |state| !state.is_empty()).await;

    let IconView::Pixels(pixmap) = &settled.items[0].icon else {
        panic!("an item with no icon name should be drawn from its pixels");
    };
    assert_eq!(
        (pixmap.width, pixmap.height),
        (24, 12),
        "the picture keeps both of its dimensions and the bigger one wins"
    );
    assert_eq!(pixmap.rgba.len(), 24 * 12 * 4);
    assert_eq!(
        &pixmap.rgba[..4],
        &[0x35, 0x84, 0xe4, 0xff],
        "ARGB from the bus arrives as RGBA for GTK"
    );
}

#[tokio::test]
async fn a_menu_comes_down_whole_and_about_to_show_is_asked_first() {
    let bus = private_bus!();
    let mut with_menu = recipe("menued");
    with_menu.menu = Some(DEFAULT_MENU.to_string());

    let tray = Tray::start(TARGET, Some(bus.address().to_string()));
    let mut state = tray.state();
    wait_for(&mut state, "an empty tray", TrayState::is_empty).await;

    let item = start(&bus, &with_menu).await;
    wait_for(&mut state, "the item", |state| !state.is_empty()).await;
    let id = item.item_id().await;

    let menu = tray.handle().menu(&id).await.expect("the menu arrives");
    let labels: Vec<&str> = menu.rows().map(|row| row.label.as_str()).collect();
    assert_eq!(
        labels,
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
        ]
    );
    assert_eq!(menu.children[1].toggle, ToggleKind::Checkmark);
    assert!(menu.children[1].toggle_state.is_on());
    assert_eq!(menu.children[2].kind, MenuKind::Separator);
    assert!(!menu.children[6].enabled);
    assert!(!menu.children[7].visible);
    assert!(menu.children[9].has_submenu);
    assert_eq!(menu.children[9].children[0].label, "About");

    assert!(
        item.calls().iter().any(|call| call == "AboutToShow(0)"),
        "the application is told the menu is opening before it is asked for it: {:?}",
        item.calls()
    );

    // A menu is fetched fresh every time, so an application that rebuilds it
    // between openings is drawn as it is now rather than as it was.
    let control = control(&bus, &item).await;
    control
        .set_menu(r#"[{"label": "Only This"}]"#)
        .await
        .expect("the application rebuilds its menu");
    let menu = tray.handle().menu(&id).await.expect("the new menu arrives");
    assert_eq!(
        menu.rows()
            .map(|row| row.label.as_str())
            .collect::<Vec<_>>(),
        ["Only This"]
    );
}

#[tokio::test]
async fn an_item_with_no_menu_says_so_rather_than_hanging() {
    let bus = private_bus!();
    let tray = Tray::start(TARGET, Some(bus.address().to_string()));
    let mut state = tray.state();
    wait_for(&mut state, "an empty tray", TrayState::is_empty).await;

    let item = start(&bus, &recipe("menuless")).await;
    let settled = wait_for(&mut state, "the item", |state| !state.is_empty()).await;
    assert!(!settled.items[0].has_menu);

    let error = tray
        .handle()
        .menu(&item.item_id().await)
        .await
        .expect_err("there is no menu to fetch");
    assert!(matches!(error, SvcError::NoTrayMenu(_)), "{error:?}");
    assert_eq!(error.user_message(), "That tray icon has no menu");
}

#[tokio::test]
async fn an_item_that_is_a_menu_with_no_menu_is_asked_to_show_its_own() {
    // Some applications set ItemIsMenu and publish no dbusmenu object at all.
    // The specification's answer is ContextMenu: hand the job back.
    let bus = private_bus!();
    let mut menuless = recipe("selfmenued");
    menuless.item_is_menu = true;
    menuless.menu = None;

    let tray = Tray::start(TARGET, Some(bus.address().to_string()));
    let mut state = tray.state();
    wait_for(&mut state, "an empty tray", TrayState::is_empty).await;

    let item = start(&bus, &menuless).await;
    let settled = wait_for(&mut state, "the item", |state| !state.is_empty()).await;
    assert!(settled.items[0].item_is_menu);
    assert!(!settled.items[0].has_menu);

    tray.handle()
        .context_menu(&item.item_id().await)
        .await
        .expect("the request is sent");

    let calls = tokio::time::timeout(PATIENCE, item.acted())
        .await
        .expect("ContextMenu arrives at the application");
    assert!(
        calls.iter().any(|call| call.starts_with("ContextMenu")),
        "{calls:?}"
    );
}

#[tokio::test]
async fn every_click_and_scroll_reaches_the_application() {
    let bus = private_bus!();
    let mut clicked = recipe("clicked");
    clicked.menu = Some(DEFAULT_MENU.to_string());

    let tray = Tray::start(TARGET, Some(bus.address().to_string()));
    let mut state = tray.state();
    wait_for(&mut state, "an empty tray", TrayState::is_empty).await;

    let item = start(&bus, &clicked).await;
    wait_for(&mut state, "the item", |state| !state.is_empty()).await;
    let id = item.item_id().await;
    let handle = tray.handle();

    handle.activate(&id).await.expect("the click is sent");
    handle
        .secondary_activate(&id)
        .await
        .expect("the middle click is sent");
    handle
        .scroll(&id, 120, ScrollAxis::Vertical)
        .await
        .expect("the scroll is sent");
    handle
        .scroll(&id, -1, ScrollAxis::Horizontal)
        .await
        .expect("a sideways scroll is sent");
    handle
        .menu_event(&id, 1, MenuEvent::Clicked)
        .await
        .expect("the menu event is sent");

    let calls = tokio::time::timeout(PATIENCE, async {
        loop {
            let calls = item.calls();
            if calls.len() >= 5 {
                return calls;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("only {:?} arrived", item.calls()));

    assert!(calls.contains(&"Activate(0,0)".to_string()), "{calls:?}");
    assert!(
        calls.contains(&"SecondaryActivate(0,0)".to_string()),
        "{calls:?}"
    );
    assert!(
        calls.contains(&"Scroll(120,vertical)".to_string()),
        "the scroll the `system-tray` crate had no way to send: {calls:?}"
    );
    assert!(
        calls.contains(&"Scroll(-1,horizontal)".to_string()),
        "{calls:?}"
    );
    assert!(calls.contains(&"Event(1,clicked)".to_string()), "{calls:?}");
}

#[tokio::test]
async fn an_item_served_at_its_own_path_is_still_reachable() {
    // Dropbox and the Ayatana indicators serve their items at a path of their
    // own rather than at /StatusNotifierItem. The identifier carries the path
    // precisely so a click still lands on the right object.
    let bus = private_bus!();
    let tray = Tray::start(TARGET, Some(bus.address().to_string()));
    let mut state = tray.state();
    wait_for(&mut state, "an empty tray", TrayState::is_empty).await;

    let item = start(&bus, &recipe("pathed")).await;
    let settled = wait_for(&mut state, "the item", |state| !state.is_empty()).await;

    let id = &settled.items[0].id;
    assert!(
        id.ends_with("/StatusNotifierItem"),
        "the identifier carries the object path: {id}"
    );
    assert!(
        id.starts_with(':'),
        "and the unique name of the connection that registered it: {id}"
    );
    assert_eq!(id, &item.item_id().await);
}

#[tokio::test]
async fn a_burst_of_re_registrations_does_not_rebuild_the_bar() {
    let bus = private_bus!();
    let tray = Tray::start(TARGET, Some(bus.address().to_string()));
    let mut state = tray.state();
    wait_for(&mut state, "an empty tray", TrayState::is_empty).await;

    let item = start(&bus, &recipe("flapping")).await;
    let control = control(&bus, &item).await;
    wait_for(&mut state, "the item", |state| !state.is_empty()).await;

    let before = state.borrow_and_update().clone();

    // What a chat client reconnecting does: announce itself over and over.
    for _ in 0..5 {
        control.reregister().await.expect("the watcher takes it");
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;

    assert!(
        !state.has_changed().expect("the tray service is alive"),
        "an item that is already on the bar must not be taken off and put back"
    );
    assert_eq!(**state.borrow(), *before);
    assert_eq!(state.borrow().items.len(), 1);
}

#[tokio::test]
async fn a_burst_of_changes_is_published_once_it_settles() {
    let bus = private_bus!();
    let tray = Tray::start(TARGET, Some(bus.address().to_string()));
    let mut state = tray.state();
    wait_for(&mut state, "an empty tray", TrayState::is_empty).await;

    let item = start(&bus, &recipe("flickering")).await;
    let control = control(&bus, &item).await;
    wait_for(&mut state, "the item", |state| !state.is_empty()).await;
    state.borrow_and_update();

    // Five icons in a fifth of a second, the way a spinner behaves.
    for name in ["one", "two", "three", "four", "five"] {
        control
            .set_icon_name(&format!("weather-{name}-symbolic"))
            .await
            .expect("the item changes its icon");
        tokio::time::sleep(Duration::from_millis(40)).await;
    }

    let settled = wait_for(&mut state, "the last icon", |state| {
        matches!(
            state.items.first().map(|item| &item.icon),
            Some(IconView::Themed { name, .. }) if name == "weather-five-symbolic"
        )
    })
    .await;
    assert_eq!(settled.items.len(), 1);

    // And the tray settles rather than churning on: nothing more is published
    // once the burst has stopped.
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        !state.has_changed().expect("the tray service is alive"),
        "a burst that has ended must stop republishing"
    );
}

#[tokio::test]
async fn several_items_keep_their_places() {
    let bus = private_bus!();
    let tray = Tray::start(TARGET, Some(bus.address().to_string()));
    let mut state = tray.state();
    wait_for(&mut state, "an empty tray", TrayState::is_empty).await;

    let mut items = Vec::new();
    for name in ["alpha", "beta", "gamma"] {
        items.push(start(&bus, &recipe(name)).await);
    }
    let settled = wait_for(&mut state, "all three items", |state| {
        state.items.len() == 3
    })
    .await;

    let order: Vec<&str> = settled.items.iter().map(|item| item.id.as_str()).collect();
    let mut sorted = order.clone();
    sorted.sort_unstable();
    assert_eq!(order, sorted, "items are published in identifier order");

    // A fourth arriving must not move the three that were there.
    let _extra = start(&bus, &recipe("delta")).await;
    let settled = wait_for(&mut state, "the fourth item", |state| {
        state.items.len() == 4
    })
    .await;
    let after: Vec<&str> = settled
        .items
        .iter()
        .map(|item| item.id.as_str())
        .filter(|id| order.contains(id))
        .collect();
    assert_eq!(after, order, "the icons that were there kept their places");
}

#[tokio::test]
async fn an_item_that_publishes_nothing_usable_gets_the_placeholder() {
    let bus = private_bus!();
    let mut bare = recipe("bare");
    bare.icon_name = None;
    bare.title = String::new();
    bare.tooltip_title = String::new();

    let tray = Tray::start(TARGET, Some(bus.address().to_string()));
    let mut state = tray.state();
    wait_for(&mut state, "an empty tray", TrayState::is_empty).await;

    let _item = start(&bus, &bare).await;
    let settled = wait_for(&mut state, "the item", |state| !state.is_empty()).await;

    let view = &settled.items[0];
    assert_eq!(view.icon, IconView::Fallback);
    assert_eq!(view.tooltip, None);
    assert_eq!(
        view.tooltip_text(),
        view.id,
        "an item with nothing to say is at least identified"
    );
}
