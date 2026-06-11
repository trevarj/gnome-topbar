//! On-Screen Display (OSD) overlay for brightness and volume changes.
//!
//! - Small overlay window with icon + slider
//! - Layer-shell OVERLAY, non-intrusive, auto-hiding
//! - Reacts to `BrightnessService` and `AudioService` changes, ignoring the initial sync

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use tracing::trace;

use crate::services::audio::{AudioService, valid_volume_percent};
use crate::services::brightness::BrightnessService;
use crate::services::callbacks::CallbackId;
use crate::styles::{color, osd};

use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{Align, Application, Box as GtkBox, Image, Label, Orientation, Scale};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use tracing::{debug, warn};

use gnome_topbar_core::config::OsdConfig;

use crate::services::audio::AudioSnapshot;
use crate::services::background_effect::{BackgroundEffectManager, attach_blur_surface_lifecycle};
use crate::services::brightness::BrightnessSnapshot;
use crate::services::config_manager::{ConfigManager, ThemeCallbackGuard};
use crate::services::icons::{IconHandle, IconsService};
use crate::services::ipc::IpcMessage;
use crate::services::surfaces::SurfaceStyleManager;
use crate::widgets::animation::{Animation, AnimationParams, Easing};
use crate::widgets::css::POPOVER_ANIMATION_MS;
use crate::widgets::layer_shell_popover::{ANIM_SCALE_FROM, AnimDirection};
use crate::widgets::scale_box::ScaleBox;

/// Valid OSD positions for anchoring.
const VALID_POSITIONS: &[&str] = &["bottom", "left", "right", "top"];
const DEFAULT_POSITION: &str = "bottom";

fn normalize_position(position: &str) -> String {
    if VALID_POSITIONS.contains(&position) {
        position.to_string()
    } else {
        warn!(
            "Invalid OSD position '{}', using '{}'. Valid options: {}",
            position,
            DEFAULT_POSITION,
            VALID_POSITIONS.join(", ")
        );
        DEFAULT_POSITION.to_string()
    }
}

/// Simple OSD widget containing an icon and a fat slider.
///
/// This is a lightweight container without the full BaseWidget machinery.
pub struct OsdWidget {
    root: GtkBox,
    /// Normal content: icon + slider in a row
    normal_content: GtkBox,
    icon_handle: IconHandle,
    scale: Scale,
    value_label: Label,
    /// Unavailable content: big icon + message centered
    unavailable_content: GtkBox,
    unavailable_icon: Image,
    unavailable_label: Label,
}

impl OsdWidget {
    pub fn new(orientation: Orientation, icon_size: i32, show_value: bool) -> Self {
        let root = GtkBox::new(Orientation::Vertical, 0);
        root.add_css_class(osd::WIDGET);

        // === Normal content: icon + slider ===
        let normal_content = GtkBox::new(orientation, 4);
        normal_content.add_css_class(osd::NORMAL);

        let icon_handle =
            IconsService::global().create_icon("audio-volume-medium-symbolic", &[osd::ICON]);
        let icon_widget = icon_handle.widget();
        icon_widget.set_size_request(icon_size, icon_size);
        icon_widget.set_valign(Align::Center);
        icon_widget.set_halign(Align::Center);

        // Slider (display only)
        let scale = Scale::with_range(orientation, 0.0, 100.0, 1.0);
        scale.set_draw_value(false);
        scale.set_sensitive(false);
        scale.add_css_class(osd::SLIDER);

        if orientation == Orientation::Horizontal {
            scale.set_hexpand(true);
            scale.set_size_request(200, -1);
        } else {
            scale.set_vexpand(true);
            scale.set_size_request(-1, 200);
            // High values at top
            scale.set_inverted(true);
        }

        let value_label = Label::new(Some("0"));
        value_label.add_css_class(osd::VALUE);
        value_label.set_valign(Align::Center);
        value_label.set_halign(Align::Center);
        value_label.set_visible(show_value);

        // Vertical: value on top, icon on bottom. Horizontal: icon left, value right.
        if orientation == Orientation::Vertical {
            normal_content.append(&value_label);
            normal_content.append(&scale);
            normal_content.append(&icon_widget);
        } else {
            normal_content.append(&icon_widget);
            normal_content.append(&scale);
            normal_content.append(&value_label);
        }

        root.append(&normal_content);

        // === Unavailable content: centered icon + label ===
        let unavailable_content = GtkBox::new(Orientation::Vertical, 8);
        unavailable_content.add_css_class(osd::UNAVAILABLE);
        unavailable_content.set_valign(Align::Center);
        unavailable_content.set_halign(Align::Center);
        unavailable_content.set_visible(false);

        let unavailable_icon = Image::from_icon_name("audio-volume-muted-symbolic");
        unavailable_icon.set_pixel_size(32);
        unavailable_icon.add_css_class(osd::UNAVAILABLE_ICON);
        unavailable_icon.add_css_class(color::MUTED);
        unavailable_content.append(&unavailable_icon);

        let unavailable_label = Label::new(Some("Unavailable"));
        unavailable_label.add_css_class(osd::UNAVAILABLE_LABEL);
        unavailable_label.add_css_class(color::MUTED);
        unavailable_content.append(&unavailable_label);

        root.append(&unavailable_content);

        Self {
            root,
            normal_content,
            icon_handle,
            scale,
            value_label,
            unavailable_content,
            unavailable_icon,
            unavailable_label,
        }
    }

