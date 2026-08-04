//! The control panel's notifications column: GNOME's message list.
//!
//! ```text
//! header        "Notifications" + Clear, only while there is something to clear
//! scroll
//! └── list
//!     └── group   app icon, name, count, chevron, clear
//!         └── rows  summary, body, age, close   (while expanded)
//! empty         the designed state for a column with nothing in it
//! dnd           Do Not Disturb
//! ```
//!
//! Expansion is per-open state, which is what v1 did and what GNOME does: the
//! flag lives on the row widget, and the rows are rebuilt whenever the history
//! changes or the panel is reopened, so a group the user opened last time comes
//! back collapsed. The list is a place to catch up, not a tree to navigate.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use chrono::{DateTime, Local};
use gtk4::prelude::*;
use gtk4::{Align, Button, Image, Label, Orientation, PolicyType, ScrolledWindow, Switch, pango};
use topbar_services::{CloseReason, GroupView, NotifState, NotificationView, Services};

use crate::bridge::{self, ActionScope, BindingGuard};
use crate::style::classes;
use crate::widgets::notifications::{ROW_ICON, absolute_time, icon, markup, relative_time};

/// Adwaita's bell. There is no plain `notifications-symbolic` in Adwaita 50,
/// and `notifications-disabled-symbolic` would read as "DND is on" rather than
/// "nothing has arrived".
const EMPTY_ICON: &str = "preferences-system-notifications-symbolic";
/// Tallest the list grows before it starts scrolling.
const MAX_LIST_HEIGHT: i32 = 460;
/// Where this column's failures are reported.
const SCOPE: ActionScope = ActionScope::Toast {
    widget: "notifications",
};

/// The notifications column.
pub struct Column {
    root: gtk4::Box,
    /// Header title and Clear, hidden while there is nothing to clear.
    header: gtk4::Box,
    clear_all: Button,
    /// The history, one child per application group.
    list: gtk4::Box,
    /// Shown while [`Column::list`] is empty.
    empty: gtk4::Box,
    /// Do Not Disturb.
    dnd: Switch,
    /// Set while the switch is being driven from a snapshot, so echoing the
    /// service's own state back at it does not look like a user toggle.
    syncing: Rc<Cell<bool>>,
    /// Every age label on screen, with the moment it is counting from.
    ///
    /// Retained rather than searched for: the minute tick has to be cheap, and
    /// walking the widget tree once a minute to find labels by class would be
    /// both slower and easier to get wrong.
    ages: RefCell<Vec<(i64, Label)>>,
    services: Services,
    binding: RefCell<Option<BindingGuard>>,
}

impl Column {
    /// Build the column and subscribe it to the daemon.
    pub fn new(services: &Services) -> Rc<Self> {
        let root = gtk4::Box::new(Orientation::Vertical, 0);
        root.add_css_class(classes::CONTROL_PANEL_COLUMN);

        let header = gtk4::Box::new(Orientation::Horizontal, 8);
        header.add_css_class(classes::NOTIFICATION_HEADER);

        let title = Label::new(Some("Notifications"));
        title.add_css_class(classes::CARD_TITLE);
        title.set_xalign(0.0);
        title.set_hexpand(true);
        header.append(&title);

        let clear_all = Button::with_label("Clear");
        clear_all.add_css_class(classes::NOTIFICATION_CLEAR_ALL);
        clear_all.set_focus_on_click(false);
        header.append(&clear_all);

        let list = gtk4::Box::new(Orientation::Vertical, 8);
        list.add_css_class(classes::NOTIFICATION_LIST);

        let scroll = ScrolledWindow::new();
        scroll.set_policy(PolicyType::Never, PolicyType::Automatic);
        scroll.set_propagate_natural_height(true);
        scroll.set_max_content_height(MAX_LIST_HEIGHT);
        scroll.set_vexpand(true);
        scroll.set_child(Some(&list));

        let empty = gtk4::Box::new(Orientation::Vertical, 12);
        empty.add_css_class(classes::EMPTY_STATE);
        empty.set_vexpand(true);
        empty.set_valign(Align::Center);
        empty.set_halign(Align::Center);

        let placeholder = Image::from_icon_name(EMPTY_ICON);
        placeholder.add_css_class(classes::EMPTY_STATE_ICON);
        empty.append(&placeholder);

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

        dnd_row.append(&dnd_label);
        dnd_row.append(&dnd);

        root.append(&header);
        root.append(&scroll);
        root.append(&empty);
        root.append(&dnd_row);

        let column = Rc::new(Self {
            root,
            header,
            clear_all,
            list,
            empty,
            dnd,
            syncing: Rc::new(Cell::new(false)),
            ages: RefCell::new(Vec::new()),
            services: services.clone(),
            binding: RefCell::new(None),
        });

        column.clear_all.connect_clicked({
            let handle = services.notifications.handle().clone();
            move |_| {
                let handle = handle.clone();
                bridge::act(SCOPE, async move { handle.clear_all().await });
            }
        });

        column.dnd.connect_state_set({
            let handle = services.notifications.handle().clone();
            let syncing = Rc::clone(&column.syncing);
            move |_, wanted| {
                if !syncing.get() {
                    let handle = handle.clone();
                    bridge::act(SCOPE, async move { handle.set_dnd(wanted).await });
                }
                gtk4::glib::Propagation::Proceed
            }
        });

        let binding = bridge::bind_state(&column.root, services.notifications.state(), {
            let column = Rc::downgrade(&column);
            move |_, state| {
                if let Some(column) = column.upgrade() {
                    column.render(state);
                }
            }
        });
        *column.binding.borrow_mut() = Some(binding);

        column
    }

