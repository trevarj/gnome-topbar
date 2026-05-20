//! Battery health card for Quick Settings panel.
//!
//! Shows firmware/kernel charge-limit state exposed through the shared
//! BatteryService. Controls use UPower when available, with direct sysfs writes
//! only when this process already has permission.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Box as GtkBox, Button, Label, ListBox, Orientation, Revealer};

use crate::services::battery::{BatteryService, BatterySnapshot};
use crate::services::surfaces::SurfaceStyleManager;
use crate::styles::{button as button_style, color, qs, row};
use crate::widgets::battery::{
    battery_display_state_from_snapshot, battery_icon_name, battery_state_text, readable_pct,
    rounded_pct_value,
};

use super::components::{ListRow, ToggleCard};
use super::ui_helpers::{
    CARD_REVEALER_DURATION_MS, ExpandableCard, ExpandableCardBase, build_slide_down_revealer,
    clear_list_box, create_qs_list_box, set_icon_active, set_subtitle_active,
};

/// State for the Battery Health card in the Quick Settings panel.
pub struct BatteryHealthCardState {
    pub base: ExpandableCardBase,
    pub card_box: RefCell<Option<gtk4::Widget>>,
    pub health_button: RefCell<Option<Button>>,
    pub full_button: RefCell<Option<Button>>,
    pub control_note: RefCell<Option<Label>>,
    pub updating_toggle: Cell<bool>,
}

impl BatteryHealthCardState {
    pub fn new() -> Self {
        Self {
            base: ExpandableCardBase::new(),
            card_box: RefCell::new(None),
            health_button: RefCell::new(None),
            full_button: RefCell::new(None),
            control_note: RefCell::new(None),
            updating_toggle: Cell::new(false),
        }
    }
}

impl Default for BatteryHealthCardState {
    fn default() -> Self {
        Self::new()
    }
}

impl ExpandableCard for BatteryHealthCardState {
    fn base(&self) -> &ExpandableCardBase {
        &self.base
    }
}

/// Build the Battery Health card and revealer for the Quick Settings panel.
pub fn build_battery_health_card(
    state: &Rc<BatteryHealthCardState>,
) -> (gtk4::Widget, Revealer, Option<Button>) {
    let snapshot = BatteryService::global().snapshot();
    let active = snapshot.health_limit_active();
    let subtitle = battery_health_subtitle(&snapshot);
    let can_control = battery_health_can_control(&snapshot);

    let card = ToggleCard::builder()
        .icon(&battery_health_icon_name(&snapshot))
        .label("Battery Health")
        .subtitle(&subtitle)
        .active(active)
        .sensitive(can_control)
        .icon_active(active)
        .with_expander(true)
        .build();

    card.card.add_css_class(qs::BATTERY_HEALTH);

    *state.card_box.borrow_mut() = Some(card.card.clone());
    *state.base.toggle.borrow_mut() = Some(card.toggle.clone());
    *state.base.card_icon.borrow_mut() = Some(card.icon_handle.clone());
    *state.base.subtitle.borrow_mut() = card.subtitle.clone();
    *state.base.arrow.borrow_mut() = card.expander_icon.clone();

    {
        let state = Rc::clone(state);
        card.toggle.connect_toggled(move |toggle| {
            if state.updating_toggle.get() {
                return;
            }
            BatteryService::global().set_health_limit_enabled(toggle.is_active());
        });
    }

    let details = build_battery_health_details(state);
    let revealer = build_slide_down_revealer(Some(&details), CARD_REVEALER_DURATION_MS);
    *state.base.revealer.borrow_mut() = Some(revealer.clone());

    (card.card, revealer, card.expander_button)
}

