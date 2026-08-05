//! The panel's top row: the battery, and the two things you do at the end of
//! the day.
//!
//! ```text
//! [ 🔋 62% ]                              (🔒)  (⏻)
//!   ↑ opens the health card                ↑     ↑ opens the power section
//!                                          └ locks the session outright
//! ```
//!
//! The lock button is the only control in the panel that acts on a click with
//! no confirmation, and deliberately so: locking a screen is free to undo, and
//! a lock button that needed holding would be the wrong shape for the one
//! thing people do in a hurry.

use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Align, Button, Image, Label, Orientation};
use topbar_services::{BatteryState, Services};

use crate::anim::ripple;
use crate::bridge::{self, BindingGuard};
use crate::style::{classes, icons};
use crate::surfaces::inline::{self, names};
use crate::widgets::expander::{Accordion, Section};
use crate::widgets::quick_settings::{attempt, set_icon, set_text};

/// Space between the pill and the buttons.
const SPACING: i32 = 8;

/// The header row.
pub struct Header {
    root: gtk4::Box,
    pill: Button,
    icon: Image,
    percent: Label,
    services: Services,
    _slots: Vec<inline::InlineSlot>,
    bindings: std::cell::RefCell<Vec<BindingGuard>>,
}

impl Header {
    /// Build the row.
    ///
    /// `battery_section` and `power_section` are the two things it opens; both
    /// belong to the panel, which is what puts them in the same accordion as
    /// the Power Mode toggle.
    pub fn new(
        services: &Services,
        accordion: &Rc<Accordion>,
        battery_section: &Rc<Section>,
        power_section: &Rc<Section>,
        lock_command: Option<String>,
        show_battery: bool,
    ) -> Rc<Self> {
        // A column, so a failed lock command can put a caption under the whole
        // row rather than pushing the buttons sideways.
        let root = gtk4::Box::new(Orientation::Vertical, 0);
        let row = gtk4::Box::new(Orientation::Horizontal, SPACING);
        row.add_css_class(classes::QS_HEADER);
        root.append(&row);

        let pill = Button::new();
        pill.add_css_class(classes::QS_BATTERY_PILL);
        pill.set_valign(Align::Center);

        let content = gtk4::Box::new(Orientation::Horizontal, 0);
        let icon = Image::new();
        icon.add_css_class(classes::QS_ICON);
        content.append(&icon);
        let percent = Label::new(None);
        percent.add_css_class(classes::QS_BATTERY_PERCENT);
        content.append(&percent);
        pill.set_child(Some(&content));
        ripple::install(&pill);
        pill.set_visible(false);
        if show_battery {
            row.append(&pill);
        }

        let spacer = gtk4::Box::new(Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        row.append(&spacer);

        let lock = round_button(icons::LOCK, "Lock");
        row.append(&lock);
        let power = round_button(icons::SHUT_DOWN, "Power off");
        row.append(&power);

        let (lock_error, lock_slot) = inline::slot(names::LOCK);
        root.append(&lock_error);

        let header = Rc::new(Self {
            root,
            pill,
            icon,
            percent,
            services: services.clone(),
            _slots: vec![lock_slot],
            bindings: std::cell::RefCell::new(Vec::new()),
        });

        header.pill.connect_clicked({
            let accordion = Rc::clone(accordion);
            let section = Rc::clone(battery_section);
            move |_| accordion.toggle(&section)
        });
        power.connect_clicked({
            let accordion = Rc::clone(accordion);
            let section = Rc::clone(power_section);
            move |_| accordion.toggle(&section)
        });
        lock.connect_clicked(move |_| {
            let Some(command) = lock_command.clone() else {
                inline::report(names::LOCK, "No lock command is configured");
                return;
            };
            attempt(names::LOCK, async move {
                topbar_services::proc::run(&command).await
            });
        });

        let binding = bridge::bind_state(&header.root, services.battery.state(), {
            let header = Rc::downgrade(&header);
            move |_: &gtk4::Box, state: &BatteryState| {
                if let Some(header) = header.upgrade() {
                    header.render(state);
                }
            }
        });
        header.bindings.borrow_mut().push(binding);

        header
    }

    /// The widget to put in the panel.
    pub fn root(&self) -> &gtk4::Box {
        &self.root
    }

    /// Re-render from current state.
    pub fn refresh(&self) {
        self.render(&self.services.battery.current());
    }

    /// Draw the battery pill.
    fn render(&self, state: &BatteryState) {
        self.pill.set_visible(state.available);
        if !state.available {
            return;
        }
        set_icon(&self.icon, &state.icon());
        set_text(
            &self.percent,
            &state
                .rounded_percent()
                .map_or_else(|| "—".to_string(), |percent| format!("{percent}%")),
        );
        if state.is_low() {
            self.icon.add_css_class(classes::QS_ICON_URGENT);
        } else {
            self.icon.remove_css_class(classes::QS_ICON_URGENT);
        }
    }
}

/// One of the header's round icon buttons.
fn round_button(icon: &str, tooltip: &str) -> Button {
    let button = Button::new();
    button.add_css_class(classes::QS_ROUND_BUTTON);
    button.set_child(Some(&Image::from_icon_name(icon)));
    ripple::install(&button);
    button.set_valign(Align::Center);
    button.set_tooltip_text(Some(tooltip));
    button
}