    /// The widget to put in the panel's left column.
    pub fn root(&self) -> &gtk4::Box {
        &self.root
    }

    /// Re-render from current state.
    ///
    /// Called on every open as well as on every change, so a panel that has
    /// been sitting closed for an hour never shows an hour-old list — and the
    /// ages in it are recomputed at the same moment.
    pub fn refresh(&self) {
        let receiver = self.services.notifications.state();
        let state = receiver.borrow().clone();
        self.render(&state);

        // Opening the panel is what "seen" means.
        let handle = self.services.notifications.handle().clone();
        bridge::act(SCOPE, async move { handle.mark_seen().await });
    }

    /// Re-time every row, on the clock's minute tick.
    pub fn retime(&self, now: DateTime<Local>) {
        for (timestamp, label) in self.ages.borrow().iter() {
            let text = relative_time(*timestamp, now);
            if label.text() != text {
                label.set_text(&text);
            }
        }
    }

    /// Draw `state`.
    fn render(&self, state: &NotifState) {
        self.syncing.set(true);
        if self.dnd.is_active() != state.dnd {
            self.dnd.set_active(state.dnd);
        }
        self.syncing.set(false);

        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        self.ages.borrow_mut().clear();

        let now = Local::now();
        for group in &state.history {
            self.list.append(&self.build_group(group, now));
        }

        let empty = state.history.is_empty();
        self.list.set_visible(!empty);
        // A Clear button over an empty list is a button that does nothing.
        self.header.set_visible(!empty);
        self.empty.set_visible(empty);
    }

