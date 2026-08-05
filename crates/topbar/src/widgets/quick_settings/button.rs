//! The Quick Settings button: a pill of status icons.
//!
//! ```text
//!  [ 📶 🔒 🔊 ᛒ 🔋 ● ● ]
//!    │  │  │  │  │  │ └ something is watching the screen
//!    │  │  │  │  │  └ something is listening
//!    │  │  │  │  └ the battery, when there is one
//!    │  │  │  └ bluetooth, when something is connected over it
//!    │  │  └ the output volume — always
//!    │  └ a VPN
//!    └ the network
//! ```
//!
//! The order is fixed and the icons come and go inside it, which is what stops
//! the pill's contents rearranging themselves under the pointer whenever
//! something plugs in. Every slot was written out in [`model::ORDER`] before
//! there was anything to put in half of them, so each milestone added an icon
//! rather than a layout.
//!
//! The two dots at the end are the privacy indicators, and they are the panel's
//! one sanctioned unbounded animation: something recording you or watching your
//! screen is worth a heartbeat rather than a static dot. Both breathe on the
//! same [`Animation`], because at most one thing is worth looking at and two
//! dots pulsing out of phase would read as a fault.

use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Image, Orientation};
use topbar_services::{AudioState, BatteryState, BtState, NetworkState, PrivacyState, Services};

use crate::anim::{Animation, AnimationParams, Easing, motion_enabled};
use crate::bridge::{self, BindingGuard};
use crate::style::{classes, icons};
use crate::widgets::quick_settings::model::{Indicator, Indicators};
use crate::widgets::quick_settings::set_icon;

/// One full cycle of the privacy dot's pulse.
///
/// The plan sanctions exactly one unbounded loop in the panel and this is it,
/// on the grounds that something recording you is worth a heartbeat rather
/// than a static dot. It stops the moment the dot is hidden.
const PULSE_MS: u64 = 2000;
/// How far down the pulse dims.
const PULSE_FLOOR: f64 = 0.45;

/// The row of status icons.
pub struct IndicatorRow {
    root: gtk4::Box,
    audio: Image,
    battery: Image,
    network: Image,
    bluetooth: Image,
    microphone: gtk4::Box,
    screen_share: gtk4::Box,
    /// Each drawn indicator beside the slot it occupies, so visibility is
    /// applied from [`Indicators::visible`] rather than icon by icon.
    slots: Vec<(Indicator, gtk4::Widget)>,
    pulse: Animation,
    /// Whether the pulse is running, so it is started once rather than per
    /// state change.
    pulsing: std::cell::Cell<bool>,
    services: Services,
    /// `[widgets.quick_settings] battery`.
    show_battery: bool,
    bindings: std::cell::RefCell<Vec<BindingGuard>>,
}