fn build_battery_health_details(state: &Rc<BatteryHealthCardState>) -> GtkBox {
    let container = GtkBox::new(Orientation::Vertical, 0);
    container.add_css_class(qs::BATTERY_HEALTH_DETAILS);

    let controls = build_battery_health_controls(state);
    container.append(&controls);

    let control_note = Label::new(None);
    control_note.set_xalign(0.0);
    control_note.add_css_class(qs::BATTERY_HEALTH_CONTROL_NOTE);
    control_note.add_css_class(row::QS_SUBTITLE);
    control_note.add_css_class(color::MUTED);
    container.append(&control_note);
    *state.control_note.borrow_mut() = Some(control_note);

    let list_box = create_qs_list_box();
    list_box.add_css_class(qs::BATTERY_HEALTH_LIST);
    container.append(&list_box);

    *state.base.list_box.borrow_mut() = Some(list_box.clone());

    let snapshot = BatteryService::global().snapshot();
    update_battery_health_controls(state, &snapshot);
    populate_battery_health_details(state, &snapshot);

    container
}

fn build_battery_health_controls(state: &Rc<BatteryHealthCardState>) -> GtkBox {
    let controls = GtkBox::new(Orientation::Horizontal, 8);
    controls.add_css_class(qs::BATTERY_HEALTH_CONTROLS);
    controls.set_homogeneous(true);

    let health_button = profile_button("Health 80%");
    let full_button = profile_button("Full 100%");

    health_button.connect_clicked(|_| {
        BatteryService::global().set_health_limit_enabled(true);
    });
    full_button.connect_clicked(|_| {
        BatteryService::global().set_health_limit_enabled(false);
    });

    controls.append(&health_button);
    controls.append(&full_button);

    *state.health_button.borrow_mut() = Some(health_button);
    *state.full_button.borrow_mut() = Some(full_button);

    controls
}

fn profile_button(label: &str) -> Button {
    let btn = crate::widgets::base::vp_button_with_label(label);
    btn.add_css_class(button_style::CARD);
    btn.add_css_class(qs::BATTERY_HEALTH_PROFILE_BUTTON);
    btn.set_has_frame(false);
    btn
}

/// Handle battery snapshot changes from BatteryService.
pub fn on_battery_health_changed(state: &BatteryHealthCardState, snapshot: &BatterySnapshot) {
    let active = snapshot.health_limit_active();
    let can_control = battery_health_can_control(snapshot);

    if let Some(toggle) = state.base.toggle.borrow().as_ref() {
        state.updating_toggle.set(true);
        if toggle.is_active() != active {
            toggle.set_active(active);
        }
        state.updating_toggle.set(false);
        toggle.set_sensitive(can_control);
    }

    if let Some(icon) = state.base.card_icon.borrow().as_ref() {
        icon.set_icon(&battery_health_icon_name(snapshot));
        set_icon_active(icon, active);
    }

    if let Some(subtitle) = state.base.subtitle.borrow().as_ref() {
        subtitle.set_label(&battery_health_subtitle(snapshot));
        subtitle.set_visible(snapshot.available);
        set_subtitle_active(subtitle, active);
    }

    update_battery_health_controls(state, snapshot);
    populate_battery_health_details(state, snapshot);

    if let Some(list_box) = state.base.list_box.borrow().as_ref() {
        SurfaceStyleManager::global().apply_pango_attrs_all(list_box);
    }
}

fn update_battery_health_controls(state: &BatteryHealthCardState, snapshot: &BatterySnapshot) {
    let can_control = battery_health_can_control(snapshot);

    if let Some(button) = state.health_button.borrow().as_ref() {
        button.set_sensitive(can_control);
        set_profile_button_active(button, snapshot.health_limit_active());
    }
    if let Some(button) = state.full_button.borrow().as_ref() {
        button.set_sensitive(can_control);
        set_profile_button_active(button, full_charge_active(snapshot));
    }
    if let Some(label) = state.control_note.borrow().as_ref() {
        label.set_label(&control_note(snapshot));
    }
}

fn set_profile_button_active(button: &Button, active: bool) {
    if active {
        button.remove_css_class(button_style::CARD);
        button.add_css_class(button_style::ACCENT);
    } else {
        button.remove_css_class(button_style::ACCENT);
        button.add_css_class(button_style::CARD);
    }
}

