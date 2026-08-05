//! The control panel's calendar.
//!
//! A fixed six-week grid, always 42 cells, so the panel is exactly as tall in
//! February as in a month that spills over six weeks — GTK's own `GtkCalendar`
//! changes height between months, which makes everything below it jump.
//!
//! Weeks start on Monday and the week numbers are ISO, because the two have to
//! agree: an ISO week number names a Monday-to-Sunday week, so numbering a
//! Sunday-first grid would put the number beside the wrong row.
//!
//! The month arithmetic is pure and lives at the top of this file with its own
//! tests; the widget below only turns dates into labels and classes.

use std::cell::Cell;
use std::rc::Rc;

use chrono::{Datelike, Days, Months, NaiveDate};
use gtk4::prelude::*;
use gtk4::{Align, Button, Grid, Image, Label, Orientation, glib};

use crate::anim::ripple;
use crate::style::classes;
use crate::widgets::set_class;

/// Weeks in the fixed window.
const WEEKS: usize = 6;
/// Cells in the grid.
const CELLS: usize = WEEKS * 7;
/// Column headers, Monday first.
const WEEKDAYS: [&str; 7] = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];
/// Adwaita's chevrons.
const PREV_ICON: &str = "go-previous-symbolic";
/// Adwaita's chevrons.
const NEXT_ICON: &str = "go-next-symbolic";

// ---------------------------------------------------------------------------
// Month arithmetic
// ---------------------------------------------------------------------------

/// A keyboard move inside the grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Left arrow.
    PrevDay,
    /// Right arrow.
    NextDay,
    /// Up arrow.
    PrevWeek,
    /// Down arrow.
    NextWeek,
    /// Page Up.
    PrevMonth,
    /// Page Down.
    NextMonth,
}

/// The first day of `date`'s month.
pub fn month_start(date: NaiveDate) -> NaiveDate {
    date.with_day(1).unwrap_or(date)
}

/// Move `date` by whole months, clamping the day into the shorter month.
///
/// The 31st of March a month back is the 28th of February, not the 3rd of
/// March: paging through the calendar must not skip a month.
pub fn shift_month(date: NaiveDate, months: i32) -> NaiveDate {
    let shifted = if months >= 0 {
        date.checked_add_months(Months::new(months as u32))
    } else {
        date.checked_sub_months(Months::new(months.unsigned_abs()))
    };
    shifted.unwrap_or(date)
}

/// How many days of the previous month lead the grid for `month`.
///
/// Zero when the month starts on a Monday, six when it starts on a Sunday.
pub fn leading_offset(month: NaiveDate) -> u32 {
    month_start(month).weekday().num_days_from_monday()
}

/// The first date in the window showing `month`.
pub fn grid_start(month: NaiveDate) -> NaiveDate {
    let start = month_start(month);
    start
        .checked_sub_days(Days::new(u64::from(leading_offset(start))))
        .unwrap_or(start)
}

/// The 42 dates of the window showing `month`, in reading order.
pub fn grid(month: NaiveDate) -> Vec<NaiveDate> {
    let start = grid_start(month);
    (0..CELLS)
        .map(|offset| {
            start
                .checked_add_days(Days::new(offset as u64))
                .unwrap_or(start)
        })
        .collect()
}

/// The ISO-8601 week number of `date`.
pub fn iso_week(date: NaiveDate) -> u32 {
    date.iso_week().week()
}

/// The header over the grid, e.g. `August 2026`.
pub fn header(month: NaiveDate) -> String {
    month.format("%B %Y").to_string()
}

/// Where `step` moves the selection from `from`.
pub fn step(from: NaiveDate, step: Step) -> NaiveDate {
    match step {
        Step::PrevDay => from.checked_sub_days(Days::new(1)).unwrap_or(from),
        Step::NextDay => from.checked_add_days(Days::new(1)).unwrap_or(from),
        Step::PrevWeek => from.checked_sub_days(Days::new(7)).unwrap_or(from),
        Step::NextWeek => from.checked_add_days(Days::new(7)).unwrap_or(from),
        Step::PrevMonth => shift_month(from, -1),
        Step::NextMonth => shift_month(from, 1),
    }
}

/// Whether two dates fall in the same month.
fn same_month(left: NaiveDate, right: NaiveDate) -> bool {
    left.year() == right.year() && left.month() == right.month()
}

// ---------------------------------------------------------------------------
// The widget
// ---------------------------------------------------------------------------