    /// One application's notifications, as a collapsible card.
    fn build_group(&self, group: &GroupView, now: DateTime<Local>) -> gtk4::Box {
        let card = gtk4::Box::new(Orientation::Vertical, 0);
        card.add_css_class(classes::CARD);
        card.add_css_class(classes::NOTIFICATION_GROUP);

        let rows = gtk4::Box::new(Orientation::Vertical, 4);
        rows.add_css_class(classes::NOTIFICATION_GROUP_LIST);
        for notification in &group.notifications {
            rows.append(&self.build_row(notification, now));
        }

        let chevron = Image::from_icon_name("pan-end-symbolic");
        chevron.add_css_class(classes::NOTIFICATION_CHEVRON);
        chevron.set_valign(Align::Center);

        // The whole header is the expander, so hitting the app name works.
        let expander = Button::new();
        expander.add_css_class(classes::NOTIFICATION_GROUP_HEADER);
        expander.set_focus_on_click(false);
        expander.set_hexpand(true);

        let content = gtk4::Box::new(Orientation::Horizontal, 8);
        let app_icon = icon::image(&group.newest().icon, ROW_ICON);
        app_icon.add_css_class(classes::NOTIFICATION_ICON);
        app_icon.set_valign(Align::Center);
        content.append(&app_icon);

        let name = Label::new(Some(&group.app_name));
        name.add_css_class(classes::NOTIFICATION_APP);
        name.set_xalign(0.0);
        name.set_hexpand(true);
        name.set_ellipsize(pango::EllipsizeMode::End);
        content.append(&name);

        if group.count() > 1 {
            let count = Label::new(Some(&group.count().to_string()));
            count.add_css_class(classes::NOTIFICATION_COUNT);
            count.set_valign(Align::Center);
            content.append(&count);
        }

        content.append(&chevron);
        expander.set_child(Some(&content));

        let header = gtk4::Box::new(Orientation::Horizontal, 2);
        header.append(&expander);

        let clear = Button::from_icon_name("user-trash-symbolic");
        clear.add_css_class(classes::NOTIFICATION_GROUP_CLEAR);
        clear.set_focus_on_click(false);
        clear.set_valign(Align::Center);
        clear.set_tooltip_text(Some(&format!("Clear {}", group.app_name)));
        clear.connect_clicked({
            let handle = self.services.notifications.handle().clone();
            let key = group.key.clone();
            move |button| {
                // The card is about to go; a second click on the way out would
                // close notifications that arrived in between.
                button.set_sensitive(false);
                let handle = handle.clone();
                let key = key.clone();
                bridge::act(SCOPE, async move { handle.clear_group(key).await });
            }
        });
        header.append(&clear);

        // A group of one is its own summary, so it is simply open: expanding
        // it would show the same line again.
        if group.count() == 1 {
            chevron.set_visible(false);
            expander.set_sensitive(false);
            expander.add_css_class(classes::NOTIFICATION_GROUP_SINGLE);
        } else {
            rows.set_visible(false);
            expander.connect_clicked({
                let rows = rows.clone();
                let chevron = chevron.clone();
                move |_| {
                    let expanded = !rows.is_visible();
                    rows.set_visible(expanded);
                    chevron.set_icon_name(Some(if expanded {
                        "pan-down-symbolic"
                    } else {
                        "pan-end-symbolic"
                    }));
                }
            });
        }

        card.append(&header);
        card.append(&rows);
        card
    }

    /// One notification inside a group.
    fn build_row(&self, notification: &NotificationView, now: DateTime<Local>) -> gtk4::Box {
        let row = gtk4::Box::new(Orientation::Horizontal, 8);
        row.add_css_class(classes::NOTIFICATION_ROW);

        let text = gtk4::Box::new(Orientation::Vertical, 2);
        text.set_hexpand(true);

        let top = gtk4::Box::new(Orientation::Horizontal, 6);
        let summary = Label::new(Some(&notification.summary));
        summary.add_css_class(classes::NOTIFICATION_SUMMARY);
        summary.set_xalign(0.0);
        summary.set_hexpand(true);
        summary.set_ellipsize(pango::EllipsizeMode::End);
        summary.set_single_line_mode(true);
        top.append(&summary);

        let age = Label::new(Some(&relative_time(notification.timestamp, now)));
        age.add_css_class(classes::NOTIFICATION_TIME);
        age.set_valign(Align::Start);
        age.set_tooltip_text(Some(&absolute_time(notification.timestamp)));
        self.ages
            .borrow_mut()
            .push((notification.timestamp, age.clone()));
        top.append(&age);
        text.append(&top);

        if !notification.body.is_empty() {
            let body = Label::new(None);
            body.add_css_class(classes::NOTIFICATION_BODY);
            body.set_xalign(0.0);
            body.set_wrap(true);
            body.set_wrap_mode(pango::WrapMode::WordChar);
            body.set_lines(2);
            body.set_ellipsize(pango::EllipsizeMode::End);
            markup::apply(&body, &notification.body);
            text.append(&body);
        }
        row.append(&text);

        let close = Button::from_icon_name("window-close-symbolic");
        close.add_css_class(classes::NOTIFICATION_CLOSE);
        close.set_focus_on_click(false);
        close.set_valign(Align::Start);
        close.connect_clicked({
            let handle = self.services.notifications.handle().clone();
            let id = notification.id;
            move |button| {
                button.set_sensitive(false);
                let handle = handle.clone();
                bridge::act(SCOPE, async move {
                    handle.dismiss(id, CloseReason::Dismissed).await
                });
            }
        });
        row.append(&close);

        row
    }
}