    pub fn widget(&self) -> &GtkBox {
        &self.root
    }

    pub fn set_value(&self, value: u32, max_value: u32) {
        let max_value = max_value.max(1);
        // `max_value` is the recommended UI scale maximum. The actual audio
        // value may exceed it; GTK saturates the bar while the label stays exact.
        set_scale_range_if_changed(&self.scale, 0.0, max_value as f64);
        set_scale_value_if_changed(&self.scale, value as f64);
        set_label_text_if_changed(&self.value_label, &value.to_string());
        // Show normal content, hide unavailable
        set_visible_if_changed(&self.normal_content, true);
        set_visible_if_changed(&self.unavailable_content, false);
    }

    /// Set the widget to "unavailable" state with icon and message.
    pub fn set_unavailable(&self, icon_name: &str, message: &str) {
        // Update unavailable content
        set_image_icon_if_changed(&self.unavailable_icon, icon_name);
        set_label_text_if_changed(&self.unavailable_label, message);
        // Show unavailable content, hide normal
        set_visible_if_changed(&self.normal_content, false);
        set_visible_if_changed(&self.unavailable_content, true);
    }

    pub fn set_icon(&self, icon_name: &str) {
        self.icon_handle.set_icon(icon_name);
    }
}

fn set_label_text_if_changed(label: &Label, text: &str) {
    if label.text().as_str() != text {
        label.set_text(text);
    }
}

fn set_visible_if_changed<W: IsA<gtk4::Widget>>(widget: &W, visible: bool) {
    if widget.as_ref().is_visible() != visible {
        widget.as_ref().set_visible(visible);
    }
}

fn set_scale_range_if_changed(scale: &Scale, lower: f64, upper: f64) {
    let adjustment = scale.adjustment();
    if (adjustment.lower() - lower).abs() >= f64::EPSILON
        || (adjustment.upper() - upper).abs() >= f64::EPSILON
    {
        scale.set_range(lower, upper);
    }
}

fn set_scale_value_if_changed(scale: &Scale, value: f64) {
    if (scale.value() - value).abs() >= f64::EPSILON {
        scale.set_value(value);
    }
}

fn set_image_icon_if_changed(image: &Image, icon_name: &str) {
    if image.icon_name().as_deref() != Some(icon_name) {
        image.set_icon_name(Some(icon_name));
    }
}

