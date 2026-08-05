//! The Quick Settings panel: GNOME 45's aggregate menu, top to bottom.
//!
//! ```text
//! ┌─ 360px ───────────────────────────────┐
//! │ [🔋 62%]                    (🔒) (⏻) │  header
//! │ ┄┄┄┄┄ battery health / power ┄┄┄┄┄┄┄┄ │  one expandable open at a time
//! │ 🔊 ──────●────────────────────      ⌄ │  sliders
//! │ 🎤 ────●──────────────────────        │  only while something records
//! │ ☀  ──────────●────────────────        │
//! │ ┌───────────────┬───────────────────┐ │
//! │ │ 📶 Usadba  ⌄ │ 🔒 VPN         ⌄ │ │  toggle grid
//! │ │ ☕ Caffeine   │ ⚡ Balanced    ⌄ │ │
//! │ └───────────────┴───────────────────┘ │
//! │ 🖧  Wired · 1 Gb/s                     │  only while a cable is doing
//! └───────────────────────────────────────┘     something
//! ```
//!
//! It is built on the shared popover host rather than on a layer window of its
//! own. That is what gives it, for free, the things every other menu in the
//! panel already has: exactly one open at a time, a click-catcher, Escape to
//! dismiss, keyboard focus taken only while it is up and handed back the
//! moment it starts closing, and content built once and retained rather than
//! rebuilt on every open.
//!
//! The height is bounded by a scroller, not by the surface: the panel grows
//! with its content until it would run past the work area, and after that the
//! content scrolls inside it. Expanding a card therefore changes the surface
//! height once — see [`super::expander`] for why once matters.

use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Orientation, PolicyType, ScrolledWindow};
use topbar_core::config::QuickSettingsConfig;
use topbar_services::{NetworkState, Services};

use crate::bridge::{self, BindingGuard};
use crate::style::classes;
use crate::surfaces::popovers::PopoverContent;
use crate::widgets::quick_settings::cards::{
    battery::BatteryCard, header::Header, network::WiredRow, power::PowerSection, sliders::Sliders,
    toggles::Toggles,
};
use crate::widgets::quick_settings::expander::{Accordion, Section};

/// The panel's width, from the UX spec. GNOME's own is 360 too.
pub const WIDTH: i32 = 360;
/// How much of the monitor the panel may take before its content scrolls.
///
/// The bar is already excluded by the compositor — the popover host asks for
/// an exclusive zone of zero — so this is a margin against the bottom of the
/// screen rather than against the bar.
const BOTTOM_MARGIN: i32 = 48;

/// One of the panel's expandable blocks, for the smoke hook.
#[cfg(debug_assertions)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Block {
    /// The battery-health card, under the header's pill.
    BatteryHealth,
    /// The power section, under the header's power button.
    Power,
    /// The Power Mode toggle's radio rows.
    PowerMode,
    /// The Wi-Fi toggle's network list.
    WiFi,
    /// The Bluetooth toggle's device list.
    Bluetooth,
    /// The VPN toggle's profile list.
    Vpn,
}

/// The Quick Settings panel.
pub struct Panel {
    root: gtk4::Box,
    scroll: ScrolledWindow,
    accordion: Rc<Accordion>,
    header: Rc<Header>,
    battery: Rc<BatteryCard>,
    /// The two blocks the header opens. Kept only in a debug build: the
    /// accordion owns them, and nothing but the smoke hook opens one without
    /// a pointer.
    #[cfg(debug_assertions)]
    battery_section: Rc<Section>,
    power: Rc<PowerSection>,
    #[cfg(debug_assertions)]
    power_section: Rc<Section>,
    sliders: Rc<Sliders>,
    toggles: Rc<Toggles>,
    /// Held so the binding that draws it has something to upgrade to.
    _wired: Rc<WiredRow>,
    /// The monitor it is drawn on, for the height bound.
    monitor: gtk4::gdk::Monitor,
    bindings: std::cell::RefCell<Vec<BindingGuard>>,
}

