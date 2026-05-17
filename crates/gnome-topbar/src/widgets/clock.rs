//! Clock widget - displays the current time.
//!
//! Updates on minute boundaries to minimize CPU usage.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::Timelike;
use gnome_topbar_core::config::WidgetEntry;
use gtk4::glib::{self, SourceId};
use gtk4::prelude::*;
use gtk4::{Align, Application, Box as GtkBox, Button, Label, Orientation, Overlay, Widget};
use tracing::debug;

use crate::services::callbacks::CallbackId;
use crate::services::config_manager::ConfigManager;
use crate::services::icons::{IconHandle, IconsService};
use crate::services::media::{MediaService, MediaSnapshot, PlaybackStatus};
use crate::services::notification::{NotificationService, URGENCY_CRITICAL};
use crate::services::tooltip::TooltipManager;
use crate::styles::widget as wgt;
use crate::styles::{button, icon, media};
use crate::widgets::WidgetConfig;
use crate::widgets::base::{BaseWidget, vp_button};
use crate::widgets::calendar_popover::build_clock_calendar_popover;
use crate::widgets::control_panel::build_clock_control_panel;
use crate::widgets::media_components::{ArtState, art_radius_percent, show_player_icon_in_art};
use crate::widgets::notifications_toast::NotificationToastManager;
use crate::widgets::rounded_picture::RoundedPicture;
use crate::widgets::warn_unknown_options;

/// Default format string for the clock display.
const DEFAULT_FORMAT: &str = "%a %d %H:%M";
const CLOCK_MEDIA_ART_SCALE: f64 = 0.66;

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
    /// Whether the compact media companion beside the clock shows album art.
    pub media_thumbnail: bool,
    /// Whether the compact media play/pause button pulses while playing.
    pub media_eq: bool,
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
                "media_thumbnail",
                "media_eq",
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

        let media_thumbnail = entry
            .options
            .get("media_thumbnail")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let media_eq = entry
            .options
            .get("media_eq")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        Self {
            format,
            show_week_numbers,
            control_panel,
            control_panel_weather_widget,
            media_thumbnail,
            media_eq,
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
            media_thumbnail: false,
            media_eq: true,
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
    /// Optional compact media controls shown beside the clock when the clock
    /// owns the combined control panel.
    _media_companion: Option<ClockMediaCompanion>,
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
        let media_thumbnail = config.media_thumbnail;
        let media_eq = config.media_eq;

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
        let media_companion = control_panel
            .then(|| ClockMediaCompanion::new(base.content(), media_thumbnail, media_eq));

        let widget = Self {
            base,
            label,
            format: config.format,
            timer_source,
            _media_companion: media_companion,
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
        badge.set_size_request(8, 8);
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
        self.container
            .set_visible(!service.backend_available() || count > 0);

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

        if !to_toast.is_empty()
            && let (Some(toast_manager), Some(app)) =
                (&*self.toast_manager.borrow(), self.get_application())
        {
            for id in &to_toast {
                if let Some(notification) = service.get(*id) {
                    toast_manager.show(&app, &notification);
                }
            }
        }

        *self.known_ids.borrow_mut() = current;
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
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        self.last_seen_timestamp.set(now);
    }
}

struct ClockMediaCompanion {
    media_callback_id: CallbackId,
    _art_state: Rc<RefCell<ArtState>>,
}

#[derive(Clone)]
struct ClockMediaRefs {
    container: GtkBox,
    art_picture: Option<RoundedPicture>,
    play_pause_btn: Button,
    play_pause_icon: IconHandle,
    pulse_enabled: bool,
    art_state: Rc<RefCell<ArtState>>,
    art_size: i32,
}

