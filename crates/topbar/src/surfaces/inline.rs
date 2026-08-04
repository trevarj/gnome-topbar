//! Where a Quick Settings row puts its failure.
//!
//! Most failed actions raise a banner. A Quick Settings row must not: the user
//! is looking straight at the control that just flipped back, and a toast
//! covering the top of the screen to say what the row could say in situ is the
//! kind of thing that makes a panel feel like it is shouting.
//!
//! So a row registers a slot — a hidden red caption underneath it — under a
//! name, and [`crate::bridge::act`] with
//! [`ActionScope::Inline`](crate::bridge::ActionScope::Inline) routes the
//! failure into it. The slot is held weakly, so a rebuilt panel takes its
//! captions with it and a report with nowhere to go falls back to the log line
//! `report` always writes.
//!
//! Names are `&'static str` because that is what an `ActionScope` carries;
//! [`names`] has one constant per row so a typo is a compile error rather than
//! a message that silently lands nowhere.

use std::cell::RefCell;

use gtk4::prelude::*;

use crate::style::classes;

/// The rows that can report inline. One constant per slot.
pub mod names {
    /// The output-volume slider.
    pub const VOLUME: &str = "quick_settings.volume";
    /// The microphone slider.
    pub const MICROPHONE: &str = "quick_settings.microphone";
    /// The brightness slider.
    pub const BRIGHTNESS: &str = "quick_settings.brightness";
    /// The Caffeine toggle.
    pub const CAFFEINE: &str = "quick_settings.caffeine";
    /// The Power Mode toggle.
    pub const POWER_MODE: &str = "quick_settings.power_mode";
    /// The battery-health card's charge-limit buttons.
    pub const BATTERY: &str = "quick_settings.battery";
    /// The header's lock button, and the bar button's right-click command.
    pub const LOCK: &str = "quick_settings.lock";
    /// The Suspend row.
    pub const SUSPEND: &str = "quick_settings.power.suspend";
    /// The Restart row.
    pub const RESTART: &str = "quick_settings.power.restart";
    /// The Shut Down row.
    pub const SHUT_DOWN: &str = "quick_settings.power.shut_down";
    /// The Log Out row.
    pub const LOG_OUT: &str = "quick_settings.power.log_out";

    /// Every name, for the coverage test.
    #[cfg(test)]
    pub const ALL: &[&str] = &[
        VOLUME, MICROPHONE, BRIGHTNESS, CAFFEINE, POWER_MODE, BATTERY, LOCK, SUSPEND, RESTART,
        SHUT_DOWN, LOG_OUT,
    ];
}

/// One registered caption.
struct Slot {
    name: &'static str,
    label: glib::WeakRef<gtk4::Label>,
}

use gtk4::glib;

thread_local! {
    /// Every registered slot, in registration order.
    ///
    /// A `Vec` rather than a map: there is one entry per Quick Settings row
    /// per monitor, which is a couple of dozen pointers at the very most, and
    /// it is only walked when something has already failed.
    static SLOTS: RefCell<Vec<Slot>> = const { RefCell::new(Vec::new()) };
}

/// A row's claim on its caption.
///
/// Held for as long as the row lives and never called: it exists so that
/// dropping the row releases its registration. The caption itself belongs to
/// the row's own layout, and writing into it goes through [`report`] so that
/// every message takes the same path whether the row raised it or a service
/// did.
pub struct InlineSlot {
    label: gtk4::Label,
    name: &'static str,
}

impl Drop for InlineSlot {
    fn drop(&mut self) {
        let name = self.name;
        SLOTS.with_borrow_mut(|slots| {
            slots.retain(|slot| {
                slot.name != name
                    || slot
                        .label
                        .upgrade()
                        .is_some_and(|label| label != self.label)
            });
        });
    }
}

/// Build a caption for `name` and register it.
///
/// The label starts hidden and takes no vertical space until something goes
/// wrong, so a panel with nothing failing has no gaps reserved in it.
pub fn slot(name: &'static str) -> (gtk4::Label, InlineSlot) {
    let label = gtk4::Label::new(None);
    label.add_css_class(classes::INLINE_ERROR);
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
    label.set_visible(false);

    SLOTS.with_borrow_mut(|slots| {
        slots.retain(|slot| slot.label.upgrade().is_some());
        slots.push(Slot {
            name,
            label: {
                let weak = glib::WeakRef::new();
                weak.set(Some(&label));
                weak
            },
        });
    });

    let handle = InlineSlot {
        label: label.clone(),
        name,
    };
    (label, handle)
}

/// Put `message` in every live slot called `name`.
///
/// Returns whether anything took it, so the caller can fall back to a banner
/// when the row that would have shown it has gone away.
///
/// Every live slot rather than one: with two monitors there are two panels and
/// two captions of the same name, and the user is looking at whichever is
/// open. Writing to both is cheaper than working out which, and the message
/// clears on the next attempt either way.
pub fn report(name: &str, message: &str) -> bool {
    SLOTS.with_borrow_mut(|slots| {
        slots.retain(|slot| slot.label.upgrade().is_some());
        let mut delivered = false;
        for slot in slots.iter().filter(|slot| slot.name == name) {
            if let Some(label) = slot.label.upgrade() {
                show(&label, message);
                delivered = true;
            }
        }
        delivered
    })
}

/// Clear every live slot called `name`.
pub fn clear(name: &str) {
    SLOTS.with_borrow(|slots| {
        for slot in slots.iter().filter(|slot| slot.name == name) {
            if let Some(label) = slot.label.upgrade() {
                label.set_visible(false);
                label.set_text("");
            }
        }
    });
}

/// Show a message in one caption.
fn show(label: &gtk4::Label, message: &str) {
    label.set_text(message);
    label.set_visible(true);
}

/// How many slots are registered. For the tests.
#[cfg(test)]
fn registered() -> usize {
    SLOTS.with_borrow(|slots| {
        slots
            .iter()
            .filter(|slot| slot.label.upgrade().is_some())
            .count()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn every_slot_name_is_distinct_and_namespaced() {
        let unique: BTreeSet<&&str> = names::ALL.iter().collect();
        assert_eq!(unique.len(), names::ALL.len(), "duplicate slot name");
        for name in names::ALL {
            assert!(
                name.starts_with("quick_settings."),
                "{name} is not namespaced to the widget that owns it"
            );
        }
    }

    #[test]
    fn a_report_with_no_slot_registered_says_it_went_nowhere() {
        assert!(
            !report("quick_settings.nothing_is_here", "boom"),
            "a caller has to know when to fall back to a banner"
        );
    }

    #[test]
    fn the_registry_starts_empty_on_this_thread() {
        // The registry is thread-local and GTK is not initialised here, so a
        // unit test sees only what it registers itself.
        assert_eq!(registered(), 0);
    }
}