fn battery_health_can_control(snapshot: &BatterySnapshot) -> bool {
    snapshot.available
        && snapshot.charge_control_available
        && (snapshot.charge_control_upower_available || snapshot.charge_control_writable)
}

fn full_charge_active(snapshot: &BatterySnapshot) -> bool {
    snapshot.charge_stop_threshold == Some(100)
}

fn populate_battery_health_details(state: &BatteryHealthCardState, snapshot: &BatterySnapshot) {
    let Some(list_box) = state.base.list_box.borrow().as_ref().cloned() else {
        return;
    };

    clear_list_box(&list_box);

    if !snapshot.available {
        append_detail_row(&list_box, "Status", "No system battery detected");
        return;
    }

    append_detail_row(&list_box, "Charge limit", &charge_limit_summary(snapshot));
    append_detail_row(&list_box, "Status", &status_summary(snapshot));
    append_detail_row(
        &list_box,
        "Battery health",
        &battery_health_summary(snapshot),
    );
    append_detail_row(
        &list_box,
        "Charge behavior",
        &charge_behaviour_summary(snapshot),
    );
    append_detail_row(&list_box, "Power", &power_summary(snapshot));
}

fn append_detail_row(list_box: &ListBox, title: &str, subtitle: &str) {
    let row = ListRow::builder().title(title).subtitle(subtitle).build();
    row.row.set_activatable(false);
    row.row.set_focusable(false);
    list_box.append(&row.row);
}

fn battery_health_icon_name(snapshot: &BatterySnapshot) -> String {
    let Some(percent) = snapshot.percent else {
        return "battery-missing".to_string();
    };

    battery_icon_name(
        rounded_pct_value(percent),
        battery_display_state_from_snapshot(snapshot),
    )
}

pub fn battery_health_subtitle(snapshot: &BatterySnapshot) -> String {
    if !snapshot.available {
        return "Unavailable".to_string();
    }

    let mut parts = Vec::new();
    if let Some(stop) = snapshot.charge_stop_threshold {
        if snapshot.health_limit_active() {
            parts.push(format!("Limit {stop}%"));
        } else {
            parts.push("Full charge".to_string());
        }
    } else {
        parts.push("No limit reported".to_string());
    }

    if let Some(percent) = snapshot.percent.map(rounded_pct_value) {
        parts.push(readable_pct(percent));
    }

    parts.join(" • ")
}

fn charge_limit_summary(snapshot: &BatterySnapshot) -> String {
    match (
        snapshot.charge_start_threshold,
        snapshot.charge_stop_threshold,
    ) {
        (Some(start), Some(stop)) => format!("Starts at {start}%, stops at {stop}%"),
        (Some(start), None) => format!("Starts at {start}%"),
        (None, Some(stop)) => format!("Stops at {stop}%"),
        (None, None) => "No charge limit reported".to_string(),
    }
}

fn control_note(snapshot: &BatterySnapshot) -> String {
    if !snapshot.available {
        return "No system battery detected".to_string();
    }
    if !snapshot.charge_control_available {
        return "Charge limit controls are not exposed".to_string();
    }
    if snapshot.charge_control_upower_available {
        return "Charge limit controls are available through UPower".to_string();
    }
    if snapshot.charge_control_writable {
        return "Charge limit controls are directly writable".to_string();
    }
    "Charge limit is exposed, but UPower cannot manage it".to_string()
}

fn status_summary(snapshot: &BatterySnapshot) -> String {
    let state = battery_state_text(battery_display_state_from_snapshot(snapshot));
    match snapshot.percent.map(rounded_pct_value) {
        Some(percent) => format!("{} • {state}", readable_pct(percent)),
        None => state.to_string(),
    }
}

fn battery_health_summary(snapshot: &BatterySnapshot) -> String {
    let mut parts = Vec::new();

    if let Some(percent) = design_capacity_percent(snapshot) {
        parts.push(format!("{percent}% design capacity"));
    }
    if let Some(cycles) = snapshot.cycle_count {
        let suffix = if cycles == 1 { "cycle" } else { "cycles" };
        parts.push(format!("{cycles} {suffix}"));
    }

    if parts.is_empty() {
        "Not reported".to_string()
    } else {
        parts.join(" • ")
    }
}