impl ClockMediaCompanion {
    fn new(parent: &GtkBox, show_thumbnail: bool, pulse_enabled: bool) -> Self {
        let art_size = clock_media_art_size();
        let art_state = Rc::new(RefCell::new(ArtState::new()));

        let container = GtkBox::new(Orientation::Horizontal, 2);
        container.add_css_class(wgt::CLOCK_MEDIA);
        container.set_halign(Align::Center);
        container.set_valign(Align::Center);
        container.set_visible(false);

        let art_picture = show_thumbnail.then(|| {
            let art_picture = RoundedPicture::new();
            art_picture.set_pixel_size(art_size);
            art_picture.set_corner_radius(art_size as f32 * art_radius_percent());
            art_picture.add_css_class(wgt::CLOCK_MEDIA_ART);
            art_picture.add_css_class(media::ART_SMALL);
            art_picture.set_visible(false);
            container.append(&art_picture);
            art_picture
        });

        let icons = IconsService::global();
        let play_pause_icon = icons.create_icon("media-playback-start", &[icon::ICON]);
        let play_pause_btn = vp_button();
        play_pause_btn.set_has_frame(false);
        play_pause_btn.set_focus_on_click(false);
        play_pause_btn.set_size_request(24, 24);
        play_pause_btn.set_halign(Align::Center);
        play_pause_btn.set_valign(Align::Center);
        play_pause_btn.set_child(Some(&play_pause_icon.widget()));
        play_pause_btn.add_css_class(media::CONTROL_BTN);
        play_pause_btn.add_css_class(media::CONTROL_BTN_PRIMARY);
        play_pause_btn.add_css_class(button::COMPACT);
        play_pause_btn.add_css_class(wgt::CLOCK_MEDIA_PLAY_PAUSE);
        TooltipManager::global().set_styled_tooltip(&play_pause_btn, "Play/Pause");
        play_pause_btn.connect_clicked(|_| {
            MediaService::global().play_pause();
        });
        container.append(&play_pause_btn);

        parent.append(&container);

        let refs = ClockMediaRefs {
            container,
            art_picture,
            play_pause_btn,
            play_pause_icon,
            pulse_enabled,
            art_state: art_state.clone(),
            art_size,
        };

        let media_callback_id = MediaService::global().connect(move |snapshot| {
            update_clock_media(&refs, snapshot);
        });

        Self {
            media_callback_id,
            _art_state: art_state,
        }
    }
}

impl Drop for ClockMediaCompanion {
    fn drop(&mut self) {
        MediaService::global().disconnect(self.media_callback_id);
    }
}

fn clock_media_art_size() -> i32 {
    let size = (ConfigManager::global().bar_size() as f64 * CLOCK_MEDIA_ART_SCALE).round() as i32;
    size.clamp(16, 28)
}

fn update_clock_media(refs: &ClockMediaRefs, snapshot: &MediaSnapshot) {
    let should_show = snapshot.available
        && snapshot.has_metadata()
        && snapshot.playback_status != PlaybackStatus::Stopped;

    refs.container.set_visible(should_show);
    if !should_show {
        if let Some(ref art_picture) = refs.art_picture {
            art_picture.set_visible(false);
        }
        refs.play_pause_btn.remove_css_class(media::PLAYING);
        refs.play_pause_btn.remove_css_class(media::PAUSED);
        refs.play_pause_btn.remove_css_class(media::STOPPED);
        return;
    }

    refs.play_pause_icon
        .set_icon(match snapshot.playback_status {
            PlaybackStatus::Playing => "media-playback-pause",
            PlaybackStatus::Paused | PlaybackStatus::Stopped => "media-playback-start",
        });
    refs.play_pause_btn
        .set_sensitive(snapshot.can_play || snapshot.can_pause);

    refs.play_pause_btn.remove_css_class(media::PLAYING);
    refs.play_pause_btn.remove_css_class(media::PAUSED);
    refs.play_pause_btn.remove_css_class(media::STOPPED);
    match snapshot.playback_status {
        PlaybackStatus::Playing if refs.pulse_enabled => {
            refs.play_pause_btn.add_css_class(media::PLAYING);
        }
        PlaybackStatus::Paused => refs.play_pause_btn.add_css_class(media::PAUSED),
        PlaybackStatus::Stopped => refs.play_pause_btn.add_css_class(media::STOPPED),
        PlaybackStatus::Playing => {}
    }

    if let Some(ref art_picture) = refs.art_picture {
        let art_url = snapshot.metadata.art_url.as_deref();
        let picture_for_failure = art_picture.clone();
        let player_id = snapshot.player_id.clone();
        let art_state_for_failure = refs.art_state.clone();
        let art_size = refs.art_size;
        let on_failure = move || {
            let generation = art_state_for_failure.borrow().generation;
            show_player_icon_in_art(
                &picture_for_failure,
                player_id.as_deref(),
                &art_state_for_failure,
                generation,
                art_size,
            );
        };

        ArtState::debounced_load(
            &refs.art_state,
            art_url,
            snapshot.player_id.as_deref(),
            art_picture.clone(),
            || {},
            on_failure,
        );
    }
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
        assert!(!config.media_thumbnail);
        assert!(config.media_eq);
    }

    #[test]
    fn test_clock_config_control_panel_options() {
        let mut options = HashMap::new();
        options.insert("control_panel".to_string(), Value::Boolean(true));
        options.insert(
            "control_panel_weather_widget".to_string(),
            Value::String("custom-weather".to_string()),
        );
        options.insert("media_thumbnail".to_string(), Value::Boolean(true));
        options.insert("media_eq".to_string(), Value::Boolean(false));

        let config = ClockConfig::from_entry(&make_widget_entry("clock", options));

        assert!(config.control_panel);
        assert_eq!(
            config.control_panel_weather_widget.as_deref(),
            Some("custom-weather")
        );
        assert!(config.media_thumbnail);
        assert!(!config.media_eq);
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
}