/// Overlay window for displaying the OSD.
///
/// Uses layer-shell to create a floating overlay that:
/// - Appears above other windows (OVERLAY layer)
/// - Does not take keyboard focus
/// - Does not reserve screen space (exclusive_zone = 0)
/// - Auto-hides after a timeout
pub struct OsdOverlay {
    window: gtk4::Window,
    anim_shell: ScaleBox,
    osd_widget: OsdWidget,
    timeout_ms: u32,
    hide_source: RefCell<Option<glib::SourceId>>,
    /// Frame-clock animation driving the entrance/exit opacity + scale fade.
    anim: Animation,

    // Brightness state tracking.
    brightness_baseline_seen: Cell<bool>,
    last_brightness: Cell<u32>,

    // Audio state tracking.
    audio_baseline_seen: Cell<bool>,
    last_volume: Cell<u32>,
    last_muted: Cell<bool>,

    // Callback IDs for deterministic cleanup.
    brightness_callback_id: Cell<Option<CallbackId>>,
    audio_callback_id: Cell<Option<CallbackId>>,
    theme_callback_guard: RefCell<Option<ThemeCallbackGuard>>,
}

impl OsdOverlay {
    /// Create a new OSD overlay bound to the given application and config.
    ///
    /// The overlay subscribes to the global `BrightnessService` and will
    /// show when the brightness percentage changes (after the initial sync).
    pub fn new(app: &Application, osd_config: &OsdConfig) -> Rc<Self> {
        let position = normalize_position(&osd_config.position);
        let timeout_ms = osd_config.timeout_ms;

        let window = gtk4::Window::builder()
            .application(app)
            .decorated(false)
            .resizable(false)
            .build();

        window.add_css_class(osd::WRAPPER);

        // Set up layer shell defaults.
        Self::setup_layer_shell_defaults(&window);

        // Layout/orientation based on position.
        let is_vertical = matches!(position.as_str(), "left" | "right");
        let orientation = if is_vertical {
            Orientation::Vertical
        } else {
            Orientation::Horizontal
        };

        // Content container with surface styling.
        let container = GtkBox::new(Orientation::Vertical, 0);
        container.add_css_class(osd::CONTAINER);
        container.add_css_class(osd::OSD);
        if is_vertical {
            container.add_css_class(osd::VERTICAL);
        } else {
            container.add_css_class(osd::HORIZONTAL);
        }

        // Apply theme surface styles with larger widget radius for pill shape at max radius.
        SurfaceStyleManager::global().apply_surface_styles_with_radius(
            &container,
            false,
            "var(--radius-widget-lg)",
        );

        // Child OSD widget.
        let osd_widget = OsdWidget::new(orientation, 24, osd_config.show_value);
        container.append(osd_widget.widget());
        let anim_shell = ScaleBox::new();
        anim_shell.set_opacity(0.0);
        anim_shell.set_scale(ANIM_SCALE_FROM);
        anim_shell.set_child(&container);
        window.set_child(Some(&anim_shell));

        // Apply Pango font attributes to all labels if enabled in config.
        // This is the central hook for OSD - widgets create standard
        // GTK labels, and we apply Pango attributes here after the tree is built.
        SurfaceStyleManager::global().apply_pango_attrs_all(&container);

        // Anchor window according to position.
        Self::apply_position(&window, &position);

        let theme_callback_guard = attach_blur_surface_lifecycle(
            &window,
            |win: &gtk4::Window| win.child(),
            || {
                // Same as `--radius-widget-lg: calc(widget-radius * 2)` in theme CSS.
                ConfigManager::global().widget_border_radius() as i32 * 2
            },
        );

        let anim = Animation::new(&anim_shell);

        let overlay = Rc::new(Self {
            window,
            anim_shell,
            osd_widget,
            timeout_ms,
            hide_source: RefCell::new(None),
            anim,
            brightness_baseline_seen: Cell::new(false),
            last_brightness: Cell::new(0),
            audio_baseline_seen: Cell::new(false),
            last_volume: Cell::new(0),
            last_muted: Cell::new(false),
            brightness_callback_id: Cell::new(None),
            audio_callback_id: Cell::new(None),
            theme_callback_guard: RefCell::new(Some(theme_callback_guard)),
        });

        overlay.connect_brightness();
        overlay.connect_audio();

        overlay
    }

