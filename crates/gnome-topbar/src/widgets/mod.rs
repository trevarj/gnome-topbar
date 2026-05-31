//! Widget implementations for the gnome-topbar bar.
//!
//! Each widget is a self-contained GTK4 component that displays
//! some piece of information (time, battery status, etc.).
//!
//! The `WidgetFactory` constructs widgets from config entries,
//! and `BarState` owns the widget handles to keep them alive.
//!
//! # Widget Configuration
//!
//! Widget configs implement the `WidgetConfig` trait for parsing from TOML.
//! The first CSS class passed to `BaseWidget::new()` determines the widget's
//! identity for per-widget styling (e.g., `[widgets.clock].background_color`).
//! This class is also used to generate popover class names like `clock-popover`.

mod animation;
mod base;
mod battery;
mod calendar_popover;
mod clock;
mod control_panel;
mod custom;
mod headset;
mod keyboard_layout;
pub mod layer_shell_popover;
mod marquee_label;
mod media_components;
mod media_popover;
mod media_visualizer;
mod notifications_common;
mod notifications_panel;
mod notifications_toast;
mod os_logo;
mod osd;
pub(crate) mod ripple;
mod rounded_picture;
pub(crate) mod scale_box;
mod tray;
mod updates_common;
mod weather;
mod workspaces;

pub mod css;

pub mod quick_settings;

pub use base::BaseWidget;
pub use clock::{ClockConfig, ClockWidget};
pub use osd::OsdOverlay;
pub use quick_settings::QuickSettingsWindowHandle;
pub use quick_settings::{QuickSettingsConfig, QuickSettingsWidget};
pub use tray::{TrayConfig, TrayWidget};
pub use workspaces::{WorkspacesConfig, WorkspacesWidget};

pub use custom::{CustomConfig, CustomWidget};
pub use headset::{HeadsetConfig, HeadsetWidget};
pub use keyboard_layout::{KeyboardLayoutConfig, KeyboardLayoutWidget};
pub use os_logo::{OsLogoConfig, OsLogoWidget};
pub use weather::{WeatherConfig, WeatherWidget};

use gnome_topbar_core::config::WidgetEntry;
use gtk4::Widget;
use gtk4::prelude::*;
use std::any::Any;
use tracing::warn;

/// The kind of shared popover a widget opens when clicked.
///
/// Used by merge-group logic to identify adjacent widgets that can be
/// visually merged into a single button with shared hover/ripple/popover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PopoverKind {
    #[allow(dead_code)]
    System,
    /// Widget has no popover or its popover is not mergeable.
    Unmergeable,
}

/// Return the popover kind for a given widget name.
pub(crate) fn popover_kind_for(widget_name: &str) -> PopoverKind {
    let _ = widget_name;
    PopoverKind::Unmergeable
}

/// Trait for widget configuration types.
///
/// All widget configs should implement this trait to provide a consistent
/// interface for constructing configuration from TOML entries and defaulting.
///
/// # Example
///
/// ```ignore
/// #[derive(Debug, Clone)]
/// pub struct MyWidgetConfig {
///     pub enabled: bool,
/// }
///
/// impl WidgetConfig for MyWidgetConfig {
///     fn from_entry(entry: &WidgetEntry) -> Self {
///         warn_unknown_options("my_widget", entry, &["enabled"]);
///         let enabled = entry
///             .options
///             .get("enabled")
///             .and_then(|v| v.as_bool())
///             .unwrap_or(true);
///         Self { enabled }
///     }
/// }
///
/// impl Default for MyWidgetConfig {
///     fn default() -> Self {
///         Self { enabled: true }
///     }
/// }
/// ```
pub trait WidgetConfig: Sized + Default {
    /// Create configuration from a widget entry.
    ///
    /// Implementations should extract options from `entry.options` and
    /// fall back to sensible defaults for missing or invalid values.
    fn from_entry(entry: &WidgetEntry) -> Self;
}

/// Log warnings for unknown options in a widget entry.
///
/// Call this at the start of `from_entry()` implementations to warn users
/// about potential typos in their configuration.
///
/// # Example
///
/// ```ignore
/// impl WidgetConfig for MyWidgetConfig {
///     fn from_entry(entry: &WidgetEntry) -> Self {
///         warn_unknown_options("my_widget", entry, &["option_a", "option_b"]);
///         // ... parse options ...
///     }
/// }
/// ```
pub fn warn_unknown_options(widget_name: &str, entry: &WidgetEntry, known_keys: &[&str]) {
    for key in entry.options.keys() {
        if !known_keys.contains(&key.as_str()) {
            warn!(
                "Unknown option '{}' for widget '{}' - possible typo?",
                key, widget_name
            );
        }
    }
}

/// A built widget with its GTK widget and ownership handle.
pub struct BuiltWidget {
    /// The GTK widget to add to the container.
    pub widget: Widget,
    /// Opaque handle to keep the Rust-side state alive (timers, callbacks, etc.).
    pub handle: Box<dyn Any>,
}

/// Factory for constructing widgets from configuration entries.
pub struct WidgetFactory;

