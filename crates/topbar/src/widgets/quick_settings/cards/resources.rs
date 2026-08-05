//! The resource overview: CPU, memory, swap and every disk, as bars.
//!
//! ```text
//! ┌───────────────────────────────────────┐
//! │ System                                │
//! │ CPU        ▓▓▓▓▓▓░░░░░░░░░░░░░░   34% │
//! │ Memory     ▓▓▓▓▓▓▓▓▓▓▓░░░░░░░░  7.2 / 16 GiB
//! │ Swap       ▓░░░░░░░░░░░░░░░░░░    2%  │
//! │ /          ▓▓▓▓▓▓▓▓▓▓▓▓▓▓░░░░  312 / 465 GiB
//! └───────────────────────────────────────┘
//! ```
//!
//! **This component is mounted twice.** Quick Settings puts it in a card;
//! M10's `system_monitor` widget puts the same [`ResourceOverview`] in its own
//! popover, the way the weather forecast is shared between the clock panel and
//! the weather widget. So it owns its subscription and its own rendering and
//! knows nothing about either host — [`ResourceOverview::new`] and
//! [`ResourceOverview::root`] are the whole interface.
//!
//! The rows are rebuilt only when the *set of disks* changes. A snapshot
//! arrives every five seconds and the numbers in it move every time; rebuilding
//! four rows at that rate would be four allocations and a relayout a second for
//! something the eye reads as a bar sliding.

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Align, Label, LevelBar, Orientation};
use topbar_services::resources::model::used_of;
use topbar_services::{ResourceState, Services};

use crate::bridge::{self, BindingGuard};
use crate::style::classes;
use crate::widgets::quick_settings::set_text;

/// Where a bar starts reading as a problem.
///
/// Only a tint, and only on the bar: the card is a statement of fact, and the
/// widget that *acts* on a threshold is M10's `system_monitor`, which has its
/// own configurable ones. Something the user should look at should look like
/// something to look at, even here.
const WARNING_PCT: u8 = 90;

/// Space between a row's caption, its bar and its reading.
const GAP: i32 = 10;

/// How wide the caption column is, so the bars line up.
const CAPTION_CHARS: i32 = 7;

/// One metered row: a caption, a bar and a reading.
struct Meter {
    row: gtk4::Box,
    caption: Label,
    bar: LevelBar,
    value: Label,
}

impl Meter {
    /// Build a row.
    fn new(caption: &str) -> Self {
        let row = gtk4::Box::new(Orientation::Horizontal, GAP);
        row.add_css_class(classes::QS_METER_ROW);
        row.set_valign(Align::Center);

        let label = Label::new(Some(caption));
        label.add_css_class(classes::QS_CARD_LINE);
        label.set_xalign(0.0);
        label.set_width_chars(CAPTION_CHARS);
        label.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
        row.append(&label);

        // A `LevelBar` rather than a drawn rectangle: it is the widget GTK has
        // for exactly this, it scales with the theme's own metrics, and it is
        // one `set_value` per frame rather than a snapshot callback.
        let bar = LevelBar::new();
        bar.add_css_class(classes::QS_METER);
        bar.set_min_value(0.0);
        bar.set_max_value(100.0);
        bar.set_mode(gtk4::LevelBarMode::Continuous);
        bar.set_hexpand(true);
        bar.set_valign(Align::Center);
        // A fresh `LevelBar` carries three offsets — low, high and full — and
        // each one puts a style class on the fill when the value crosses it.
        // They are meant for a battery gauge, where low is bad and full is
        // good; on a *usage* bar that reading is inverted, and the theme would
        // tint a nearly-empty disk as the warning. The panel decides its own
        // threshold, so the built-in ones go.
        for offset in [
            gtk4::LEVEL_BAR_OFFSET_LOW,
            gtk4::LEVEL_BAR_OFFSET_HIGH,
            gtk4::LEVEL_BAR_OFFSET_FULL,
        ] {
            bar.remove_offset_value(Some(offset));
        }
        row.append(&bar);

        // Tabular figures and a fixed alignment, so a reading going from 9% to
        // 10% does not move the bar beside it.
        let value = Label::new(None);
        value.add_css_class(classes::QS_METER_VALUE);
        value.set_xalign(1.0);
        row.append(&value);

        Self {
            row,
            caption: label,
            bar,
            value,
        }
    }

    /// Draw one reading.
    fn set(&self, caption: &str, percent: u8, reading: &str) {
        set_text(&self.caption, caption);
        set_text(&self.value, reading);
        self.bar.set_value(f64::from(percent));
        if percent >= WARNING_PCT {
            self.bar.add_css_class(classes::QS_METER_WARNING);
        } else {
            self.bar.remove_css_class(classes::QS_METER_WARNING);
        }
    }
}