/// The calendar.
pub struct Calendar {
    root: gtk4::Box,
    title: Label,
    cells: Vec<Button>,
    weeks: Vec<Label>,
    /// The month on screen, as its first day.
    view: Cell<NaiveDate>,
    /// The day with the accent ring.
    selected: Cell<NaiveDate>,
    /// The day with the accent circle.
    today: Cell<NaiveDate>,
}

impl Calendar {
    /// Build a calendar showing `today`.
    pub fn new(today: NaiveDate, show_week_numbers: bool) -> Rc<Self> {
        let root = gtk4::Box::new(Orientation::Vertical, 6);
        root.add_css_class(classes::CALENDAR);

        let title = Label::new(None);
        title.add_css_class(classes::CALENDAR_TITLE);

        let grid_widget = Grid::new();
        grid_widget.add_css_class(classes::CALENDAR_GRID);
        grid_widget.set_row_homogeneous(true);
        grid_widget.set_column_homogeneous(false);
        grid_widget.set_halign(Align::Center);

        // The week-number column shifts the days right by one when it is on.
        let day_column = i32::from(show_week_numbers);

        for (index, weekday) in WEEKDAYS.iter().enumerate() {
            let label = Label::new(Some(weekday));
            label.add_css_class(classes::CALENDAR_WEEKDAY);
            grid_widget.attach(&label, day_column + index as i32, 0, 1, 1);
        }

        let weeks: Vec<Label> = (0..WEEKS)
            .map(|week| {
                let label = Label::new(None);
                label.add_css_class(classes::CALENDAR_WEEK);
                if show_week_numbers {
                    grid_widget.attach(&label, 0, week as i32 + 1, 1, 1);
                }
                label
            })
            .collect();

        let cells: Vec<Button> = (0..CELLS)
            .map(|index| {
                let button = Button::new();
                button.add_css_class(classes::CALENDAR_DAY);
                button.set_has_frame(false);
                let column = day_column + (index % 7) as i32;
                let row = (index / 7) as i32 + 1;
                grid_widget.attach(&button, column, row, 1, 1);
                button
            })
            .collect();

        let header_row = gtk4::Box::new(Orientation::Horizontal, 0);
        header_row.add_css_class(classes::CALENDAR_HEADER);
        let previous = chevron(PREV_ICON, "Previous month");
        let next = chevron(NEXT_ICON, "Next month");

        // The title is a button too: clicking the month name comes back to
        // today, which is the only way back once you have paged away.
        let home = Button::new();
        home.add_css_class(classes::CALENDAR_MONTH);
        home.set_has_frame(false);
        home.set_hexpand(true);
        home.set_child(Some(&title));
        ripple::install(&home);
        home.set_tooltip_text(Some("Back to today"));

        header_row.append(&previous);
        header_row.append(&home);
        header_row.append(&next);

        root.append(&header_row);
        root.append(&grid_widget);

        let calendar = Rc::new(Self {
            root,
            title,
            cells,
            weeks,
            view: Cell::new(month_start(today)),
            selected: Cell::new(today),
            today: Cell::new(today),
        });

        calendar.connect(&previous, Step::PrevMonth);
        calendar.connect(&next, Step::NextMonth);
        home.connect_clicked({
            let calendar = Rc::downgrade(&calendar);
            move |_| {
                if let Some(calendar) = calendar.upgrade() {
                    calendar.go_to_today();
                }
            }
        });
        calendar.connect_days();
        calendar.connect_keys();
        calendar.render();
        calendar
    }

    /// The widget to put in the panel's right column.
    pub fn root(&self) -> &gtk4::Box {
        &self.root
    }

    /// Show `today`'s month with `today` selected.
    ///
    /// Called on every open: a calendar left on last March is not what anyone
    /// wants to see when they check the date, and the panel may have been
    /// built on the other side of midnight.
    pub fn reset(&self, today: NaiveDate) {
        self.today.set(today);
        self.selected.set(today);
        self.view.set(month_start(today));
        self.render();
    }

    /// Draw the header, the week numbers, and all 42 cells.
    fn render(&self) {
        let view = self.view.get();
        let today = self.today.get();
        let selected = self.selected.get();

        set_text(&self.title, &header(view));

        let days = grid(view);
        for (week, label) in self.weeks.iter().enumerate() {
            set_text(&label.clone(), &iso_week(days[week * 7]).to_string());
        }

        for (cell, date) in self.cells.iter().zip(days) {
            let text = date.day().to_string();
            if cell.label().as_deref() != Some(text.as_str()) {
                cell.set_label(&text);
            }
            set_class(cell, classes::CALENDAR_OUTSIDE, !same_month(date, view));
            set_class(cell, classes::CALENDAR_TODAY, date == today);
            set_class(cell, classes::CALENDAR_SELECTED, date == selected);
        }
    }

