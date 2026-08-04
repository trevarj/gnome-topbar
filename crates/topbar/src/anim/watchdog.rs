//! Opt-in main-thread stall detector.
//!
//! The v1 defect this exists to catch is blocking I/O on the GTK main thread:
//! a socket read, a `call_sync`, or a `std::process::Command` in a click
//! handler. v2's architecture makes that structurally hard — services live in
//! a crate that cannot name a widget type — but "hard" is not "measured", so
//! this measures. Two instruments, because they fail differently:
//!
//! - **Frame gaps.** How long between frame-clock ticks. This is the metric
//!   that matters on a real session, but it is only meaningful while the
//!   compositor is actually sending frame callbacks: a nested niri whose
//!   window is not visible throttles to a fallback tick every couple of
//!   seconds, and every gap then looks like a stall that is not one.
//! - **Timer lateness.** How late a short repeating timeout fires. The main
//!   loop owes this timer a wake-up regardless of what the compositor is
//!   drawing, so lateness is a direct reading of "was the main thread busy",
//!   and it stays honest in a throttled nested session.
//!
//! Off unless `GNOME_TOPBAR_FRAME_WATCHDOG=1`. The tick callback keeps the
//! frame clock running continuously, which is what you want while stress
//! testing and emphatically not what you want in a panel that should sit idle.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk4::glib;
use gtk4::prelude::*;
use tracing::{info, warn};

/// Set this to `1` to turn the watchdog on.
const ENV_VAR: &str = "GNOME_TOPBAR_FRAME_WATCHDOG";
/// One vsync interval at 60 Hz, in microseconds.
///
/// A 60 Hz frame clock ticks every ~16.7 ms, so "over 16 ms" is the normal
/// case and not a stall; the first honest signal is a frame that was asked for
/// and never arrived.
const VSYNC_US: i64 = 16_667;
/// Gap past which a frame was certainly dropped, in microseconds.
const DROPPED_US: i64 = 2 * VSYNC_US;
/// Gap the user would feel as a hitch, in milliseconds.
const STALL_MS: i64 = 100;
/// Period of the lateness probe.
const PROBE_PERIOD: Duration = Duration::from_millis(8);
/// Lateness past which the main thread was doing something it should not.
const LATE_MS: u128 = 16;
/// How often to log the running summary.
const SUMMARY_INTERVAL: Duration = Duration::from_secs(5);

thread_local! {
    /// Only the first bar installs a watchdog; a second would double the log
    /// lines and the wake-ups without saying anything new.
    static INSTALLED: Cell<bool> = const { Cell::new(false) };
}

/// Attach both instruments, if the environment asks for them.
pub fn install(widget: &impl IsA<gtk4::Widget>) {
    if std::env::var(ENV_VAR).ok().as_deref() != Some("1") {
        return;
    }
    if INSTALLED.with(|installed| installed.replace(true)) {
        return;
    }

    info!(
        "frame watchdog on: summary every {}s",
        SUMMARY_INTERVAL.as_secs()
    );
    watch_frames(widget);
    watch_main_loop();
}

/// Frame-clock health: gaps between ticks.
fn watch_frames(widget: &impl IsA<gtk4::Widget>) {
    #[derive(Default)]
    struct Frames {
        last_us: Cell<i64>,
        window_start_us: Cell<i64>,
        count: Cell<u64>,
        over_vsync: Cell<u64>,
        dropped: Cell<u64>,
        worst_us: Cell<i64>,
    }

    let frames = Rc::new(Frames::default());
    widget.as_ref().add_tick_callback(move |_widget, clock| {
        let now = clock.frame_time();
        let previous = frames.last_us.replace(now);
        if previous == 0 {
            frames.window_start_us.set(now);
            return glib::ControlFlow::Continue;
        }

        let gap = now - previous;
        frames.count.set(frames.count.get() + 1);
        if gap > VSYNC_US {
            frames.over_vsync.set(frames.over_vsync.get() + 1);
        }
        if gap > DROPPED_US {
            frames.dropped.set(frames.dropped.get() + 1);
        }
        if gap > frames.worst_us.get() {
            frames.worst_us.set(gap);
        }

        if now - frames.window_start_us.get() >= SUMMARY_INTERVAL.as_micros() as i64 {
            info!(
                "frames {}, over-16ms {}, dropped {}, worst {:.1}ms",
                frames.count.get(),
                frames.over_vsync.get(),
                frames.dropped.get(),
                frames.worst_us.get() as f64 / 1_000.0,
            );
            frames.window_start_us.set(now);
            frames.count.set(0);
            frames.over_vsync.set(0);
            frames.dropped.set(0);
            frames.worst_us.set(0);
        }

        glib::ControlFlow::Continue
    });
}

/// Main-loop health: how late a short repeating timeout fires.
fn watch_main_loop() {
    struct Probe {
        due: Cell<Instant>,
        window_start: Cell<Instant>,
        ticks: Cell<u64>,
        late: Cell<u64>,
        worst_us: Cell<u128>,
    }

    let now = Instant::now();
    let probe = Rc::new(Probe {
        due: Cell::new(now + PROBE_PERIOD),
        window_start: Cell::new(now),
        ticks: Cell::new(0),
        late: Cell::new(0),
        worst_us: Cell::new(0),
    });

    glib::timeout_add_local(PROBE_PERIOD, move || {
        let now = Instant::now();
        let lateness = now.saturating_duration_since(probe.due.get()).as_micros();
        probe.due.set(now + PROBE_PERIOD);
        probe.ticks.set(probe.ticks.get() + 1);
        if lateness / 1_000 >= LATE_MS {
            probe.late.set(probe.late.get() + 1);
        }
        if lateness > probe.worst_us.get() {
            probe.worst_us.set(lateness);
        }
        if lateness / 1_000 >= STALL_MS as u128 {
            warn!("main loop blocked for {:.1}ms", lateness as f64 / 1_000.0);
        }

        if now.duration_since(probe.window_start.get()) >= SUMMARY_INTERVAL {
            info!(
                "main loop: {} wake-ups, {} later than {LATE_MS}ms, worst {:.1}ms",
                probe.ticks.get(),
                probe.late.get(),
                probe.worst_us.get() as f64 / 1_000.0,
            );
            probe.window_start.set(now);
            probe.ticks.set(0);
            probe.late.set(0);
            probe.worst_us.set(0);
        }

        glib::ControlFlow::Continue
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 16 ms is not a stall at 60 Hz — it is the normal frame interval. The
    /// first honest signal is a frame the clock asked for and never got.
    const _ORDERED: () = {
        assert!(DROPPED_US > VSYNC_US);
        assert!(STALL_MS * 1_000 > DROPPED_US);
        // The probe has to fire more often than the lateness it reports, or it
        // would call its own period a stall.
        assert!(PROBE_PERIOD.as_millis() < LATE_MS);
    };

    #[test]
    fn the_watchdog_is_off_by_default() {
        assert!(
            std::env::var(ENV_VAR).is_err(),
            "{ENV_VAR} leaked into the test env"
        );
    }
}