impl WidgetFactory {
    /// Build a widget from a config entry.
    ///
    /// Returns `None` if the widget type is not recognized.
    ///
    /// The `output_id` parameter is the monitor connector name (e.g., "eDP-1")
    /// used for per-monitor filtering in widgets like workspaces.
    pub fn build(
        entry: &WidgetEntry,
        qs_handle: Option<&QuickSettingsWindowHandle>,
        output_id: Option<&str>,
    ) -> Option<BuiltWidget> {
        match entry.name.as_str() {
            "clock" => {
                let cfg = ClockConfig::from_entry(entry);
                let clock = ClockWidget::new(cfg);
                let root = clock.widget().clone().upcast::<Widget>();
                Some(BuiltWidget {
                    widget: root,
                    handle: Box::new(clock),
                })
            }
            "workspaces" => {
                let cfg = WorkspacesConfig::from_entry(entry);
                let workspaces = WorkspacesWidget::new(cfg, output_id.map(|s| s.to_string()));
                let root = workspaces.widget().clone().upcast::<Widget>();
                Some(BuiltWidget {
                    widget: root,
                    handle: Box::new(workspaces),
                })
            }
            "tray" => {
                let cfg = TrayConfig::from_entry(entry);
                let tray = TrayWidget::new(cfg);
                let root = tray.widget().clone().upcast::<Widget>();
                Some(BuiltWidget {
                    widget: root,
                    handle: Box::new(tray),
                })
            }
            "quick_settings" => {
                let cfg = QuickSettingsConfig::from_entry(entry);

                let qs_handle = match qs_handle {
                    Some(handle) => handle.clone(),
                    None => {
                        warn!(
                            "quick_settings widget requested but no QuickSettingsWindowHandle was provided; skipping"
                        );
                        return None;
                    }
                };

                let widget = QuickSettingsWidget::new(cfg, qs_handle);
                let root = widget.widget().clone().upcast::<Widget>();
                Some(BuiltWidget {
                    widget: root,
                    handle: Box::new(widget),
                })
            }
            "keyboard_layout" => {
                let cfg = KeyboardLayoutConfig::from_entry(entry);
                let keyboard_layout = KeyboardLayoutWidget::new(cfg);
                let root = keyboard_layout.widget().clone().upcast::<Widget>();
                Some(BuiltWidget {
                    widget: root,
                    handle: Box::new(keyboard_layout),
                })
            }
            "weather" => {
                let cfg = WeatherConfig::from_entry(entry);
                let weather = WeatherWidget::new(cfg);
                let root = weather.widget().clone().upcast::<Widget>();
                Some(BuiltWidget {
                    widget: root,
                    handle: Box::new(weather),
                })
            }
            "headset" => {
                let cfg = HeadsetConfig::from_entry(entry);
                let headset = HeadsetWidget::new(cfg);
                let root = headset.widget().clone().upcast::<Widget>();
                Some(BuiltWidget {
                    widget: root,
                    handle: Box::new(headset),
                })
            }
            "os_logo" => {
                let cfg = OsLogoConfig::from_entry(entry);
                let os_logo = OsLogoWidget::new(cfg);
                let root = os_logo.widget().clone().upcast::<Widget>();
                Some(BuiltWidget {
                    widget: root,
                    handle: Box::new(os_logo),
                })
            }
            name if name.starts_with("custom-") => {
                let custom_id = &name["custom-".len()..];
                if custom_id.is_empty() {
                    warn!(
                        "Custom widget name must have an ID after 'custom-', e.g., 'custom-power'"
                    );
                    return None;
                }
                let cfg = CustomConfig::from_entry(entry);
                let widget = CustomWidget::new(custom_id, cfg);
                let root = widget.widget().clone().upcast::<Widget>();
                Some(BuiltWidget {
                    widget: root,
                    handle: Box::new(widget),
                })
            }
            name => {
                warn!("Unknown widget type: '{}', skipping", name);
                None
            }
        }
    }
}

/// Holds widget handles to keep them alive for the lifetime of the bar.
///
/// When widgets are created, their Rust-side state (timers, callbacks, etc.)
/// must be kept alive. This struct owns those handles.
pub struct BarState {
    /// Widget handles that must be kept alive.
    widget_handles: Vec<Box<dyn Any>>,
}

impl BarState {
    /// Create a new empty bar state.
    pub fn new() -> Self {
        Self {
            widget_handles: Vec::new(),
        }
    }

    /// Add a widget handle to be kept alive.
    pub fn add_handle(&mut self, handle: Box<dyn Any>) {
        self.widget_handles.push(handle);
    }

    /// Get the number of widget handles being held.
    pub fn handle_count(&self) -> usize {
        self.widget_handles.len()
    }
}

impl Default for BarState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popover_kind_system_widgets() {
        assert_eq!(popover_kind_for("cpu"), PopoverKind::Unmergeable);
        assert_eq!(popover_kind_for("memory"), PopoverKind::Unmergeable);
        assert_eq!(popover_kind_for("gpu"), PopoverKind::Unmergeable);
        assert_eq!(popover_kind_for("network_speed"), PopoverKind::Unmergeable);
    }

    #[test]
    fn popover_kind_non_system_widgets() {
        assert_eq!(popover_kind_for("clock"), PopoverKind::Unmergeable);
        assert_eq!(popover_kind_for("battery"), PopoverKind::Unmergeable);
        assert_eq!(popover_kind_for("media"), PopoverKind::Unmergeable);
        assert_eq!(popover_kind_for("unknown"), PopoverKind::Unmergeable);
    }
}
