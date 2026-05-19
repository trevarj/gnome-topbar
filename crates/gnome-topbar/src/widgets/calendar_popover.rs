use std::cell::{Cell, RefCell};
use std::rc::Rc;

use chrono::{Datelike, Local, NaiveDate};
use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Calendar, Label, Orientation, Overlay, Widget};

use crate::services::icons::IconsService;
use crate::services::tooltip::TooltipManager;
use crate::styles::{calendar as cal, icon, surface};

fn month_start(date: NaiveDate) -> NaiveDate {
    NaiveDate::from_ymd_opt(date.year(), date.month(), 1)
        .expect("existing NaiveDate month must have a first day")
}

fn shift_month(date: NaiveDate, delta_months: i32) -> Option<NaiveDate> {
    let month_index = date.year().checked_mul(12)? + date.month0() as i32;
    let shifted = month_index.checked_add(delta_months)?;
    let year = shifted.div_euclid(12);
    let month0 = shifted.rem_euclid(12) as u32;

    NaiveDate::from_ymd_opt(year, month0 + 1, 1)
}

fn header_text(date: NaiveDate) -> String {
    date.format("%B %Y").to_string()
}

fn same_month(left: NaiveDate, right: NaiveDate) -> bool {
    left.year() == right.year() && left.month() == right.month()
}

