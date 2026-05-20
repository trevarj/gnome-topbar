//! Clock widget - displays the current time.
//!
//! Updates on minute boundaries to minimize CPU usage.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::Timelike;
use gnome_topbar_core::config::WidgetEntry;
use gtk4::glib::{self, SourceId};
use gtk4::prelude::*;
use gtk4::{Align, Application, Box as GtkBox, Label, Orientation, Overlay, Widget};
use tracing::debug;

use crate::services::callbacks::CallbackId;
use crate::services::icons::{IconHandle, IconsService};
use crate::services::notification::{NotificationService, URGENCY_CRITICAL};
use crate::services::tooltip::TooltipManager;
use crate::styles::widget as wgt;
use crate::widgets::WidgetConfig;
use crate::widgets::base::BaseWidget;
use crate::widgets::calendar_popover::build_clock_calendar_popover;
use crate::widgets::control_panel::build_clock_control_panel;
use crate::widgets::notifications_toast::NotificationToastManager;
use crate::widgets::warn_unknown_options;

/// Default format string for the clock display.
const DEFAULT_FORMAT: &str = "%a %d %H:%M";
const MAX_TOASTS_PER_BURST: u32 = 3;
const TOAST_BURST_WINDOW_SECS: f64 = 2.0;

/// Configuration for the clock widget.

#[derive(Debug, Clone)]
pub struct ClockConfig {
    /// strftime format string for the clock display.
    pub format: String,
    /// Whether to show week numbers in the calendar popover.
    pub show_week_numbers: bool,
    /// Whether the clock opens the combined control panel instead of the
    /// calendar-only popover.
    pub control_panel: bool,
    /// Optional custom widget name whose exec output is shown in the control
    /// panel weather card.
    pub control_panel_weather_widget: Option<String>,
}

impl WidgetConfig for ClockConfig {
    fn from_entry(entry: &WidgetEntry) -> Self {
        warn_unknown_options(
            "clock",
            entry,
            &[
                "format",
                "show_week_numbers",
                "control_panel",
                "control_panel_weather_widget",
            ],
        );

        let format = entry
            .options
            .get("format")
            .and_then(|v| v.as_str())
            .unwrap_or(DEFAULT_FORMAT)
            .to_string();

        let show_week_numbers = entry
            .options
            .get("show_week_numbers")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let control_panel = entry
            .options
            .get("control_panel")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let control_panel_weather_widget = entry
            .options
            .get("control_panel_weather_widget")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string);

        Self {
            format,
            show_week_numbers,
            control_panel,
            control_panel_weather_widget,
        }
    }
}

impl Default for ClockConfig {
    fn default() -> Self {
        Self {
            format: DEFAULT_FORMAT.to_string(),
            show_week_numbers: true,
            control_panel: false,
            control_panel_weather_widget: None,
        }
    }
}

/// Clock widget that displays and updates the current time.
pub struct ClockWidget {
    /// Shared base widget container.
    base: BaseWidget,
    /// The label displaying the time.
    label: Label,
    /// The format string for strftime.
    format: String,
    /// Active timer source ID for cancellation on drop.
    /// The Rc<RefCell<>> allows the closure to update the ID when
    /// it transitions from the one-shot to the repeating timer.
    timer_source: Rc<RefCell<Option<SourceId>>>,
    /// Optional compact notification indicator shown beside the clock when the
    /// clock owns the combined control panel.
    _notification_companion: Option<ClockNotificationCompanion>,
}

