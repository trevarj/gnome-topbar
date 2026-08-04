//! The Quick Settings button: a pill of status icons.
//!
//! ```text
//!  [ 📶 🔒 🔊 ᛒ 🔋 ● ]
//!    │  │  │  │  │  └ something is listening / watching
//!    │  │  │  │  └ the battery, when there is one
//!    │  │  │  └ bluetooth        (M9c)
//!    │  │  └ the output volume   — always
//!    │  └ a VPN                  (M9b)
//!    └ the network               (M9b)
//! ```
//!
//! The order is fixed and the icons come and go inside it, which is what stops
//! the pill's contents rearranging themselves under the pointer whenever
//! something plugs in. Every icon has a slot in [`model::ORDER`] today, even
//! the four that nothing draws yet — so M9b and M9c add an icon rather than a
//! layout.

use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Image, Orientation};
use topbar_services::{AudioState, BatteryState, Services};

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
    microphone: gtk4::Box,
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

        // A dot rather than a microphone icon: GNOME draws a dot, and at 18px
        // on a black bar an outlined microphone reads as a smudge.
        let microphone = gtk4::Box::new(Orientation::Horizontal, 0);
        microphone.add_css_class(classes::QS_PRIVACY_DOT);
        microphone.set_valign(gtk4::Align::Center);
        microphone.set_visible(false);

        // Appended in the published order, so the widgets M9b and M9c add have
        // an obvious place to go. Nothing is appended for the four that do not
        // exist yet, which is what keeps them from moving anything when they
        // arrive.
        let mut slots: Vec<(Indicator, gtk4::Widget)> = Vec::new();
        for indicator in crate::widgets::quick_settings::model::ORDER {
            let widget: Option<gtk4::Widget> = match indicator {
                Indicator::Audio => Some(audio.clone().upcast()),
                Indicator::Battery => Some(battery.clone().upcast()),
                Indicator::Microphone => Some(microphone.clone().upcast()),
                Indicator::Network
                | Indicator::Vpn
                | Indicator::Bluetooth
                | Indicator::ScreenShare => None,
            };
            if let Some(widget) = widget {
                root.append(&widget);
                slots.push((*indicator, widget));
            }
        }
        content.append(&root);

        let indicators = Rc::new(Self {
            pulse: Animation::new(&microphone),
            root,
            audio,
            battery,
            microphone,
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
        indicators
            .bindings
            .borrow_mut()
            .extend([audio_binding, battery_binding]);

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
        lines.join("\n")
    }

    /// Draw the audio icons.
    fn render_audio(&self, state: &AudioState) {
        set_icon(
            &self.audio,
            icons::volume(state.sink_volume_pct, state.sink_muted),
        );
        self.apply_visibility(state, &self.services.battery.current());
        self.set_pulsing(state.source_in_use);
    }

    /// Draw the battery icon.
    fn render_battery(&self, state: &BatteryState) {
        set_icon(&self.battery, &state.icon());
        if state.is_low() {
            self.battery.add_css_class(classes::QS_ICON_URGENT);
        } else {
            self.battery.remove_css_class(classes::QS_ICON_URGENT);
        }
        self.apply_visibility(&self.services.audio.current(), state);
    }

    /// Show exactly the indicators the state calls for.
    ///
    /// One place rather than one per service, so the order and the rules stay
    /// in [`Indicators`] where they are tested, and two services publishing at
    /// once cannot leave the pill in a state neither of them intended.
    fn apply_visibility(&self, audio: &AudioState, battery: &BatteryState) {
        let indicators = Indicators::read(audio, battery, self.show_battery);
        let visible = indicators.visible();
        for (indicator, widget) in &self.slots {
            widget.set_visible(visible.contains(indicator));
        }
    }

    /// Start or stop the privacy dot's heartbeat.
    fn set_pulsing(&self, pulsing: bool) {
        if pulsing == self.pulsing.get() {
            return;
        }
        self.pulsing.set(pulsing);

        if !pulsing {
            self.pulse.cancel();
            self.microphone.set_opacity(1.0);
            return;
        }
        // A dot that pulsed with animations off would be the one thing on the
        // panel still moving, which is exactly what the setting is for.
        if !motion_enabled() {
            self.microphone.set_opacity(1.0);
            return;
        }
        breathe(&self.pulse, &self.microphone);
    }
}

/// One breath of the privacy dot, which schedules the next.
///
/// The loop lives in the completion callback rather than inside the animator,
/// so it ends by construction: a hidden or dropped dot has nothing to upgrade
/// to and the chain simply stops. That is the whole reason this is the one
/// sanctioned unbounded animation in the panel.
fn breathe(pulse: &Animation, dot: &gtk4::Box) {
    let painting = dot.clone();
    let on_frame = move |progress: f64| {
        // A cosine rather than a triangle: the turn at each end is what makes
        // it read as breathing instead of blinking.
        let wave = (1.0 - (progress * std::f64::consts::TAU).cos()) / 2.0;
        painting.set_opacity(1.0 - (1.0 - PULSE_FLOOR) * wave);
    };
    let on_done = {
        let dot = dot.downgrade();
        let pulse = pulse.clone();
        move || {
            if let Some(dot) = dot.upgrade()
                && dot.is_visible()
            {
                breathe(&pulse, &dot);
            }
        }
    };
    pulse.start(
        AnimationParams::new(PULSE_MS).with_easing(Easing::Linear),
        Box::new(on_frame),
        Some(Box::new(on_done)),
    );
}