impl Panel {
    /// Build the panel for one bar.
    pub fn new(
        services: &Services,
        config: &QuickSettingsConfig,
        monitor: &gtk4::gdk::Monitor,
    ) -> Rc<Self> {
        let root = gtk4::Box::new(Orientation::Vertical, 0);
        root.add_css_class(classes::QS_PANEL);
        root.set_size_request(WIDTH, -1);

        let content = gtk4::Box::new(Orientation::Vertical, 0);
        content.add_css_class(classes::QS_CONTENT);

        let accordion = Accordion::new();

        let battery = BatteryCard::new(services);
        let battery_section = Section::new(battery.root());
        accordion.add(&battery_section);

        let power = PowerSection::new(services);
        let power_section = Section::new(power.root());
        accordion.add(&power_section);

        let header = Header::new(
            services,
            &accordion,
            &battery_section,
            &power_section,
            config.on_click_right.clone(),
            config.battery,
        );
        content.append(header.root());
        if config.battery_health {
            content.append(battery_section.root());
        }
        if config.power {
            content.append(power_section.root());
        }

        let sliders = Sliders::new(services, config.audio, config.mic, config.brightness);
        content.append(sliders.root());

        let toggles = Toggles::new(
            services,
            &accordion,
            config.idle_inhibitor,
            config.network,
            config.bluetooth,
            config.vpn,
            config.vpn_close_on_connect,
        );
        content.append(toggles.root());

        // Under the grid rather than inside it: a cable is a statement, not a
        // control, and a non-interactive pill sitting among four that respond
        // to a click would read as one that had stopped working. v1 put the
        // same information in a section header inside its network card.
        let wired = WiredRow::new();
        if config.network {
            content.append(wired.root());
        }

        // Vertical only: the panel is a fixed width and a horizontal scrollbar
        // would mean something in it had overflowed, which is a layout bug
        // rather than something to offer the user a way around.
        let scroll = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .vscrollbar_policy(PolicyType::Automatic)
            .propagate_natural_height(true)
            .child(&content)
            .build();
        scroll.add_css_class(classes::QS_SCROLL);
        root.append(&scroll);

        let panel = Rc::new(Self {
            root,
            scroll,
            accordion,
            header,
            battery,
            #[cfg(debug_assertions)]
            battery_section,
            power,
            #[cfg(debug_assertions)]
            power_section,
            sliders,
            toggles,
            _wired: Rc::clone(&wired),
            monitor: monitor.clone(),
            bindings: std::cell::RefCell::new(Vec::new()),
        });

        let binding = bridge::bind_state(&panel.root, services.network.state(), {
            let wired = Rc::downgrade(&wired);
            move |_: &gtk4::Box, state: &NetworkState| {
                if let Some(wired) = wired.upgrade() {
                    wired.render(state);
                }
            }
        });
        panel.bindings.borrow_mut().push(binding);

        panel
    }

    /// Open one of the panel's expandable blocks, without a pointer.
    ///
    /// The nested-niri smoke session has no synthetic input, so this is the
    /// only way an *expanded* card reaches a screenshot. Debug builds only.
    #[cfg(debug_assertions)]
    pub fn expand(&self, block: Block) {
        match block {
            Block::BatteryHealth => self.accordion.toggle(&self.battery_section),
            Block::Power => self.accordion.toggle(&self.power_section),
            Block::PowerMode => self.toggles.expand_power_mode(),
            Block::Bluetooth => self.toggles.expand_bluetooth(),
            Block::WiFi => self.toggles.expand_wifi(),
            Block::Vpn => self.toggles.expand_vpn(),
        }
    }

    /// The power section, for the smoke hook that paints a held row.
    #[cfg(debug_assertions)]
    pub fn power(&self) -> &Rc<PowerSection> {
        &self.power
    }

    /// Bound the panel's height to the monitor it is on.
    ///
    /// Done on every open rather than once: a monitor can change resolution,
    /// and a panel that remembered a 4K height on a 1080p screen would hang off
    /// the bottom of it.
    fn clamp_height(&self) {
        let height = self.monitor.geometry().height();
        self.scroll
            .set_max_content_height((height - BOTTOM_MARGIN).max(240));
    }
}

impl PopoverContent for Panel {
    fn root(&self) -> gtk4::Widget {
        self.root.clone().upcast()
    }

    fn refresh(&self) {
        self.clamp_height();
        self.header.refresh();
        self.battery.refresh();
        self.sliders.refresh();
        self.toggles.refresh();
    }

    fn closed(&self) {
        // Everything the user opened goes back: a panel that reopened onto the
        // power section three days later would be a surprise, and a row still
        // counting down behind a closed popover would be worse than one.
        self.power.cancel_holds();
        self.sliders.collapse();
        self.accordion.collapse_all();
        self.toggles.sync_chevrons();
    }
}
