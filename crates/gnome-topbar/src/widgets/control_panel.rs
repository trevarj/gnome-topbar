//! Combined clock control panel.
//!
//! This reuses the existing notification, media, and calendar popover
//! components so the clock can open a GNOME-like overview panel.

use std::cell::{Cell, RefCell};
use std::process::{Command, Stdio};
use std::rc::Rc;

use chrono::{Local, Timelike};
use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Label, Orientation, Widget};
use gtk4::{gio, glib};

use crate::services::config_manager::ConfigManager;
use crate::services::media::MediaService;
use crate::styles::surface;
use crate::widgets::calendar_popover::build_clock_calendar_popover;
use crate::widgets::custom::build_exec_display;
use crate::widgets::media_popover::build_media_popover_with_controller;
use crate::widgets::notifications_panel::build_control_panel_content as build_notifications_content;

/// Build the clock control panel content and return a refresh callback.
pub fn build_clock_control_panel(
    show_week_numbers: bool,
    weather_widget_name: Option<String>,
) -> (Widget, Rc<dyn Fn()>) {
    let root = GtkBox::new(Orientation::Horizontal, 12);
    root.add_css_class("control-panel");

    let left_col = GtkBox::new(Orientation::Vertical, 10);
    left_col.add_css_class("control-panel-left");
    left_col.set_size_request(380, -1);
    left_col.set_vexpand(true);
    left_col.set_valign(Align::Fill);

    let suppress_rebuild = Rc::new(Cell::new(false));
    let notifications = build_notifications_content(suppress_rebuild);
    notifications.add_css_class("control-panel-notifications");
    notifications.set_vexpand(true);
    notifications.set_valign(Align::Fill);
    left_col.append(&notifications);

    let right_col = GtkBox::new(Orientation::Vertical, 10);
    right_col.add_css_class("control-panel-right");
    right_col.set_size_request(360, -1);
    right_col.set_vexpand(false);
    right_col.set_valign(Align::Start);

    let time_card = build_time_weather_card(weather_widget_name);
    time_card.container.set_vexpand(false);
    time_card.container.set_valign(Align::Start);
    right_col.append(&time_card.container);

    let (media_widget, media_controller) = build_media_popover_with_controller();
    media_widget.add_css_class("control-panel-media");
    media_widget.set_vexpand(false);
    media_widget.set_valign(Align::Start);
    right_col.append(&media_widget);

    let media_controller_for_update = media_controller.clone();
    let media_service = MediaService::global();
    let media_callback_id = media_service.connect(move |snapshot| {
        media_controller_for_update.update_from_snapshot(snapshot);
    });
    root.connect_destroy(move |_| {
        MediaService::global().disconnect(media_callback_id);
    });

    let (calendar_widget, calendar_refresh) = build_clock_calendar_popover(show_week_numbers);
    calendar_widget.add_css_class("control-panel-calendar");
    calendar_widget.set_vexpand(false);
    calendar_widget.set_valign(Align::Start);
    right_col.append(&calendar_widget);

    root.append(&left_col);
    root.append(&right_col);

    let time_tick = schedule_time_tick(&time_card.time_label, &time_card.date_label);
    root.connect_destroy(move |_| {
        if let Some(source_id) = time_tick.borrow_mut().take() {
            source_id.remove();
        }
    });

    let refresh = {
        let time_label = time_card.time_label.clone();
        let date_label = time_card.date_label.clone();
        let weather_label = time_card.weather_label.clone();
        Rc::new(move || {
            refresh_time_labels(&time_label, &date_label);
            refresh_weather_label(&weather_label);
            media_controller.update_from_snapshot(&MediaService::global().snapshot());
            calendar_refresh();
        }) as Rc<dyn Fn()>
    };

    refresh();
    (root.upcast(), refresh)
}

struct TimeWeatherCard {
    container: GtkBox,
    time_label: Label,
    date_label: Label,
    weather_label: Option<(Label, String)>,
}