    /// Show the overlay with a specific icon + value.
    pub fn show_value(self: &Rc<Self>, icon_name: &str, value: u32, max_value: u32) {
        self.osd_widget.set_icon(icon_name);
        self.osd_widget.set_value(value, max_value);

        self.show_window();
        self.reset_hide_timer();
    }

    /// Brightness-specific helper: compute icon from percent and show.
    pub fn show_brightness(self: &Rc<Self>, value: u32) {
        let icon = if value == 0 {
            "display-brightness-off-symbolic"
        } else if value < 33 {
            "display-brightness-low-symbolic"
        } else if value < 67 {
            "display-brightness-medium-symbolic"
        } else {
            "display-brightness-high-symbolic"
        };
        self.show_value(icon, value, 100);
    }

    /// Volume-specific helper: compute icon from volume/mute state and show.
    pub fn show_volume(self: &Rc<Self>, volume: u32, muted: bool, max_volume: u32) {
        let icon = if muted || volume == 0 {
            "audio-volume-muted-symbolic"
        } else if volume < 33 {
            "audio-volume-low-symbolic"
        } else if volume < 67 {
            "audio-volume-medium-symbolic"
        } else {
            "audio-volume-high-symbolic"
        };
        self.show_value(icon, volume, max_volume);
    }

    /// Show OSD indicating volume control is unavailable (device not ready).
    pub fn show_volume_unavailable(self: &Rc<Self>) {
        self.osd_widget
            .set_unavailable("audio-volume-muted-symbolic", "Play audio to enable");

        self.show_window();
        self.reset_hide_timer();
    }

    // Internal: layer shell

    fn setup_layer_shell_defaults(window: &gtk4::Window) {
        if gdk::Display::default().is_some() {
            window.init_layer_shell();
            window.set_namespace(Some("gnome-topbar-osd"));
            window.set_layer(Layer::Overlay);
            window.set_exclusive_zone(0);

            if let Err(err) = std::panic::catch_unwind(|| {
                window.set_keyboard_mode(KeyboardMode::None);
            }) {
                debug!("OsdOverlay: failed to set keyboard mode: {:?}", err);
            }
        }
    }

    fn apply_position(window: &gtk4::Window, position: &str) {
        for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
            window.set_anchor(edge, false);
        }

