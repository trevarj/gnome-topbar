//! What will not fit on the bar, behind a chevron.
//!
//! ```text
//!  ▣ ▤ ▥ ▦ ▧ ▨ ▩ ▤ ▥ ▦ ⋯      ten icons and the chevron
//!                    ┌──────────────┐
//!                    │  ▩  ▧  ▨     │  the rest, in a grid
//!                    │  ▥  ▦        │
//!                    └──────────────┘
//! ```
//!
//! The icons in the popover behave exactly as the ones on the bar do — the
//! same four gestures, the same service calls — because from the application's
//! point of view there is no difference between the two places its icon might
//! be sitting.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use gtk4::prelude::*;
use gtk4::{Grid, Image, Label, Orientation};
use topbar_services::{ItemView, ScrollAxis};

use crate::style::classes;
use crate::surfaces::popovers::PopoverContent;
use crate::surfaces::tooltip::{self, TooltipHandle};

use super::{Inner, flat_button, icon};

/// How many icons go in a row of the popover's grid.
const COLUMNS: i32 = 5;
/// Icon size in the popover, a little larger than on the bar.
const ICON_SIZE: i32 = 24;

/// How many icons go on the bar, and whether a chevron is needed.
///
/// The chevron takes a place of its own, so the last inline icon gives way to
/// it: eleven icons in ten places is ten icons, not eleven. Everything fits or
/// there is no chevron at all — one hidden icon behind a chevron would waste
/// exactly the room it saved.
pub fn split(total: usize, max_icons: usize) -> (usize, bool) {
    if total <= max_icons {
        (total, false)
    } else {
        (max_icons.saturating_sub(1), true)
    }
}

/// The chevron's popover.
pub struct Overflow {
    root: gtk4::Box,
    grid: Grid,
    empty: Label,
    /// What was last drawn into it.
    items: RefCell<Vec<ItemView>>,
    /// The widget that owns it, for the service calls a click makes.
    owner: Weak<Inner>,
    tooltips: RefCell<Vec<TooltipHandle>>,
}

impl Overflow {
    /// Build the popover for the widget that will own it.
    pub fn new(owner: Weak<Inner>) -> Rc<Self> {
        let root = gtk4::Box::new(Orientation::Vertical, 0);
        root.add_css_class(classes::TRAY_OVERFLOW_POPOVER);

        let grid = Grid::new();
        grid.add_css_class(classes::TRAY_OVERFLOW_GRID);
        grid.set_row_spacing(4);
        grid.set_column_spacing(4);
        root.append(&grid);

        let empty = Label::new(Some("Nothing hidden"));
        empty.add_css_class(classes::EMPTY_STATE_LABEL);
        empty.set_visible(false);
        root.append(&empty);

        Rc::new(Self {
            root,
            grid,
            empty,
            items: RefCell::new(Vec::new()),
            owner,
            tooltips: RefCell::new(Vec::new()),
        })
    }

    /// Draw the items that did not fit.
    ///
    /// Rebuilt when the set changes rather than diffed: the overflow is a
    /// handful of icons that changes only when applications come and go, and
    /// the machinery to avoid the rebuild would cost more than the rebuild.
    pub fn render(&self, items: &[ItemView], inner: &Inner) {
        if *self.items.borrow() == items {
            return;
        }

        while let Some(child) = self.grid.first_child() {
            self.grid.remove(&child);
        }
        self.tooltips.borrow_mut().clear();

        self.empty.set_visible(items.is_empty());
        self.grid.set_visible(!items.is_empty());

        for (index, item) in items.iter().enumerate() {
            let button = flat_button(classes::TRAY_ITEM);
            let image = Image::new();
            image.add_css_class(classes::TRAY_ITEM_ICON);
            icon::apply(&image, &item.icon, ICON_SIZE, inner.contrast);
            button.append(&image);

            self.tooltips
                .borrow_mut()
                .push(tooltip::attach(&button, item.tooltip_text()));
            self.wire(&button, &item.id);

            let index = index as i32;
            self.grid
                .attach(&button, index % COLUMNS, index / COLUMNS, 1, 1);
        }

        *self.items.borrow_mut() = items.to_vec();
    }