impl IndicatorRow {
    /// Build the icon row into `content`.
    pub fn new(content: &gtk4::Box, services: &Services, show_battery: bool) -> Rc<Self> {
        let root = gtk4::Box::new(Orientation::Horizontal, 0);
        root.add_css_class(classes::QS_INDICATOR);

        let audio = Image::new();
        audio.add_css_class(classes::QS_ICON);

        let battery = Image::new();
        battery.add_css_class(classes::QS_ICON);
        battery.set_visible(false);

        let network = Image::new();
        network.add_css_class(classes::QS_ICON);
        network.set_visible(false);

        // Tinted with the accent, because a tunnel is a *state the user chose*
        // rather than a reading, and it is the one icon on the pill that says
        // "your traffic is not going where it usually goes".
        let vpn = Image::from_icon_name(icons::VPN);
        vpn.add_css_class(classes::QS_ICON);
        vpn.add_css_class(classes::QS_ICON_ACCENT);
        vpn.set_visible(false);

        let bluetooth = Image::from_icon_name(icons::BLUETOOTH_ACTIVE);
        bluetooth.add_css_class(classes::QS_ICON);
        bluetooth.set_visible(false);

        // Dots rather than icons: GNOME draws dots, and at 18px on a black bar
        // an outlined microphone reads as a smudge.
        let microphone = privacy_dot();
        let screen_share = privacy_dot();

        // Appended in the published order.
        let mut slots: Vec<(Indicator, gtk4::Widget)> = Vec::new();
        for indicator in crate::widgets::quick_settings::model::ORDER {
            let widget: gtk4::Widget = match indicator {
                Indicator::Audio => audio.clone().upcast(),
                Indicator::Battery => battery.clone().upcast(),
                Indicator::Microphone => microphone.clone().upcast(),
                Indicator::Network => network.clone().upcast(),
                Indicator::Vpn => vpn.clone().upcast(),
                Indicator::Bluetooth => bluetooth.clone().upcast(),
                Indicator::ScreenShare => screen_share.clone().upcast(),
            };
            root.append(&widget);
            slots.push((*indicator, widget));
        }
        content.append(&root);

        let indicators = Rc::new(Self {
            pulse: Animation::new(&root),
            root,
            audio,
            battery,
            network,
            bluetooth,
            microphone,
            screen_share,
            slots,
            pulsing: std::cell::Cell::new(false),
            services: services.clone(),
            show_battery,
            bindings: std::cell::RefCell::new(Vec::new()),
        });

        let audio_binding = bridge::bind_state(&indicators.root, services.audio.state(), {
            let indicators = Rc::downgrade(&indicators);
            move |_: &gtk4::Box, state: &AudioState| {
                if let Some(indicators) = indicators.upgrade() {
                    indicators.render_audio(state);
                }
            }
        });
        let battery_binding = bridge::bind_state(&indicators.root, services.battery.state(), {
            let indicators = Rc::downgrade(&indicators);
            move |_: &gtk4::Box, state: &BatteryState| {
                if let Some(indicators) = indicators.upgrade() {
                    indicators.render_battery(state);
                }
            }
        });
        let network_binding = bridge::bind_state(&indicators.root, services.network.state(), {
            let indicators = Rc::downgrade(&indicators);
            move |_: &gtk4::Box, state: &NetworkState| {
                if let Some(indicators) = indicators.upgrade() {
                    indicators.render_network(state);
                }
            }
        });
        let bluetooth_binding = bridge::bind_state(&indicators.root, services.bluetooth.state(), {
            let indicators = Rc::downgrade(&indicators);
            move |_: &gtk4::Box, state: &BtState| {
                if let Some(indicators) = indicators.upgrade() {
                    indicators.render_bluetooth(state);
                }
            }
        });
        let privacy_binding = bridge::bind_state(&indicators.root, services.privacy.state(), {
            let indicators = Rc::downgrade(&indicators);
            move |_: &gtk4::Box, _: &PrivacyState| {
                if let Some(indicators) = indicators.upgrade() {
                    indicators.apply_visibility();
                }
            }
        });
        indicators.bindings.borrow_mut().extend([
            audio_binding,
            battery_binding,
            network_binding,
            bluetooth_binding,
            privacy_binding,
        ]);

        indicators
    }

    /// A one-line summary for the button's tooltip.
    pub fn tooltip(&self) -> String {
        let audio = self.services.audio.current();
        let battery = self.services.battery.current();
        let profiles = self.services.power_profiles.current();

        let mut lines = Vec::new();
        if audio.sink_muted {
            lines.push("Volume muted".to_string());
        } else {
            lines.push(format!("Volume {}%", audio.sink_volume_pct));
        }
        if battery.available
            && let Some(percent) = battery.rounded_percent()
        {
            lines.push(format!("Battery {percent}% · {}", battery.status.label()));
        }
        // The battery pill's tooltip names the active profile, per the plan:
        // it is the one place a user looks to ask "why is this slow today".
        if let Some(active) = &profiles.active {
            lines.push(format!("Power mode: {}", active.label));
        }
        if audio.source_in_use {
            lines.push("Microphone in use".to_string());
        }
        let bluetooth = self.services.bluetooth.current();
        if let Some(device) = bluetooth.first_connected() {
            lines.push(match device.battery_pct {
                Some(percent) => format!("Bluetooth: {} · {percent}%", device.alias),
                None => format!("Bluetooth: {}", device.alias),
            });
        }
        if self.screen_share.is_visible() {
            lines.push("Screen is being shared".to_string());
        }
        lines.join("\n")
    }

    /// Draw the audio icons.
    fn render_audio(&self, state: &AudioState) {
        set_icon(
            &self.audio,
            icons::volume(state.sink_volume_pct, state.sink_muted),
        );
        self.apply_visibility();
    }