    /// Select `date`, following it into another month if it is not on screen.
    fn select(&self, date: NaiveDate) {
        self.selected.set(date);
        self.view.set(month_start(date));
        self.render();
    }

    /// Move the selection and keep the keyboard on it.
    fn step(&self, step: Step) {
        let target = self::step(self.selected.get(), step);
        self.select(target);
        self.focus_selected();
    }

    /// Come back to the current month and day.
    fn go_to_today(&self) {
        self.select(self.today.get());
    }

    /// Put the keyboard on whichever cell is selected.
    fn focus_selected(&self) {
        let selected = self.selected.get();
        let Some(index) = grid(self.view.get())
            .iter()
            .position(|date| *date == selected)
        else {
            return;
        };
        if let Some(cell) = self.cells.get(index) {
            cell.grab_focus();
        }
    }

    /// Wire a chevron to a month step. Chevrons page the *view*, and take the
    /// selection with them so the keyboard stays where the eye is.
    fn connect(self: &Rc<Self>, button: &Button, direction: Step) {
        button.connect_clicked({
            let calendar = Rc::downgrade(self);
            move |_| {
                if let Some(calendar) = calendar.upgrade() {
                    let view = calendar.view.get();
                    let months = if direction == Step::NextMonth { 1 } else { -1 };
                    calendar.select(shift_month(view, months));
                }
            }
        });
    }

    /// Clicking a day selects it — including a day belonging to the month
    /// either side, which navigates there the way GNOME's calendar does.
    fn connect_days(self: &Rc<Self>) {
        for (index, cell) in self.cells.iter().enumerate() {
            cell.connect_clicked({
                let calendar = Rc::downgrade(self);
                move |_| {
                    if let Some(calendar) = calendar.upgrade()
                        && let Some(date) = grid(calendar.view.get()).get(index).copied()
                    {
                        calendar.select(date);
                    }
                }
            });
        }
    }

    /// Arrows walk the grid, Page Up/Down change month.
    ///
    /// The controller captures, so the grid's own arrow-key focus movement
    /// never gets a chance to disagree with the selection.
    fn connect_keys(self: &Rc<Self>) {
        let keys = gtk4::EventControllerKey::new();
        keys.set_propagation_phase(gtk4::PropagationPhase::Capture);
        keys.connect_key_pressed({
            let calendar = Rc::downgrade(self);
            move |_, key, _, _| {
                let step = match key {
                    gtk4::gdk::Key::Left => Step::PrevDay,
                    gtk4::gdk::Key::Right => Step::NextDay,
                    gtk4::gdk::Key::Up => Step::PrevWeek,
                    gtk4::gdk::Key::Down => Step::NextWeek,
                    gtk4::gdk::Key::Page_Up => Step::PrevMonth,
                    gtk4::gdk::Key::Page_Down => Step::NextMonth,
                    _ => return glib::Propagation::Proceed,
                };
                let Some(calendar) = calendar.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                calendar.step(step);
                glib::Propagation::Stop
            }
        });
        self.root.add_controller(keys);
    }
}

/// A flat icon button for stepping a month.
fn chevron(icon: &str, tooltip: &str) -> Button {
    let button = Button::new();
    button.add_css_class(classes::CALENDAR_NAV);
    button.set_has_frame(false);
    button.set_child(Some(&Image::from_icon_name(icon)));
    ripple::install(&button);
    button.set_tooltip_text(Some(tooltip));
    button.set_valign(Align::Center);
    button
}