        match position {
            "bottom" => {
                window.set_anchor(Edge::Bottom, true);
                window.set_margin(Edge::Bottom, 48);
            }
            "top" => {
                window.set_anchor(Edge::Top, true);
                window.set_margin(Edge::Top, 48);
            }
            "left" => {
                window.set_anchor(Edge::Left, true);
                window.set_margin(Edge::Left, 24);
            }
            "right" => {
                window.set_anchor(Edge::Right, true);
                window.set_margin(Edge::Right, 24);
            }
            // normalize_position guarantees only valid values, but match must be exhaustive
            _ => unreachable!("Invalid position after normalization"),
        }
    }

    fn reset_hide_timer(self: &Rc<Self>) {
        if self.timeout_ms == 0 {
            return;
        }

        if let Some(src) = self.hide_source.borrow_mut().take() {
            src.remove();
        }

        let timeout = self.timeout_ms;
        let this_weak = Rc::downgrade(self);

        let source_id = glib::timeout_add_local(Duration::from_millis(timeout as u64), move || {
            if let Some(this) = this_weak.upgrade() {
                *this.hide_source.borrow_mut() = None;
                this.hide_window();
            }
            glib::ControlFlow::Break
        });

        *self.hide_source.borrow_mut() = Some(source_id);
    }

    fn show_window(self: &Rc<Self>) {
        let was_visible = self.window.is_visible();
        self.window.set_visible(true);
        self.window.present();

        if !ConfigManager::global().animations_enabled() {
            self.anim.cancel();
            self.anim_shell.set_opacity(1.0);
            self.anim_shell.set_scale(1.0);
            return;
        }

        // Do not replay the entrance animation on repeated volume/brightness
        // updates while the OSD is already fully visible.
        let already_open = was_visible
            && !self.anim.is_running()
            && self.anim_shell.opacity() >= 1.0 - f64::EPSILON;
        if already_open {
            self.anim_shell.set_opacity(1.0);
            self.anim_shell.set_scale(1.0);
            return;
        }

        self.start_animation(AnimDirection::Opening);
    }

    fn hide_window(self: &Rc<Self>) {
        if let Some(blur) = BackgroundEffectManager::global() {
            blur.remove_blur_region(&self.window);
        }

        if !ConfigManager::global().animations_enabled() {
            self.anim.cancel();
            self.anim_shell.set_opacity(0.0);
            self.anim_shell.set_scale(ANIM_SCALE_FROM);
            self.window.set_visible(false);
            return;
        }

        self.start_animation(AnimDirection::Closing);
    }

    /// Drive the opacity + scale fade toward `direction`'s target.
    ///
    /// The shell's live opacity is the current visual progress (the per-frame
    /// callback writes it), so a mid-flight reversal starts from there. The
    /// segment duration is proportional to the remaining distance, matching the
    /// popover so a half-open OSD that reverses takes proportionally less time.
    fn start_animation(self: &Rc<Self>, direction: AnimDirection) {
        let target = match direction {
            AnimDirection::Opening => 1.0,
            AnimDirection::Closing => 0.0,
        };
        let start = self.anim_shell.opacity();
        let distance = (target - start).abs();

        let anim_shell = self.anim_shell.clone();
        let apply = move |progress: f64| {
            anim_shell.set_opacity(progress);
            let scale = ANIM_SCALE_FROM + (1.0 - ANIM_SCALE_FROM) * progress;
            anim_shell.set_scale(scale);
        };

        let window = self.window.clone();
        let on_done = move || {
            if direction == AnimDirection::Closing {
                window.set_visible(false);
            }
        };

        let on_frame = move |eased: f64| apply(start + (target - start) * eased);

        let duration_ms = (POPOVER_ANIMATION_MS as f64 * distance).round() as u64;
        self.anim.start(
            AnimationParams::new(duration_ms).with_easing(Easing::EaseOutQuintic),
            Box::new(on_frame),
            Some(Box::new(on_done)),
        );
    }

    // Internal: brightness integration

    fn connect_brightness(self: &Rc<Self>) {
        let service = BrightnessService::global();
        let this_weak = Rc::downgrade(self);

        let id = service.connect(move |snapshot: &BrightnessSnapshot| {
            if let Some(this) = this_weak.upgrade() {
                this.on_brightness_changed(snapshot);
            }
        });
        self.brightness_callback_id.set(Some(id));
    }

    fn on_brightness_changed(self: &Rc<Self>, snapshot: &BrightnessSnapshot) {
        // Ignore if brightness is not currently controllable/meaningful.
        if !snapshot.available {
            // Reset baseline so that when it becomes available again we treat
            // the next value as a fresh baseline.
            self.brightness_baseline_seen.set(false);
            return;
        }

        let value = snapshot.percent.clamp(0, 100);

        // Use an explicit readiness + baseline handshake instead of a
        // time-based grace period. We only start showing OSD once the
        // service reports itself as ready and we've captured a baseline.
        let service_ready = BrightnessService::global().is_ready();
        if !service_ready {
            self.brightness_baseline_seen.set(false);
            self.last_brightness.set(value);
            return;
        }

        if !self.brightness_baseline_seen.get() {
            self.brightness_baseline_seen.set(true);
            self.last_brightness.set(value);
            return;
        }

        if self.last_brightness.get() == value {
            return;
        }

        self.last_brightness.set(value);
        self.show_brightness(value);
    }

    // Internal: audio integration

    fn connect_audio(self: &Rc<Self>) {
        let service = AudioService::global();
        let this_weak = Rc::downgrade(self);

        let id = service.connect(move |snapshot: &AudioSnapshot| {
            if let Some(this) = this_weak.upgrade() {
                this.on_audio_changed(snapshot);
            }
        });
        self.audio_callback_id.set(Some(id));
    }

    fn on_audio_changed(self: &Rc<Self>, snapshot: &AudioSnapshot) {
        // Ignore if audio is not currently controllable/meaningful.
        if !snapshot.available {
            // Reset baseline so that when it becomes available again we treat
            // the next value as a fresh baseline.
            self.audio_baseline_seen.set(false);
            return;
        }

        let volume = snapshot.volume;
        let muted = snapshot.muted;
        let control_available = snapshot.control_available;

        let service = AudioService::global();

        // Keep the OSD quiet while the audio service is in its initial
        // post-connection settle period. Pulse may emit several updates as
        // devices are discovered and defaults are resolved. We track the
        // latest values during this time so that when the settle period
        // ends, we have a proper baseline to compare against.
        if service.in_initial_settle() {
            self.audio_baseline_seen.set(true);
            self.last_volume.set(volume);
            self.last_muted.set(muted);
            return;
        }

        // If we haven't seen any values yet (service wasn't ready), treat
        // this as baseline establishment.
        if !service.is_ready() || !self.audio_baseline_seen.get() {
            self.audio_baseline_seen.set(true);
            self.last_volume.set(volume);
            self.last_muted.set(muted);
            return;
        }

        // Check if anything changed from our tracked baseline.
        if self.last_volume.get() == volume && self.last_muted.get() == muted {
            return;
        }

        self.last_volume.set(volume);
        self.last_muted.set(muted);

        // If control is not available (sink suspended), show a "blocked" icon
        if !control_available {
            self.show_volume_unavailable();
            return;
        }

        self.show_volume(volume, muted, AudioService::global().user_max_percent());
    }

    // Public: IPC message handling (called by the panel-level IPC listener)

    /// Handle an OSD-relevant IPC message from the panel listener.
    ///
    /// Only OSD-visual messages (Volume, VolumeUnavailable, Brightness) are
    /// handled here. Non-OSD messages (e.g., ToggleInhibitor) are handled
    /// at the panel level and should not be passed to this method.
    pub fn handle_ipc_message(self: &Rc<Self>, msg: &IpcMessage) {
        match msg {
            IpcMessage::Volume { percent, muted } => {
                debug!("OSD: received volume {}% muted={}", percent, muted);
                let percent = valid_volume_percent(*percent);
                // Notify AudioService of the external volume request so
                // behavioral detection can track whether the backend responded.
                let audio = AudioService::global();
                audio.note_external_volume_request(percent);

                // Check if control is available before showing normal volume OSD
                let snapshot = audio.current();
                if snapshot.available && !snapshot.control_available {
                    // Backend is up but not accepting volume changes
                    self.show_volume_unavailable();
                } else {
                    self.show_volume(percent, *muted, AudioService::global().user_max_percent());
                }
            }
            IpcMessage::VolumeUnavailable => {
                debug!("OSD: received volume_unavailable");
                self.show_volume_unavailable();
            }
            IpcMessage::Brightness { percent } => {
                debug!("OSD: received brightness {}%", percent);
                self.show_brightness(*percent);
            }
            // Non-OSD messages are handled at the panel level, not here.
            IpcMessage::ToggleInhibitor
            | IpcMessage::Bar { .. }
            | IpcMessage::Popover { .. }
            | IpcMessage::Reload => trace!("OSD: ignoring non-OSD message: {:?}", msg),
        }
    }
}

impl Drop for OsdOverlay {
    fn drop(&mut self) {
        if let Some(id) = self.brightness_callback_id.take() {
            BrightnessService::global().disconnect(id);
        }
        if let Some(id) = self.audio_callback_id.take() {
            AudioService::global().disconnect(id);
        }
        // ThemeCallbackGuard handles disconnect_theme_callback on drop.
        drop(self.theme_callback_guard.borrow_mut().take());
    }
}