impl ClockWidget {
    /// Create a new clock widget with the given configuration.
    pub fn new(config: ClockConfig) -> Self {
        let base = BaseWidget::new(&[wgt::CLOCK]);

        let label = base.add_label(Some("--:--"), &[wgt::CLOCK_LABEL]);

        let show_week_numbers = config.show_week_numbers;
        let control_panel = config.control_panel;
        let control_panel_weather_widget = config.control_panel_weather_widget.clone();

        let notification_companion =
            control_panel.then(|| ClockNotificationCompanion::new(base.widget(), base.content()));

        // Shared slot for the calendar refresh callback. Populated by the
        // builder on first open, invoked by on_show on every subsequent open.
        type RefreshSlot = Rc<RefCell<Option<Rc<dyn Fn()>>>>;
        let refresh_slot: RefreshSlot = Rc::new(RefCell::new(None));

        let refresh_for_builder = refresh_slot.clone();
        let menu_handle = base.create_menu(move || {
            let (widget, refresh) = if control_panel {
                build_clock_control_panel(show_week_numbers, control_panel_weather_widget.clone())
            } else {
                build_clock_calendar_popover(show_week_numbers)
            };
            *refresh_for_builder.borrow_mut() = Some(refresh);
            widget
        });

        // Rebuild the combined panel on open so notification history is fresh.
        menu_handle.set_reuse_content(!control_panel);
        let notification_for_show = notification_companion
            .as_ref()
            .map(|companion| Rc::clone(&companion.inner));
        menu_handle.set_on_show(move || {
            if let Some(ref inner) = notification_for_show {
                inner.mark_as_seen();
                inner.on_service_update(&NotificationService::global());
            }
            if let Some(ref cb) = *refresh_slot.borrow() {
                cb();
            }
        });

        let timer_source = Rc::new(RefCell::new(None));

        let widget = Self {
            base,
            label,
            format: config.format,
            timer_source,
            _notification_companion: notification_companion,
        };

        widget.update_time();
        widget.schedule_minute_tick();

        widget
    }

    /// Get the root GTK widget for embedding in the bar.
    pub fn widget(&self) -> &gtk4::Box {
        self.base.widget()
    }

    /// Update the displayed time.
    fn update_time(&self) {
        let now = chrono::Local::now();
        let text = now.format(&self.format).to_string();
        self.label.set_label(&text);
        debug!("Clock updated: {}", text);
    }

    /// Schedule the next tick on the next minute boundary.
    fn schedule_minute_tick(&self) {
        let now = chrono::Local::now();
        let delay_seconds = 60 - now.second();

        let label = self.label.clone();
        let format = self.format.clone();
        let timer_source = Rc::clone(&self.timer_source);

        let source_id = glib::timeout_add_seconds_local_once(delay_seconds, move || {
            let now = chrono::Local::now();
            let text = now.format(&format).to_string();
            label.set_label(&text);

            let label_clone = label.clone();
            let format_clone = format.clone();
            let timer_source_clone = Rc::clone(&timer_source);
            let repeating_id = glib::timeout_add_seconds_local(60, move || {
                let now = chrono::Local::now();
                let text = now.format(&format_clone).to_string();
                label_clone.set_label(&text);
                glib::ControlFlow::Continue
            });

            *timer_source_clone.borrow_mut() = Some(repeating_id);
        });

        *self.timer_source.borrow_mut() = Some(source_id);

        debug!("Clock tick scheduled in {} seconds", delay_seconds);
    }
}

impl Drop for ClockWidget {
    fn drop(&mut self) {
        // Cancel any active timer to prevent callbacks after widget is dropped
        if let Some(source_id) = self.timer_source.borrow_mut().take() {
            source_id.remove();
            debug!("Clock timer cancelled on drop");
        }
    }
}

struct ClockNotificationCompanion {
    inner: Rc<ClockNotificationInner>,
    service_callback_id: CallbackId,
}

struct ClockNotificationInner {
    icon_handle: IconHandle,
    badge: Widget,
    container: GtkBox,
    parent_widget: GtkBox,
    known_ids: RefCell<HashMap<u32, f64>>,
    toast_manager: RefCell<Option<Rc<NotificationToastManager>>>,
    last_seen_timestamp: Cell<f64>,
    toast_burst_started_at: Cell<f64>,
    toast_burst_count: Cell<u32>,
    app: RefCell<Option<Application>>,
}

