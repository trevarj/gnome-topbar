//! The system tray: one pill of application icons.
//!
//! ```text
//!  ▣ ▤ ▥ ▦ ⋯          left  → activate (or the menu, if the item says so)
//!                     middle→ secondary activate
//!                     right → the application's own menu
//!                     scroll→ the application's own scroll handler
//! ```
//!
//! The widget is invisible when there is nothing in it — not an empty pill, no
//! widget at all — because a tray with no icons has nothing to say. Past
//! `max_icons` the last place is given to a chevron whose popover holds the
//! rest, so a machine that has collected fourteen indicators does not push the
//! clock off centre.
//!
//! An item asking for attention is tinted with the panel's warning colour and
//! pulses twice before settling into it: twice is enough to catch the eye
//! moving past, and a loop would be a light that never stops blinking.

mod icon;
mod menu;
mod overflow;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use gtk4::prelude::*;
use gtk4::{Image, Orientation, Overlay};
use topbar_core::Config;
use topbar_core::config::TrayConfig;
use topbar_services::{ItemView, ScrollAxis, TrayState, TrayStatus};
use tracing::debug;

use crate::anim::{Animation, AnimationParams, Easing, motion_enabled};
use crate::bar::BarContext;
use crate::bridge::{self, ActionScope, BindingGuard};
use crate::style::classes;
use crate::surfaces::layer_popover::Anchored;
use crate::surfaces::popovers::{self, PopoverContent, PopoverHandle};
use crate::surfaces::tooltip::{self, TooltipHandle};
use crate::widgets::set_class;
use crate::widgets::shell::WidgetShell;

use self::icon::Contrast;
use self::menu::TrayMenu;
use self::overflow::Overflow;

/// Widget name, for CSS classes and the popover registry.
pub const WIDGET_NAME: &str = "tray";
/// Icon size when `widgets.tray.pixmap_icon_size` says nothing.
const DEFAULT_ICON_SIZE: i32 = topbar_services::tray::DEFAULT_ICON_SIZE;
/// The chevron that opens the overflow popover.
const OVERFLOW_ICON: &str = "view-more-symbolic";
/// How long the attention pulse runs, over both of its cycles.
const PULSE_MS: u64 = 1200;
/// How many cycles it runs for. Two: enough to notice, few enough to stop.
const PULSE_CYCLES: f64 = 2.0;
/// How far the tint dips at the bottom of a pulse.
const PULSE_DEPTH: f64 = 0.75;
/// One notch of a wheel, as applications expect to be told about it.
const SCROLL_NOTCH: i32 = 120;

/// The tray widget.
pub struct TrayWidget {
    shell: WidgetShell,
    _inner: Rc<Inner>,
    /// The overflow chevron's claim on the popover host.
    _overflow: PopoverHandle,
    _binding: BindingGuard,
}

impl TrayWidget {
    /// Build the widget from `[widgets.tray]`.
    pub fn new(config: &Config, context: &BarContext) -> Self {
        let settings = &config.widgets.tray;
        let shell = WidgetShell::new(classes::TRAY);

        let icons = gtk4::Box::new(Orientation::Horizontal, 0);
        icons.add_css_class(classes::TRAY_ICONS);
        shell.content().append(&icons);

        let chevron = flat_button(classes::TRAY_OVERFLOW);
        let chevron_icon = Image::from_icon_name(OVERFLOW_ICON);
        chevron_icon.set_pixel_size(icon_size(settings));
        chevron.append(&chevron_icon);
        chevron.set_visible(false);
        shell.content().append(&chevron);

        let menu = TrayMenu::new(&context.services);
        menu::connect_back(&menu);

        let inner = Rc::new_cyclic(|me: &Weak<Inner>| Inner {
            root: shell.root().clone(),
            icons,
            chevron: chevron.clone(),
            buttons: RefCell::new(HashMap::new()),
            order: RefCell::new(Vec::new()),
            menu,
            overflow: Overflow::new(me.clone()),
            host: context.popovers(),
            services: context.services.clone(),
            contrast: Contrast::of(config),
            icon_size: icon_size(settings),
            max_icons: settings.max_icons as usize,
            me: me.clone(),
        });

        // The chevron's popover is a registered one, so `topbar popover show
        // tray` has something to address and the smoke run can photograph it.
        // Every item's menu is opened against the host directly instead: its
        // anchor is whichever icon was clicked, which a fixed registration
        // could not name.
        let overflow = {
            let content = Rc::clone(&inner.overflow);
            popovers::attach(context, WIDGET_NAME, &chevron, move || {
                Rc::clone(&content) as Rc<dyn PopoverContent>
            })
        };

        let binding = bridge::bind_state(shell.root(), context.services.tray.state(), {
            let inner = Rc::downgrade(&inner);
            move |_: &gtk4::Box, state: &TrayState| {
                if let Some(inner) = inner.upgrade() {
                    inner.render(state);
                }
            }
        });

        // There is no synthetic pointer in the dev shell, so these are the only
        // way an open tray menu is ever photographed.
        //
        // Each waits for the tray to have something in it first: the smoke hook
        // fires a second after start-up, and the applications it is meant to
        // photograph are started by the driver a second after *that*.
        popovers::register_smoke_action(&format!("{WIDGET_NAME}-menu"), {
            let inner = Rc::downgrade(&inner);
            move || when_ready(&inner, Inner::has_items, |inner| inner.open_first_menu())
        });
        popovers::register_smoke_action(&format!("{WIDGET_NAME}-submenu"), {
            let inner = Rc::downgrade(&inner);
            move || {
                when_ready(&inner, Inner::has_items, |inner| {
                    inner.open_first_menu();
                    inner.enter_last_submenu();
                })
            }
        });
        popovers::register_smoke_action(&format!("{WIDGET_NAME}-overflow"), {
            let inner = Rc::downgrade(&inner);
            // Waits for the chevron rather than for the first icon: the
            // applications a scenario is about arrive one at a time, and the
            // chevron only appears once enough of them have.
            move || when_ready(&inner, Inner::is_overflowing, Inner::open_overflow)
        });

        Self {
            shell,
            _inner: inner,
            _overflow: overflow,
            _binding: binding,
        }
    }

