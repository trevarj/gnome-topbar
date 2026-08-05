//! The battery-health card: what the battery is doing, and where it stops.
//!
//! ```text
//! Battery
//! 62% · Discharging · 2h 15m left
//!
//! Charge limit          [ Full ] [ 80% ]
//! Charging stops at 80%, resumes below 75%
//! ```
//!
//! The charge limit is the reason this card exists. Keeping a laptop battery
//! between roughly 20% and 80% is the single largest thing a user can do about
//! its lifespan, and the kernel exposes it as two files — which on a stock
//! system are owned by root.
//!
//! That is not engineered around. When neither the files nor UPower will take
//! a write the buttons are **disabled and the card says why**, naming the udev
//! rule that fixes it. A panel that silently hid the controls would leave the
//! user thinking their machine could not do it; one that pretended the write
//! had worked would be worse.

use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Align, Button, Label, Orientation};
use topbar_services::battery::{FULL_PRESET, LIMIT_PRESET, duration};
use topbar_services::{BatteryState, Services};

use crate::anim::ripple;
use crate::bridge::{self, BindingGuard};
use crate::style::classes;
use crate::surfaces::inline::{self, names};
use crate::widgets::quick_settings::{attempt, set_text};

/// What to do about root-owned threshold files.
///
/// Named in full rather than gestured at: a user who reads this should be able
/// to act on it without going looking.
const UDEV_HINT: &str = "The kernel's charge-limit files are read-only for this \
user. A udev rule granting write access to \
/sys/class/power_supply/*/charge_control_*_threshold enables these buttons.";

/// The battery-health card.
pub struct BatteryCard {
    root: gtk4::Box,
    summary: Label,
    estimate: Label,
    limit_row: gtk4::Box,
    full: Button,
    limited: Button,
    detail: Label,
    hint: Label,
    services: Services,
    _slots: Vec<inline::InlineSlot>,
    bindings: std::cell::RefCell<Vec<BindingGuard>>,
}

impl BatteryCard {
    /// Build the card.
    pub fn new(services: &Services) -> Rc<Self> {
        let root = gtk4::Box::new(Orientation::Vertical, 6);
        root.add_css_class(classes::QS_CARD);

        let title = Label::new(Some("Battery"));
        title.add_css_class(classes::QS_CARD_TITLE);
        title.set_xalign(0.0);
        root.append(&title);

        let summary = Label::new(None);
        summary.add_css_class(classes::QS_CARD_LINE);
        summary.set_xalign(0.0);
        root.append(&summary);

        let estimate = Label::new(None);
        estimate.add_css_class(classes::QS_CARD_LINE);
        estimate.set_xalign(0.0);
        estimate.set_visible(false);
        root.append(&estimate);

        let limit_row = gtk4::Box::new(Orientation::Horizontal, 8);
        limit_row.add_css_class(classes::QS_LIMIT_ROW);

        let caption = Label::new(Some("Charge limit"));
        caption.add_css_class(classes::QS_CARD_LINE);
        caption.set_xalign(0.0);
        caption.set_hexpand(true);
        limit_row.append(&caption);

        let full = limit_button("Full");
        limit_row.append(&full);
        let limited = limit_button(&format!("{}%", LIMIT_PRESET.1));
        limit_row.append(&limited);
        root.append(&limit_row);

        let detail = Label::new(None);
        detail.add_css_class(classes::QS_CARD_LINE);
        detail.set_xalign(0.0);
        detail.set_visible(false);
        root.append(&detail);

        let hint = Label::new(Some(UDEV_HINT));
        hint.add_css_class(classes::QS_HINT);
        hint.set_xalign(0.0);
        hint.set_wrap(true);
        hint.set_visible(false);
        root.append(&hint);

        let (error, slot) = inline::slot(names::BATTERY);
        root.append(&error);

        let card = Rc::new(Self {
            root,
            summary,
            estimate,
            limit_row,
            full,
            limited,
            detail,
            hint,
            services: services.clone(),
            _slots: vec![slot],
            bindings: std::cell::RefCell::new(Vec::new()),
        });

        for (button, preset) in [(&card.full, FULL_PRESET), (&card.limited, LIMIT_PRESET)] {
            button.connect_clicked({
                let battery = services.battery.handle().clone();
                move |_| {
                    let battery = battery.clone();
                    attempt(names::BATTERY, async move {
                        battery.set_thresholds(preset.0, preset.1).await
                    });
                }
            });
        }

        let binding = bridge::bind_state(&card.root, services.battery.state(), {
            let card = Rc::downgrade(&card);
            move |_: &gtk4::Box, state: &BatteryState| {
                if let Some(card) = card.upgrade() {
                    card.render(state);
                }
            }
        });
        card.bindings.borrow_mut().push(binding);

        card
    }

    /// The widget to put in the panel.
    pub fn root(&self) -> &gtk4::Box {
        &self.root
    }

    /// Re-render from current state.
    pub fn refresh(&self) {
        self.render(&self.services.battery.current());
    }

    /// Draw the card.
    fn render(&self, state: &BatteryState) {
        let charge = state
            .rounded_percent()
            .map_or_else(|| "Unknown".to_string(), |percent| format!("{percent}%"));
        set_text(
            &self.summary,
            &format!("{charge} · {}", state.status.label()),
        );

        // Whichever estimate applies, and nothing at all when neither does —
        // an empty "time remaining" line is worse than no line.
        let remaining = if state.status.is_charging() {
            state
                .time_to_full
                .and_then(duration)
                .map(|left| format!("{left} until full"))
        } else {
            state
                .time_to_empty
                .and_then(duration)
                .map(|left| format!("{left} remaining"))
        };
        match remaining {
            Some(text) => {
                set_text(&self.estimate, &text);
                self.estimate.set_visible(true);
            }
            None => self.estimate.set_visible(false),
        }

        // A battery whose firmware exposes no limit at all has no row for one.
        let has_limit = state.thresholds.is_some() || state.upower_thresholds;
        self.limit_row.set_visible(has_limit);
        if !has_limit {
            self.detail.set_visible(false);
            self.hint.set_visible(false);
            return;
        }

        let settable = state.can_set_thresholds();
        self.full.set_sensitive(settable);
        self.limited.set_sensitive(settable);
        self.hint.set_visible(!settable);

        match state.thresholds {
            Some(limits) => {
                set_checked(&self.full, !limits.limited());
                set_checked(&self.limited, limits.limited());
                set_text(
                    &self.detail,
                    &format!(
                        "Charging stops at {}%, resumes below {}%",
                        limits.end, limits.start
                    ),
                );
                self.detail.set_visible(true);
            }
            None => {
                set_checked(&self.full, false);
                set_checked(&self.limited, false);
                self.detail.set_visible(false);
            }
        }
    }
}

/// One of the two charge-limit buttons.
fn limit_button(label: &str) -> Button {
    let button = Button::with_label(label);
    button.add_css_class(classes::QS_LIMIT_BUTTON);
    // Every other button in the panel answers a press with a ripple. These two
    // and the password box's were the only ones that did nothing at all until
    // the write came back, which on a machine where the write is refused is
    // nothing at all, ever.
    ripple::install(&button);
    button.set_valign(Align::Center);
    button
}

/// Mark the preset that is in force.
fn set_checked(button: &Button, checked: bool) {
    if checked {
        button.add_css_class(classes::CHECKED);
    } else {
        button.remove_css_class(classes::CHECKED);
    }
}