impl ClockNotificationCompanion {
    fn new(parent_widget: &GtkBox, parent_content: &GtkBox) -> Self {
        let icons = IconsService::global();

        let container = GtkBox::new(Orientation::Horizontal, 0);
        container.add_css_class(wgt::CLOCK_NOTIFICATIONS);
        container.set_halign(Align::Center);
        container.set_valign(Align::Center);
        container.set_visible(false);

        let overlay = Overlay::new();
        overlay.set_valign(Align::Center);

        let icon_handle = icons.create_icon("notifications", &[wgt::NOTIFICATION_ICON]);
        overlay.set_child(Some(&icon_handle.widget()));

        let badge = GtkBox::new(Orientation::Horizontal, 0);
        badge.add_css_class(wgt::NOTIFICATION_BADGE);
        badge.add_css_class(wgt::NOTIFICATION_BADGE_DOT);
        badge.set_visible(false);
        badge.set_halign(Align::End);
        badge.set_valign(Align::Start);
        badge.set_size_request(4, 4);
        overlay.add_overlay(&badge);

        container.append(&overlay);
        parent_content.append(&container);

        let inner = Rc::new(ClockNotificationInner {
            icon_handle,
            badge: badge.upcast(),
            container,
            parent_widget: parent_widget.clone(),
            known_ids: RefCell::new(HashMap::new()),
            toast_manager: RefCell::new(None),
            last_seen_timestamp: Cell::new(0.0),
            toast_burst_started_at: Cell::new(0.0),
            toast_burst_count: Cell::new(0),
            app: RefCell::new(None),
        });

        let service = NotificationService::global();
        *inner.known_ids.borrow_mut() = service
            .notifications()
            .iter()
            .map(|n| (n.id, n.timestamp))
            .collect();

        {
            let service_for_action = NotificationService::global();
            let on_action = move |id: u32, action_id: &str| {
                service_for_action.invoke_action(id, action_id);
            };

            let inner_weak_for_toast = Rc::downgrade(&inner);
            let on_toast_removed = move || {
                let inner_weak = inner_weak_for_toast.clone();
                glib::idle_add_local_once(move || {
                    if let Some(inner) = inner_weak.upgrade() {
                        inner.on_service_update(&NotificationService::global());
                    }
                });
            };

            let manager = NotificationToastManager::new(on_action, on_toast_removed);
            *inner.toast_manager.borrow_mut() = Some(manager);
        }

        let service_callback_id = {
            let inner_weak = Rc::downgrade(&inner);
            service.connect(move |svc| {
                if let Some(inner) = inner_weak.upgrade() {
                    inner.on_service_update(svc);
                }
            })
        };
        inner.on_service_update(&service);

        Self {
            inner,
            service_callback_id,
        }
    }
}

impl Drop for ClockNotificationCompanion {
    fn drop(&mut self) {
        NotificationService::global().disconnect(self.service_callback_id);
    }
}

impl ClockNotificationInner {
    fn on_service_update(&self, service: &NotificationService) {
        self.show_new_toasts(service);

        let count = service.history_count();
        self.container.set_visible(notification_indicator_visible(
            service.backend_available(),
            count,
            service.is_muted(),
        ));

        let unread = self.calculate_unread_count(service);
        self.badge.set_visible(unread > 0);

        let has_critical = service
            .history_notifications()
            .iter()
            .any(|n| n.urgency == URGENCY_CRITICAL);
        if has_critical {
            self.icon_handle.add_css_class(wgt::HAS_CRITICAL);
        } else {
            self.icon_handle.remove_css_class(wgt::HAS_CRITICAL);
        }

        let tooltip_manager = TooltipManager::global();
        if !service.backend_available() {
            self.icon_handle.add_css_class(wgt::BACKEND_UNAVAILABLE);
            tooltip_manager.set_styled_tooltip(
                &self.parent_widget,
                "Notification daemon unavailable (another daemon is running)",
            );
            return;
        }

        self.icon_handle.remove_css_class(wgt::BACKEND_UNAVAILABLE);
        self.icon_handle.set_icon(if service.is_muted() {
            "notifications-disabled"
        } else {
            "notifications"
        });

        let tooltip = if count > 0 {
            if unread > 0 {
                if unread == 1 {
                    format!("1 new notification ({} total)", count)
                } else {
                    format!("{} new notifications ({} total)", unread, count)
                }
            } else if count == 1 {
                "1 notification".to_string()
            } else {
                format!("{} notifications", count)
            }
        } else if service.is_muted() {
            "Notifications muted".to_string()
        } else {
            "No notifications".to_string()
        };
        tooltip_manager.set_styled_tooltip(&self.parent_widget, &tooltip);
    }

