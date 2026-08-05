//! The system monitor: nothing at all, until something is wrong.
//!
//! ```text
//!                                    healthy: zero width
//!  ⚙ 97%                             CPU over its threshold
//!  ⚙ 97%  ▤ 91%                      and memory with it
//! ```
//!
//! **Alert-only, by decision.** v1 drew a permanent `CPU 12% MEM 43%` readout,
//! which is a number the eye stops seeing after a day and a relayout every five
//! seconds for the privilege. This one is invisible while the machine is
//! healthy and fades in when a metric crosses its configured threshold; the
//! full picture is one click away, in the same resource card Quick Settings
//! draws.
//!
//! ## Why hysteresis
//!
//! A threshold on a moving number flickers. CPU on a laptop crosses 90% for one
//! sample every time a browser tab loads, and a widget that appears and
//! disappears with it is worse than one that is always there. So each metric
//! runs through [`Tracker`]:
//!
//! - it takes **two consecutive samples** at or above the threshold to appear,
//! - **two consecutive samples five points below it** to go away again,
//! - and the same two samples at threshold + 8 to escalate to urgent.
//!
//! Between "threshold − 5" and the threshold nothing changes at all, which is
//! the dead band that stops a number hovering on the line from oscillating. The
//! whole thing is a pure state machine with no clock in it, which is why the
//! tests below can walk it a sample at a time.

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Image, Label, Orientation};
use topbar_core::config::SystemMonitorConfig;
use topbar_services::{ResourceState, Services};

use crate::anim::{Animation, AnimationParams, Easing};
use crate::bar::BarContext;
use crate::bridge::{self, BindingGuard};
use crate::style::{classes, icons};
use crate::surfaces::popovers::{self, PopoverContent, PopoverHandle};
use crate::surfaces::tooltip::TooltipHandle;
use crate::widgets::quick_settings::cards::resources::ResourceOverview;
use crate::widgets::shell::WidgetShell;
use crate::widgets::{install_click_commands, set_class};

/// Widget name, for the popover registry and failed click commands.
const WIDGET_NAME: &str = "system_monitor";

/// How long the widget takes to fade in when a metric first crosses.
const FADE_MS: u64 = 150;

/// How far past its threshold a metric has to go to read as urgent.
const ESCALATE: u8 = 8;

/// How far below its threshold a metric has to fall to go quiet again.
const RELEASE: u8 = 5;

/// How many consecutive samples any change of level needs.
const CONSECUTIVE: u8 = 2;

/// One metric's alert level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Level {
    /// Below the threshold, or on its way back down through the dead band.
    #[default]
    Quiet,
    /// Over the threshold.
    Warning,
    /// Well over it.
    Urgent,
}

/// Which of the three the panel watches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Metric {
    /// Processor use since the last sample.
    Cpu,
    /// Memory in use.
    Memory,
    /// The fullest mounted filesystem.
    Disk,
}

/// One metric's hysteresis.
#[derive(Debug, Clone)]
struct Tracker {
    /// Where the user put the line.
    threshold: u8,
    /// The level currently in force.
    level: Level,
    /// A level the samples are arguing for, and how many have argued for it.
    pending: Option<(Level, u8)>,
}

impl Tracker {
    /// A tracker watching `threshold`, starting quiet.
    fn new(threshold: u8) -> Self {
        Self {
            threshold: threshold.clamp(1, 100),
            level: Level::Quiet,
            pending: None,
        }
    }

    /// The level `value` argues for, holding the current one inside the band.
    ///
    /// The dead band is deliberately asymmetric: crossing *up* happens at the
    /// threshold, crossing *down* five points below it. A number sitting on the
    /// line therefore stays wherever it already was.
    fn wanted(&self, value: u8) -> Level {
        if value >= self.threshold.saturating_add(ESCALATE) {
            Level::Urgent
        } else if value >= self.threshold {
            Level::Warning
        } else if value < self.release() {
            Level::Quiet
        } else {
            self.level
        }
    }