    /// Give one icon in the grid the same four gestures it would have on the
    /// bar.
    fn wire(&self, button: &gtk4::Box, id: &str) {
        for (which, action) in [
            (gtk4::gdk::BUTTON_PRIMARY, 0u8),
            (gtk4::gdk::BUTTON_MIDDLE, 1),
            (gtk4::gdk::BUTTON_SECONDARY, 2),
        ] {
            let click = gtk4::GestureClick::new();
            click.set_button(which);
            click.connect_released({
                let owner = self.owner.clone();
                let id = id.to_string();
                move |gesture, _, _, _| {
                    let (Some(inner), Some(anchor)) = (owner.upgrade(), gesture.widget()) else {
                        return;
                    };
                    match action {
                        0 => inner.activate(&id, &anchor),
                        1 => inner.secondary_activate(&id),
                        _ => inner.open_menu(&id, &anchor),
                    }
                }
            });
            button.add_controller(click);
        }

        let scroll = gtk4::EventControllerScroll::new(
            gtk4::EventControllerScrollFlags::BOTH_AXES
                | gtk4::EventControllerScrollFlags::DISCRETE,
        );
        scroll.connect_scroll({
            let owner = self.owner.clone();
            let id = id.to_string();
            move |_, x, y| {
                if let Some(inner) = owner.upgrade() {
                    for (delta, axis) in [(y, ScrollAxis::Vertical), (x, ScrollAxis::Horizontal)] {
                        let notches = (delta * 120.0).round() as i32;
                        if notches != 0 {
                            inner.scroll(&id, notches, axis);
                        }
                    }
                }
                gtk4::glib::Propagation::Stop
            }
        });
        button.add_controller(scroll);
    }
}

impl PopoverContent for Overflow {
    fn root(&self) -> gtk4::Widget {
        self.root.clone().upcast()
    }

    /// Nothing to do: the grid is written by the render pass that decides what
    /// is in it, which has already run by the time the chevron can be clicked.
    fn refresh(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn everything_that_fits_stays_on_the_bar() {
        assert_eq!(split(0, 12), (0, false));
        assert_eq!(split(1, 12), (1, false));
        assert_eq!(split(11, 12), (11, false));
        assert_eq!(
            split(12, 12),
            (12, false),
            "exactly full needs no chevron: there is nothing behind it"
        );
    }

    #[test]
    fn one_too_many_costs_the_last_place_to_the_chevron() {
        // Thirteen icons in twelve places: eleven inline, two hidden.
        assert_eq!(split(13, 12), (11, true));
        assert_eq!(split(14, 12), (11, true));
        assert_eq!(split(40, 12), (11, true));
    }

    #[test]
    fn the_split_never_promises_more_icons_than_there_are() {
        for total in 0..30usize {
            for max in 0..15usize {
                let (inline, chevron) = split(total, max);
                assert!(inline <= total, "{inline} of {total}");
                assert!(
                    inline <= max || max == 0,
                    "{inline} inline breaks a limit of {max}"
                );
                assert_eq!(
                    chevron,
                    inline < total,
                    "the chevron is shown exactly when something is behind it"
                );
            }
        }
    }

    #[test]
    fn a_limit_of_one_puts_everything_behind_the_chevron() {
        assert_eq!(split(1, 1), (1, false));
        assert_eq!(split(2, 1), (0, true), "one place, and the chevron took it");
    }

    #[test]
    fn a_limit_of_zero_hides_every_icon_rather_than_underflowing() {
        assert_eq!(split(0, 0), (0, false));
        assert_eq!(split(5, 0), (0, true));
    }
}
