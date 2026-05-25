//! Alert-only CPU and memory threshold widget.

use std::cell::RefCell;
use std::fs;
use std::rc::Rc;

use gnome_topbar_core::config::WidgetEntry;
use gtk4::gio;
use gtk4::glib::{self, SourceId};
use gtk4::prelude::*;
use gtk4::{Box as GtkBox, Label};
use tracing::warn;

use crate::services::icons::{IconHandle, IconsService};
use crate::services::tooltip::TooltipManager;
use crate::styles::{color, icon, widget as wgt};
use crate::widgets::base::BaseWidget;
use crate::widgets::{WidgetConfig, warn_unknown_options};

fn set_label_if_changed(label: &Label, text: &str) {
    if label.label().as_str() != text {
        label.set_label(text);
    }
}

fn set_visible_if_changed(widget: &impl IsA<gtk4::Widget>, visible: bool) {
    if widget.as_ref().is_visible() != visible {
        widget.as_ref().set_visible(visible);
    }
}

const DEFAULT_INTERVAL_SECS: u64 = 5;
const DEFAULT_CPU_THRESHOLD: u8 = 90;
const DEFAULT_MEMORY_THRESHOLD: u8 = 90;

const KNOWN_OPTIONS: &[&str] = &[
    "interval",
    "cpu_threshold",
    "memory_threshold",
    "show_cpu",
    "show_memory",
    "tooltip",
    "max_chars",
];

#[derive(Debug, Clone)]
pub struct SystemMonitorConfig {
    interval: u64,
    cpu_threshold: u8,
    memory_threshold: u8,
    show_cpu: bool,
    show_memory: bool,
    tooltip: String,
    max_chars: Option<usize>,
}

impl Default for SystemMonitorConfig {
    fn default() -> Self {
        Self {
            interval: DEFAULT_INTERVAL_SECS,
            cpu_threshold: DEFAULT_CPU_THRESHOLD,
            memory_threshold: DEFAULT_MEMORY_THRESHOLD,
            show_cpu: true,
            show_memory: true,
            tooltip: "System monitor".to_string(),
            max_chars: None,
        }
    }
}

impl WidgetConfig for SystemMonitorConfig {
    fn from_entry(entry: &WidgetEntry) -> Self {
        warn_unknown_options("system_monitor", entry, KNOWN_OPTIONS);
        let default = Self::default();

        Self {
            interval: entry
                .options
                .get("interval")
                .and_then(|v| v.as_integer())
                .filter(|v| *v >= 0)
                .map(|v| v as u64)
                .unwrap_or(default.interval),
            cpu_threshold: percent_option(entry, "cpu_threshold", default.cpu_threshold),
            memory_threshold: percent_option(entry, "memory_threshold", default.memory_threshold),
            show_cpu: entry
                .options
                .get("show_cpu")
                .and_then(|v| v.as_bool())
                .unwrap_or(default.show_cpu),
            show_memory: entry
                .options
                .get("show_memory")
                .and_then(|v| v.as_bool())
                .unwrap_or(default.show_memory),
            tooltip: entry
                .options
                .get("tooltip")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or(&default.tooltip)
                .to_string(),
            max_chars: entry
                .options
                .get("max_chars")
                .and_then(|v| v.as_integer())
                .filter(|v| *v > 0)
                .map(|v| v as usize),
        }
    }
}