    /// The reading a metric has to fall below to be clear of its threshold.
    ///
    /// Five points below it, except on a threshold too low to have five points
    /// underneath — at least one is always kept, because a release point of
    /// zero is one nothing can ever be below, and the widget would then never
    /// go away again on a `cpu_threshold = 5`.
    fn release(&self) -> u8 {
        self.threshold.saturating_sub(RELEASE).max(1)
    }

    /// Feed one sample, returning the level now in force.
    fn sample(&mut self, value: u8) -> Level {
        let wanted = self.wanted(value);
        if wanted == self.level {
            // The argument is over; whatever was building up is abandoned.
            self.pending = None;
            return self.level;
        }

        let seen = match self.pending {
            Some((level, seen)) if level == wanted => seen + 1,
            _ => 1,
        };
        if seen >= CONSECUTIVE {
            self.level = wanted;
            self.pending = None;
        } else {
            self.pending = Some((wanted, seen));
        }
        self.level
    }
}

/// One thing worth saying about the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Alert {
    /// Which metric crossed.
    pub metric: Metric,
    /// How badly.
    pub level: Level,
    /// What it read at the sample that decided it.
    pub percent: u8,
}

/// The three trackers, and the reading they last saw.
#[derive(Debug, Clone)]
pub struct Monitor {
    cpu: Tracker,
    memory: Tracker,
    disk: Tracker,
    /// The last percentages, for the tooltip. CPU is absent for one sample.
    last: (Option<u8>, u8, u8),
}

impl Monitor {
    /// Watch the three thresholds `settings` configures.
    pub fn new(settings: &SystemMonitorConfig) -> Self {
        Self {
            cpu: Tracker::new(settings.cpu_threshold as u8),
            memory: Tracker::new(settings.memory_threshold as u8),
            disk: Tracker::new(settings.disk_threshold as u8),
            last: (None, 0, 0),
        }
    }

    /// Feed one snapshot, returning everything now worth saying.
    ///
    /// A snapshot with no CPU reading in it — the first five seconds of a
    /// session, and the one sample after a resume — is not fed to the CPU
    /// tracker at all. The service leaves the field empty precisely because it
    /// has no delta to compute, and treating that as a zero would count as a
    /// sample arguing for quiet.
    pub fn sample(&mut self, state: &ResourceState) -> Vec<Alert> {
        let disk = state
            .disks
            .iter()
            .map(|disk| disk.used_pct)
            .max()
            .unwrap_or(0);
        self.last = (state.cpu_pct, state.memory.used_pct, disk);

        let mut alerts = Vec::new();
        if let Some(cpu) = state.cpu_pct {
            push(&mut alerts, Metric::Cpu, self.cpu.sample(cpu), cpu);
        }
        push(
            &mut alerts,
            Metric::Memory,
            self.memory.sample(state.memory.used_pct),
            state.memory.used_pct,
        );
        push(&mut alerts, Metric::Disk, self.disk.sample(disk), disk);
        alerts
    }

    /// Every metric, whatever it reads. The tooltip says all three, always.
    pub fn tooltip(&self) -> String {
        let (cpu, memory, disk) = self.last;
        let cpu = match cpu {
            Some(percent) => format!("{percent}%"),
            // The first sample of a session, and the one after a resume.
            None => "—".to_string(),
        };
        format!("CPU {cpu} · Memory {memory}% · Disk {disk}%")
    }
}

/// Add an alert, unless the metric is quiet.
fn push(alerts: &mut Vec<Alert>, metric: Metric, level: Level, percent: u8) {
    if level == Level::Quiet {
        return;
    }
    alerts.push(Alert {
        metric,
        level,
        percent,
    });
}

