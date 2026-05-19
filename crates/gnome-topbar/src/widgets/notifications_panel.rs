//! Control-panel notification content.
//!
//! This module renders the notification list embedded in the clock control
//! panel. It is the only notification history UI; notifications are not exposed
//! as a standalone bar widget.

use gtk4::gdk::{self, Monitor};
use gtk4::prelude::*;
use gtk4::{
    Align, Box as GtkBox, Button, Image, Label, Orientation, PolicyType, Revealer,
    RevealerTransitionType, ScrolledWindow, glib,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;
use tracing::debug;

use crate::services::config_manager::ConfigManager;
use crate::services::icons::IconsService;
use crate::services::notification::{
    Notification, NotificationService, URGENCY_CRITICAL, URGENCY_LOW,
};
use crate::services::tooltip::TooltipManager;
use crate::styles::{button, card, color, notification as notif, surface};

use super::css::DISMISS_ANIMATION_MS;
use super::layer_shell_popover::{
    calculate_bar_exclusive_zone_from_values, calculate_popover_bar_margin,
};
use super::notifications_common::{
    BODY_TRUNCATE_THRESHOLD, POPOVER_WIDTH, create_notification_image_widget, format_timestamp,
    notification_primary_action, sanitize_body_markup,
};

/// Dismiss animation as a Duration for timeout callbacks.
const DISMISS_ANIMATION_DURATION: Duration = Duration::from_millis(DISMISS_ANIMATION_MS);

type HeaderClearButton = Rc<RefCell<Option<Button>>>;

/// Estimated header height (title + padding + separator space).
const HEADER_HEIGHT_ESTIMATE: i32 = 48;

/// Total vertical padding from surface styles, shadow margin, and list padding.
/// Surface padding (16px top + 16px bottom) + shadow margin (8px top + 8px bottom)
/// + notification-list top padding (8px).
const CONTAINER_VERTICAL_OVERHEAD: i32 = 64;

/// Minimum margin from the far screen edge.
const FAR_EDGE_MARGIN: i32 = 8;

/// Below this height, don't bother constraining the scroll area.
const MIN_HEIGHT_THRESHOLD: i32 = 100;

/// Fallback max scroll height when monitor geometry is unavailable.
const FALLBACK_MAX_HEIGHT: i32 = 500;

/// Compute the maximum ScrolledWindow height based on monitor geometry.
///
/// Uses the same approach as quick settings: subtract the bar exclusive zone,
/// bar margin, container overhead, and far edge margin from the monitor height.
fn compute_max_scroll_height() -> i32 {
    let monitor_opt = gdk::Display::default().and_then(|display| {
        let monitors = display.monitors();
        monitors
            .item(0)
            .and_then(|obj| obj.downcast::<Monitor>().ok())
    });

    let Some(monitor) = monitor_opt else {
        return FALLBACK_MAX_HEIGHT;
    };

    let geom = monitor.geometry();

    let config_mgr = ConfigManager::global();
    let bar_size = config_mgr.bar_size() as i32;
    let bar_padding = config_mgr.bar_padding() as i32;
    let bar_opacity = config_mgr.bar_background_opacity();
    let screen_margin = config_mgr.screen_margin() as i32;
    let popover_offset = config_mgr.popover_offset() as i32;

    let bar_exclusive_zone =
        calculate_bar_exclusive_zone_from_values(bar_size, bar_padding, bar_opacity, screen_margin)
            + popover_offset;

    let bar_margin = calculate_popover_bar_margin();

    let max_height = geom.height()
        - bar_exclusive_zone
        - bar_margin
        - HEADER_HEIGHT_ESTIMATE
        - CONTAINER_VERTICAL_OVERHEAD
        - FAR_EDGE_MARGIN;

    if max_height > MIN_HEIGHT_THRESHOLD {
        max_height
    } else {
        FALLBACK_MAX_HEIGHT
    }
}

/// Build notification content for the embedded clock control panel.
pub(crate) fn build_control_panel_content(suppress_rebuild: Rc<Cell<bool>>) -> gtk4::Widget {
    let root = GtkBox::new(Orientation::Vertical, 0);
    root.add_css_class(notif::POPOVER);
    root.set_size_request(POPOVER_WIDTH, -1);
    root.set_vexpand(true);
    root.set_valign(Align::Fill);

    let notification_list = GtkBox::new(Orientation::Vertical, 4);
    notification_list.add_css_class(notif::LIST);
    notification_list.set_vexpand(true);

    let header_clear_button = Rc::new(RefCell::new(None));
    let header = build_header(&notification_list, &suppress_rebuild, &header_clear_button);
    root.append(&header);

    populate_notification_list(&notification_list, &suppress_rebuild, &header_clear_button);

    let max_height = compute_max_scroll_height();

    let scrolled = ScrolledWindow::new();
    scrolled.set_policy(PolicyType::Never, PolicyType::Automatic);
    scrolled.set_propagate_natural_height(false);
    scrolled.set_max_content_height(max_height);
    scrolled.set_vexpand(true);
    scrolled.set_valign(Align::Fill);
    scrolled.add_css_class(notif::SCROLL);

    scrolled.set_child(Some(&notification_list));
    root.append(&scrolled);

    root.upcast()
}

fn build_header(
    notification_list: &GtkBox,
    suppress_rebuild: &Rc<Cell<bool>>,
    header_clear_button: &HeaderClearButton,
) -> GtkBox {
    let header = GtkBox::new(Orientation::Horizontal, 8);
    header.add_css_class(notif::HEADER);

    let title = Label::new(Some("Notifications"));
    title.add_css_class(surface::POPOVER_TITLE);
    title.set_hexpand(true);
    title.set_xalign(0.0);
    title.set_valign(Align::Start);
    header.append(&title);

    let service = NotificationService::global();
    let tooltip_manager = TooltipManager::global();
    let icons = IconsService::global();

    // Mute toggle button (always visible)
    let mute_btn = crate::widgets::base::vp_button();
    mute_btn.set_has_frame(false);
    mute_btn.set_focus_on_click(false);
    mute_btn.add_css_class(surface::POPOVER_ICON_BTN);
    mute_btn.add_css_class(notif::HEADER_ICON_BTN);
    mute_btn.set_valign(Align::Start);

    let is_muted = service.is_muted();
    if is_muted {
        mute_btn.add_css_class(notif::MUTE_ACTIVE);
    }

    let mute_icon_handle = icons.create_icon(
        if is_muted {
            "notifications-disabled"
        } else {
            "notifications"
        },
        &[color::PRIMARY, notif::HEADER_ICON],
    );
    let mute_icon_widget = mute_icon_handle.widget();
    mute_icon_widget.set_halign(Align::Center);
    mute_icon_widget.set_valign(Align::Center);
    mute_btn.set_child(Some(&mute_icon_widget));
    tooltip_manager.set_styled_tooltip(
        &mute_btn,
        if is_muted {
            "Unmute notifications"
        } else {
            "Mute notifications"
        },
    );

    // Store icon handle in RefCell for the click handler
    let mute_icon_handle = Rc::new(RefCell::new(mute_icon_handle));
    let mute_icon_handle_clone = Rc::clone(&mute_icon_handle);

    mute_btn.connect_clicked(move |btn| {
        let service = NotificationService::global();
        let tooltip_manager = TooltipManager::global();
        tooltip_manager.cancel_and_hide();
        service.toggle_muted();

        // Update icon and tooltip
        let is_muted = service.is_muted();
        mute_icon_handle_clone.borrow().set_icon(if is_muted {
            "notifications-disabled"
        } else {
            "notifications"
        });
        if is_muted {
            btn.add_css_class(notif::MUTE_ACTIVE);
        } else {
            btn.remove_css_class(notif::MUTE_ACTIVE);
        }
        tooltip_manager.set_styled_tooltip(
            btn,
            if is_muted {
                "Unmute notifications"
            } else {
                "Mute notifications"
            },
        );
    });

    header.append(&mute_btn);

    // Clear all button (only when there are history notifications)
    let count = service.history_count();

    if count > 0 {
        let clear_btn = crate::widgets::base::vp_button();
        clear_btn.set_has_frame(false);
        clear_btn.set_focus_on_click(false);
        clear_btn.add_css_class(surface::POPOVER_ICON_BTN);
        clear_btn.add_css_class(notif::HEADER_ICON_BTN);
        clear_btn.set_valign(Align::Start);
        tooltip_manager.set_styled_tooltip(&clear_btn, "Clear all notifications");

        let clear_icon =
            icons.create_icon("user-trash-symbolic", &[color::PRIMARY, notif::HEADER_ICON]);
        let clear_icon_widget = clear_icon.widget();
        clear_icon_widget.set_halign(Align::Center);
        clear_icon_widget.set_valign(Align::Center);
        clear_btn.set_child(Some(&clear_icon_widget));

        let clear_btn_for_click = clear_btn.clone();
        let list_for_clear = notification_list.clone();
        let suppress_for_clear = Rc::clone(suppress_rebuild);
        let header_clear_for_click = Rc::clone(header_clear_button);

        clear_btn.connect_clicked(move |_| {
            TooltipManager::global().cancel_and_hide();
            suppress_for_clear.set(true);
            NotificationService::global().close_all();
            clear_btn_for_click.set_visible(false);
            clear_notification_list_to_empty(
                &list_for_clear,
                &suppress_for_clear,
                &header_clear_for_click,
            );
        });

        *header_clear_button.borrow_mut() = Some(clear_btn.clone());
        header.append(&clear_btn);
    }

    header
}

/// Populate the notification list with current notifications or empty state.
fn populate_notification_list(
    list: &GtkBox,
    suppress_rebuild: &Rc<Cell<bool>>,
    header_clear_button: &HeaderClearButton,
) {
    let service = NotificationService::global();

    if !service.backend_available() {
        set_header_clear_visible(header_clear_button, false);
        add_empty_state(
            list,
            "Another notification daemon is running.\nDisable it to use this notification center.",
        );
        return;
    }

    // Transient notifications bypass the popover history per the freedesktop spec.
    let mut notifications: Vec<Notification> = service.history_notifications();

    if notifications.is_empty() {
        set_header_clear_visible(header_clear_button, false);
        add_empty_state(list, "No notifications");
        return;
    }

    set_header_clear_visible(header_clear_button, true);

    // Sort by timestamp (newest first)
    notifications.sort_by(|a, b| {
        b.timestamp
            .partial_cmp(&a.timestamp)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    populate_grouped_notifications(list, notifications, suppress_rebuild, header_clear_button);
}

#[derive(Debug)]
struct AppNotificationGroup {
    notifications: Vec<Notification>,
}

impl AppNotificationGroup {
    fn app_name(&self) -> &str {
        self.notifications
            .first()
            .map(|notification| notification.app_name.as_str())
            .unwrap_or("Notifications")
    }

    fn notification_ids(&self) -> Vec<u32> {
        self.notifications
            .iter()
            .map(|notification| notification.id)
            .collect()
    }
}

fn populate_grouped_notifications(
    list: &GtkBox,
    notifications: Vec<Notification>,
    suppress_rebuild: &Rc<Cell<bool>>,
    header_clear_button: &HeaderClearButton,
) {
    for group in group_notifications_by_app(notifications) {
        if group.notifications.len() == 1 {
            let notification = &group.notifications[0];
            let revealer = build_dismiss_revealer(true);

            let row = build_notification_row(
                notification,
                list,
                &revealer,
                suppress_rebuild,
                None,
                None,
                header_clear_button,
            );
            revealer.set_child(Some(&row));
            list.append(&revealer);
            continue;
        }

        let revealer = build_dismiss_revealer(true);

        let group_card = build_notification_group(
            &group,
            list,
            &revealer,
            suppress_rebuild,
            header_clear_button,
        );
        revealer.set_child(Some(&group_card));
        list.append(&revealer);
    }
}

fn build_dismiss_revealer(reveal_child: bool) -> Revealer {
    let revealer = Revealer::new();
    revealer.set_transition_type(RevealerTransitionType::SlideDown);
    revealer.set_transition_duration(
        ConfigManager::global().animation_duration(DISMISS_ANIMATION_MS as u32),
    );
    revealer.set_reveal_child(reveal_child);
    revealer
}

fn group_notifications_by_app(notifications: Vec<Notification>) -> Vec<AppNotificationGroup> {
    let mut groups = Vec::<AppNotificationGroup>::new();
    let mut group_indices = HashMap::<String, usize>::new();

    for notification in notifications {
        let key = notification_group_key(&notification);
        if let Some(index) = group_indices.get(&key).copied() {
            groups[index].notifications.push(notification);
        } else {
            group_indices.insert(key, groups.len());
            groups.push(AppNotificationGroup {
                notifications: vec![notification],
            });
        }
    }

    groups
}

fn notification_group_key(notification: &Notification) -> String {
    notification
        .desktop_entry
        .as_deref()
        .filter(|entry| !entry.is_empty())
        .unwrap_or(&notification.app_name)
        .to_ascii_lowercase()
}

fn notification_count_text(count: usize) -> String {
    match count {
        1 => "1 notification".to_string(),
        _ => format!("{count} notifications"),
    }
}

fn invoke_notification_action_from_ui(source: &str, notification_id: u32, action_id: &str) {
    debug!(
        "NotificationsPanel: invoking action from {}, id={}, action_key={}",
        source, notification_id, action_id
    );
    NotificationService::global().invoke_action(notification_id, action_id);
}

fn add_empty_state(list: &GtkBox, message: &str) {
    let empty = GtkBox::new(Orientation::Vertical, 8);
    empty.add_css_class(notif::EMPTY);
    empty.set_valign(Align::Center);
    empty.set_halign(Align::Center);
    empty.set_vexpand(true);

    // Icon
    let empty_icon = Image::from_icon_name("notifications-disabled-symbolic");
    empty_icon.set_pixel_size(32);
    empty_icon.add_css_class(notif::EMPTY_ICON);
    empty_icon.add_css_class(color::MUTED);
    empty_icon.set_opacity(0.5);
    empty.append(&empty_icon);

    // Message
    let label = Label::new(Some(message));
    label.add_css_class(notif::EMPTY_LABEL);
    label.add_css_class(color::MUTED);
    label.set_justify(gtk4::Justification::Center);
    label.set_wrap(true);
    label.set_max_width_chars(50);
    empty.append(&label);

    list.append(&empty);
}

fn clear_notification_list_to_empty(
    list: &GtkBox,
    suppress_rebuild: &Rc<Cell<bool>>,
    header_clear_button: &HeaderClearButton,
) {
    suppress_rebuild.set(true);
    set_header_clear_visible(header_clear_button, false);

    let mut child = list.first_child();
    let mut animated_any = false;
    while let Some(widget) = child {
        child = widget.next_sibling();
        if let Ok(revealer) = widget.downcast::<Revealer>() {
            mark_revealer_child_dismissing(&revealer);
            revealer.set_reveal_child(false);
            animated_any = true;
        }
    }

    if !animated_any {
        remove_all_children(list);
        add_empty_state(list, "No notifications");
        suppress_rebuild.set(false);
        return;
    }

    let list_for_timeout = list.clone();
    let suppress_for_timeout = Rc::clone(suppress_rebuild);
    glib::timeout_add_local_once(dismiss_duration(), move || {
        remove_all_children(&list_for_timeout);
        add_empty_state(&list_for_timeout, "No notifications");
        suppress_for_timeout.set(false);
    });
}

fn set_header_clear_visible(header_clear_button: &HeaderClearButton, visible: bool) {
    if let Some(button) = header_clear_button.borrow().as_ref() {
        button.set_visible(visible);
    }
}

fn remove_all_children(list: &GtkBox) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
}

fn mark_revealer_child_dismissing(revealer: &Revealer) {
    if let Some(child) = revealer.child() {
        child.add_css_class(notif::ROW_DISMISSING);
    }
}

fn dismiss_duration() -> Duration {
    dismiss_duration_for_animations(ConfigManager::global().animations_enabled())
}

fn dismiss_duration_for_animations(animations_enabled: bool) -> Duration {
    if animations_enabled {
        DISMISS_ANIMATION_DURATION
    } else {
        Duration::ZERO
    }
}

fn build_notification_group(
    group: &AppNotificationGroup,
    outer_list: &GtkBox,
    group_revealer: &Revealer,
    suppress_rebuild: &Rc<Cell<bool>>,
    header_clear_button: &HeaderClearButton,
) -> GtkBox {
    let latest = &group.notifications[0];

    let card = GtkBox::new(Orientation::Vertical, 0);
    card.add_css_class(notif::APP_GROUP);
    card.add_css_class(card::BASE);

    let header_row = GtkBox::new(Orientation::Horizontal, 2);

    let expand_btn = Button::new();
    expand_btn.set_has_frame(false);
    expand_btn.add_css_class(notif::GROUP_HEADER);
    expand_btn.add_css_class(button::RESET);
    expand_btn.set_focus_on_click(false);
    expand_btn.set_hexpand(true);
    TooltipManager::global().set_styled_tooltip(&expand_btn, "Expand notification group");

    let expand_content = GtkBox::new(Orientation::Horizontal, 8);

    let icon = create_notification_image_widget(latest);
    icon.add_css_class(notif::ROW_ICON);
    expand_content.append(&icon);

    let content = GtkBox::new(Orientation::Vertical, 2);
    content.set_hexpand(true);
    content.add_css_class(notif::ROW_CONTENT);

    let top_row = GtkBox::new(Orientation::Horizontal, 4);
    let app_label = Label::new(Some(&latest.app_name));
    app_label.add_css_class(notif::APP_NAME);
    app_label.add_css_class(color::MUTED);
    app_label.set_xalign(0.0);
    app_label.set_hexpand(true);
    app_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    top_row.append(&app_label);

    let count_label = Label::new(Some(&notification_count_text(group.notifications.len())));
    count_label.add_css_class(notif::GROUP_COUNT);
    count_label.add_css_class(color::MUTED);
    top_row.append(&count_label);
    content.append(&top_row);

    let summary = if latest.summary.is_empty() {
        latest.body.as_str()
    } else {
        latest.summary.as_str()
    };
    if !summary.is_empty() {
        let summary_label = Label::new(Some(summary));
        summary_label.add_css_class(notif::SUMMARY);
        summary_label.set_xalign(0.0);
        summary_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        summary_label.set_single_line_mode(true);
        content.append(&summary_label);
    }

    expand_content.append(&content);

    let arrow = Image::from_icon_name("pan-end-symbolic");
    arrow.add_css_class(notif::DISMISS_ICON);
    arrow.set_valign(Align::Center);
    expand_content.append(&arrow);

    expand_btn.set_child(Some(&expand_content));
    header_row.append(&expand_btn);

    let clear_btn = Button::new();
    clear_btn.set_has_frame(false);
    clear_btn.add_css_class(notif::GROUP_CLEAR);
    clear_btn.add_css_class(button::RESET);
    clear_btn.set_focus_on_click(false);
    clear_btn.set_valign(Align::Center);
    TooltipManager::global().set_styled_tooltip(
        &clear_btn,
        &format!("Clear {} notifications", group.app_name()),
    );

    let clear_icon = IconsService::global().create_icon(
        "user-trash-symbolic",
        &[color::PRIMARY, notif::DISMISS_ICON],
    );
    let clear_icon_widget = clear_icon.widget();
    clear_icon_widget.set_halign(Align::Center);
    clear_icon_widget.set_valign(Align::Center);
    clear_btn.set_child(Some(&clear_icon_widget));

    header_row.append(&clear_btn);
    card.append(&header_row);

    let child_list = GtkBox::new(Orientation::Vertical, 4);
    child_list.add_css_class(notif::GROUP_LIST);

    let child_revealer = build_dismiss_revealer(false);
    child_revealer.set_child(Some(&child_list));

    let expanded = Rc::new(Cell::new(false));
    let expanded_for_click = Rc::clone(&expanded);
    let child_revealer_for_click = child_revealer.clone();
    let arrow_for_click = arrow.clone();
    let expand_btn_for_tooltip = expand_btn.clone();
    expand_btn.connect_clicked(move |_| {
        let next = !expanded_for_click.get();
        expanded_for_click.set(next);
        child_revealer_for_click.set_reveal_child(next);
        arrow_for_click.set_icon_name(Some(if next {
            "pan-down-symbolic"
        } else {
            "pan-end-symbolic"
        }));
        TooltipManager::global().set_styled_tooltip(
            &expand_btn_for_tooltip,
            if next {
                "Collapse notification group"
            } else {
                "Expand notification group"
            },
        );
    });

    card.append(&child_revealer);

    let notification_ids = group.notification_ids();
    let outer_list_for_clear = outer_list.clone();
    let group_revealer_for_clear = group_revealer.clone();
    let card_for_clear = card.clone();
    let suppress_for_clear = Rc::clone(suppress_rebuild);
    let header_clear_for_clear = Rc::clone(header_clear_button);
    clear_btn.connect_clicked(move |btn| {
        TooltipManager::global().cancel_and_hide();
        btn.set_sensitive(false);

        suppress_for_clear.set(true);
        for notification_id in &notification_ids {
            NotificationService::global().close(*notification_id);
        }

        card_for_clear.add_css_class(notif::ROW_DISMISSING);
        group_revealer_for_clear.set_reveal_child(false);

        let outer_list = outer_list_for_clear.clone();
        let group_revealer = group_revealer_for_clear.clone();
        let suppress_for_timeout = Rc::clone(&suppress_for_clear);
        let header_clear_for_timeout = Rc::clone(&header_clear_for_clear);
        glib::timeout_add_local_once(dismiss_duration(), move || {
            outer_list.remove(&group_revealer);
            if outer_list.first_child().is_none() {
                set_header_clear_visible(&header_clear_for_timeout, false);
                add_empty_state(&outer_list, "No notifications");
            }
            suppress_for_timeout.set(false);
        });
    });

    for notification in &group.notifications {
        let row_revealer = build_dismiss_revealer(true);

        let outer_list_for_empty = outer_list.clone();
        let group_revealer_for_empty = group_revealer.clone();
        let header_clear_for_empty = Rc::clone(header_clear_button);
        let after_empty = Rc::new(move || {
            outer_list_for_empty.remove(&group_revealer_for_empty);
            if outer_list_for_empty.first_child().is_none() {
                set_header_clear_visible(&header_clear_for_empty, false);
                add_empty_state(&outer_list_for_empty, "No notifications");
            }
        });

        let child_list_for_count = child_list.clone();
        let count_label_for_dismiss = count_label.clone();
        let after_group_child_dismiss = Rc::new(move || {
            count_label_for_dismiss.set_label(&notification_count_text(widget_child_count(
                &child_list_for_count,
            )));
        });

        let row = build_notification_row(
            notification,
            &child_list,
            &row_revealer,
            suppress_rebuild,
            Some(after_empty),
            Some(after_group_child_dismiss),
            header_clear_button,
        );
        row_revealer.set_child(Some(&row));
        child_list.append(&row_revealer);
    }

    card
}

fn widget_child_count(container: &GtkBox) -> usize {
    let mut count = 0;
    let mut child = container.first_child();
    while let Some(widget) = child {
        count += 1;
        child = widget.next_sibling();
    }
    count
}

fn build_notification_row(
    notification: &Notification,
    list: &GtkBox,
    revealer: &Revealer,
    suppress_rebuild: &Rc<Cell<bool>>,
    on_list_empty: Option<Rc<dyn Fn()>>,
    on_after_dismiss: Option<Rc<dyn Fn()>>,
    header_clear_button: &HeaderClearButton,
) -> GtkBox {
    let card = GtkBox::new(Orientation::Vertical, 0);
    card.add_css_class(notif::ROW);
    card.add_css_class(card::BASE);

    // Add urgency class
    if notification.urgency == URGENCY_CRITICAL {
        card.add_css_class(notif::CRITICAL);
    } else if notification.urgency == URGENCY_LOW {
        card.add_css_class(notif::LOW);
    }

    // Main content row: icon + text + dismiss
    let main_row = GtkBox::new(Orientation::Horizontal, 8);
    card.append(&main_row);

    // App icon / avatar in a centered column
    let icon_container = GtkBox::new(Orientation::Vertical, 0);
    icon_container.set_halign(Align::Center);
    icon_container.set_valign(Align::Start);
    icon_container.set_width_request(56);

    let icon = create_notification_image_widget(notification);
    icon.add_css_class(notif::ROW_ICON);
    icon.set_halign(Align::Center);
    icon_container.append(&icon);

    main_row.append(&icon_container);

    // Content area
    let content = GtkBox::new(Orientation::Vertical, 2);
    content.set_hexpand(true);
    content.add_css_class(notif::ROW_CONTENT);

    // Top row: app name + timestamp
    let top_row = GtkBox::new(Orientation::Horizontal, 4);

    let app_label = Label::new(Some(&notification.app_name));
    app_label.add_css_class(notif::APP_NAME);
    app_label.add_css_class(color::MUTED);
    app_label.set_xalign(0.0);
    app_label.set_hexpand(true);
    app_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    top_row.append(&app_label);

    let time_label = Label::new(Some(&format_timestamp(notification.timestamp)));
    time_label.add_css_class(notif::TIMESTAMP);
    time_label.add_css_class(color::MUTED);
    top_row.append(&time_label);

    content.append(&top_row);

    // Summary
    if !notification.summary.is_empty() {
        let summary_label = Label::new(Some(&notification.summary));
        summary_label.add_css_class(notif::SUMMARY);
        summary_label.set_xalign(0.0);
        summary_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        summary_label.set_single_line_mode(true);
        content.append(&summary_label);
    }

    // Body with expandable support for long text
    // Use a single label with dynamic line limiting to avoid breaking markup tags
    let mut body_label_opt: Option<Label> = None;

    if !notification.body.is_empty() {
        // Sanitize markup and clean up for display
        let body_markup = sanitize_body_markup(&notification.body);
        let body_clean = body_markup.trim();
        let needs_expansion = body_clean.chars().count() > BODY_TRUNCATE_THRESHOLD;

        let body_label = Label::new(None);
        body_label.set_markup(body_clean);
        body_label.add_css_class(notif::BODY);
        body_label.add_css_class(color::MUTED);
        body_label.set_xalign(0.0);
        body_label.set_wrap(true);
        body_label.set_wrap_mode(gtk4::pango::WrapMode::WordChar);

        if needs_expansion {
            // Start collapsed: limit to 2 lines with ellipsis
            body_label.set_lines(2);
            body_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            body_label.set_vexpand(false);
            body_label_opt = Some(body_label.clone());
        } else {
            // Short body - no line limit
            body_label.set_lines(-1);
            body_label.set_ellipsize(gtk4::pango::EllipsizeMode::None);
        }

        // Handle link activation manually to avoid Wayland protocol errors.
        // Protocol error 71 often occurs when gtk_show_uri triggers a focus switch or
        // interaction that conflicts with the layer shell surface state.
        body_label.connect_activate_link(move |_, uri| {
            // Use xdg-open via spawn_command_line_async for a detached process
            let cmd = format!("xdg-open '{}'", uri.replace("'", "'\\''"));
            // We ignore the result here because this is a fire-and-forget operation
            // and we can't do much if xdg-open fails to launch from here anyway.
            let _ = glib::spawn_command_line_async(&cmd);

            glib::Propagation::Stop // Stop propagation to default handler
        });

        content.append(&body_label);
    }

    main_row.append(&content);

    let dismiss_btn = Button::new();
    dismiss_btn.set_has_frame(false);
    dismiss_btn.add_css_class(notif::DISMISS_BTN);
    dismiss_btn.add_css_class(button::RESET);
    dismiss_btn.set_valign(Align::Start);
    dismiss_btn.set_tooltip_text(Some("Dismiss"));

    let dismiss_icon = Image::from_icon_name("window-close-symbolic");
    dismiss_icon.add_css_class(notif::DISMISS_ICON);
    dismiss_icon.set_halign(Align::Center);
    dismiss_icon.set_valign(Align::Center);
    dismiss_btn.set_child(Some(&dismiss_icon));

    let notification_id = notification.id;
    let card_for_dismiss = card.clone();
    let revealer_for_dismiss = revealer.clone();
    let list_for_dismiss = list.clone();
    let suppress = Rc::clone(suppress_rebuild);
    let on_list_empty_for_dismiss = on_list_empty.clone();
    let on_after_dismiss_for_dismiss = on_after_dismiss.clone();
    let header_clear_for_dismiss = Rc::clone(header_clear_button);
    dismiss_btn.connect_clicked(move |btn| {
        // Prevent double-clicks from leaving suppress_rebuild stuck.
        btn.set_sensitive(false);

        suppress.set(true);
        NotificationService::global().close(notification_id);
        schedule_notification_row_removal(
            &card_for_dismiss,
            &revealer_for_dismiss,
            &list_for_dismiss,
            &suppress,
            &on_list_empty_for_dismiss,
            &on_after_dismiss_for_dismiss,
            &header_clear_for_dismiss,
        );
    });

    main_row.append(&dismiss_btn);

    let has_expand = body_label_opt.is_some();
    let primary_action = notification_primary_action(&notification.actions);

    if let Some(primary_id) = primary_action.clone() {
        let click_gesture = gtk4::GestureClick::new();
        click_gesture.set_button(1);
        let notification_id = notification.id;
        let card_for_action = card.clone();
        let revealer_for_action = revealer.clone();
        let list_for_action = list.clone();
        let suppress_for_action = Rc::clone(suppress_rebuild);
        let on_list_empty_for_action = on_list_empty.clone();
        let on_after_dismiss_for_action = on_after_dismiss.clone();
        let header_clear_for_action = Rc::clone(header_clear_button);
        click_gesture.connect_pressed(move |gesture, n_press, _, _| {
            if n_press == 1 {
                gesture.set_state(gtk4::EventSequenceState::Claimed);
                suppress_for_action.set(true);
                invoke_notification_action_from_ui(
                    "row-primary-click",
                    notification_id,
                    &primary_id,
                );
                schedule_notification_row_removal(
                    &card_for_action,
                    &revealer_for_action,
                    &list_for_action,
                    &suppress_for_action,
                    &on_list_empty_for_action,
                    &on_after_dismiss_for_action,
                    &header_clear_for_action,
                );
            }
        });
        content.add_controller(click_gesture);
        content.add_css_class(notif::TOAST_CLICKABLE);
    }

    // Actions at the bottom (non-default actions) and optional expand button.
    // If a non-default action is promoted to primary "Open", keep it out of the
    // secondary action row to avoid duplicate buttons for the same action id.
    let non_default_actions: Vec<_> = notification
        .actions
        .iter()
        .filter(|(id, _)| id != "default" && Some(id.as_str()) != primary_action.as_deref())
        .collect();

    if !non_default_actions.is_empty() || has_expand || primary_action.is_some() {
        let actions_row = GtkBox::new(Orientation::Horizontal, 8);
        actions_row.add_css_class(notif::ACTIONS);

        // Optional expand button on the left
        if let Some(body_label) = body_label_opt {
            let expand_btn = crate::widgets::base::vp_button_with_label("Show more");
            expand_btn.add_css_class(notif::ACTION_BTN);
            expand_btn.add_css_class(button::GHOST);

            // Store expanded state in a Cell
            let is_expanded = Rc::new(Cell::new(false));
            let is_expanded_clone = Rc::clone(&is_expanded);

            expand_btn.connect_clicked(move |btn| {
                let expanded = is_expanded_clone.get();
                let new_state = !expanded;
                is_expanded_clone.set(new_state);

                if new_state {
                    // Expanded: remove line limit and ellipsis
                    body_label.set_lines(-1);
                    body_label.set_ellipsize(gtk4::pango::EllipsizeMode::None);
                    // Ensure the label can expand vertically in the container
                    body_label.set_vexpand(true);
                    btn.set_label("Show less");
                } else {
                    // Collapsed: limit to 2 lines with ellipsis
                    body_label.set_lines(2);
                    body_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
                    body_label.set_vexpand(false);
                    btn.set_label("Show more");
                }
            });

            actions_row.append(&expand_btn);
        }

        // Spacer between expand button and actions
        if has_expand && (!non_default_actions.is_empty() || primary_action.is_some()) {
            let spacer = GtkBox::new(Orientation::Horizontal, 0);
            spacer.set_hexpand(true);
            actions_row.append(&spacer);
        } else if !has_expand {
            actions_row.set_halign(Align::End);
        }

        // Primary "Open" action button, if available
        if let Some(primary_id) = primary_action {
            let open_btn = crate::widgets::base::vp_button_with_label("Open");
            open_btn.add_css_class(notif::ACTION_BTN);
            open_btn.add_css_class(button::GHOST);

            let notification_id = notification.id;
            let card_for_action = card.clone();
            let revealer_for_action = revealer.clone();
            let list_for_action = list.clone();
            let suppress_for_action = Rc::clone(suppress_rebuild);
            let on_list_empty_for_action = on_list_empty.clone();
            let on_after_dismiss_for_action = on_after_dismiss.clone();
            let header_clear_for_action = Rc::clone(header_clear_button);
            open_btn.connect_clicked(move |btn| {
                btn.set_sensitive(false);
                suppress_for_action.set(true);
                invoke_notification_action_from_ui("open-button", notification_id, &primary_id);
                schedule_notification_row_removal(
                    &card_for_action,
                    &revealer_for_action,
                    &list_for_action,
                    &suppress_for_action,
                    &on_list_empty_for_action,
                    &on_after_dismiss_for_action,
                    &header_clear_for_action,
                );
            });

            actions_row.append(&open_btn);
        }

        // Action buttons on the right (non-default actions like "Mark as Read", "Reply", etc.)
        for (action_id, action_label) in non_default_actions {
            let action_btn = crate::widgets::base::vp_button_with_label(action_label);
            action_btn.add_css_class(notif::ACTION_BTN);
            action_btn.add_css_class(button::GHOST);

            let notification_id = notification.id;
            let action_id = action_id.clone();
            let card_for_action = card.clone();
            let revealer_for_action = revealer.clone();
            let list_for_action = list.clone();
            let suppress_for_action = Rc::clone(suppress_rebuild);
            let on_list_empty_for_action = on_list_empty.clone();
            let on_after_dismiss_for_action = on_after_dismiss.clone();
            let header_clear_for_action = Rc::clone(header_clear_button);
            action_btn.connect_clicked(move |btn| {
                btn.set_sensitive(false);
                suppress_for_action.set(true);
                invoke_notification_action_from_ui("action-button", notification_id, &action_id);
                schedule_notification_row_removal(
                    &card_for_action,
                    &revealer_for_action,
                    &list_for_action,
                    &suppress_for_action,
                    &on_list_empty_for_action,
                    &on_after_dismiss_for_action,
                    &header_clear_for_action,
                );
            });

            actions_row.append(&action_btn);
        }

        card.append(&actions_row);
    }

    card
}

fn schedule_notification_row_removal(
    card: &GtkBox,
    revealer: &Revealer,
    list: &GtkBox,
    suppress_rebuild: &Rc<Cell<bool>>,
    on_list_empty: &Option<Rc<dyn Fn()>>,
    on_after_dismiss: &Option<Rc<dyn Fn()>>,
    header_clear_button: &HeaderClearButton,
) {
    suppress_rebuild.set(true);

    // Fade out the row content and collapse height via the Revealer.
    card.add_css_class(notif::ROW_DISMISSING);
    revealer.set_reveal_child(false);

    let revealer = revealer.clone();
    let list = list.clone();
    let on_list_empty = on_list_empty.clone();
    let on_after_dismiss = on_after_dismiss.clone();
    let header_clear_button = Rc::clone(header_clear_button);
    let suppress_for_timeout = Rc::clone(suppress_rebuild);
    glib::timeout_add_local_once(dismiss_duration(), move || {
        list.remove(&revealer);
        if list.first_child().is_none() {
            set_header_clear_visible(&header_clear_button, false);
            if let Some(ref on_list_empty) = on_list_empty {
                on_list_empty();
            } else {
                add_empty_state(&list, "No notifications");
            }
        } else if let Some(ref on_after_dismiss) = on_after_dismiss {
            on_after_dismiss();
        }
        suppress_for_timeout.set(false);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notification(id: u32, app_name: &str, desktop_entry: Option<&str>) -> Notification {
        Notification {
            id,
            app_name: app_name.to_string(),
            app_icon: String::new(),
            summary: format!("notification {id}"),
            body: String::new(),
            actions: Vec::new(),
            urgency: 1,
            timestamp: id as f64,
            expire_timeout: -1,
            desktop_entry: desktop_entry.map(str::to_string),
            image_path: None,
            image_data: None,
            transient: false,
        }
    }

    #[test]
    fn groups_notifications_by_desktop_entry_before_app_name() {
        let groups = group_notifications_by_app(vec![
            notification(3, "Chat", Some("org.example.Chat")),
            notification(2, "Mail", None),
            notification(1, "Chat Renamed", Some("org.example.Chat")),
        ]);

        assert_eq!(groups.len(), 2);
        assert_eq!(
            groups[0]
                .notifications
                .iter()
                .map(|notification| notification.id)
                .collect::<Vec<_>>(),
            vec![3, 1]
        );
        assert_eq!(groups[1].notifications[0].id, 2);
    }

    #[test]
    fn preserves_first_seen_group_order() {
        let groups = group_notifications_by_app(vec![
            notification(4, "Calendar", None),
            notification(3, "Chat", None),
            notification(2, "Calendar", None),
            notification(1, "Chat", None),
        ]);

        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].notifications[0].app_name, "Calendar");
        assert_eq!(
            groups[0]
                .notifications
                .iter()
                .map(|notification| notification.id)
                .collect::<Vec<_>>(),
            vec![4, 2]
        );
        assert_eq!(groups[1].notifications[0].app_name, "Chat");
    }

    #[test]
    fn grouped_notification_ids_preserve_newest_first_order() {
        let groups = group_notifications_by_app(vec![
            notification(5, "Chat", None),
            notification(4, "Chat", None),
            notification(3, "Chat", None),
        ]);

        assert_eq!(groups[0].notification_ids(), vec![5, 4, 3]);
    }

    #[test]
    fn groups_match_desktop_entry_case_insensitively() {
        let groups = group_notifications_by_app(vec![
            notification(2, "Chat", Some("Org.Example.Chat")),
            notification(1, "Chat", Some("org.example.chat")),
        ]);

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].notification_ids(), vec![2, 1]);
    }

    #[test]
    fn empty_desktop_entry_falls_back_to_app_name() {
        let groups = group_notifications_by_app(vec![
            notification(2, "Mail", Some("")),
            notification(1, "Mail", None),
        ]);

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].notification_ids(), vec![2, 1]);
    }

    #[test]
    fn notification_count_text_uses_singular_for_one() {
        assert_eq!(notification_count_text(1), "1 notification");
        assert_eq!(notification_count_text(2), "2 notifications");
    }

    #[test]
    fn dismiss_duration_respects_animation_flag() {
        assert_eq!(
            dismiss_duration_for_animations(true),
            DISMISS_ANIMATION_DURATION
        );
        assert_eq!(dismiss_duration_for_animations(false), Duration::ZERO);
    }
}
