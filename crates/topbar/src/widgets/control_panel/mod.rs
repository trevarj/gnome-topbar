//! The clock's control panel — GNOME's date menu.
//!
//! Two fixed-width columns: notifications on the left, and on the right the
//! time, the calendar, and the weather forecast. The widths are fixed and the
//! cards are all present from the first frame, including the ones later
//! milestones fill in, because a panel that reflows as its contents arrive
//! reads as broken.
//!
//! The panel is built once and kept for the clock's lifetime; every open calls
//! [`ControlPanel::refresh`], so nothing it shows can be older than the click
//! that opened it.

mod calendar;
mod media;
mod notifications;
mod world_clock;

use std::rc::Rc;

use chrono::{DateTime, Local, Utc};
use gtk4::prelude::*;
use gtk4::{Align, Label, Orientation};
use topbar_core::config::ClockConfig;
use topbar_services::Services;

use crate::style::classes;
use crate::surfaces::popovers::PopoverContent;
use crate::widgets::clock::MinuteListener;
use crate::widgets::control_panel::calendar::Calendar;

/// Width of the notifications column, in pixels.
const LEFT_WIDTH: i32 = 380;
/// Width of the time/calendar/weather column, in pixels.
const RIGHT_WIDTH: i32 = 360;
/// Gap between the columns and the divider between them.
const COLUMN_GAP: i32 = 12;
/// Vertical gap between the cards in a column.
const CARD_GAP: i32 = 10;
/// Height reserved for the forecast M6 will draw, so the column is already
/// its final size.
const FORECAST_HEIGHT: i32 = 110;

/// Format of the large local time.
const TIME_FORMAT: &str = "%H:%M";
/// Format of the date under it.
const DATE_FORMAT: &str = "%A, %B %-d";

/// The control panel.
pub struct ControlPanel {
    root: gtk4::Box,
    time: Label,
    date: Label,
    clocks: Vec<world_clock::Row>,
    media: Rc<media::Card>,
    calendar: Rc<Calendar>,
    notifications: Rc<notifications::Column>,
}

impl ControlPanel {
    /// Build the panel from `[widgets.clock]`.
    pub fn new(config: &ClockConfig, services: &Services) -> Rc<Self> {
        let root = gtk4::Box::new(Orientation::Horizontal, COLUMN_GAP);
        root.add_css_class(classes::CONTROL_PANEL);
        root.set_size_request(LEFT_WIDTH + 2 * COLUMN_GAP + 1 + RIGHT_WIDTH, -1);

        let notifications = notifications::Column::new(services);
        let left = notifications.root();
        left.set_size_request(LEFT_WIDTH, -1);
        left.set_hexpand(false);

        let divider = gtk4::Box::new(Orientation::Vertical, 0);
        divider.add_css_class(classes::CONTROL_PANEL_DIVIDER);
        divider.set_size_request(1, -1);
        divider.set_vexpand(true);

        let right = gtk4::Box::new(Orientation::Vertical, CARD_GAP);
        right.add_css_class(classes::CONTROL_PANEL_COLUMN);
        right.set_size_request(RIGHT_WIDTH, -1);
        right.set_hexpand(false);
        right.set_valign(Align::Start);

        let time = Label::new(None);
        time.add_css_class(classes::CONTROL_PANEL_TIME);
        time.set_xalign(0.0);

        let date = Label::new(None);
        date.add_css_class(classes::CONTROL_PANEL_DATE);
        date.set_xalign(0.0);

        let clocks: Vec<world_clock::Row> = world_clock::resolve(&config.world_clocks)
            .into_iter()
            .map(world_clock::Row::new)
            .collect();

        let calendar = Calendar::new(Local::now().date_naive(), config.show_week_numbers);
        calendar.root().add_css_class(classes::CARD);

        // Media sits between the time and the calendar, where GNOME puts it,
        // and is the one card in the column that comes and goes: it is hidden
        // outright while no player is on the bus.
        let media = media::Card::new(services);

        right.append(&time_card(&time, &date, &clocks));
        right.append(media.root());
        right.append(calendar.root());
        right.append(&forecast_card());

        root.append(left);
        root.append(&divider);
        root.append(&right);

        Rc::new(Self {
            root,
            time,
            date,
            clocks,
            media,
            calendar,
            notifications,
        })
    }

    /// Re-render everything for `now`.
    fn render(&self, now: DateTime<Local>) {
        set_text(&self.time, &now.format(TIME_FORMAT).to_string());
        set_text(&self.date, &now.format(DATE_FORMAT).to_string());

        let utc = now.with_timezone(&Utc);
        for clock in &self.clocks {
            clock.render(utc);
        }
    }
}

impl PopoverContent for ControlPanel {
    fn root(&self) -> gtk4::Widget {
        self.root.clone().upcast()
    }

    fn refresh(&self) {
        let now = Local::now();
        self.render(now);
        // Back to this month with today selected: a calendar left on last
        // March is not what anyone opens the date menu to see, and the panel
        // may have been built on the other side of midnight.
        self.calendar.reset(now.date_naive());
        self.notifications.refresh();
        self.media.refresh();
    }

    fn closed(&self) {
        // Nothing is looking at the playback position any more, so nothing
        // should be asking a player for it.
        self.media.closed();
    }
}

impl MinuteListener for ControlPanel {
    fn on_minute(&self, now: DateTime<Local>) {
        self.render(now);
        // "5m ago" is only true for a minute, and the history is the one place
        // in the panel where that shows.
        self.notifications.retime(now);
    }
}

/// The card carrying the local time, the date, and the world clocks.
fn time_card(time: &Label, date: &Label, clocks: &[world_clock::Row]) -> gtk4::Box {
    let card = gtk4::Box::new(Orientation::Vertical, 2);
    card.add_css_class(classes::CARD);
    card.append(time);
    card.append(date);

    if clocks.is_empty() {
        return card;
    }

    let zones = gtk4::Box::new(Orientation::Vertical, 4);
    zones.set_margin_top(10);
    for clock in clocks {
        zones.append(clock.root());
    }
    card.append(&zones);
    card
}

/// The forecast card M6 fills in.
///
/// It is here now, at its final size, so adding the forecast later does not
/// move the calendar above it.
fn forecast_card() -> gtk4::Box {
    let card = gtk4::Box::new(Orientation::Vertical, 6);
    card.add_css_class(classes::CARD);

    let title = Label::new(Some("Weather"));
    title.add_css_class(classes::CARD_TITLE);
    title.set_xalign(0.0);

    let body = Label::new(Some("No forecast yet"));
    body.add_css_class(classes::PLACEHOLDER);
    body.set_xalign(0.0);
    body.set_valign(Align::Start);
    body.set_size_request(-1, FORECAST_HEIGHT);

    card.append(&title);
    card.append(&body);
    card
}

/// Set a label only when the text actually changed: a needless `set_text`
/// costs a relayout of the whole panel.
fn set_text(label: &Label, text: &str) {
    if label.text() != text {
        label.set_text(text);
    }
}