    /// The widget to put in a bar section.
    pub fn root(&self) -> gtk4::Widget {
        self.shell.root().clone().upcast()
    }
}

/// How often a waiting smoke action looks to see whether the tray has filled.
const SMOKE_POLL: std::time::Duration = std::time::Duration::from_millis(250);
/// How long it waits before giving up and saying so.
const SMOKE_PATIENCE: u32 = 60;

/// Run `action` as soon as `ready` says the tray is worth photographing.
///
/// The smoke hook fires on a timer, and the applications a tray scenario is
/// about are started by the driver script afterwards — one at a time, because
/// the driver waits for each to reach the bus. Waiting for the state rather
/// than for the clock is the same discipline `scripts/smoke-shot.sh` applies
/// to the screenshot itself.
fn when_ready(
    inner: &Weak<Inner>,
    ready: impl Fn(&Inner) -> bool + 'static,
    action: impl Fn(&Inner) + 'static,
) {
    let inner = inner.clone();
    let mut left = SMOKE_PATIENCE;
    gtk4::glib::timeout_add_local(SMOKE_POLL, move || {
        let Some(inner) = inner.upgrade() else {
            return gtk4::glib::ControlFlow::Break;
        };
        if ready(&inner) {
            action(&inner);
            return gtk4::glib::ControlFlow::Break;
        }
        left -= 1;
        if left == 0 {
            tracing::warn!("the tray never reached the state to be opened in");
            return gtk4::glib::ControlFlow::Break;
        }
        gtk4::glib::ControlFlow::Continue
    });
}

/// `widgets.tray.pixmap_icon_size`, or the panel's own symbolic size.
fn icon_size(config: &TrayConfig) -> i32 {
    config
        .pixmap_icon_size
        .map_or(DEFAULT_ICON_SIZE, |size| size as i32)
}

/// Everything the render closure touches.
pub struct Inner {
    /// The shell's outer box, hidden wholesale when the tray is empty.
    root: gtk4::Box,
    /// Where the item buttons live.
    icons: gtk4::Box,
    /// The overflow chevron.
    chevron: gtk4::Box,
    /// One button per item, by identifier.
    buttons: RefCell<HashMap<String, ItemButton>>,
    /// The identifiers currently on the bar, in the order they are drawn.
    order: RefCell<Vec<String>>,
    /// The one menu popover, pointed at whichever item was right-clicked.
    menu: Rc<TrayMenu>,
    /// The chevron's popover.
    overflow: Rc<Overflow>,
    host: Rc<crate::surfaces::layer_popover::LayerPopover>,
    services: topbar_services::Services,
    contrast: Contrast,
    icon_size: i32,
    max_icons: usize,
    me: Weak<Self>,
}

