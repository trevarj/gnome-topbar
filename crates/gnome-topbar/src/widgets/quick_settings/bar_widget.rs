//! Quick Settings bar widget - slim indicator that toggles the
//! global Quick Settings window.
//!
//! Renders status icons (network, VPN, audio, battery, bluetooth) and toggles
//! the keep-alive QS window on click.

use gtk4::gdk::BUTTON_PRIMARY;
use gtk4::prelude::*;
use gtk4::{Box as GtkBox, GestureClick};
use tracing::{debug, warn};

use super::super::battery::{
    BatteryDisplayState, battery_display_state_from_snapshot, battery_icon_name,
    battery_state_text, readable_pct, rounded_pct_value,
};
use super::QuickSettingsWindowHandle;
use super::audio_card::volume_icon_name;
use super::bluetooth_card::bt_icon_name;
use super::network_card::{NetworkIconContext, mobile_state_icon_name, network_icon_name};
use super::vpn_card::vpn_icon_name;
use crate::services::audio::{AudioService, AudioSnapshot};
use crate::services::battery::{BatteryService, BatterySnapshot};
use crate::services::bluetooth::{BluetoothService, BluetoothSnapshot};
use crate::services::callbacks::CallbackId;
use crate::services::config_manager::ConfigManager;
use crate::services::icons::IconHandle;
use crate::services::network::{NetworkService, NetworkSnapshot};
use crate::services::tooltip::TooltipManager;
use crate::services::vpn::{VpnService, VpnSnapshot};
use crate::styles::{icon, qs, state, widget};
use crate::widgets::BaseWidget;
use crate::widgets::WidgetConfig;
use crate::widgets::base::trigger_ripple_from_gesture;
use crate::widgets::warn_unknown_options;
use gnome_topbar_core::config::WidgetEntry;

macro_rules! set_visible_if_changed {
    ($widget:expr, $visible:expr $(,)?) => {{
        let widget = $widget.clone();
        let widget_ref: &gtk4::Widget = widget.as_ref();
        if widget_ref.is_visible() != $visible {
            widget_ref.set_visible($visible);
        }
    }};
}

macro_rules! set_css_class {
    ($widget:expr, $class_name:expr, $enabled:expr $(,)?) => {{
        let widget = $widget.clone();
        let widget: &gtk4::Widget = widget.as_ref();
        if $enabled {
            if !widget.has_css_class($class_name) {
                widget.add_css_class($class_name);
            }
        } else if widget.has_css_class($class_name) {
            widget.remove_css_class($class_name);
        }
    }};
}

/// Configuration for which cards are shown in Quick Settings.
///
/// These options are set in the `[widgets.quick_settings]` TOML section
/// alongside widget-level settings — see [`QuickSettingsConfig`] for a
/// complete example.
#[derive(Debug, Clone)]
pub struct QuickSettingsCardsConfig {
    /// Whether the unified Network card/icon is shown.
    /// Controls both the bar icons (Wi-Fi + cellular) and the QS Network card.
    /// Cellular UI within the card is driven by runtime modem detection.
    pub network: bool,
    pub bluetooth: bool,
    pub vpn: bool,
    pub idle_inhibitor: bool,
    pub updates: bool,
    pub audio: bool,
    pub mic: bool,
    pub brightness: bool,
    pub power: bool,
    pub battery_health: bool,
    pub resource_overview: bool,
    /// Close the Quick Settings panel when a VPN connection succeeds.
    /// Defaults to `true`. Useful when VPN connections trigger password prompts.
    pub vpn_close_on_connect: bool,
}

impl Default for QuickSettingsCardsConfig {
    fn default() -> Self {
        Self {
            network: true,
            bluetooth: true,
            vpn: true,
            idle_inhibitor: true,
            updates: true,
            audio: true,
            mic: true,
            brightness: true,
            power: true,
            battery_health: true,
            resource_overview: true,
            vpn_close_on_connect: true,
        }
    }
}