/// The alert-only system monitor widget.
pub struct SystemMonitorWidget {
    shell: WidgetShell,
    /// Holds the rows and the state machine the render closure drives.
    _inner: Rc<Inner>,
    /// The popover's claim on the host.
    _popover: PopoverHandle,
    /// Keeps the widget subscribed to the sampler.
    _binding: BindingGuard,
}

impl SystemMonitorWidget {
    /// Build the widget from `[widgets.system_monitor]`.
    pub fn new(settings: &SystemMonitorConfig, context: &BarContext) -> Self {
        let shell = WidgetShell::new(classes::SYSTEM_MONITOR);
        shell.make_interactive();
        // Hidden until something is wrong, which is nearly always.
        shell.root().set_visible(false);

        let rows = [Metric::Cpu, Metric::Memory, Metric::Disk].map(|metric| {
            let row = Row::new(metric);
            shell.content().append(&row.root);
            row
        });

        let inner = Rc::new(Inner {
            wrapper: shell.root().clone(),
            rows,
            tooltip: shell.set_tooltip(&settings.tooltip),
            configured_tooltip: settings.tooltip.clone(),
            monitor: RefCell::new(Monitor::new(settings)),
            fade: Animation::new(shell.root()),
        });

        // One sampler, retuned rather than duplicated: the widget's interval is
        // the rate *it* wants, and the Quick Settings card wants the default.
        // Whichever is shorter serves both.
        let interval = std::time::Duration::from_secs(settings.interval.max(1))
            .min(topbar_services::resources::DEFAULT_INTERVAL);
        let handle = context.services.resources.handle().clone();
        topbar_services::Runtime::handle().spawn(async move { handle.configure(interval).await });

        let binding = bridge::bind_state(shell.root(), context.services.resources.state(), {
            let inner = Rc::downgrade(&inner);
            move |_: &gtk4::Box, state: &ResourceState| {
                if let Some(inner) = inner.upgrade() {
                    inner.render(state);
                }
            }
        });

        let popover = {
            let services = context.services.clone();
            popovers::attach(context, WIDGET_NAME, shell.root(), move || {
                Rc::new(Popover::new(&services)) as Rc<dyn PopoverContent>
            })
        };

        install_click_commands(
            shell.root(),
            WIDGET_NAME,
            settings.on_click_right.as_deref(),
            settings.on_click_middle.as_deref(),
        );

        Self {
            shell,
            _inner: inner,
            _popover: popover,
            _binding: binding,
        }
    }

    /// The widget to put in a bar section.
    pub fn root(&self) -> gtk4::Widget {
        self.shell.root().clone().upcast()
    }
}

/// One metric's icon and reading, shown only while that metric is alerting.
struct Row {
    root: gtk4::Box,
    value: Label,
}

impl Row {
    /// Build a row for `metric`.
    fn new(metric: Metric) -> Self {
        let root = gtk4::Box::new(Orientation::Horizontal, 4);
        root.set_visible(false);

        let image = Image::from_icon_name(icons::first_available(match metric {
            Metric::Cpu => icons::CPU,
            Metric::Memory => icons::MEMORY,
            Metric::Disk => icons::DISK,
        }));
        image.add_css_class(classes::SYSTEM_MONITOR_ICON);
        root.append(&image);

        let value = Label::new(None);
        value.add_css_class(classes::SYSTEM_MONITOR_VALUE);
        root.append(&value);

        Self { root, value }
    }

    /// Draw one alert, or take the row off the bar.
    fn set(&self, alert: Option<&Alert>) {
        let Some(alert) = alert else {
            self.root.set_visible(false);
            return;
        };
        let text = format!("{}%", alert.percent);
        if self.value.text() != text {
            self.value.set_text(&text);
        }
        set_class(
            &self.root,
            classes::STATE_URGENT,
            alert.level == Level::Urgent,
        );
        set_class(
            &self.root,
            classes::STATE_WARNING,
            alert.level == Level::Warning,
        );
        self.root.set_visible(true);
    }
}