impl Inner {
    /// Draw a published tray.
    fn render(&self, state: &TrayState) {
        let (inline, overflowing) = overflow::split(state.items.len(), self.max_icons);

        self.sync(&state.items[..inline]);
        self.overflow.render(&state.items[inline..], self);
        self.chevron.set_visible(overflowing);

        // The whole widget goes, not just its contents: an empty pill on the
        // bar would be a button that does nothing.
        set_visible(&self.root, !state.items.is_empty());

        // A menu whose item has left the bus has nothing behind it any more.
        if let Some(open) = self.menu.showing()
            && state.item(&open).is_none()
        {
            self.menu.forget();
            if self.host.is_open(&menu_name(&open)) {
                self.host.close();
            }
        }
    }

    /// Bring the row of buttons in line with `items`.
    fn sync(&self, items: &[ItemView]) {
        let wanted: Vec<String> = items.iter().map(|item| item.id.clone()).collect();

        {
            let mut buttons = self.buttons.borrow_mut();
            buttons.retain(|id, button| {
                let keep = wanted.contains(id);
                if !keep {
                    self.icons.remove(&button.root);
                }
                keep
            });

            for item in items {
                let button = buttons
                    .entry(item.id.clone())
                    .or_insert_with(|| ItemButton::new(&item.id, self));
                button.update(item, self.icon_size, self.contrast);
            }
        }

        if *self.order.borrow() == wanted {
            return;
        }
        // The order changed, so the row is rebuilt. Logged because "the tray
        // rebuilt once" is exactly what the smoke run asserts about a burst of
        // re-registrations.
        debug!("tray: rebuilding {} icon(s)", wanted.len());

        while let Some(child) = self.icons.first_child() {
            self.icons.remove(&child);
        }
        let buttons = self.buttons.borrow();
        for id in &wanted {
            if let Some(button) = buttons.get(id) {
                self.icons.append(&button.root);
            }
        }
        *self.order.borrow_mut() = wanted;
    }

    /// A left click: activate, unless the item would rather show its menu.
    pub fn activate(&self, id: &str, anchor: &gtk4::Widget) {
        let item_is_menu = self
            .services
            .tray
            .state()
            .borrow()
            .item(id)
            .is_some_and(|item| item.item_is_menu);
        if item_is_menu {
            self.open_menu(id, anchor);
            return;
        }
        let handle = self.services.tray.handle().clone();
        let id = id.to_string();
        bridge::act(scope(), async move { handle.activate(&id).await });
    }

    /// A middle click.
    pub fn secondary_activate(&self, id: &str) {
        let handle = self.services.tray.handle().clone();
        let id = id.to_string();
        bridge::act(scope(), async move { handle.secondary_activate(&id).await });
    }

    /// A scroll over the icon.
    pub fn scroll(&self, id: &str, delta: i32, axis: ScrollAxis) {
        let handle = self.services.tray.handle().clone();
        let id = id.to_string();
        bridge::act(
            scope(),
            async move { handle.scroll(&id, delta, axis).await },
        );
    }

    /// A right click: the application's own menu, under the icon.
    pub fn open_menu(&self, id: &str, anchor: &gtk4::Widget) {
        // An item may say it is a menu and then publish nothing to draw. The
        // specification has an answer for that — hand the job back — and it is
        // a better one than a toast saying the panel found no menu.
        let has_menu = self
            .services
            .tray
            .state()
            .borrow()
            .item(id)
            .is_some_and(|item| item.has_menu);
        if !has_menu {
            let handle = self.services.tray.handle().clone();
            let id = id.to_string();
            bridge::act(scope(), async move { handle.context_menu(&id).await });
            return;
        }

        let name = menu_name(id);
        if self.host.is_open(&name) {
            self.host.close();
            return;
        }

        // Aimed before the open, so the surface is measured and placed around
        // rows it already has rather than growing under the pointer.
        self.menu.aim_at(id);

        let menu = Rc::clone(&self.menu);
        let closing = Rc::clone(&self.menu);
        self.host.open(Anchored {
            name,
            content: menu.root(),
            anchor: anchor.clone(),
            refresh: Rc::new(move || menu.refresh()),
            closed: Rc::new(move || closing.closed()),
        });
    }

    /// Open the first item's menu. The smoke hook's way in.
    fn open_first_menu(&self) {
        let order = self.order.borrow().clone();
        let Some(id) = order.first() else {
            return;
        };
        let anchor = self
            .buttons
            .borrow()
            .get(id)
            .map(|button| button.root.clone().upcast::<gtk4::Widget>());
        if let Some(anchor) = anchor {
            self.open_menu(id, &anchor);
        }
    }