    fn calculate_unread_count(&self, service: &NotificationService) -> usize {
        if !service.backend_available() {
            return 0;
        }

        let active_toast_ids = self
            .toast_manager
            .borrow()
            .as_ref()
            .map(|tm| tm.active_ids())
            .unwrap_or_default();

        let last_seen = self.last_seen_timestamp.get();
        service
            .history_notifications()
            .iter()
            .filter(|n| {
                if active_toast_ids.contains(&n.id) {
                    return false;
                }
                last_seen <= 0.0 || n.timestamp > last_seen
            })
            .count()
    }

    fn show_new_toasts(&self, service: &NotificationService) {
        if !service.backend_available() {
            return;
        }

        let current: HashMap<u32, f64> = service
            .notifications()
            .iter()
            .map(|n| (n.id, n.timestamp))
            .collect();
        let current_ids: HashSet<u32> = current.keys().copied().collect();

        if let Some(toast_manager) = self.toast_manager.borrow().as_ref() {
            toast_manager.sync_with_service_ids(&current_ids);
        }

        if service.is_muted() {
            *self.known_ids.borrow_mut() = current;
            return;
        }

        let known = self.known_ids.borrow().clone();
        let to_toast: Vec<u32> = current
            .iter()
            .filter(|(id, ts)| match known.get(id) {
                None => true,
                Some(prev_ts) => *ts > prev_ts,
            })
            .map(|(id, _)| *id)
            .collect();

        if !to_toast.is_empty() {
            debug!(
                "ClockNotificationCompanion: {} new toast candidate(s)",
                to_toast.len()
            );
        }

        let mut suppressed_transients = Vec::new();
        if !to_toast.is_empty()
            && let (Some(toast_manager), Some(app)) =
                (&*self.toast_manager.borrow(), self.get_application())
        {
            for id in &to_toast {
                if let Some(notification) = service.get(*id) {
                    let now = now_secs();
                    if self.consume_toast_burst_slot(now) {
                        toast_manager.show(&app, &notification);
                    } else {
                        debug!(
                            "ClockNotificationCompanion: suppressing toast during burst id={}, app={}, transient={}",
                            notification.id, notification.app_name, notification.transient
                        );
                        if notification.transient {
                            suppressed_transients.push(notification.id);
                        }
                    }
                }
            }
        }

        *self.known_ids.borrow_mut() = current;

        for id in suppressed_transients {
            NotificationService::global().close(id);
        }
    }

    fn get_application(&self) -> Option<Application> {
        if let Some(app) = self.app.borrow().as_ref() {
            return Some(app.clone());
        }

        let root = self.parent_widget.root()?;
        let window = root.downcast_ref::<gtk4::Window>()?;
        let app = window.application()?;
        *self.app.borrow_mut() = Some(app.clone());
        Some(app)
    }

    fn mark_as_seen(&self) {
        self.last_seen_timestamp.set(now_secs());
    }