    /// Draw the network icon and the VPN badge.
    fn render_network(&self, state: &NetworkState) {
        if let Some(name) = crate::widgets::quick_settings::model::network_icon(state) {
            set_icon(&self.network, name);
        }
        self.apply_visibility();
    }

    /// Draw the battery icon.
    fn render_battery(&self, state: &BatteryState) {
        set_icon(&self.battery, &state.icon());
        if state.is_low() {
            self.battery.add_css_class(classes::QS_ICON_URGENT);
        } else {
            self.battery.remove_css_class(classes::QS_ICON_URGENT);
        }
        self.apply_visibility();
    }

    /// Draw the Bluetooth icon.
    fn render_bluetooth(&self, state: &BtState) {
        set_icon(
            &self.bluetooth,
            icons::bluetooth(state.powered, state.connected_count() > 0),
        );
        self.apply_visibility();
    }

    /// Show exactly the indicators the state calls for.
    ///
    /// Every service is read here rather than each render pass carrying the
    /// three it did not change: the order and the rules stay in [`Indicators`]
    /// where they are tested, and two services publishing in the same frame
    /// cannot leave the pill in a state neither of them intended.
    fn apply_visibility(&self) {
        let indicators = Indicators::read(
            &self.services.audio.current(),
            &self.services.battery.current(),
            &self.services.network.current(),
            &self.services.bluetooth.current(),
            self.services.privacy.current().screen_sharing,
            self.show_battery,
        );
        let visible = indicators.visible();
        for (indicator, widget) in &self.slots {
            widget.set_visible(visible.contains(indicator));
        }
        self.set_pulsing(
            indicators.shows(Indicator::Microphone) || indicators.shows(Indicator::ScreenShare),
        );
    }

    /// Start or stop the privacy dots' heartbeat.
    ///
    /// One animation for both, because at most one of them is worth looking at
    /// and two dots breathing out of phase would read as a fault rather than as
    /// a warning.
    fn set_pulsing(&self, pulsing: bool) {
        if pulsing == self.pulsing.get() {
            return;
        }
        self.pulsing.set(pulsing);

        let dots = [self.microphone.clone(), self.screen_share.clone()];
        if !pulsing {
            self.pulse.cancel();
            for dot in &dots {
                dot.set_opacity(1.0);
            }
            return;
        }
        // A dot that pulsed with animations off would be the one thing on the
        // panel still moving, which is exactly what the setting is for.
        if !motion_enabled() {
            for dot in &dots {
                dot.set_opacity(1.0);
            }
            return;
        }
        breathe(&self.pulse, &self.root, dots);
    }
}

/// One privacy dot.
fn privacy_dot() -> gtk4::Box {
    let dot = gtk4::Box::new(Orientation::Horizontal, 0);
    dot.add_css_class(classes::QS_PRIVACY_DOT);
    dot.set_valign(gtk4::Align::Center);
    dot.set_visible(false);
    dot
}

/// One breath of the privacy dots, which schedules the next.
///
/// The loop lives in the completion callback rather than inside the animator,
/// so it ends by construction: once neither dot is visible the chain simply
/// stops, and a dropped row has nothing to upgrade to. That is the whole reason
/// this is the one sanctioned unbounded animation in the panel.
fn breathe(pulse: &Animation, anchor: &gtk4::Box, dots: [gtk4::Box; 2]) {
    let painting = dots.clone();
    let on_frame = move |progress: f64| {
        // A cosine rather than a triangle: the turn at each end is what makes
        // it read as breathing instead of blinking.
        let wave = (1.0 - (progress * std::f64::consts::TAU).cos()) / 2.0;
        let opacity = 1.0 - (1.0 - PULSE_FLOOR) * wave;
        for dot in &painting {
            dot.set_opacity(opacity);
        }
    };
    let on_done = {
        let anchor = anchor.downgrade();
        let pulse = pulse.clone();
        move || {
            let Some(anchor) = anchor.upgrade() else {
                return;
            };
            if dots.iter().any(gtk4::prelude::WidgetExt::is_visible) {
                breathe(&pulse, &anchor, dots);
            }
        }
    };
    pulse.start(
        AnimationParams::new(PULSE_MS).with_easing(Easing::Linear),
        Box::new(on_frame),
        Some(Box::new(on_done)),
    );
}