    /// Whether there is anything on the bar at all.
    fn has_items(&self) -> bool {
        !self.order.borrow().is_empty()
    }

    /// Whether more icons arrived than there was room for.
    fn is_overflowing(&self) -> bool {
        self.chevron.is_visible()
    }

    /// Open the overflow popover. The smoke hook's way in, again.
    fn open_overflow(&self) {
        popovers::dispatch(
            &topbar_core::ipc::PopoverAction::Show(WIDGET_NAME.to_string()),
            None,
        );
    }

    /// Walk into the last submenu on screen. The smoke hook's way in, again.
    fn enter_last_submenu(&self) {
        let menu = Rc::clone(&self.menu);
        // After the fetch the open kicked off, which is a round trip away.
        gtk4::glib::timeout_add_local_once(std::time::Duration::from_millis(600), move || {
            menu.enter_last_submenu();
        });
    }
}

/// Where a failed tray action is reported.
fn scope() -> ActionScope {
    ActionScope::Toast {
        widget: WIDGET_NAME,
    }
}

/// The popover-host name one item's menu is opened under.
///
/// Per item rather than per widget: the host swaps content only when the name
/// changes, and right-clicking a second icon has to move the surface under it.
fn menu_name(id: &str) -> String {
    format!("{WIDGET_NAME}:{id}")
}

/// One tray icon on the bar.
struct ItemButton {
    root: gtk4::Box,
    image: Image,
    /// The warning tint behind an item that wants attention.
    tint: gtk4::Box,
    tooltip: TooltipHandle,
    /// The pulse, so it can be restarted or cancelled.
    pulse: Animation,
    /// What was last drawn, so an unchanged item costs nothing.
    drawn: RefCell<Option<ItemView>>,
    /// Whether the item was already shouting last time it was drawn.
    shouting: Cell<bool>,
}

impl ItemButton {
    /// Build a button for `id` and wire its four interactions.
    fn new(id: &str, inner: &Inner) -> Self {
        let root = flat_button(classes::TRAY_ITEM);

        let tint = gtk4::Box::new(Orientation::Horizontal, 0);
        tint.add_css_class(classes::TRAY_ITEM_TINT);
        tint.set_opacity(0.0);

        let image = Image::new();
        image.add_css_class(classes::TRAY_ITEM_ICON);

        let overlay = Overlay::new();
        overlay.set_child(Some(&tint));
        overlay.add_overlay(&image);
        overlay.set_measure_overlay(&image, true);
        root.append(&overlay);

        let tooltip = tooltip::attach(&root, "");
        let pulse = Animation::new(&tint);

        for (button, action) in [
            (gtk4::gdk::BUTTON_PRIMARY, Action::Activate),
            (gtk4::gdk::BUTTON_MIDDLE, Action::Secondary),
            (gtk4::gdk::BUTTON_SECONDARY, Action::Menu),
        ] {
            let click = gtk4::GestureClick::new();
            click.set_button(button);
            click.connect_released({
                let inner = inner.me.clone();
                let id = id.to_string();
                move |gesture, _, _, _| {
                    let (Some(inner), Some(anchor)) = (inner.upgrade(), gesture.widget()) else {
                        return;
                    };
                    match action {
                        Action::Activate => inner.activate(&id, &anchor),
                        Action::Secondary => inner.secondary_activate(&id),
                        Action::Menu => inner.open_menu(&id, &anchor),
                    }
                }
            });
            root.add_controller(click);
        }

        let scroll = gtk4::EventControllerScroll::new(
            gtk4::EventControllerScrollFlags::BOTH_AXES
                | gtk4::EventControllerScrollFlags::DISCRETE,
        );
        scroll.connect_scroll({
            let inner = inner.me.clone();
            let id = id.to_string();
            move |_, x, y| {
                if let Some(inner) = inner.upgrade() {
                    for (delta, axis) in [(y, ScrollAxis::Vertical), (x, ScrollAxis::Horizontal)] {
                        let notches = (delta * f64::from(SCROLL_NOTCH)).round() as i32;
                        if notches != 0 {
                            inner.scroll(&id, notches, axis);
                        }
                    }
                }
                gtk4::glib::Propagation::Stop
            }
        });
        root.add_controller(scroll);

        Self {
            root,
            image,
            tint,
            tooltip,
            pulse,
            drawn: RefCell::new(None),
            shouting: Cell::new(false),
        }
    }