/// Everything the render closure touches.
struct Inner {
    wrapper: gtk4::Box,
    /// One row per metric, in the order they are declared.
    rows: [Row; 3],
    tooltip: TooltipHandle,
    /// `tooltip`, the line the readings are put under.
    configured_tooltip: String,
    monitor: RefCell<Monitor>,
    fade: Animation,
}

impl Inner {
    /// Feed a snapshot through the state machine and draw the result.
    fn render(&self, state: &ResourceState) {
        let mut monitor = self.monitor.borrow_mut();
        let alerts = monitor.sample(state);
        self.tooltip.set_text(&format!(
            "{}\n{}",
            self.configured_tooltip,
            monitor.tooltip()
        ));
        drop(monitor);

        for (row, metric) in self
            .rows
            .iter()
            .zip([Metric::Cpu, Metric::Memory, Metric::Disk])
        {
            row.set(alerts.iter().find(|alert| alert.metric == metric));
        }

        let wanted = !alerts.is_empty();
        if wanted == self.wrapper.is_visible() {
            return;
        }
        if !wanted {
            self.fade.cancel();
            self.wrapper.set_opacity(1.0);
            self.wrapper.set_visible(false);
            return;
        }

        // Appearing is the whole event, so it is worth 150ms of fade rather
        // than a widget popping into the middle of the bar.
        self.wrapper.set_opacity(0.0);
        self.wrapper.set_visible(true);
        let wrapper = self.wrapper.clone();
        self.fade.start(
            AnimationParams::new(FADE_MS).with_easing(Easing::EaseOutCubic),
            Box::new(move |progress| wrapper.set_opacity(progress)),
            None,
        );
    }
}

/// The popover: the shared resource overview and nothing else.
struct Popover {
    overview: Rc<ResourceOverview>,
    root: gtk4::Box,
}

impl Popover {
    /// Build it around the same component Quick Settings mounts.
    fn new(services: &Services) -> Self {
        let root = gtk4::Box::new(Orientation::Vertical, 0);
        root.add_css_class(classes::SYSTEM_MONITOR_POPOVER);
        // No title: the popover is what the click asked for, and a heading
        // reading "System" above four rows of system readings is furniture.
        let overview = ResourceOverview::new(services, None);
        root.append(overview.root());
        Self { overview, root }
    }
}

impl PopoverContent for Popover {
    fn root(&self) -> gtk4::Widget {
        self.root.clone().upcast()
    }

    fn refresh(&self) {
        self.overview.refresh();
    }
}

#[cfg(test)]
mod tests {
    use topbar_services::{Disk, Memory};

    use super::*;

    /// A snapshot with the three readings in it.
    fn snapshot(cpu: Option<u8>, memory: u8, disk: u8) -> ResourceState {
        ResourceState {
            cpu_pct: cpu,
            memory: Memory {
                used_pct: memory,
                ..Memory::default()
            },
            disks: vec![Disk {
                mount: "/".to_string(),
                used_pct: disk,
                ..Disk::default()
            }],
        }
    }

    fn settings() -> SystemMonitorConfig {
        SystemMonitorConfig::default()
    }

    // --- the state machine, metric by metric ---

    #[test]
    fn one_sample_over_the_line_is_not_enough() {
        let mut tracker = Tracker::new(90);
        assert_eq!(tracker.sample(95), Level::Quiet, "a browser tab loading");
        assert_eq!(tracker.sample(10), Level::Quiet);
    }

    #[test]
    fn two_consecutive_samples_over_the_line_are() {
        let mut tracker = Tracker::new(90);
        assert_eq!(tracker.sample(90), Level::Quiet);
        assert_eq!(tracker.sample(90), Level::Warning, "exactly at it counts");
    }

    #[test]
    fn a_sample_below_the_line_resets_the_count() {
        let mut tracker = Tracker::new(90);
        tracker.sample(95);
        assert_eq!(tracker.sample(50), Level::Quiet);
        assert_eq!(tracker.sample(95), Level::Quiet, "counting starts again");
        assert_eq!(tracker.sample(95), Level::Warning);
    }