fn percent_option(entry: &WidgetEntry, key: &str, default: u8) -> u8 {
    entry
        .options
        .get(key)
        .and_then(|v| v.as_integer())
        .map(|v| v.clamp(0, 100) as u8)
        .unwrap_or(default)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CpuSample {
    idle: u64,
    total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SystemMonitorDisplay {
    cpu_text: Option<String>,
    memory_text: Option<String>,
    tooltip: String,
}

pub struct SystemMonitorWidget {
    base: BaseWidget,
    _cpu_row: GtkBox,
    _cpu_label: Label,
    _cpu_icon: IconHandle,
    _memory_row: GtkBox,
    _memory_label: Label,
    _memory_icon: IconHandle,
    _timer: Rc<RefCell<Option<SourceId>>>,
    _last_cpu: Rc<RefCell<Option<CpuSample>>>,
}

impl SystemMonitorWidget {
    pub fn new(config: SystemMonitorConfig) -> Self {
        let base = BaseWidget::new(&[wgt::SYSTEM_MONITOR]);
        base.set_tooltip(&config.tooltip);

        let (cpu_row, cpu_label, cpu_icon) = build_alert_pair("cpu-symbolic");
        let (memory_row, memory_label, memory_icon) = build_alert_pair("ram-symbolic");
        base.content().append(&cpu_row);
        base.content().append(&memory_row);
        base.widget().set_visible(false);

        let timer = Rc::new(RefCell::new(None));
        let last_cpu = Rc::new(RefCell::new(None));
        refresh_system_monitor(
            base.widget(),
            &cpu_row,
            &cpu_label,
            &memory_row,
            &memory_label,
            &config,
            &last_cpu,
        );

        if config.interval > 0 {
            let root_for_timer = base.widget().clone();
            let cpu_row_for_timer = cpu_row.clone();
            let cpu_label_for_timer = cpu_label.clone();
            let memory_row_for_timer = memory_row.clone();
            let memory_label_for_timer = memory_label.clone();
            let config_for_timer = config.clone();
            let last_cpu_for_timer = Rc::clone(&last_cpu);
            let interval = u32::try_from(config.interval).unwrap_or(u32::MAX);
            let source = glib::timeout_add_seconds_local(interval, move || {
                refresh_system_monitor(
                    &root_for_timer,
                    &cpu_row_for_timer,
                    &cpu_label_for_timer,
                    &memory_row_for_timer,
                    &memory_label_for_timer,
                    &config_for_timer,
                    &last_cpu_for_timer,
                );
                glib::ControlFlow::Continue
            });
            *timer.borrow_mut() = Some(source);
        }

        Self {
            base,
            _cpu_row: cpu_row,
            _cpu_label: cpu_label,
            _cpu_icon: cpu_icon,
            _memory_row: memory_row,
            _memory_label: memory_label,
            _memory_icon: memory_icon,
            _timer: timer,
            _last_cpu: last_cpu,
        }
    }

    pub fn widget(&self) -> &GtkBox {
        self.base.widget()
    }
}

impl Drop for SystemMonitorWidget {
    fn drop(&mut self) {
        if let Some(source_id) = self._timer.borrow_mut().take() {
            source_id.remove();
        }
    }
}

fn build_alert_pair(icon_name: &str) -> (GtkBox, Label, IconHandle) {
    let row = GtkBox::new(gtk4::Orientation::Horizontal, 0);
    row.add_css_class(wgt::SYSTEM_MONITOR);
    row.set_spacing(2);
    row.set_visible(false);

    let icon_handle = IconsService::global().create_icon(icon_name, &[icon::ICON, color::ERROR]);
    row.append(&icon_handle.widget());

    let label = Label::new(None);
    label.add_css_class(wgt::SYSTEM_MONITOR);
    label.add_css_class(color::ERROR);
    label.set_xalign(0.5);
    row.append(&label);

    (row, label, icon_handle)
}

fn refresh_system_monitor(
    root: &GtkBox,
    cpu_row: &GtkBox,
    cpu_label: &Label,
    memory_row: &GtkBox,
    memory_label: &Label,
    config: &SystemMonitorConfig,
    last_cpu: &Rc<RefCell<Option<CpuSample>>>,
) {
    let root = root.clone();
    let cpu_row = cpu_row.clone();
    let cpu_label = cpu_label.clone();
    let memory_row = memory_row.clone();
    let memory_label = memory_label.clone();
    let config = config.clone();
    let previous_cpu = *last_cpu.borrow();
    let last_cpu = Rc::clone(last_cpu);
    let max_chars = config.max_chars;

    glib::spawn_future_local(async move {
        let result =
            gio::spawn_blocking(move || fetch_system_monitor_display(&config, previous_cpu)).await;

        match result {
            Ok(Ok((display, cpu_sample))) => {
                *last_cpu.borrow_mut() = cpu_sample;
                if let Some(display) = display {
                    update_alert_pair(&cpu_row, &cpu_label, display.cpu_text.as_deref(), max_chars);
                    update_alert_pair(
                        &memory_row,
                        &memory_label,
                        display.memory_text.as_deref(),
                        max_chars,
                    );
                    TooltipManager::global().set_styled_tooltip(&root, &display.tooltip);
                    set_visible_if_changed(&root, true);
                } else {
                    set_label_if_changed(&cpu_label, "");
                    set_label_if_changed(&memory_label, "");
                    set_visible_if_changed(&cpu_row, false);
                    set_visible_if_changed(&memory_row, false);
                    set_visible_if_changed(&root, false);
                }
            }
            Ok(Err(err)) => {
                warn!("system monitor update failed: {}", err);
                set_visible_if_changed(&root, false);
            }
            Err(err) => {
                warn!("system monitor task failed: {:?}", err);
                set_visible_if_changed(&root, false);
            }
        }
    });
}

fn update_alert_pair(row: &GtkBox, label: &Label, text: Option<&str>, max_chars: Option<usize>) {
    if let Some(text) = text {
        set_label_if_changed(label, &truncate_label(text, max_chars));
        set_visible_if_changed(row, true);
    } else {
        set_label_if_changed(label, "");
        set_visible_if_changed(row, false);
    }
}

fn fetch_system_monitor_display(
    config: &SystemMonitorConfig,
    previous_cpu: Option<CpuSample>,
) -> Result<(Option<SystemMonitorDisplay>, Option<CpuSample>), String> {
    let cpu_sample = if config.show_cpu {
        Some(read_cpu_sample()?)
    } else {
        None
    };
    let cpu_usage = previous_cpu
        .zip(cpu_sample)
        .and_then(|(previous, current)| cpu_usage_percent(previous, current));
    let memory_usage = if config.show_memory {
        Some(read_memory_usage_percent()?)
    } else {
        None
    };

    let display = build_display(config, cpu_usage, memory_usage);
    Ok((display, cpu_sample))
}

fn build_display(
    config: &SystemMonitorConfig,
    cpu_usage: Option<u8>,
    memory_usage: Option<u8>,
) -> Option<SystemMonitorDisplay> {
    let cpu_alert = config
        .show_cpu
        .then_some(cpu_usage)
        .flatten()
        .filter(|usage| *usage >= config.cpu_threshold);
    let memory_alert = config
        .show_memory
        .then_some(memory_usage)
        .flatten()
        .filter(|usage| *usage >= config.memory_threshold);

    if cpu_alert.is_none() && memory_alert.is_none() {
        return None;
    }

    let cpu_text = cpu_alert.map(|usage| format!("{usage}%"));
    let memory_text = memory_alert.map(|usage| format!("{usage}%"));

    let mut tooltip_parts = Vec::new();
    if let Some(usage) = cpu_usage {
        tooltip_parts.push(format!("CPU {usage}% (alert at {}%)", config.cpu_threshold));
    }
    if let Some(usage) = memory_usage {
        tooltip_parts.push(format!(
            "Memory {usage}% (alert at {}%)",
            config.memory_threshold
        ));
    }

    Some(SystemMonitorDisplay {
        cpu_text,
        memory_text,
        tooltip: if tooltip_parts.is_empty() {
            config.tooltip.clone()
        } else {
            tooltip_parts.join("\n")
        },
    })
}

fn read_cpu_sample() -> Result<CpuSample, String> {
    let stat = fs::read_to_string("/proc/stat").map_err(|e| format!("read /proc/stat: {e}"))?;
    parse_cpu_sample(&stat).ok_or_else(|| "parse /proc/stat cpu line".to_string())
}

pub(crate) fn parse_cpu_sample(stat: &str) -> Option<CpuSample> {
    let line = stat.lines().find(|line| line.starts_with("cpu "))?;
    let values: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|part| part.parse::<u64>().ok())
        .collect();
    if values.len() < 4 {
        return None;
    }

    let idle = values.get(3).copied().unwrap_or(0) + values.get(4).copied().unwrap_or(0);
    let total = values.iter().sum();
    Some(CpuSample { idle, total })
}

pub(crate) fn cpu_usage_percent(previous: CpuSample, current: CpuSample) -> Option<u8> {
    let total_delta = current.total.checked_sub(previous.total)?;
    let idle_delta = current.idle.checked_sub(previous.idle)?;
    if total_delta == 0 || idle_delta > total_delta {
        return None;
    }

    let busy_delta = total_delta - idle_delta;
    Some(((busy_delta * 100 + total_delta / 2) / total_delta).min(100) as u8)
}

fn read_memory_usage_percent() -> Result<u8, String> {
    let meminfo =
        fs::read_to_string("/proc/meminfo").map_err(|e| format!("read /proc/meminfo: {e}"))?;
    parse_memory_usage_percent(&meminfo).ok_or_else(|| "parse /proc/meminfo".to_string())
}

pub(crate) fn parse_memory_usage_percent(meminfo: &str) -> Option<u8> {
    let mut total = None;
    let mut available = None;

    for line in meminfo.lines() {
        if let Some(value) = parse_meminfo_kib(line, "MemTotal:") {
            total = Some(value);
        } else if let Some(value) = parse_meminfo_kib(line, "MemAvailable:") {
            available = Some(value);
        }
    }

    let total = total?;
    let available = available?;
    if total == 0 || available > total {
        return None;
    }

    let used = total - available;
    Some(((used * 100 + total / 2) / total).min(100) as u8)
}

fn parse_meminfo_kib(line: &str, key: &str) -> Option<u64> {
    line.strip_prefix(key)?
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()
}

fn truncate_label(text: &str, max_chars: Option<usize>) -> String {
    let Some(max) = max_chars else {
        return text.to_string();
    };
    if text.chars().count() <= max {
        return text.to_string();
    }

    let mut truncated: String = text.chars().take(max.saturating_sub(1)).collect();
    truncated.push_str("...");
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use toml::Value;

    fn make_entry(options: HashMap<String, Value>) -> WidgetEntry {
        WidgetEntry {
            name: "system_monitor".to_string(),
            options,
        }
    }

    #[test]
    fn config_defaults_to_alert_only_thresholds() {
        let config = SystemMonitorConfig::from_entry(&make_entry(HashMap::new()));
        assert_eq!(config.interval, 5);
        assert_eq!(config.cpu_threshold, 90);
        assert_eq!(config.memory_threshold, 90);
        assert!(config.show_cpu);
        assert!(config.show_memory);
    }

    #[test]
    fn config_clamps_percent_thresholds() {
        let mut options = HashMap::new();
        options.insert("cpu_threshold".to_string(), Value::Integer(150));
        options.insert("memory_threshold".to_string(), Value::Integer(-20));
        let config = SystemMonitorConfig::from_entry(&make_entry(options));
        assert_eq!(config.cpu_threshold, 100);
        assert_eq!(config.memory_threshold, 0);
    }

    #[test]
    fn parses_proc_stat_cpu_sample() {
        let sample = parse_cpu_sample("cpu  10 20 30 40 5 0 0 0 0 0\ncpu0 1 2 3 4\n").unwrap();
        assert_eq!(sample.idle, 45);
        assert_eq!(sample.total, 105);
    }

    #[test]
    fn computes_cpu_usage_from_sample_delta() {
        let previous = CpuSample {
            idle: 100,
            total: 200,
        };
        let current = CpuSample {
            idle: 150,
            total: 300,
        };
        assert_eq!(cpu_usage_percent(previous, current), Some(50));
    }

    #[test]
    fn parses_memory_usage_from_meminfo() {
        let meminfo = "\
MemTotal:       1000000 kB
MemFree:         100000 kB
MemAvailable:    250000 kB
Buffers:          10000 kB
";
        assert_eq!(parse_memory_usage_percent(meminfo), Some(75));
    }

    #[test]
    fn build_display_hides_below_thresholds() {
        let config = SystemMonitorConfig::default();
        assert_eq!(build_display(&config, Some(20), Some(30)), None);
    }

    #[test]
    fn build_display_shows_crossed_thresholds() {
        let config = SystemMonitorConfig {
            cpu_threshold: 80,
            memory_threshold: 70,
            ..Default::default()
        };
        let display = build_display(&config, Some(81), Some(69)).unwrap();
        assert_eq!(display.cpu_text.as_deref(), Some("81%"));
        assert_eq!(display.memory_text, None);
        assert!(display.tooltip.contains("CPU 81%"));
        assert!(display.tooltip.contains("Memory 69%"));
    }
}
