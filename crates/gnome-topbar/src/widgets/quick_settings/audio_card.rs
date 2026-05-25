//! Audio card for Quick Settings panel.
//!
//! This module contains:
//! - Audio icon helpers (volume_icon_name)
//! - Audio row building (mute button, slider, expander)
//! - Audio details (sink list)
//! - State change handling

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4::pango::EllipsizeMode;
use gtk4::prelude::*;
use gtk4::{
    Align, Box as GtkBox, Button, EventControllerScroll, EventControllerScrollFlags, Label,
    ListBox, ListBoxRow, Orientation, Overlay, Revealer, Scale,
};

use super::components::SliderRow;
use super::ui_helpers::{
    AUDIO_REVEALER_DURATION_MS, add_placeholder_row, build_slide_down_revealer, clear_list_box,
    create_qs_list_box,
};
use crate::services::audio::{AudioService, AudioSnapshot, SinkInfoSnapshot};
use crate::services::icons::{IconHandle, IconsService};
use crate::services::surfaces::SurfaceStyleManager;
use crate::styles::{color, qs, row, state};
use crate::widgets::base::add_ripple_to_row;

fn set_sensitive_if_changed(widget: &impl IsA<gtk4::Widget>, sensitive: bool) {
    if widget.as_ref().is_sensitive() != sensitive {
        widget.as_ref().set_sensitive(sensitive);
    }
}

fn set_visible_if_changed(widget: &impl IsA<gtk4::Widget>, visible: bool) {
    if widget.as_ref().is_visible() != visible {
        widget.as_ref().set_visible(visible);
    }
}

fn set_tooltip_if_changed(widget: &impl IsA<gtk4::Widget>, text: Option<&str>) {
    if widget.as_ref().tooltip_text().as_deref() != text {
        widget.as_ref().set_tooltip_text(text);
    }
}

fn set_scale_range_if_changed(slider: &Scale, lower: f64, upper: f64) {
    let adjustment = slider.adjustment();
    if (adjustment.lower() - lower).abs() >= f64::EPSILON
        || (adjustment.upper() - upper).abs() >= f64::EPSILON
    {
        slider.set_range(lower, upper);
    }
}

fn set_scale_value_if_changed(slider: &Scale, value: f64) {
    if (slider.value() - value).abs() >= f64::EPSILON {
        slider.set_value(value);
    }
}

fn set_disabled_class(row: &GtkBox, disabled: bool) {
    if disabled {
        if !row.has_css_class(qs::AUDIO_ROW_DISABLED) {
            row.add_css_class(qs::AUDIO_ROW_DISABLED);
        }
    } else if row.has_css_class(qs::AUDIO_ROW_DISABLED) {
        row.remove_css_class(qs::AUDIO_ROW_DISABLED);
    }
}

/// Get the appropriate volume icon name based on volume level and mute state.
///
/// Uses standard GTK/Adwaita icon names.
pub fn volume_icon_name(volume: u32, muted: bool) -> &'static str {
    if muted {
        return "audio-volume-muted-symbolic";
    }
    if volume >= 66 {
        return "audio-volume-high-symbolic";
    }
    if volume >= 33 {
        return "audio-volume-medium-symbolic";
    }
    if volume >= 1 {
        return "audio-volume-low-symbolic";
    }
    "audio-volume-muted-symbolic"
}

/// State for the Audio card in the Quick Settings panel.
pub struct AudioCardState {
    /// Audio mute button.
    pub mute_button: RefCell<Option<Button>>,
    /// Audio volume icon handle.
    pub icon_handle: RefCell<Option<IconHandle>>,
    /// Audio volume slider.
    pub slider: RefCell<Option<Scale>>,
    /// Audio expander arrow icon handle.
    pub arrow: RefCell<Option<IconHandle>>,
    /// Audio details revealer.
    pub revealer: RefCell<Option<Revealer>>,
    /// Audio sink list box.
    pub list_box: RefCell<Option<ListBox>>,
    /// Flag to prevent slider feedback loop.
    pub updating: Cell<bool>,
    /// Audio row container (for CSS class toggling).
    pub row: RefCell<Option<GtkBox>>,
    /// Hint label shown when audio control is unavailable.
    pub hint_label: RefCell<Option<Label>>,
    /// Signature of rendered sink rows to skip unchanged list rebuilds.
    pub list_signature: RefCell<Option<String>>,
}