/// Configuration for the Quick Settings widget.
///
/// Includes card visibility toggles (see [`QuickSettingsCardsConfig`])
/// and widget-level settings.
///
/// ```toml
/// [widgets.quick_settings]
/// vpn = false                          # hide the VPN card
/// idle_inhibitor = false               # hide the idle inhibitor card
/// battery_health = true                # show battery health / charge-limit status
/// resource_overview = true             # show CPU, memory, and disk status
/// vpn_close_on_connect = true          # close panel when VPN connects successfully
/// audio_scroll_percentage = 5          # volume change per scroll tick (% points, 1..=25)
/// ```
#[derive(Debug, Clone)]
pub struct QuickSettingsConfig {
    /// Which cards to show in the Quick Settings panel.
    pub cards: QuickSettingsCardsConfig,
    /// Whether to show the battery indicator in the bar aggregate.
    pub battery: bool,
    /// Volume delta (percentage points) for scroll on QS widget/window.
    pub audio_scroll_percentage: i32,
}

impl WidgetConfig for QuickSettingsConfig {
    fn from_entry(entry: &WidgetEntry) -> Self {
        let known_options = &[
            "network",
            "bluetooth",
            "vpn",
            "idle_inhibitor",
            "updates",
            "audio",
            "mic",
            "brightness",
            "power",
            "battery_health",
            "resource_overview",
            "battery",
            "vpn_close_on_connect",
            "audio_scroll_percentage",
        ];
        warn_unknown_options("quick_settings", entry, known_options);

        let audio_scroll_percentage = entry
            .options
            .get("audio_scroll_percentage")
            .and_then(|v| v.as_integer())
            .map(|v| v as i32)
            .unwrap_or(QuickSettingsConfig::DEFAULT_AUDIO_SCROLL_PERCENTAGE);

        let audio_scroll_percentage = {
            let clamped = audio_scroll_percentage.clamp(1, 25);
            if clamped != audio_scroll_percentage {
                warn!(
                    "audio_scroll_percentage = {} is outside valid range 1..=25, clamping to {}",
                    audio_scroll_percentage, clamped
                );
            }
            clamped
        };

        let get_bool = |key: &str| -> bool {
            entry
                .options
                .get(key)
                .and_then(|v| v.as_bool())
                .unwrap_or(true) // default to true (shown)
        };

        Self {
            cards: QuickSettingsCardsConfig {
                network: get_bool("network"),
                bluetooth: get_bool("bluetooth"),
                vpn: get_bool("vpn"),
                idle_inhibitor: get_bool("idle_inhibitor"),
                updates: get_bool("updates"),
                audio: get_bool("audio"),
                mic: get_bool("mic"),
                brightness: get_bool("brightness"),
                power: get_bool("power"),
                battery_health: get_bool("battery_health"),
                resource_overview: get_bool("resource_overview"),
                vpn_close_on_connect: get_bool("vpn_close_on_connect"),
            },
            battery: get_bool("battery"),
            audio_scroll_percentage,
        }
    }
}

impl Default for QuickSettingsConfig {
    fn default() -> Self {
        Self {
            cards: QuickSettingsCardsConfig::default(),
            battery: true,
            audio_scroll_percentage: Self::DEFAULT_AUDIO_SCROLL_PERCENTAGE,
        }
    }
}

impl QuickSettingsConfig {
    const DEFAULT_AUDIO_SCROLL_PERCENTAGE: i32 = 5;

    pub(crate) fn enabled_control_panel_cards(&self) -> Vec<&'static str> {
        let cards = &self.cards;
        let mut names = Vec::new();
        if cards.network {
            names.push("network");
        }
        if cards.bluetooth {
            names.push("bluetooth");
        }
        if cards.vpn {
            names.push("vpn");
        }
        if cards.idle_inhibitor {
            names.push("idle_inhibitor");
        }
        if cards.updates {
            names.push("updates");
        }
        if cards.audio {
            names.push("audio");
        }
        if cards.mic {
            names.push("mic");
        }
        if cards.brightness {
            names.push("brightness");
        }
        if cards.battery_health {
            names.push("battery_health");
        }
        if cards.resource_overview {
            names.push("resource_overview");
        }
        if cards.power {
            names.push("power");
        }
        names
    }

    pub(crate) fn enabled_bar_indicators(&self) -> Vec<&'static str> {
        let cards = &self.cards;
        let mut names = Vec::new();
        if cards.network {
            names.push("network");
        }
        if cards.vpn {
            names.push("vpn");
        }
        if cards.network {
            names.push("mobile");
        }
        if cards.audio {
            names.push("audio");
        }
        if self.battery {
            names.push("battery");
        }
        if cards.bluetooth {
            names.push("bluetooth");
        }
        names
    }
}