    #[test]
    fn coming_back_takes_five_points_of_room_and_two_samples() {
        let mut tracker = Tracker::new(90);
        tracker.sample(95);
        tracker.sample(95);
        assert_eq!(tracker.level, Level::Warning);

        // Inside the dead band: neither over the line nor clear of it.
        assert_eq!(tracker.sample(87), Level::Warning);
        assert_eq!(tracker.sample(86), Level::Warning);
        assert_eq!(
            tracker.sample(85),
            Level::Warning,
            "threshold − 5 is still in"
        );

        assert_eq!(
            tracker.sample(84),
            Level::Warning,
            "one sample is not enough"
        );
        assert_eq!(tracker.sample(84), Level::Quiet);
    }

    #[test]
    fn a_number_hovering_on_the_line_does_not_oscillate() {
        let mut tracker = Tracker::new(90);
        tracker.sample(91);
        tracker.sample(91);
        assert_eq!(tracker.level, Level::Warning);
        // Eight samples of a machine sitting right on its threshold. Every one
        // of them is inside the band, so nothing changes at all.
        for value in [89, 90, 88, 91, 87, 90, 86, 89] {
            assert_eq!(tracker.sample(value), Level::Warning, "at {value}");
        }
    }

    #[test]
    fn eight_points_past_the_line_escalates_and_falls_back() {
        let mut tracker = Tracker::new(90);
        tracker.sample(92);
        tracker.sample(92);
        assert_eq!(tracker.level, Level::Warning);

        assert_eq!(
            tracker.sample(98),
            Level::Warning,
            "one sample is not enough"
        );
        assert_eq!(tracker.sample(98), Level::Urgent);

        assert_eq!(tracker.sample(93), Level::Urgent);
        assert_eq!(tracker.sample(93), Level::Warning, "and back down again");
    }

    #[test]
    fn a_machine_that_goes_straight_to_pegged_lands_on_urgent() {
        let mut tracker = Tracker::new(90);
        assert_eq!(tracker.sample(100), Level::Quiet);
        assert_eq!(tracker.sample(100), Level::Urgent, "skipping warning");
    }

    #[test]
    fn a_threshold_near_the_ends_does_not_wrap_around() {
        // Escalation is unreachable at 95 + 8, and that is fine: it saturates
        // rather than wrapping to 3 and making everything urgent.
        let mut high = Tracker::new(95);
        high.sample(100);
        assert_eq!(high.sample(100), Level::Warning);

        // Release at 3 − 5 would underflow. Clamping it to *zero* would be
        // worse than the underflow: nothing is below zero, so the widget could
        // never go quiet again. One point of band is always kept.
        let mut low = Tracker::new(3);
        assert_eq!(low.release(), 1);
        low.sample(50);
        low.sample(50);
        assert_eq!(low.level, Level::Urgent);
        low.sample(0);
        assert_eq!(low.sample(0), Level::Quiet);

        // And the smoke run's own threshold, which is the shape a stress test
        // takes: idle really does read zero between spinners.
        let mut five = Tracker::new(5);
        assert_eq!(five.release(), 1);
        five.sample(90);
        five.sample(90);
        assert_eq!(five.level, Level::Urgent);
        five.sample(0);
        assert_eq!(five.sample(0), Level::Quiet);
    }

    #[test]
    fn a_threshold_outside_the_legal_range_is_clamped_rather_than_trusted() {
        // Validation rejects these, so this only covers a `Config` built by
        // hand — but a threshold of zero would make the widget permanently on.
        assert_eq!(Tracker::new(0).threshold, 1);
        assert_eq!(Tracker::new(200).threshold, 100);
    }

    // --- the three metrics together ---

    #[test]
    fn a_healthy_machine_says_nothing() {
        let mut monitor = Monitor::new(&settings());
        for _ in 0..5 {
            assert!(monitor.sample(&snapshot(Some(12), 43, 31)).is_empty());
        }
    }