    /// Draw `item`, doing nothing at all if it has not moved.
    fn update(&self, item: &ItemView, size: i32, contrast: Contrast) {
        if self.drawn.borrow().as_ref() == Some(item) {
            return;
        }

        let pixels = icon::apply(&self.image, &item.icon, size, contrast);
        self.tooltip.set_text(item.tooltip_text());

        let shouting = item.status == TrayStatus::NeedsAttention;
        // A themed icon takes its colour from CSS and can be tinted; a picture
        // cannot be recoloured without ruining whatever it is a picture of.
        set_class(
            &self.image,
            classes::TRAY_ITEM_ATTENTION,
            shouting && !pixels,
        );
        set_class(&self.root, classes::TRAY_ITEM_SHOUTING, shouting);

        if shouting && !self.shouting.replace(true) {
            self.start_pulse();
        } else if !shouting && self.shouting.replace(false) {
            self.pulse.cancel();
            self.tint.set_opacity(0.0);
        }

        *self.drawn.borrow_mut() = Some(item.clone());
    }

    /// Pulse the tint twice, then leave it on.
    fn start_pulse(&self) {
        if !motion_enabled() {
            self.tint.set_opacity(1.0);
            return;
        }
        let tint = self.tint.clone();
        self.pulse.start(
            AnimationParams::new(PULSE_MS).with_easing(Easing::Linear),
            Box::new(move |progress| tint.set_opacity(pulse(progress))),
            Some(Box::new({
                let tint = self.tint.clone();
                move || tint.set_opacity(1.0)
            })),
        );
    }
}

/// Which of the three buttons was pressed.
#[derive(Debug, Clone, Copy)]
enum Action {
    Activate,
    Secondary,
    Menu,
}

/// The tint's opacity part-way through the attention pulse.
///
/// Full at both ends and dipping twice in between, so the item is *already*
/// tinted on the first frame and stays tinted on the last: the pulse draws the
/// eye to a state that is there either way rather than being the state itself.
fn pulse(progress: f64) -> f64 {
    let phase = progress.clamp(0.0, 1.0) * PULSE_CYCLES * std::f64::consts::TAU;
    1.0 - PULSE_DEPTH * (0.5 - 0.5 * phase.cos())
}

/// A borderless, pointer-cursored box that behaves like a button.
///
/// A `Box` rather than a `Button`: GTK's own button claims the pointer
/// sequence for its internal gesture, and a tray icon needs three separate
/// button gestures and a scroll controller of its own.
fn flat_button(class: &str) -> gtk4::Box {
    let button = gtk4::Box::new(Orientation::Horizontal, 0);
    button.add_css_class(class);
    button.set_cursor_from_name(Some("pointer"));
    button
}

/// Show or hide, only when it is a change.
fn set_visible(widget: &impl IsA<gtk4::Widget>, visible: bool) {
    let widget = widget.as_ref();
    if widget.is_visible() != visible {
        widget.set_visible(visible);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pulse_starts_and_ends_on_a_fully_tinted_icon() {
        assert!((pulse(0.0) - 1.0).abs() < 1e-9);
        assert!((pulse(1.0) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn the_pulse_dips_twice_and_no_more() {
        // Sampled finely enough to catch a third dip if there were one.
        let samples: Vec<f64> = (0..=400)
            .map(|step| pulse(f64::from(step) / 400.0))
            .collect();
        let dips = samples
            .windows(3)
            .filter(|window| window[1] < window[0] && window[1] <= window[2])
            .count();
        assert_eq!(dips, PULSE_CYCLES as usize, "two cycles, then steady");
    }

    #[test]
    fn the_pulse_never_makes_the_tint_disappear_entirely() {
        for step in 0..=100 {
            let opacity = pulse(f64::from(step) / 100.0);
            assert!(
                (1.0 - PULSE_DEPTH..=1.0).contains(&opacity),
                "opacity {opacity} at {step}%"
            );
        }
    }

    #[test]
    fn out_of_range_progress_is_clamped_rather_than_overshooting() {
        assert!((pulse(-0.5) - 1.0).abs() < 1e-9);
        assert!((pulse(1.5) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn a_menu_is_named_after_the_item_it_belongs_to() {
        assert_eq!(
            menu_name(":1.42/StatusNotifierItem"),
            "tray::1.42/StatusNotifierItem"
        );
        assert_ne!(menu_name(":1.1/x"), menu_name(":1.2/x"));
    }

    #[test]
    fn the_icon_size_follows_the_configuration() {
        let mut config = TrayConfig::default();
        assert_eq!(icon_size(&config), DEFAULT_ICON_SIZE);
        config.pixmap_icon_size = Some(24);
        assert_eq!(icon_size(&config), 24);
    }
}
