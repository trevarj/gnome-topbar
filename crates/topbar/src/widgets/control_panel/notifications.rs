//! The control panel's notifications column.
//!
//! M3 builds the frame and nothing else: the designed empty state, and the Do
//! Not Disturb row that sits at its foot. The column is fixed-width and its
//! parts are laid out now precisely so that M4 — which brings the daemon, the
//! history list and the DND switch's wiring — changes what is *inside*
//! [`Column::list`] without moving anything around it.
//!
//! M4's entry points are [`Column::list`] (append the history there and hide
//! the empty state) and [`Column::dnd`] (make the switch live).

use gtk4::prelude::*;
use gtk4::{Align, Image, Label, Orientation, Switch};

use crate::style::classes;

/// Adwaita's bell. There is no plain `notifications-symbolic` in Adwaita 50,
/// and `notifications-disabled-symbolic` would read as "DND is on" rather than
/// "nothing has arrived".
const EMPTY_ICON: &str = "preferences-system-notifications-symbolic";

/// The notifications column.
pub struct Column {
    root: gtk4::Box,
    /// Where M4 appends notification groups.
    list: gtk4::Box,
    /// Shown while [`Column::list`] is empty.
    empty: gtk4::Box,
    /// The Do Not Disturb switch. Inert until M4 gives it a service.
    dnd: Switch,
}

impl Column {
    /// Build the column.
    pub fn new() -> Self {
        let root = gtk4::Box::new(Orientation::Vertical, 0);
        root.add_css_class(classes::CONTROL_PANEL_COLUMN);

        let list = gtk4::Box::new(Orientation::Vertical, 8);
        list.set_vexpand(true);
        list.set_visible(false);

        let empty = gtk4::Box::new(Orientation::Vertical, 12);
        empty.add_css_class(classes::EMPTY_STATE);
        empty.set_vexpand(true);
        empty.set_valign(Align::Center);
        empty.set_halign(Align::Center);

        let icon = Image::from_icon_name(EMPTY_ICON);
        icon.add_css_class(classes::EMPTY_STATE_ICON);
        empty.append(&icon);

        let caption = Label::new(Some("No Notifications"));
        caption.add_css_class(classes::EMPTY_STATE_LABEL);
        empty.append(&caption);

        let dnd_row = gtk4::Box::new(Orientation::Horizontal, 8);
        dnd_row.add_css_class(classes::DND_ROW);

        let dnd_label = Label::new(Some("Do Not Disturb"));
        dnd_label.add_css_class(classes::DND_LABEL);
        dnd_label.set_xalign(0.0);
        dnd_label.set_hexpand(true);

        let dnd = Switch::new();
        dnd.set_valign(Align::Center);
        // Nothing behind it yet, and a switch that flips without doing
        // anything is worse than one that is visibly not ready.
        dnd.set_sensitive(false);

        dnd_row.append(&dnd_label);
        dnd_row.append(&dnd);

        root.append(&list);
        root.append(&empty);
        root.append(&dnd_row);

        Self {
            root,
            list,
            empty,
            dnd,
        }
    }

    /// The widget to put in the panel's left column.
    pub fn root(&self) -> &gtk4::Box {
        &self.root
    }

    /// Re-render from current state.
    ///
    /// Until M4 there is no state, so this only re-asserts which of the list
    /// and the empty state is on screen.
    pub fn refresh(&self) {
        let empty = self.list.first_child().is_none();
        self.list.set_visible(!empty);
        self.empty.set_visible(empty);
        self.dnd.set_sensitive(false);
    }
}