/// Build a calendar popover for the clock widget.
///
/// Shows a month view calendar with custom previous/next navigation, a
/// "go to today" button, and a header label. Toggles a `show-today` CSS class
/// when the currently viewed month matches the real current month.
///
/// Returns the widget and a refresh callback. The refresh callback navigates
/// the calendar to the real current date — call it on each open so the user
/// always sees today's month, even when the widget is reused across cycles.
pub fn build_clock_calendar_popover(show_week_numbers: bool) -> (Widget, Rc<dyn Fn()>) {
    // "Today" is stored in a Cell so the on_show refresh callback can update
    // it when the popover is reused across midnight boundaries.
    let today = Rc::new(Cell::new(Local::now().date_naive()));
    let current_date = Rc::new(RefCell::new(today.get()));
    // Flag to prevent signal handler from interfering during programmatic updates
    let updating = Rc::new(Cell::new(false));

    // Main container
    let container = GtkBox::new(Orientation::Vertical, 0);
    container.add_css_class(cal::POPOVER);

    // Header: left-aligned label + right-aligned navigation buttons
    let header_box = GtkBox::new(Orientation::Horizontal, 8);
    header_box.add_css_class(cal::HEADER);

    // Month/year label - left-aligned, expands to push nav buttons right
    let header_label = Label::new(None);
    header_label.add_css_class(surface::POPOVER_TITLE);
    header_label.set_valign(Align::Center);
    header_label.set_hexpand(true);
    header_label.set_xalign(0.0);

    header_box.append(&header_label);

    // Navigation button group: [prev] [today] [next]
    let nav_box = GtkBox::new(Orientation::Horizontal, 0);
    nav_box.set_valign(Align::Start);

    let prev_button = crate::widgets::base::vp_button_from_icon_name("go-previous-symbolic");
    prev_button.add_css_class(surface::POPOVER_ICON_BTN);
    prev_button.add_css_class(cal::NAV_BUTTON);
    prev_button.set_has_frame(false);
    prev_button.set_focus_on_click(false);

    let icons = IconsService::global();
    let today_icon = icons.create_icon("calendar-today", &[icon::ICON]);
    today_icon.widget().set_halign(Align::Center);
    today_icon.widget().set_valign(Align::Center);
    let today_button = crate::widgets::base::vp_button();
    today_button.set_child(Some(&today_icon.widget()));
    today_button.add_css_class(surface::POPOVER_ICON_BTN);
    today_button.add_css_class(cal::NAV_BUTTON);
    today_button.set_has_frame(false);
    today_button.set_focus_on_click(false);
    TooltipManager::global().set_styled_tooltip(&today_button, "Go to today");

    let next_button = crate::widgets::base::vp_button_from_icon_name("go-next-symbolic");
    next_button.add_css_class(surface::POPOVER_ICON_BTN);
    next_button.add_css_class(cal::NAV_BUTTON);
    next_button.set_has_frame(false);
    next_button.set_focus_on_click(false);

    nav_box.append(&prev_button);
    nav_box.append(&today_button);
    nav_box.append(&next_button);

    header_box.append(&nav_box);
    container.append(&header_box);

    // Calendar widget
    let calendar = Calendar::new();
    calendar.set_show_heading(false);
    calendar.set_show_week_numbers(show_week_numbers);
    calendar.add_css_class(cal::WIDGET);
    calendar.add_css_class(cal::GRID);
    calendar.set_halign(Align::Fill);
    // Initially show today styling since we start in the current month
    calendar.add_css_class(cal::SHOW_TODAY);

    // Wrapper to center the calendar+overlay in the popover
    let wrapper = GtkBox::new(Orientation::Vertical, 0);
    wrapper.set_halign(Align::Center);

    if show_week_numbers {
        // Week number header "w"
        // We use an Overlay to position the "w" label precisely over the top-left corner
        // of the calendar, aligning it with the week number column.
        let overlay = Overlay::new();
        overlay.set_child(Some(&calendar));

        let w_label = Label::new(Some("w"));
        w_label.add_css_class("week-number-header");
        w_label.set_halign(Align::Start);
        w_label.set_valign(Align::Start);

        overlay.add_overlay(&w_label);
        wrapper.append(&overlay);
    } else {
        // No week numbers, just append calendar directly
        wrapper.append(&calendar);
    }

    container.append(&wrapper);

    // Helper closures --------------------------------------------------------

    // Update header label text from a NaiveDate (Month YYYY).
    let update_header = {
        let header_label = header_label.clone();
        move |date: NaiveDate| {
            header_label.set_label(&header_text(date));
        }
    };

    // Sync the GtkCalendar display and the `show-today` CSS class based on
    // the logical date representing the visible month.
    let update_calendar = {
        let calendar = calendar.clone();
        let updating = updating.clone();
        let today = today.clone();
        move |date: NaiveDate| {
            let today = today.get();
            let is_current_month = same_month(date, today);

            // Guard against notify signals emitted by programmatic calendar updates.
            updating.set(true);

            // Avoid invalid intermediate states when moving from longer to shorter months.
            calendar.set_day(1);
            calendar.set_year(date.year());
            // GtkCalendar expects month in the 0-11 range (i32)
            calendar.set_month(date.month0() as i32);
            // Now set the actual day (today's day if current month, otherwise keep at 1)
            if is_current_month {
                calendar.set_day(today.day() as i32);
            }

            updating.set(false);

            if is_current_month {
                calendar.add_css_class(cal::SHOW_TODAY);
            } else {
                calendar.remove_css_class(cal::SHOW_TODAY);
            }
        }
    };

    // Initial sync to today's month.
    {
        let date = *current_date.borrow();
        update_header(date);
        update_calendar(date);
    }

    // Navigation button handlers ---------------------------------------------

    {
        let current_date = current_date.clone();
        let update_header = update_header.clone();
        let update_calendar = update_calendar.clone();
        prev_button.connect_clicked(move |_| {
            let new_date = shift_month(*current_date.borrow(), -1);
            if let Some(new_date) = new_date {
                *current_date.borrow_mut() = new_date;
                update_header(new_date);
                update_calendar(new_date);
            }
        });
    }

    {
        let current_date = current_date.clone();
        let update_header = update_header.clone();
        let update_calendar = update_calendar.clone();
        let today = today.clone();
        today_button.connect_clicked(move |_| {
            let new_date = month_start(today.get());
            *current_date.borrow_mut() = new_date;
            update_header(new_date);
            update_calendar(new_date);
        });
    }

    {
        let current_date = current_date.clone();
        let update_header = update_header.clone();
        let update_calendar = update_calendar.clone();
        next_button.connect_clicked(move |_| {
            let new_date = shift_month(*current_date.borrow(), 1);
            if let Some(new_date) = new_date {
                *current_date.borrow_mut() = new_date;
                update_header(new_date);
                update_calendar(new_date);
            }
        });
    }

    // Calendar internal navigation (e.g., selecting a day that moves between
    // months) should also keep `current_date` and the header / CSS in sync.
    {
        let current_date = current_date.clone();
        let update_header = update_header.clone();
        let update_calendar = update_calendar.clone();
        let updating = updating.clone();
        calendar.connect_day_selected(move |cal: &Calendar| {
            // Skip if we're in a programmatic update
            if updating.get() {
                return;
            }

            let year = cal.year();
            // GtkCalendar months are 0-11
            let month = (cal.month() + 1) as u32;
            let current = *current_date.borrow();

            // Only update if the calendar's month/year differs from our tracked state
            // (i.e., user clicked a day in a different month)
            if (month != current.month() || year != current.year())
                && let Some(date) = NaiveDate::from_ymd_opt(year, month, 1)
            {
                *current_date.borrow_mut() = date;
                update_header(date);
                update_calendar(date);
            }
        });
    }

    {
        let sync_visible_month: Rc<dyn Fn(&Calendar)> = {
            let current_date = current_date.clone();
            let update_header = update_header.clone();
            let update_calendar = update_calendar.clone();
            let updating = updating.clone();
            Rc::new(move |cal: &Calendar| {
                if updating.get() {
                    return;
                }

                let year = cal.year();
                let month = (cal.month() + 1) as u32;
                let current = *current_date.borrow();

                if (month != current.month() || year != current.year())
                    && let Some(date) = NaiveDate::from_ymd_opt(year, month, 1)
                {
                    *current_date.borrow_mut() = date;
                    update_header(date);
                    update_calendar(date);
                }
            })
        };

        calendar.connect_month_notify({
            let sync_visible_month = sync_visible_month.clone();
            move |cal| sync_visible_month(cal)
        });

        calendar.connect_year_notify(move |cal| sync_visible_month(cal));
    }

    // Refresh callback — navigates calendar to the real current date.
    // Called by on_show when the popover is reused across open/close cycles.
    let refresh: Rc<dyn Fn()> = {
        Rc::new(move || {
            let new_today = Local::now().date_naive();
            today.set(new_today);
            let new_date = month_start(new_today);
            *current_date.borrow_mut() = new_date;
            update_header(new_date);
            update_calendar(new_date);
        })
    };

    (container.upcast::<Widget>(), refresh)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    #[test]
    fn month_start_keeps_year_month_and_sets_first_day() {
        assert_eq!(month_start(date(2026, 5, 19)), date(2026, 5, 1));
    }

    #[test]
    fn shift_month_wraps_year_boundaries() {
        assert_eq!(shift_month(date(2026, 1, 31), -1), Some(date(2025, 12, 1)));
        assert_eq!(shift_month(date(2026, 12, 31), 1), Some(date(2027, 1, 1)));
    }

    #[test]
    fn shift_month_handles_larger_offsets() {
        assert_eq!(shift_month(date(2026, 5, 19), -15), Some(date(2025, 2, 1)));
        assert_eq!(shift_month(date(2026, 5, 19), 20), Some(date(2028, 1, 1)));
    }

    #[test]
    fn header_text_formats_month_and_year() {
        assert_eq!(header_text(date(2026, 5, 1)), "May 2026");
    }

    #[test]
    fn same_month_ignores_day() {
        assert!(same_month(date(2026, 5, 1), date(2026, 5, 31)));
        assert!(!same_month(date(2026, 5, 1), date(2026, 6, 1)));
    }
}
