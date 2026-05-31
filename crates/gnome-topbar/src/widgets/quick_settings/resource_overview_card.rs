//! Resource overview card for Quick Settings.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk4::gio;
use gtk4::glib::{self, SourceId};
use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Label, ListBox, Orientation, Revealer};
use tracing::warn;

use crate::services::icons::IconsService;
use crate::services::resource_monitor::{
    CpuSample, DiskSnapshot, ResourceLevel, ResourceSnapshot, cpu_level, disk_level, memory_level,
    read_resource_snapshot,
};
use crate::services::surfaces::SurfaceStyleManager;
use crate::styles::{button, card, color, icon, qs};

use super::components::{CardLabel, ExpanderButton, ListRow};
use super::ui_helpers::{
    CARD_REVEALER_DURATION_MS, ExpandableCard, ExpandableCardBase, build_slide_down_revealer,
    clear_list_box, create_qs_list_box,
};

const COLLAPSED_INTERVAL: Duration = Duration::from_secs(5);
const EXPANDED_INTERVAL: Duration = Duration::from_secs(2);

pub struct ResourceOverviewCardState {
    pub base: ExpandableCardBase,
    summary_label: RefCell<Option<Label>>,
    timer: RefCell<Option<SourceId>>,
    last_cpu: RefCell<Option<CpuSample>>,
    expanded: Cell<bool>,
    generation: Cell<u64>,
}

impl ResourceOverviewCardState {
    pub fn new() -> Self {
        Self {
            base: ExpandableCardBase::new(),
            summary_label: RefCell::new(None),
            timer: RefCell::new(None),
            last_cpu: RefCell::new(None),
            expanded: Cell::new(false),
            generation: Cell::new(0),
        }
    }

    pub fn start_polling(self: &Rc<Self>) {
        self.restart_timer();
    }

    pub fn stop_polling(&self) {
        self.generation.set(self.generation.get().wrapping_add(1));
        if let Some(source) = self.timer.borrow_mut().take() {
            source.remove();
        }
        *self.last_cpu.borrow_mut() = None;
        self.expanded.set(false);
    }

    pub fn set_expanded(self: &Rc<Self>, expanded: bool) {
        if self.expanded.get() == expanded {
            return;
        }
        self.expanded.set(expanded);
        if self.timer.borrow().is_some() {
            self.restart_timer();
        }
    }

    fn restart_timer(self: &Rc<Self>) {
        if let Some(source) = self.timer.borrow_mut().take() {
            source.remove();
        }

        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);
        refresh(self, generation);

        let interval = if self.expanded.get() {
            EXPANDED_INTERVAL
        } else {
            COLLAPSED_INTERVAL
        };
        let state = Rc::clone(self);
        let source = glib::timeout_add_local(interval, move || {
            refresh(&state, generation);
            glib::ControlFlow::Continue
        });
        *self.timer.borrow_mut() = Some(source);
    }
}

impl Default for ResourceOverviewCardState {
    fn default() -> Self {
        Self::new()
    }
}

impl ExpandableCard for ResourceOverviewCardState {
    fn base(&self) -> &ExpandableCardBase {
        &self.base
    }
}

pub fn build_resource_overview_card(
    state: &Rc<ResourceOverviewCardState>,
) -> (gtk4::Widget, Revealer, Option<gtk4::Button>) {
    let card_box = GtkBox::new(Orientation::Horizontal, 4);
    card_box.add_css_class(card::QS);
    card_box.add_css_class(card::BASE);
    card_box.add_css_class(qs::RESOURCE_OVERVIEW);
    card_box.set_hexpand(true);

    let content = GtkBox::new(Orientation::Horizontal, 6);
    content.add_css_class(button::RESET);
    content.set_hexpand(true);
    content.set_vexpand(true);
    content.set_halign(Align::Fill);
    content.set_valign(Align::Fill);

    let overview_icon = IconsService::global().create_icon(
        "cpu-symbolic",
        &[icon::TEXT, qs::TOGGLE_ICON, color::PRIMARY],
    );
    overview_icon.widget().set_valign(Align::Center);
    content.append(&overview_icon.widget());
    *state.base.card_icon.borrow_mut() = Some(overview_icon);

    let label_result = CardLabel::new("Resources")
        .subtitle("CPU -- • RAM -- • / --")
        .width_chars(16)
        .title_class(qs::TOGGLE_LABEL)
        .subtitle_class(qs::TOGGLE_SUBTITLE)
        .build();
    *state.summary_label.borrow_mut() = label_result.subtitle.clone();
    content.append(&label_result.container);
    card_box.append(&content);

    let expander = ExpanderButton::new().build();
    card_box.append(&expander.button);
    *state.base.arrow.borrow_mut() = Some(expander.icon_handle.clone());

    let details = build_resource_details(state);
    let revealer = build_slide_down_revealer(Some(&details), CARD_REVEALER_DURATION_MS);
    *state.base.revealer.borrow_mut() = Some(revealer.clone());

    (card_box.upcast(), revealer, Some(expander.button))
}