impl AudioCardState {
    pub fn new() -> Self {
        Self {
            mute_button: RefCell::new(None),
            icon_handle: RefCell::new(None),
            slider: RefCell::new(None),
            arrow: RefCell::new(None),
            revealer: RefCell::new(None),
            list_box: RefCell::new(None),
            updating: Cell::new(false),
            row: RefCell::new(None),
            hint_label: RefCell::new(None),
            list_signature: RefCell::new(None),
        }
    }
}

impl Default for AudioCardState {
    fn default() -> Self {
        Self::new()
    }
}

/// Container for audio row widgets.
pub struct AudioRowWidgets {
    /// The outer row container.
    pub row: GtkBox,
    /// The mute toggle button.
    pub mute_button: Button,
    /// Handle to the volume icon.
    pub icon_handle: IconHandle,
    /// The volume slider.
    pub slider: Scale,
    /// The expander button for sink list.
    pub expander_button: Button,
    /// Handle to the expander arrow icon.
    pub arrow_handle: IconHandle,
}

/// Build the audio row with mute button, volume slider, and expander.
///
/// Uses `SliderRow` for consistent styling with other slider rows.
pub fn build_audio_row() -> AudioRowWidgets {
    let result = SliderRow::builder()
        .icon("audio-volume-high-symbolic")
        .interactive_icon(true) // Mute button is clickable
        // The slider is an interactive control, so keep its range capped to
        // what GNOME Topbar is allowed to request. Programmatic updates are
        // guarded to avoid writing external over-cap values back to Pulse.
        .range(0.0, AudioService::global().user_max_percent() as f64)
        .step(1.0)
        .with_expander(true) // Sink list expander
        .build();

    AudioRowWidgets {
        row: result.container,
        mute_button: result.icon_button,
        icon_handle: result.icon_handle,
        slider: result.slider,
        expander_button: result.expander_button.expect("expander requested"),
        arrow_handle: result.expander_icon.expect("expander requested"),
    }
}

/// Update the slider from the backend state without causing write-back.
///
/// External volume can exceed GNOME Topbar's configured cap, but this is an
/// interactive control: keep the range capped to the values GNOME Topbar may
/// request. GTK will visually saturate over-cap values at the maximum, while
/// the tooltip preserves the true backend volume.
pub fn set_volume_slider_display(slider: &Scale, volume: u32) {
    let max_percent = AudioService::global().user_max_percent().max(1);
    set_scale_range_if_changed(slider, 0.0, max_percent as f64);
    set_scale_value_if_changed(slider, volume as f64);
    set_tooltip_if_changed(slider, Some(&format!("{volume}%")));
}

/// Container for audio details (sink list) widgets.
pub struct AudioDetailsWidgets {
    /// The revealer for accordion behavior.
    pub revealer: Revealer,
    /// The list box for sinks.
    pub list_box: ListBox,
}

/// Build the audio details section with sink list.
///
/// # CSS Classes Applied
///
/// - `.qs-audio-details` on the container
/// - `.qs-section-header` on the header
/// - `.qs-list` on the list box
pub fn build_audio_details() -> AudioDetailsWidgets {
    let container = GtkBox::new(Orientation::Vertical, 8);
    container.add_css_class(qs::AUDIO_DETAILS);

    // Section header
    let header = Label::new(Some("Sound"));
    header.set_xalign(0.0);
    header.add_css_class(qs::SECTION_HEADER);
    container.append(&header);

    // Sink list
    let list_box = create_qs_list_box();
    container.append(&list_box);

    // Wrap in the standard audio revealer.
    let revealer = build_slide_down_revealer(Some(&container), AUDIO_REVEALER_DURATION_MS);

    AudioDetailsWidgets { revealer, list_box }
}

/// Create a hint label for when audio control is unavailable.
pub fn build_audio_hint_label() -> Label {
    let label = Label::new(Some(
        "Audio sink suspended. Play audio to enable volume control.",
    ));
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.set_max_width_chars(40);
    label.add_css_class(qs::MUTED_LABEL);
    label.add_css_class(qs::AUDIO_HINT);
    label.add_css_class(color::MUTED);
    label
}