    #[test]
    fn each_metric_crosses_on_its_own() {
        let mut monitor = Monitor::new(&settings());
        // Memory's threshold is 85 and the CPU's is 90, so this is memory
        // alone — twice, because once is never enough.
        monitor.sample(&snapshot(Some(12), 88, 31));
        let alerts = monitor.sample(&snapshot(Some(12), 88, 31));
        assert_eq!(
            alerts,
            vec![Alert {
                metric: Metric::Memory,
                level: Level::Warning,
                percent: 88,
            }]
        );

        // Now the CPU as well, without disturbing memory.
        monitor.sample(&snapshot(Some(95), 88, 31));
        let alerts = monitor.sample(&snapshot(Some(95), 88, 31));
        assert_eq!(alerts.len(), 2);
        assert_eq!(alerts[0].metric, Metric::Cpu);
        assert_eq!(alerts[1].metric, Metric::Memory);
    }

    #[test]
    fn the_fullest_disk_is_the_one_that_speaks() {
        let mut monitor = Monitor::new(&settings());
        let state = ResourceState {
            cpu_pct: Some(5),
            disks: vec![
                Disk {
                    mount: "/".to_string(),
                    used_pct: 40,
                    ..Disk::default()
                },
                Disk {
                    mount: "/home".to_string(),
                    used_pct: 97,
                    ..Disk::default()
                },
            ],
            ..ResourceState::default()
        };
        monitor.sample(&state);
        let alerts = monitor.sample(&state);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].metric, Metric::Disk);
        assert_eq!(alerts[0].percent, 97);
    }

    #[test]
    fn a_snapshot_with_no_cpu_reading_is_not_a_vote_for_quiet() {
        let mut monitor = Monitor::new(&settings());
        monitor.sample(&snapshot(Some(95), 10, 10));
        // The resume sample: the service has no delta, so it publishes none.
        monitor.sample(&snapshot(None, 10, 10));
        // The next real reading is therefore the *second* consecutive one.
        let alerts = monitor.sample(&snapshot(Some(95), 10, 10));
        assert_eq!(alerts.len(), 1, "the gap did not reset the count");
        assert_eq!(alerts[0].metric, Metric::Cpu);
    }

    #[test]
    fn the_tooltip_says_every_metric_whatever_is_wrong() {
        let mut monitor = Monitor::new(&settings());
        monitor.sample(&snapshot(Some(97), 62, 41));
        assert_eq!(monitor.tooltip(), "CPU 97% · Memory 62% · Disk 41%");
    }

    #[test]
    fn the_tooltip_admits_it_has_no_cpu_reading_yet() {
        let mut monitor = Monitor::new(&settings());
        monitor.sample(&snapshot(None, 62, 41));
        assert_eq!(monitor.tooltip(), "CPU — · Memory 62% · Disk 41%");
    }

    #[test]
    fn a_machine_with_no_disks_reads_as_empty_rather_than_full() {
        let mut monitor = Monitor::new(&settings());
        let state = ResourceState {
            cpu_pct: Some(5),
            ..ResourceState::default()
        };
        monitor.sample(&state);
        assert!(monitor.sample(&state).is_empty());
        assert!(monitor.tooltip().ends_with("Disk 0%"));
    }

    #[test]
    fn the_live_configuration_watches_the_thresholds_it_names() {
        let config: SystemMonitorConfig = topbar_core::Config::parse(include_str!(
            "../../../topbar-core/tests/fixtures/live-config.toml"
        ))
        .expect("the live config parses")
        .0
        .widgets
        .system_monitor;

        let monitor = Monitor::new(&config);
        assert_eq!(monitor.cpu.threshold, 90);
        assert_eq!(monitor.memory.threshold, 85);
        assert_eq!(monitor.disk.threshold, 90, "the v2 key keeps its default");
    }
}