/// Set a label only when it changed.
fn set_text(label: &Label, text: &str) {
    if label.text() != text {
        label.set_text(text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).expect("a real date")
    }

    #[test]
    fn the_month_start_keeps_the_month() {
        assert_eq!(month_start(date(2026, 8, 4)), date(2026, 8, 1));
        assert_eq!(month_start(date(2026, 8, 31)), date(2026, 8, 1));
    }

    #[test]
    fn august_2026_starts_on_a_saturday() {
        // Five leading days of July: Mon 27th through Fri 31st.
        assert_eq!(leading_offset(date(2026, 8, 1)), 5);
        assert_eq!(grid_start(date(2026, 8, 15)), date(2026, 7, 27));
    }

    #[test]
    fn a_month_starting_on_monday_has_no_leading_days() {
        // 2026-06-01 is a Monday.
        assert_eq!(leading_offset(date(2026, 6, 1)), 0);
        assert_eq!(grid_start(date(2026, 6, 1)), date(2026, 6, 1));
    }

    #[test]
    fn a_month_starting_on_sunday_leads_with_a_full_week() {
        // 2026-02-01 is a Sunday, the worst case for a Monday-first grid.
        assert_eq!(leading_offset(date(2026, 2, 1)), 6);
        assert_eq!(grid_start(date(2026, 2, 1)), date(2026, 1, 26));
    }

    #[test]
    fn every_month_fits_in_the_same_six_weeks() {
        let mut month = date(2024, 1, 1);
        for _ in 0..48 {
            let days = grid(month);
            assert_eq!(days.len(), CELLS, "{month} is not 42 cells");

            // The window has to contain the whole month, or a day would be
            // missing from the calendar entirely.
            let last = shift_month(month, 1)
                .checked_sub_days(Days::new(1))
                .expect("a real date");
            assert!(days.contains(&month), "{month} misses its first day");
            assert!(days.contains(&last), "{month} misses its last day");

            // And it must be contiguous, Monday-aligned.
            assert_eq!(days[0].weekday().num_days_from_monday(), 0);
            for pair in days.windows(2) {
                assert_eq!(pair[1] - pair[0], chrono::Duration::days(1));
            }
            month = shift_month(month, 1);
        }
    }

    #[test]
    fn paging_a_month_clamps_into_the_shorter_one() {
        assert_eq!(shift_month(date(2026, 3, 31), -1), date(2026, 2, 28));
        assert_eq!(shift_month(date(2026, 1, 31), 1), date(2026, 2, 28));
        assert_eq!(shift_month(date(2024, 1, 31), 1), date(2024, 2, 29));
    }

    #[test]
    fn paging_a_month_crosses_the_year() {
        assert_eq!(shift_month(date(2026, 1, 15), -1), date(2025, 12, 15));
        assert_eq!(shift_month(date(2026, 12, 15), 1), date(2027, 1, 15));
    }

    #[test]
    fn week_numbers_follow_iso_rules() {
        // 2026 opens on a Thursday, so the 1st is already in week 1.
        assert_eq!(iso_week(date(2026, 1, 1)), 1);
        // 2026 is a 53-week year, and the first days of 2027 belong to its
        // last week — which is exactly the case a naive day-of-year gets
        // wrong.
        assert_eq!(iso_week(date(2027, 1, 1)), 53);
        assert_eq!(iso_week(date(2026, 8, 4)), 32);
    }

    #[test]
    fn the_week_column_numbers_the_row_it_sits_beside() {
        let days = grid(date(2026, 8, 1));
        let numbers: Vec<u32> = (0..WEEKS).map(|week| iso_week(days[week * 7])).collect();
        assert_eq!(numbers, vec![31, 32, 33, 34, 35, 36]);
    }

    #[test]
    fn arrows_walk_a_day_and_a_week_at_a_time() {
        let start = date(2026, 8, 4);
        assert_eq!(step(start, Step::PrevDay), date(2026, 8, 3));
        assert_eq!(step(start, Step::NextDay), date(2026, 8, 5));
        assert_eq!(step(start, Step::PrevWeek), date(2026, 7, 28));
        assert_eq!(step(start, Step::NextWeek), date(2026, 8, 11));
    }

    #[test]
    fn arrows_cross_month_and_year_boundaries() {
        assert_eq!(step(date(2026, 8, 1), Step::PrevDay), date(2026, 7, 31));
        assert_eq!(step(date(2026, 8, 31), Step::NextDay), date(2026, 9, 1));
        assert_eq!(step(date(2026, 1, 1), Step::PrevDay), date(2025, 12, 31));
        assert_eq!(step(date(2026, 12, 31), Step::NextDay), date(2027, 1, 1));
    }

    #[test]
    fn paging_keeps_the_day_where_it_can() {
        assert_eq!(step(date(2026, 8, 4), Step::PrevMonth), date(2026, 7, 4));
        assert_eq!(step(date(2026, 8, 4), Step::NextMonth), date(2026, 9, 4));
        assert_eq!(step(date(2026, 3, 31), Step::PrevMonth), date(2026, 2, 28));
    }

    #[test]
    fn the_header_names_the_month_and_year() {
        assert_eq!(header(date(2026, 8, 1)), "August 2026");
        assert_eq!(header(date(2025, 12, 31)), "December 2025");
    }

    #[test]
    fn a_days_month_decides_whether_it_is_dimmed() {
        let view = date(2026, 8, 1);
        assert!(!same_month(date(2026, 7, 31), view));
        assert!(same_month(date(2026, 8, 31), view));
        assert!(!same_month(date(2026, 9, 1), view));
        assert!(
            !same_month(date(2025, 8, 1), view),
            "same month, other year"
        );
    }
}