/// Create a sink row for the audio sink list.
///
/// # Arguments
///
/// - `description`: The human-readable sink description.
/// - `is_default`: Whether this sink is the current default.
/// - `port_available`: Whether the sink's port is available (e.g., headphones plugged in).
///   `None` means no jack detection, `Some(false)` means unavailable.
pub fn create_sink_row(
    description: &str,
    is_default: bool,
    port_available: Option<bool>,
) -> ListBoxRow {
    let list_row = ListBoxRow::new();
    list_row.add_css_class(row::QS);
    list_row.add_css_class(row::BASE);

    // Check if port is unavailable (explicitly false, not unknown/None)
    let is_unavailable = port_available == Some(false);

    let hbox = GtkBox::new(Orientation::Horizontal, 6);
    hbox.add_css_class(row::QS_CONTENT);

    // Description label
    let label = Label::new(Some(description));
    label.set_xalign(0.0);
    label.set_hexpand(true);
    label.set_ellipsize(EllipsizeMode::End);
    label.set_single_line_mode(true);
    label.set_width_chars(22);
    label.set_max_width_chars(22);
    label.add_css_class(row::QS_TITLE);
    label.add_css_class(color::PRIMARY);
    hbox.append(&label);

    // Selection indicator
    if is_default {
        // Overlay: background box + checkmark icon floating on top
        let overlay = Overlay::new();
        overlay.set_valign(Align::Center);

        // Background box (same size as unselected indicator)
        let bg = GtkBox::new(Orientation::Horizontal, 0);
        bg.add_css_class(row::QS_INDICATOR_BG);
        overlay.set_child(Some(&bg));

        // Checkmark icon (larger, overflows the background)
        let icons = IconsService::global();
        let indicator = icons.create_icon("object-select-symbolic", &[row::QS_INDICATOR]);
        indicator.widget().set_halign(Align::Center);
        indicator.widget().set_valign(Align::Center);
        overlay.add_overlay(&indicator.widget());

        hbox.append(&overlay);
    } else {
        // CSS-styled box for unselected (respects --radius-pill)
        let indicator = GtkBox::new(Orientation::Horizontal, 0);
        indicator.add_css_class(row::QS_RADIO_INDICATOR);
        hbox.append(&indicator);
    }

    // Add ripple overlay for press feedback on activatable rows.
    // Move padding from the row CSS to content margins so the ripple
    // DrawingArea fills the full row background.
    if is_unavailable {
        list_row.set_child(Some(&hbox));
    } else {
        // Transfer .qs-row padding to content margins; the CSS rule
        // `.qs-row.vp-has-ripple { padding: 0 }` zeros the row padding.
        hbox.set_margin_top(6);
        hbox.set_margin_bottom(6);
        hbox.set_margin_start(10);
        hbox.set_margin_end(10);

        add_ripple_to_row(&list_row, &hbox);
    }

    // If port is unavailable, gray out the row and make it non-activatable
    if is_unavailable {
        list_row.set_activatable(false);
        list_row.set_focusable(false);
        list_row.set_sensitive(false);
    } else {
        list_row.set_activatable(true);
        list_row.set_focusable(true);
    }

    list_row
}

/// Populate the audio sink list with available sinks.
///
/// Sinks with unavailable ports (e.g., headphones not plugged in) are shown
/// but grayed out and non-selectable.
pub fn populate_audio_sink_list(list_box: &ListBox, snapshot: &AudioSnapshot) {
    clear_list_box(list_box);

    if !snapshot.available {
        add_placeholder_row(list_box, "Audio unavailable");
        return;
    }

    if snapshot.sinks.is_empty() {
        add_placeholder_row(list_box, "No audio devices");
        return;
    }

    // Count how many sinks are actually available
    let available_count = snapshot
        .sinks
        .iter()
        .filter(|s| s.port_available != Some(false))
        .count();

    // If all sinks are unavailable, show a message
    if available_count == 0 {
        add_placeholder_row(list_box, "No audio devices available");
        return;
    }

    for sink in &snapshot.sinks {
        // Skip sinks with unavailable ports entirely - they clutter the UI
        // and can't be selected anyway
        if sink.port_available == Some(false) {
            continue;
        }

        let row = create_sink_row(&sink.description, sink.is_default, sink.port_available);
        list_box.append(&row);
    }
}