fn build_time_weather_card(weather_widget_name: Option<String>) -> TimeWeatherCard {
    let container = GtkBox::new(Orientation::Vertical, 2);
    container.add_css_class("control-panel-card");
    container.add_css_class("control-panel-time-weather");
    container.set_vexpand(false);
    container.set_valign(Align::Start);

    let time_label = Label::new(None);
    time_label.add_css_class("control-panel-time");
    time_label.set_xalign(0.0);
    time_label.set_halign(Align::Fill);

    let date_label = Label::new(None);
    date_label.add_css_class(surface::POPOVER_TITLE);
    date_label.add_css_class("control-panel-date");
    date_label.set_xalign(0.0);
    date_label.set_halign(Align::Fill);

    let weather_label = weather_widget_name.map(|widget_name| {
        let label = Label::new(None);
        label.add_css_class("control-panel-weather");
        label.set_xalign(0.0);
        label.set_halign(Align::Fill);
        (label, widget_name)
    });

    container.append(&time_label);
    container.append(&date_label);
    if let Some((ref label, _)) = weather_label {
        container.append(label);
    }

    TimeWeatherCard {
        container,
        time_label,
        date_label,
        weather_label,
    }
}

fn refresh_time_labels(time_label: &Label, date_label: &Label) {
    let now = Local::now();
    time_label.set_label(&now.format("%H:%M").to_string());
    date_label.set_label(&now.format("%A, %B %-d").to_string());
}

fn seconds_until_next_minute(second: u32) -> u32 {
    if second == 0 { 60 } else { 60 - second }
}

fn schedule_time_tick(
    time_label: &Label,
    date_label: &Label,
) -> Rc<RefCell<Option<glib::SourceId>>> {
    let source_slot = Rc::new(RefCell::new(None));
    let delay_seconds = seconds_until_next_minute(Local::now().second());

    let time_label = time_label.clone();
    let date_label = date_label.clone();
    let source_slot_for_once = Rc::clone(&source_slot);

    let source_id = glib::timeout_add_seconds_local_once(delay_seconds, move || {
        refresh_time_labels(&time_label, &date_label);

        let time_label = time_label.clone();
        let date_label = date_label.clone();
        let repeating_id = glib::timeout_add_seconds_local(60, move || {
            refresh_time_labels(&time_label, &date_label);
            glib::ControlFlow::Continue
        });

        *source_slot_for_once.borrow_mut() = Some(repeating_id);
    });

    *source_slot.borrow_mut() = Some(source_id);
    source_slot
}

fn refresh_weather_label(label: &Option<(Label, String)>) {
    let Some((label, widget_name)) = label else {
        return;
    };

    let cmd = ConfigManager::global()
        .get_widget_option(widget_name, "exec")
        .and_then(|v| v.as_str().map(str::to_string));

    let Some(cmd) = cmd else {
        return;
    };

    let label = label.clone();
    glib::spawn_future_local(async move {
        let result = gio::spawn_blocking(move || {
            Command::new("sh")
                .args(["-c", &cmd])
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output()
                .ok()
                .and_then(|output| {
                    if output.status.success() {
                        String::from_utf8(output.stdout).ok()
                    } else {
                        None
                    }
                })
                .and_then(|s| format_weather_exec_output(&s))
        })
        .await;

        if let Ok(Some(text)) = result {
            label.set_label(&text);
            label.set_visible(true);
        } else if result.is_ok() {
            label.set_label("");
            label.set_visible(false);
        }
    });
}

fn format_weather_exec_output(raw_output: &str) -> Option<String> {
    let display = build_exec_display(raw_output.trim(), "", None);
    display.visible.then_some(display.label_text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_weather_exec_output_plain_text() {
        assert_eq!(
            format_weather_exec_output("  72F Clear  \n").as_deref(),
            Some("72F Clear")
        );
    }

    #[test]
    fn test_format_weather_exec_output_waybar_json_text() {
        assert_eq!(
            format_weather_exec_output(r#"{"text":"72F Clear","tooltip":"Sunny"}"#).as_deref(),
            Some("72F Clear")
        );
    }

    #[test]
    fn test_format_weather_exec_output_waybar_json_label_fallback() {
        assert_eq!(
            format_weather_exec_output(r#"{"label":"72F Clear"}"#).as_deref(),
            Some("72F Clear")
        );
    }

    #[test]
    fn test_format_weather_exec_output_empty_hides() {
        assert_eq!(format_weather_exec_output(" \n"), None);
        assert_eq!(format_weather_exec_output(r#"{"text":""}"#), None);
    }

    #[test]
    fn test_seconds_until_next_minute() {
        assert_eq!(seconds_until_next_minute(0), 60);
        assert_eq!(seconds_until_next_minute(1), 59);
        assert_eq!(seconds_until_next_minute(30), 30);
        assert_eq!(seconds_until_next_minute(59), 1);
    }
}