fn design_capacity_percent(snapshot: &BatterySnapshot) -> Option<u16> {
    let full = snapshot.energy_full?;
    let design = snapshot.energy_full_design?;
    if !full.is_finite() || !design.is_finite() || design <= 0.0 {
        return None;
    }

    Some(((full / design) * 100.0).round().clamp(0.0, 999.0) as u16)
}

fn charge_behaviour_summary(snapshot: &BatterySnapshot) -> String {
    let Some(behaviour) = snapshot.charge_behaviour.as_ref() else {
        return "Not reported".to_string();
    };

    let current = behaviour.current.as_deref().unwrap_or("unknown");
    let alternatives: Vec<&str> = behaviour
        .options
        .iter()
        .map(String::as_str)
        .filter(|option| *option != current)
        .collect();

    if alternatives.is_empty() {
        current.to_string()
    } else {
        format!("{current} ({})", alternatives.join(", "))
    }
}

fn power_summary(snapshot: &BatterySnapshot) -> String {
    match snapshot.ac_online {
        Some(true) => "External power connected".to_string(),
        Some(false) => "On battery".to_string(),
        None => "Not reported".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::battery::{
        ChargeBehaviourSnapshot, HEALTH_CHARGE_STOP_THRESHOLD, STATE_PENDING_CHARGE,
    };

    fn snapshot() -> BatterySnapshot {
        BatterySnapshot {
            available: true,
            percent: Some(79.0),
            state: Some(STATE_PENDING_CHARGE),
            energy_rate: None,
            energy_full: Some(54.07),
            energy_full_design: Some(52.5),
            time_to_empty: None,
            time_to_full: None,
            charge_start_threshold: Some(75),
            charge_stop_threshold: Some(80),
            charge_behaviour: Some(ChargeBehaviourSnapshot {
                current: Some("auto".to_string()),
                options: vec![
                    "auto".to_string(),
                    "inhibit-charge".to_string(),
                    "force-discharge".to_string(),
                ],
            }),
            cycle_count: Some(54),
            ac_online: Some(true),
            charge_control_available: true,
            charge_control_writable: false,
            charge_control_upower_available: true,
        }
    }

    #[test]
    fn subtitle_shows_active_charge_limit_and_percent() {
        assert_eq!(battery_health_subtitle(&snapshot()), "Limit 80% • 79%");
    }

    #[test]
    fn charge_limit_summary_uses_start_and_stop_thresholds() {
        assert_eq!(
            charge_limit_summary(&snapshot()),
            "Starts at 75%, stops at 80%"
        );
    }

    #[test]
    fn health_summary_includes_design_capacity_and_cycles() {
        assert_eq!(
            battery_health_summary(&snapshot()),
            "103% design capacity • 54 cycles"
        );
    }

    #[test]
    fn charge_behaviour_summary_includes_current_and_alternatives() {
        assert_eq!(
            charge_behaviour_summary(&snapshot()),
            "auto (inhibit-charge, force-discharge)"
        );
    }

    #[test]
    fn control_note_reports_upower_when_direct_write_is_unavailable() {
        assert_eq!(
            control_note(&snapshot()),
            "Charge limit controls are available through UPower"
        );
    }

    #[test]
    fn control_state_requires_threshold_support_and_write_path() {
        let mut snapshot = snapshot();
        assert!(battery_health_can_control(&snapshot));

        snapshot.charge_control_upower_available = false;
        assert!(!battery_health_can_control(&snapshot));

        snapshot.charge_control_writable = true;
        assert!(battery_health_can_control(&snapshot));
    }

    #[test]
    fn subtitle_uses_configured_health_stop_threshold() {
        assert_eq!(HEALTH_CHARGE_STOP_THRESHOLD, 80);
    }
}