/// Bar-side Quick Settings indicator.
pub struct QuickSettingsWidget {
    base: BaseWidget,
    /// Handle to the keep-alive QS window. Stored so we can call `destroy()`
    /// on bar teardown, ensuring the window and PopoverTracker are cleaned up.
    qs_window_handle: QuickSettingsWindowHandle,
    audio_callback_id: Option<CallbackId>,
    battery_callback_id: Option<CallbackId>,
    bluetooth_callback_id: Option<CallbackId>,
    network_wifi_callback_id: Option<CallbackId>,
    network_mobile_callback_id: Option<CallbackId>,
    vpn_callback_id: Option<CallbackId>,
}

impl QuickSettingsWidget {
    pub fn new(cfg: QuickSettingsConfig, qs_window: QuickSettingsWindowHandle) -> Self {
        let cards = &cfg.cards;
        let base = BaseWidget::new(&[widget::QUICK_SETTINGS]);
        debug!(
            control_panel_cards = ?cfg.enabled_control_panel_cards(),
            bar_indicators = ?cfg.enabled_bar_indicators(),
            "Quick Settings configured"
        );

        let mut audio_callback_id = None;
        let mut battery_callback_id = None;
        let mut bluetooth_callback_id = None;
        let mut network_wifi_callback_id = None;
        let mut network_mobile_callback_id = None;
        let mut vpn_callback_id = None;

        // Build icons only for enabled cards (order: network, VPN, mobile, audio, battery, Bluetooth)
        // Network icon (Wi-Fi / Ethernet).
        //
        // Shows the primary network connection: ethernet when plugged in,
        // Wi-Fi signal otherwise. Mobile has its own separate icon slot below.
        if cards.network {
            let snapshot = NetworkService::global().snapshot();
            let wifi_enabled = snapshot.wifi_enabled().unwrap_or(false);
            let wired_connected = snapshot.wired_connected();

            let ctx = NetworkIconContext::for_bar(&snapshot);
            let wifi_icon = base.add_icon(network_icon_name(&ctx), &[icon::ICON, icon::TEXT]);
            wifi_icon.widget().add_css_class(qs::WIFI);

            if !wifi_enabled && !wired_connected {
                wifi_icon.widget().add_css_class(qs::WIFI_DISABLED_ICON);
            }
            let wifi_connecting = snapshot.wifi_connecting();
            if wifi_connecting {
                wifi_icon.set_spinning(true);
            }

            let wifi_icon_handle = wifi_icon.clone();
            network_wifi_callback_id = Some(NetworkService::global().connect(
                move |snapshot: &NetworkSnapshot| {
                    let widget = wifi_icon_handle.widget();

                    if !snapshot.available() {
                        widget.add_css_class(state::SERVICE_UNAVAILABLE);
                        widget.remove_css_class(qs::WIFI_DISABLED_ICON);
                        wifi_icon_handle.set_spinning(false);
                        wifi_icon_handle.set_icon("network-wireless-offline-symbolic");
                        TooltipManager::global()
                            .set_styled_tooltip(&widget, "Wi-Fi: Service unavailable");
                        return;
                    }
                    widget.remove_css_class(state::SERVICE_UNAVAILABLE);

                    let enabled = snapshot.wifi_enabled().unwrap_or(false);
                    let wired_connected = snapshot.wired_connected();

                    let ctx = NetworkIconContext::for_bar(snapshot);
                    wifi_icon_handle.set_icon(network_icon_name(&ctx));

                    let wifi_connecting = snapshot.wifi_connecting();
                    wifi_icon_handle.set_spinning(wifi_connecting);

                    if !enabled && !wired_connected {
                        widget.add_css_class(qs::WIFI_DISABLED_ICON);
                    } else {
                        widget.remove_css_class(qs::WIFI_DISABLED_ICON);
                    }

                    widget.remove_css_class(state::ICON_ACTIVE);

                    let tooltip = if snapshot.wired_connected() {
                        "Ethernet connected".to_string()
                    } else if snapshot.connected() {
                        let ssid = snapshot.active_ssid().unwrap_or("Connected");
                        let strength = snapshot.active_strength();
                        if strength > 0 {
                            format!("{}\nSignal: {}%", ssid, strength)
                        } else {
                            ssid.to_string()
                        }
                    } else if let Some(ssid) = snapshot.connecting_ssid() {
                        format!("Connecting to {}", ssid)
                    } else if snapshot.wifi_device_connecting() {
                        "Connecting...".to_string()
                    } else if snapshot.wifi_enabled() == Some(false) {
                        "Wi-Fi Off".to_string()
                    } else if snapshot.scanning() {
                        "Wi-Fi: Scanning...".to_string()
                    } else {
                        "Disconnected".to_string()
                    };
                    TooltipManager::global().set_styled_tooltip(&widget, &tooltip);
                },
            ));
        }

        // VPN icon — kept adjacent to the primary network icon.
        if cards.vpn {
            let vpn_snapshot = VpnService::global().snapshot();
            let vpn_any_active = vpn_snapshot.any_active;
            let vpn_icon_name_initial = vpn_icon_name();
            let vpn_icon = base.add_icon(vpn_icon_name_initial, &[icon::ICON, icon::TEXT]);

            set_visible_if_changed!(vpn_icon.widget(), vpn_snapshot.available && vpn_any_active);
            if vpn_snapshot.available && vpn_any_active {
                set_css_class!(vpn_icon.widget(), state::ICON_ACTIVE, true);
            }

            // Subscribe to VpnService updates
            let vpn_icon_handle = vpn_icon.clone();
            vpn_callback_id = Some(VpnService::global().connect(move |snapshot: &VpnSnapshot| {
                let widget = vpn_icon_handle.widget();

                if !snapshot.available || !snapshot.any_active {
                    set_visible_if_changed!(widget, false);
                    set_css_class!(widget, state::ICON_ACTIVE, false);
                    return;
                }
                set_visible_if_changed!(widget, true);

                let icon_name = vpn_icon_name();
                vpn_icon_handle.set_icon(icon_name);

                if snapshot.any_active {
                    set_css_class!(widget, state::ICON_ACTIVE, true);
                } else {
                    set_css_class!(widget, state::ICON_ACTIVE, false);
                }

                let tooltip = if snapshot.any_active {
                    let active_names: Vec<String> = snapshot
                        .connections
                        .iter()
                        .filter(|c| c.active)
                        .map(|c| c.name.clone())
                        .collect();
                    if active_names.is_empty() {
                        "VPN Connected".to_string()
                    } else {
                        active_names.join("\n")
                    }
                } else {
                    "VPN Disconnected".to_string()
                };
                TooltipManager::global().set_styled_tooltip(&widget, &tooltip);
            }));
        }

        // Mobile icon — separate from the Wi-Fi/Ethernet icon.
        // Visible when a modem with SIM and profile is available (mobile_supported).
        if cards.network {
            let snapshot = NetworkService::global().snapshot();
            let quality = snapshot.mobile_signal_quality().unwrap_or(0);
            let mobile_enabled = snapshot.mobile_enabled().unwrap_or(false);
            let initial_icon =
                mobile_state_icon_name(mobile_enabled, snapshot.mobile_active(), quality);
            let mobile_icon = base.add_icon(initial_icon, &[icon::ICON, icon::TEXT]);

            set_visible_if_changed!(mobile_icon.widget(), snapshot.mobile_supported());

            if snapshot.mobile_active() || snapshot.mobile_connecting() {
                set_css_class!(mobile_icon.widget(), state::ICON_ACTIVE, true);
            }
            if !mobile_enabled {
                set_css_class!(mobile_icon.widget(), qs::MOBILE_DISABLED_ICON, true);
            }
            if snapshot.mobile_connecting() {
                mobile_icon.set_spinning(true);
            }

            let mobile_icon_handle = mobile_icon.clone();
            network_mobile_callback_id = Some(NetworkService::global().connect(
                move |snapshot: &NetworkSnapshot| {
                    let widget = mobile_icon_handle.widget();

                    set_visible_if_changed!(widget, snapshot.mobile_supported());

                    let quality = snapshot.mobile_signal_quality().unwrap_or(0);
                    let mobile_enabled = snapshot.mobile_enabled().unwrap_or(false);
                    let icon_name =
                        mobile_state_icon_name(mobile_enabled, snapshot.mobile_active(), quality);
                    mobile_icon_handle.set_icon(icon_name);

                    // Show spinner while cellular is connecting
                    mobile_icon_handle.set_spinning(snapshot.mobile_connecting());

                    if snapshot.mobile_active() || snapshot.mobile_connecting() {
                        set_css_class!(widget, state::ICON_ACTIVE, true);
                    } else {
                        set_css_class!(widget, state::ICON_ACTIVE, false);
                    }

                    // Apply disabled styling when modem is off
                    if !mobile_enabled {
                        set_css_class!(widget, qs::MOBILE_DISABLED_ICON, true);
                    } else {
                        set_css_class!(widget, qs::MOBILE_DISABLED_ICON, false);
                    }

                    let carrier = snapshot.mobile_display_name().to_string();
                    let tooltip = if !mobile_enabled {
                        format!("{}\nOff", carrier)
                    } else if snapshot.mobile_connecting() {
                        format!("{}\nConnecting...", carrier)
                    } else if snapshot.mobile_failed() {
                        format!("{}\nConnection failed", carrier)
                    } else if snapshot.mobile_active() {
                        if let Some(tech) = snapshot.mobile_access_technology() {
                            format!("{}\nSignal: {}%\n{}", carrier, quality, tech)
                        } else {
                            format!("{}\nSignal: {}%", carrier, quality)
                        }
                    } else {
                        format!("{}\nDisconnected", carrier)
                    };
                    TooltipManager::global().set_styled_tooltip(&widget, &tooltip);
                },
            ));
        }

        // Audio icon
        if cards.audio {
            let volume_scroll_step = cfg.audio_scroll_percentage;
            let audio_snapshot = AudioService::global().current();
            let audio_icon_name_initial =
                volume_icon_name(audio_snapshot.volume, audio_snapshot.muted);
            let audio_icon = base.add_icon(audio_icon_name_initial, &[icon::ICON, icon::TEXT]);
            audio_icon.widget().add_css_class(qs::VOLUME);

            // Subscribe to AudioService updates
            let audio_icon_handle = audio_icon.clone();
            audio_callback_id = Some(AudioService::global().connect(
                move |snapshot: &AudioSnapshot| {
                    let widget = audio_icon_handle.widget();

                    if !snapshot.available {
                        widget.add_css_class(state::SERVICE_UNAVAILABLE);
                        audio_icon_handle.set_icon("audio-volume-muted-symbolic");
                        TooltipManager::global()
                            .set_styled_tooltip(&widget, "Audio: Service unavailable");
                        return;
                    }

                    // Backend present but volume control unavailable (e.g., Asahi before playback)
                    if !snapshot.control_available {
                        widget.add_css_class(state::SERVICE_UNAVAILABLE);
                        audio_icon_handle.set_icon("audio-volume-muted-symbolic");
                        TooltipManager::global()
                            .set_styled_tooltip(&widget, "Volume control unavailable");
                        return;
                    }

                    widget.remove_css_class(state::SERVICE_UNAVAILABLE);

                    let icon_name = volume_icon_name(snapshot.volume, snapshot.muted);
                    audio_icon_handle.set_icon(icon_name);

                    let tooltip = if snapshot.muted {
                        "Muted".to_string()
                    } else {
                        format!("Volume: {}%", snapshot.volume)
                    };
                    TooltipManager::global().set_styled_tooltip(&widget, &tooltip);
                },
            ));

            // Scroll wheel adjusts volume when hovering the audio icon.
            super::audio_card::attach_volume_scroll_controller(
                &audio_icon.widget(),
                volume_scroll_step,
            );
        }

        // Battery icon - part of the aggregate status area, but does not create
        // a separate battery popover. Details live in Quick Settings.
        if cfg.battery {
            let snapshot = BatteryService::global().snapshot();
            let battery_icon = base.add_icon("battery-missing", &[icon::ICON, icon::TEXT]);
            update_battery_indicator(&battery_icon, &snapshot);

            let battery_icon_handle = battery_icon.clone();
            battery_callback_id = Some(BatteryService::global().connect(
                move |snapshot: &BatterySnapshot| {
                    update_battery_indicator(&battery_icon_handle, snapshot);
                },
            ));
        }

        // Bluetooth icon
        if cards.bluetooth {
            let bt_snapshot = BluetoothService::global().snapshot();
            let bt_powered = bt_snapshot.powered;
            let bt_connected_devices = bt_snapshot.connected_devices;
            let bt_icon_name_initial = bt_icon_name(bt_powered, bt_connected_devices);
            let bt_icon = base.add_icon(bt_icon_name_initial, &[icon::ICON, icon::TEXT]);

            if bt_connected_devices > 0 {
                set_css_class!(bt_icon.widget(), state::ICON_ACTIVE, true);
            }
            set_visible_if_changed!(bt_icon.widget(), bt_powered);
            if !bt_powered {
                set_css_class!(bt_icon.widget(), qs::BT_DISABLED_ICON, true);
            }

            // Subscribe to BluetoothService updates
            let bt_icon_handle = bt_icon.clone();
            bluetooth_callback_id = Some(BluetoothService::global().connect(
                move |snapshot: &BluetoothSnapshot| {
                    let widget = bt_icon_handle.widget();

                    if !snapshot.has_adapter && snapshot.is_ready {
                        set_visible_if_changed!(widget, false);
                        set_css_class!(widget, state::SERVICE_UNAVAILABLE, true);
                        set_css_class!(widget, state::ICON_ACTIVE, false);
                        bt_icon_handle.set_icon("bluetooth-disabled-symbolic");
                        TooltipManager::global()
                            .set_styled_tooltip(&widget, "Bluetooth: No adapter found");
                        return;
                    }

                    set_css_class!(widget, state::SERVICE_UNAVAILABLE, false);

                    let powered = snapshot.powered;
                    let connected_devices = snapshot.connected_devices;
                    set_visible_if_changed!(widget, powered);

                    let icon_name = bt_icon_name(powered, connected_devices);
                    bt_icon_handle.set_icon(icon_name);

                    if connected_devices > 0 {
                        set_css_class!(widget, state::ICON_ACTIVE, true);
                    } else {
                        set_css_class!(widget, state::ICON_ACTIVE, false);
                    }

                    // Apply disabled styling when Bluetooth is off
                    if !powered {
                        set_css_class!(widget, qs::BT_DISABLED_ICON, true);
                    } else {
                        set_css_class!(widget, qs::BT_DISABLED_ICON, false);
                    }

                    let tooltip = if connected_devices > 0 {
                        let mut lines: Vec<String> = snapshot
                            .devices
                            .iter()
                            .filter(|d| d.connected)
                            .map(|d| d.name.clone())
                            .collect();
                        if lines.is_empty() {
                            lines.push("Bluetooth On".to_string());
                        }
                        lines.join("\n")
                    } else if powered {
                        "Bluetooth On".to_string()
                    } else {
                        "Bluetooth Off".to_string()
                    };
                    TooltipManager::global().set_styled_tooltip(&widget, &tooltip);
                },
            ));
        }

        base.widget().add_css_class(state::CLICKABLE);

        let gesture = GestureClick::new();
        gesture.set_button(BUTTON_PRIMARY);
        // Capture phase so this fires before BaseWidget's gesture
        gesture.set_propagation_phase(gtk4::PropagationPhase::Capture);

        {
            let ripple = base
                .ripple_handle()
                .expect("QuickSettings uses active BaseWidget")
                .clone();
            let qs_window_handle = qs_window.clone();
            let root = base.widget().clone();
            gesture.connect_pressed(move |gesture, _n_press, x, y| {
                trigger_ripple_from_gesture(gesture, x, y, &ripple);

                debug!(
                    "QuickSettingsWidget press: button={}",
                    gesture.current_button()
                );

                TooltipManager::global().cancel_and_hide();

                // Claim the sequence to prevent BaseWidget's handler from firing
                gesture.set_state(gtk4::EventSequenceState::Claimed);

                if let Some(native) = root.native() {
                    let surface = native.surface();
                    let monitor = surface.as_ref().map(|s| {
                        let display = s.display();
                        display.monitor_at_surface(s)
                    });

                    if let Some(bounds) = root.compute_bounds(&native) {
                        let screen_margin = ConfigManager::global().screen_margin() as i32;
                        let widget_center_x =
                            (bounds.x() + bounds.width() / 2.0) as i32 + screen_margin;

                        let monitor = monitor.flatten();
                        qs_window_handle.toggle_at(widget_center_x, monitor);
                    } else {
                        qs_window_handle.toggle_at(0, None);
                    }
                } else {
                    qs_window_handle.toggle_at(0, None);
                }
            });
        }

        base.widget().add_controller(gesture);

        // Store widget reference on the handle so IPC can derive anchor position.
        qs_window.set_bar_widget(base.widget().clone().upcast::<gtk4::Widget>());

        Self {
            base,
            qs_window_handle: qs_window,
            audio_callback_id,
            battery_callback_id,
            bluetooth_callback_id,
            network_wifi_callback_id,
            network_mobile_callback_id,
            vpn_callback_id,
        }
    }