/// Handle Audio state changes from AudioService.
pub fn on_audio_changed(state: &AudioCardState, snapshot: &AudioSnapshot) {
    let control_ok = snapshot.available && snapshot.control_available;

    // Update volume slider (with flag to prevent feedback loop)
    if let Some(slider) = state.slider.borrow().as_ref() {
        state.updating.set(true);
        set_volume_slider_display(slider, snapshot.volume);
        set_sensitive_if_changed(slider, control_ok);
        state.updating.set(false);
    }

    // Update mute button sensitivity
    if let Some(mute_btn) = state.mute_button.borrow().as_ref() {
        set_sensitive_if_changed(mute_btn, control_ok);
    }

    // Update audio row disabled styling
    if let Some(audio_row) = state.row.borrow().as_ref() {
        set_disabled_class(audio_row, !control_ok);
    }

    // Update hint label visibility (show when backend available but control is not)
    if let Some(hint_label) = state.hint_label.borrow().as_ref() {
        let should_show = snapshot.available && !snapshot.control_available;
        set_visible_if_changed(hint_label, should_show);
    }

    // Update volume icon based on volume and mute state
    if let Some(icon_handle) = state.icon_handle.borrow().as_ref() {
        let icon_name = volume_icon_name(snapshot.volume, snapshot.muted);
        icon_handle.set_icon(icon_name);

        // Toggle muted class for styling
        let widget = icon_handle.widget();
        set_css_class(&widget, state::MUTED, snapshot.muted);
    }

    // Update sink list
    if let Some(list_box) = state.list_box.borrow().as_ref() {
        let signature = audio_sink_list_signature(snapshot.available, &snapshot.sinks);
        if state.list_signature.borrow().as_deref() != Some(signature.as_str()) {
            *state.list_signature.borrow_mut() = Some(signature);
            populate_audio_sink_list(list_box, snapshot);
            // Apply Pango font attrs to dynamically created list rows
            SurfaceStyleManager::global().apply_pango_attrs_all(list_box);
        }
    }
}

fn audio_sink_list_signature(available: bool, sinks: &[SinkInfoSnapshot]) -> String {
    let mut signature = format!("available={available};len={}", sinks.len());
    for sink in sinks {
        signature.push('|');
        signature.push_str(&sink.name);
        signature.push('\t');
        signature.push_str(&sink.description);
        signature.push('\t');
        signature.push_str(if sink.is_default { "default" } else { "normal" });
        signature.push('\t');
        signature.push_str(match sink.port_available {
            Some(true) => "available",
            Some(false) => "unavailable",
            None => "unknown",
        });
    }
    signature
}

fn set_css_class(widget: &impl IsA<gtk4::Widget>, class: &str, enabled: bool) {
    let widget = widget.as_ref();
    if enabled {
        if !widget.has_css_class(class) {
            widget.add_css_class(class);
        }
    } else if widget.has_css_class(class) {
        widget.remove_css_class(class);
    }
}

/// Handle audio sink row activation.
pub fn on_audio_sink_row_activated(row: &ListBoxRow) {
    // Get the row index and look up the sink in the current snapshot
    let index = row.index();
    if index < 0 {
        return;
    }

    let audio = AudioService::global();
    let snapshot = audio.current();

    // The row index corresponds to the Nth *available* sink (since we skip unavailable ones)
    // Filter to only available sinks and get the one at the requested index
    let available_sinks: Vec<_> = snapshot
        .sinks
        .iter()
        .filter(|s| s.port_available != Some(false))
        .collect();

    if let Some(sink) = available_sinks.get(index as usize) {
        audio.set_default_sink(&sink.name);
    }
}

/// Attach an `EventControllerScroll` that adjusts volume on vertical scroll.
///
/// Each full scroll tick changes volume by `step` percentage points.
/// Fractional scroll events (e.g. from touchpads) are accumulated so that
/// volume only changes once a full tick is reached. The accumulator resets
/// on direction change so that reversing scroll direction feels responsive.
pub fn attach_volume_scroll_controller(widget: &impl IsA<gtk4::Widget>, step: i32) {
    let scroll = EventControllerScroll::new(EventControllerScrollFlags::VERTICAL);
    scroll.set_propagation_phase(gtk4::PropagationPhase::Capture);

    let accumulated = Rc::new(Cell::new(0.0f64));

    scroll.connect_scroll(move |_controller, _dx, dy| {
        let snapshot = AudioService::global().current();
        if !snapshot.available || !snapshot.control_available {
            accumulated.set(0.0);
            return gtk4::glib::Propagation::Proceed;
        }

        let mut acc = accumulated.get();

        // Reset accumulator on direction change to avoid a "dead zone"
        // when reversing scroll direction.
        if (acc > 0.0 && dy < 0.0) || (acc < 0.0 && dy > 0.0) {
            acc = 0.0;
        }

        acc += dy;

        let audio = AudioService::global();
        let step = step.abs();

        while acc.abs() >= 1.0 {
            let direction = if acc < 0.0 { 1 } else { -1 };
            audio.set_volume_relative(direction * step);
            acc -= acc.signum();
        }

        accumulated.set(acc);
        gtk4::glib::Propagation::Stop
    });

    widget.add_controller(scroll);
}