fn build_resource_details(state: &Rc<ResourceOverviewCardState>) -> GtkBox {
    let container = GtkBox::new(Orientation::Vertical, 0);
    container.add_css_class(qs::RESOURCE_OVERVIEW_DETAILS);

    let list_box = create_qs_list_box();
    list_box.add_css_class(qs::RESOURCE_OVERVIEW_LIST);
    container.append(&list_box);
    *state.base.list_box.borrow_mut() = Some(list_box);

    container
}

fn refresh(state: &Rc<ResourceOverviewCardState>, generation: u64) {
    let previous_cpu = *state.last_cpu.borrow();
    let state = Rc::clone(state);

    glib::spawn_future_local(async move {
        let result = gio::spawn_blocking(move || read_resource_snapshot(previous_cpu)).await;
        if state.generation.get() != generation {
            return;
        }

        match result {
            Ok(Ok((snapshot, cpu_sample))) => {
                *state.last_cpu.borrow_mut() = cpu_sample;
                update_card(&state, &snapshot);
            }
            Ok(Err(err)) => warn!("resource overview update failed: {}", err),
            Err(err) => warn!("resource overview task failed: {:?}", err),
        }
    });
}

fn update_card(state: &ResourceOverviewCardState, snapshot: &ResourceSnapshot) {
    let cpu_text = snapshot
        .cpu_usage
        .map(|usage| format!("{usage}%"))
        .unwrap_or_else(|| "--".to_string());
    let memory_text = format!("{}%", snapshot.memory.used_percent);
    let disk_text = snapshot
        .root_disk()
        .map(|disk| format!("/ {}%", disk.used_percent))
        .unwrap_or_else(|| "/ --".to_string());

    if let Some(label) = state.summary_label.borrow().as_ref() {
        let summary = format!("CPU {cpu_text} • RAM {memory_text} • {disk_text}");
        let warning = cpu_level(snapshot.cpu_usage) == ResourceLevel::Warning
            || memory_level(&snapshot.memory) == ResourceLevel::Warning
            || snapshot
                .root_disk()
                .is_some_and(|disk| disk_level(disk) == ResourceLevel::Warning);
        set_label_if_changed(label, &summary);
        set_warning_class(label, warning);
    }

    populate_details(state, snapshot);
}

fn populate_details(state: &ResourceOverviewCardState, snapshot: &ResourceSnapshot) {
    let Some(list_box) = state.base.list_box.borrow().as_ref().cloned() else {
        return;
    };

    clear_list_box(&list_box);

    append_detail_row(
        &list_box,
        "CPU",
        &snapshot
            .cpu_usage
            .map(|usage| format!("{usage}% used"))
            .unwrap_or_else(|| "Calculating usage".to_string()),
        cpu_level(snapshot.cpu_usage),
    );
    append_detail_row(
        &list_box,
        "Memory",
        &format!(
            "{} used of {} • {} available",
            format_kib(snapshot.memory.used_kib),
            format_kib(snapshot.memory.total_kib),
            format_kib(snapshot.memory.available_kib),
        ),
        memory_level(&snapshot.memory),
    );

    if snapshot.memory.swap_total_kib > 0 {
        append_detail_row(
            &list_box,
            "Swap",
            &format!(
                "{} used of {}",
                format_kib(snapshot.memory.swap_used_kib),
                format_kib(snapshot.memory.swap_total_kib),
            ),
            ResourceLevel::Normal,
        );
    }

    if snapshot.disks.is_empty() {
        append_detail_row(
            &list_box,
            "Disk",
            "No local filesystems found",
            ResourceLevel::Normal,
        );
    } else {
        for disk in &snapshot.disks {
            append_disk_row(&list_box, disk);
        }
    }

    SurfaceStyleManager::global().apply_pango_attrs_all(&list_box);
}

fn append_disk_row(list_box: &ListBox, disk: &DiskSnapshot) {
    append_detail_row(
        list_box,
        &format!("Disk {}", disk.mount_point),
        &format!(
            "{}% used • {} of {}",
            disk.used_percent,
            format_bytes(disk.used_bytes),
            format_bytes(disk.total_bytes),
        ),
        disk_level(disk),
    );
}

fn append_detail_row(list_box: &ListBox, title: &str, subtitle: &str, level: ResourceLevel) {
    let row = ListRow::builder().title(title).subtitle(subtitle).build();
    row.row.set_activatable(false);
    row.row.set_focusable(false);
    if level == ResourceLevel::Warning
        && let Some(subtitle) = row.subtitle.as_ref()
    {
        subtitle.add_css_class(color::ERROR);
    }
    list_box.append(&row.row);
}

fn set_label_if_changed(label: &Label, text: &str) {
    if label.label().as_str() != text {
        label.set_label(text);
    }
}

fn set_warning_class(label: &Label, warning: bool) {
    if warning {
        label.add_css_class(color::ERROR);
    } else {
        label.remove_css_class(color::ERROR);
    }
}

fn format_kib(kib: u64) -> String {
    format_bytes(kib.saturating_mul(1024))
}

fn format_bytes(bytes: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;

    if bytes >= 10 * 1024 * 1024 * 1024 {
        format!("{:.0} GiB", bytes as f64 / GIB)
    } else if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1} GiB", bytes as f64 / GIB)
    } else {
        format!("{:.0} MiB", bytes as f64 / MIB)
    }
}