    /// Get the root GTK widget for this bar item.
    pub fn widget(&self) -> &GtkBox {
        self.base.widget()
    }
}

impl Drop for QuickSettingsWidget {
    fn drop(&mut self) {
        self.qs_window_handle.destroy();

        if let Some(id) = self.audio_callback_id.take() {
            AudioService::global().disconnect(id);
        }
        if let Some(id) = self.battery_callback_id.take() {
            BatteryService::global().disconnect(id);
        }
        if let Some(id) = self.bluetooth_callback_id.take() {
            BluetoothService::global().disconnect(id);
        }
        if let Some(id) = self.network_wifi_callback_id.take() {
            NetworkService::global().unsubscribe(id);
        }
        if let Some(id) = self.network_mobile_callback_id.take() {
            NetworkService::global().unsubscribe(id);
        }
        if let Some(id) = self.vpn_callback_id.take() {
            VpnService::global().disconnect(id);
        }
    }
}

fn update_battery_indicator(icon_handle: &IconHandle, snapshot: &BatterySnapshot) {
    let icon_widget = icon_handle.widget();

    if !snapshot.available {
        set_visible_if_changed!(icon_widget, false);
        set_css_class!(icon_widget, state::ICON_ACTIVE, false);
        set_css_class!(icon_widget, widget::BATTERY_CHARGING, false);
        set_css_class!(icon_widget, widget::BATTERY_PLUGGED, false);
        set_css_class!(icon_widget, widget::BATTERY_FULL, false);
        set_css_class!(icon_widget, widget::BATTERY_LOW, false);
        return;
    }

    set_visible_if_changed!(icon_widget, true);
    set_css_class!(icon_widget, state::SERVICE_UNAVAILABLE, false);

    let rounded = snapshot.percent.map(rounded_pct_value);
    let display_state = battery_display_state_from_snapshot(snapshot);
    let low = matches!(rounded, Some(pct) if pct <= 20);

    set_css_class!(
        icon_widget,
        widget::BATTERY_CHARGING,
        display_state == BatteryDisplayState::Charging,
    );
    set_css_class!(
        icon_widget,
        widget::BATTERY_FULL,
        display_state == BatteryDisplayState::FullyCharged,
    );
    set_css_class!(
        icon_widget,
        widget::BATTERY_PLUGGED,
        display_state == BatteryDisplayState::PluggedNotCharging,
    );
    set_css_class!(icon_widget, widget::BATTERY_LOW, low);

    let icon_name = rounded
        .map(|pct| battery_icon_name(pct, display_state))
        .unwrap_or_else(|| "battery-missing".to_string());
    icon_handle.set_icon(&icon_name);

    let tooltip = match rounded {
        Some(pct) => {
            format!(
                "Battery: {}\nState: {}",
                readable_pct(pct),
                battery_state_text(display_state)
            )
        }
        None => "Battery: unknown".to_string(),
    };
    TooltipManager::global().set_styled_tooltip(&icon_widget, &tooltip);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use toml::Value;

    fn make_widget_entry(options: HashMap<String, Value>) -> WidgetEntry {
        WidgetEntry {
            name: "quick_settings".to_string(),
            options,
        }
    }

    #[test]
    fn test_quick_settings_config_defaults() {
        let config = QuickSettingsConfig::from_entry(&make_widget_entry(HashMap::new()));

        assert_eq!(config.audio_scroll_percentage, 5);
        assert!(config.battery);
        assert_eq!(
            config.enabled_control_panel_cards(),
            vec![
                "network",
                "bluetooth",
                "vpn",
                "idle_inhibitor",
                "updates",
                "audio",
                "mic",
                "brightness",
                "battery_health",
                "resource_overview",
                "power"
            ]
        );
        assert_eq!(
            config.enabled_bar_indicators(),
            vec!["network", "vpn", "mobile", "audio", "battery", "bluetooth"]
        );
    }

    #[test]
    fn test_quick_settings_config_card_toggles_affect_inventory() {
        let mut options = HashMap::new();
        options.insert("network".to_string(), Value::Boolean(false));
        options.insert("vpn".to_string(), Value::Boolean(false));
        options.insert("audio".to_string(), Value::Boolean(false));
        options.insert("battery".to_string(), Value::Boolean(false));
        options.insert("battery_health".to_string(), Value::Boolean(false));
        options.insert("resource_overview".to_string(), Value::Boolean(false));
        options.insert("power".to_string(), Value::Boolean(false));

        let config = QuickSettingsConfig::from_entry(&make_widget_entry(options));

        assert_eq!(
            config.enabled_control_panel_cards(),
            vec![
                "bluetooth",
                "idle_inhibitor",
                "updates",
                "mic",
                "brightness"
            ]
        );
        assert_eq!(config.enabled_bar_indicators(), vec!["bluetooth"]);
    }

    #[test]
    fn test_quick_settings_config_battery_indicator_toggle() {
        let mut options = HashMap::new();
        options.insert("battery".to_string(), Value::Boolean(false));

        let config = QuickSettingsConfig::from_entry(&make_widget_entry(options));

        assert!(!config.battery);
        assert_eq!(
            config.enabled_bar_indicators(),
            vec!["network", "vpn", "mobile", "audio", "bluetooth"]
        );
    }

    #[test]
    fn test_quick_settings_config_clamps_audio_scroll_percentage() {
        let mut low_options = HashMap::new();
        low_options.insert("audio_scroll_percentage".to_string(), Value::Integer(0));
        let low = QuickSettingsConfig::from_entry(&make_widget_entry(low_options));
        assert_eq!(low.audio_scroll_percentage, 1);

        let mut high_options = HashMap::new();
        high_options.insert("audio_scroll_percentage".to_string(), Value::Integer(50));
        let high = QuickSettingsConfig::from_entry(&make_widget_entry(high_options));
        assert_eq!(high.audio_scroll_percentage, 25);
    }

    #[test]
    fn test_quick_settings_config_ignores_non_bool_card_values() {
        let mut options = HashMap::new();
        options.insert("network".to_string(), Value::String("false".to_string()));
        options.insert("battery".to_string(), Value::String("false".to_string()));
        options.insert(
            "battery_health".to_string(),
            Value::String("false".to_string()),
        );
        options.insert(
            "resource_overview".to_string(),
            Value::String("false".to_string()),
        );
        options.insert("vpn".to_string(), Value::Integer(0));

        let config = QuickSettingsConfig::from_entry(&make_widget_entry(options));

        assert!(config.cards.network);
        assert!(config.battery);
        assert!(config.cards.battery_health);
        assert!(config.cards.resource_overview);
        assert!(config.cards.vpn);
    }
}