/// The resource overview, as a mountable component.
pub struct ResourceOverview {
    root: gtk4::Box,
    cpu: Meter,
    memory: Meter,
    swap: Meter,
    /// One row per disk, in the order the service reports them.
    disks: RefCell<Vec<Meter>>,
    /// The mount points the disk rows were last built for.
    built: RefCell<Vec<String>>,
    services: Services,
    bindings: RefCell<Vec<BindingGuard>>,
}

impl ResourceOverview {
    /// Build the component.
    ///
    /// `title` is drawn above the rows when it is given. Quick Settings names
    /// the card; the `system_monitor` popover has a header of its own and
    /// passes `None`.
    pub fn new(services: &Services, title: Option<&str>) -> Rc<Self> {
        let root = gtk4::Box::new(Orientation::Vertical, 4);
        root.add_css_class(classes::QS_CARD);
        root.add_css_class(classes::QS_RESOURCES);

        if let Some(title) = title {
            let heading = Label::new(Some(title));
            heading.add_css_class(classes::QS_CARD_TITLE);
            heading.set_xalign(0.0);
            root.append(&heading);
        }

        let cpu = Meter::new("CPU");
        root.append(&cpu.row);
        let memory = Meter::new("Memory");
        root.append(&memory.row);
        // Hidden on a machine with no swap, which is most of them now.
        let swap = Meter::new("Swap");
        swap.row.set_visible(false);
        root.append(&swap.row);

        let overview = Rc::new(Self {
            root,
            cpu,
            memory,
            swap,
            disks: RefCell::new(Vec::new()),
            built: RefCell::new(Vec::new()),
            services: services.clone(),
            bindings: RefCell::new(Vec::new()),
        });

        let binding = bridge::bind_state(&overview.root, services.resources.state(), {
            let overview = Rc::downgrade(&overview);
            move |_: &gtk4::Box, state: &ResourceState| {
                if let Some(overview) = overview.upgrade() {
                    overview.render(state);
                }
            }
        });
        overview.bindings.borrow_mut().push(binding);

        overview
    }

    /// The widget to put in a panel or a popover.
    pub fn root(&self) -> &gtk4::Box {
        &self.root
    }

    /// Re-render from current state.
    pub fn refresh(&self) {
        self.render(&self.services.resources.current());
    }

    /// Draw the overview.
    fn render(&self, state: &ResourceState) {
        match state.cpu_pct {
            Some(percent) => self.cpu.set("CPU", percent, &format!("{percent}%")),
            // The first five seconds of a session, and the one sample after a
            // resume. An em dash rather than 0%, which would be a reading.
            None => self.cpu.set("CPU", 0, "—"),
        }

        let memory = &state.memory;
        self.memory.set(
            "Memory",
            memory.used_pct,
            &used_of(memory.used_kib * 1024, memory.total_kib * 1024),
        );

        self.swap.row.set_visible(memory.has_swap());
        if let Some(percent) = memory.swap_used_pct {
            self.swap.set(
                "Swap",
                percent,
                &used_of(memory.swap_used_kib * 1024, memory.swap_total_kib * 1024),
            );
        }

        let mounts: Vec<String> = state.disks.iter().map(|disk| disk.mount.clone()).collect();
        if *self.built.borrow() != mounts {
            self.rebuild_disks(&mounts);
            *self.built.borrow_mut() = mounts;
        }
        for (meter, disk) in self.disks.borrow().iter().zip(&state.disks) {
            meter.set(&disk.mount, disk.used_pct, &used_of(disk.used, disk.total));
        }
    }

    /// Rebuild the disk rows.
    fn rebuild_disks(&self, mounts: &[String]) {
        for meter in self.disks.borrow().iter() {
            self.root.remove(&meter.row);
        }
        let mut meters = Vec::new();
        for mount in mounts {
            let meter = Meter::new(mount);
            self.root.append(&meter.row);
            meters.push(meter);
        }
        *self.disks.borrow_mut() = meters;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_warning_tint_lands_where_a_reading_becomes_worth_looking_at() {
        // Below M10's own default cpu_threshold of 90 there is nothing to
        // notice; at it, the bar says so without the card claiming to be an
        // alert.
        assert_eq!(WARNING_PCT, 90);
    }

    #[test]
    fn a_machine_with_no_swap_has_no_swap_row() {
        let none = topbar_services::Memory::default();
        assert!(!none.has_swap());
        assert_eq!(none.swap_used_pct, None);

        let some = topbar_services::Memory {
            swap_total_kib: 1024,
            swap_used_kib: 512,
            swap_used_pct: Some(50),
            ..topbar_services::Memory::default()
        };
        assert!(some.has_swap());
    }
}