    fn consume_toast_burst_slot(&self, now: f64) -> bool {
        let (started_at, count, allowed) = toast_burst_next_state(
            self.toast_burst_started_at.get(),
            self.toast_burst_count.get(),
            now,
        );
        self.toast_burst_started_at.set(started_at);
        self.toast_burst_count.set(count);
        allowed
    }
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn toast_burst_next_state(started_at: f64, count: u32, now: f64) -> (f64, u32, bool) {
    if started_at <= 0.0 || now - started_at > TOAST_BURST_WINDOW_SECS || now < started_at {
        return (now, 1, true);
    }

    if count >= MAX_TOASTS_PER_BURST {
        return (started_at, count, false);
    }

    (started_at, count + 1, true)
}

fn notification_indicator_visible(
    backend_available: bool,
    history_count: usize,
    muted: bool,
) -> bool {
    !backend_available || history_count > 0 || muted
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use toml::Value;

    fn make_widget_entry(name: &str, options: HashMap<String, Value>) -> WidgetEntry {
        WidgetEntry {
            name: name.to_string(),
            options,
        }
    }

    #[test]
    fn test_clock_config_default_format() {
        let entry = make_widget_entry("clock", HashMap::new());
        let config = ClockConfig::from_entry(&entry);
        assert_eq!(config.format, "%a %d %H:%M");
    }

    #[test]
    fn test_clock_config_custom_format() {
        let mut options = HashMap::new();
        options.insert("format".to_string(), Value::String("%H:%M".to_string()));
        let entry = make_widget_entry("clock", options);
        let config = ClockConfig::from_entry(&entry);
        assert_eq!(config.format, "%H:%M");
    }

    #[test]
    fn test_clock_config_ignores_non_string_format() {
        let mut options = HashMap::new();
        options.insert("format".to_string(), Value::Integer(123));
        let entry = make_widget_entry("clock", options);
        let config = ClockConfig::from_entry(&entry);
        // Falls back to default when format is not a string
        assert_eq!(config.format, "%a %d %H:%M");
    }

    #[test]
    fn test_clock_config_default_impl() {
        let config = ClockConfig::default();
        assert_eq!(config.format, "%a %d %H:%M");
    }

    #[test]
    fn test_clock_config_control_panel_options() {
        let mut options = HashMap::new();
        options.insert("control_panel".to_string(), Value::Boolean(true));
        options.insert(
            "control_panel_weather_widget".to_string(),
            Value::String("custom-weather".to_string()),
        );
        let config = ClockConfig::from_entry(&make_widget_entry("clock", options));

        assert!(config.control_panel);
        assert_eq!(
            config.control_panel_weather_widget.as_deref(),
            Some("custom-weather")
        );
    }

    #[test]
    fn test_clock_config_ignores_empty_weather_widget() {
        let mut options = HashMap::new();
        options.insert(
            "control_panel_weather_widget".to_string(),
            Value::String(String::new()),
        );

        let config = ClockConfig::from_entry(&make_widget_entry("clock", options));

        assert!(config.control_panel_weather_widget.is_none());
    }

    #[test]
    fn notification_indicator_remains_visible_when_muted_without_history() {
        assert!(notification_indicator_visible(true, 0, true));
    }

    #[test]
    fn notification_indicator_hides_when_available_unmuted_and_empty() {
        assert!(!notification_indicator_visible(true, 0, false));
    }

    #[test]
    fn notification_indicator_visible_for_history_or_unavailable_backend() {
        assert!(notification_indicator_visible(true, 1, false));
        assert!(notification_indicator_visible(false, 0, false));
    }

    #[test]
    fn toast_burst_allows_first_three_inside_window() {
        let mut started_at = 0.0;
        let mut count = 0;

        for _ in 0..MAX_TOASTS_PER_BURST {
            let next = toast_burst_next_state(started_at, count, 10.0);
            started_at = next.0;
            count = next.1;
            assert!(next.2);
        }

        let next = toast_burst_next_state(started_at, count, 10.5);
        assert!(!next.2);
        assert_eq!(next.1, MAX_TOASTS_PER_BURST);
    }

    #[test]
    fn toast_burst_resets_after_window_or_clock_jump() {
        assert_eq!(
            toast_burst_next_state(10.0, MAX_TOASTS_PER_BURST, 13.0),
            (13.0, 1, true)
        );
        assert_eq!(
            toast_burst_next_state(10.0, MAX_TOASTS_PER_BURST, 9.0),
            (9.0, 1, true)
        );
    }
}
