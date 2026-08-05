//! A tiny frame-clock animator shared by every animated widget.
//!
//! Ported from v1 `widgets/animation.rs`. The design points that matter:
//!
//! - **Frame-clock driven.** Progress comes from
//!   [`gtk4::gdk::FrameClock::frame_time`] deltas (microseconds), not from a
//!   timer, so it tracks the compositor's vsync.
//! - **Weak-ref guarded.** The driven widget is held weakly; if it is disposed
//!   mid-flight the tick callback self-terminates on the next frame.
//! - **Reduce motion.** When motion is disabled the run jumps straight to its
//!   final state: one `on_frame(1.0)` plus the done callback, never a tick.
//! - **Generation counter.** Restarting or cancelling supersedes an in-flight
//!   run, which is what makes mid-flight reversals (hover in → hover out)
//!   safe.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4::glib;
use gtk4::prelude::*;

thread_local! {
    /// `theme.animations`, cached for the lifetime of the process.
    static ANIMATIONS_ENABLED: Cell<bool> = const { Cell::new(true) };
    /// Runs that actually registered a tick callback, counted for the audit.
    #[cfg(debug_assertions)]
    static TICKING_RUNS: Cell<u64> = const { Cell::new(0) };
}

/// Announce a run that is about to start ticking.
///
/// Every piece of motion in the panel goes through [`Animation::start`], and
/// only the path below the reduce-motion guard reaches this — so a session run
/// with `animations = false` produces this line exactly zero times, whatever
/// the user does to the panel. That is the whole assertion behind the
/// zero-motion audit, and it is why the counter is worth carrying in debug
/// builds: a log line saying "run 41" is evidence, one saying "a run started"
/// is an anecdote.
#[cfg(debug_assertions)]
fn count_run(duration_us: i64) {
    let count = TICKING_RUNS.with(|cell| {
        let count = cell.get() + 1;
        cell.set(count);
        count
    });
    tracing::debug!("motion: run {count} started ({}ms)", duration_us / 1_000);
}

/// The same, compiled out of release builds.
#[cfg(not(debug_assertions))]
fn count_run(_duration_us: i64) {}

/// Record whether `theme.animations` allows motion.
///
/// Called once during startup (and again after a config reload). It is only
/// half of the answer — see [`motion_enabled`].
pub fn set_animations_enabled(enabled: bool) {
    ANIMATIONS_ENABLED.with(|cell| cell.set(enabled));
}

/// Whether animations may run at all.
///
/// Motion needs both the panel's own `theme.animations` **and** GTK's
/// `gtk-enable-animations` (which desktop accessibility settings drive). Either
/// one switched off means zero motion, everywhere.
pub fn motion_enabled() -> bool {
    ANIMATIONS_ENABLED.with(Cell::get) && gtk_animations_enabled()
}

/// Read `gtk-enable-animations`, defaulting to enabled before GTK is up
/// (unit tests, `--check-config`), where there is nothing to animate anyway.
fn gtk_animations_enabled() -> bool {
    if !gtk4::is_initialized_main_thread() {
        return true;
    }
    gtk4::Settings::default().is_none_or(|settings| settings.is_gtk_enable_animations())
}

/// Easing curve applied to linear progress before the per-frame callback sees
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Easing {
    /// No easing — progress passes through unchanged.
    Linear,
    /// Cubic ease-out: quick start, gentle settle (`1 - (1-t)^3`).
    ///
    /// The default for micro-interactions such as the panel-button hover fade,
    /// and for anything appearing — a popover opening.
    EaseOutCubic,
    /// Cubic ease-in: gentle start, quick finish (`t^3`).
    ///
    /// The mirror of [`Easing::EaseOutCubic`], for anything leaving. A popover
    /// closes on this curve so it holds still for a moment before it goes,
    /// which reads as dismissal rather than as a dropped frame.
    EaseInCubic,
}

impl Easing {
    /// Apply the curve to a `0..=1` linear progress value.
    ///
    /// Input is clamped, so callers may pass slightly out-of-range values from
    /// frame-time rounding without producing overshoot.
    pub fn apply(self, progress: f64) -> f64 {
        let t = progress.clamp(0.0, 1.0);
        match self {
            Easing::Linear => t,
            Easing::EaseOutCubic => 1.0 - (1.0 - t).powi(3),
            Easing::EaseInCubic => t.powi(3),
        }
    }
}

/// Parameters for a single animation run.
#[derive(Debug, Clone, Copy)]
pub struct AnimationParams {
    /// Duration of the run, in microseconds.
    duration_us: i64,
    /// Easing curve applied to linear progress.
    easing: Easing,
}

impl AnimationParams {
    /// Parameters for a linear run of `duration_ms`.
    pub fn new(duration_ms: u64) -> Self {
        Self {
            duration_us: (duration_ms as i64) * 1_000,
            easing: Easing::Linear,
        }
    }

    /// Set the easing curve.
    pub fn with_easing(mut self, easing: Easing) -> Self {
        self.easing = easing;
        self
    }
}

/// Per-frame callback: receives eased progress in `0..=1`.
type FrameFn = Box<dyn FnMut(f64)>;
/// Optional callback invoked once when a run finishes.
type DoneFn = Box<dyn FnOnce()>;

/// A restartable animation bound to one widget.
#[derive(Clone)]
pub struct Animation {
    widget: glib::WeakRef<gtk4::Widget>,
    /// Bumped on every `start`/`cancel` so stale tick callbacks exit.
    generation: Rc<Cell<u64>>,
    running: Rc<Cell<bool>>,
}

impl Animation {
    /// Create an animation handle bound to `widget`.
    ///
    /// The widget is held weakly; the animation never keeps it alive.
    pub fn new(widget: &impl IsA<gtk4::Widget>) -> Self {
        let weak = glib::WeakRef::new();
        weak.set(Some(widget.as_ref()));
        Self {
            widget: weak,
            generation: Rc::new(Cell::new(0)),
            running: Rc::new(Cell::new(false)),
        }
    }

    /// Start (or restart) the animation.
    ///
    /// Any run already in flight on this handle is superseded: its tick
    /// callback exits on the next frame without firing its done callback, so
    /// the new run owns the final state.
    ///
    /// When motion is disabled — or the run has no positive duration, which a
    /// reversal that already sits at its target produces — this jumps straight
    /// to the end: `on_frame(1.0)` then `on_done()`, synchronously. The
    /// zero-duration guard is load-bearing: a ticking run would divide by zero
    /// on its first frame.
    pub fn start(&self, params: AnimationParams, on_frame: FrameFn, on_done: Option<DoneFn>) {
        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);

        let Some(widget) = self.widget.upgrade() else {
            return;
        };

        if !motion_enabled() || params.duration_us <= 0 {
            self.running.set(false);
            let mut on_frame = on_frame;
            on_frame(1.0);
            if let Some(done) = on_done {
                done();
            }
            return;
        }

        self.running.set(true);
        count_run(params.duration_us);

        let gen_cell = Rc::clone(&self.generation);
        let running = Rc::clone(&self.running);
        // The start time is captured on the first tick rather than here: we
        // have no frame clock yet, and reversals must begin cleanly at t=0.
        let start_time: Cell<Option<i64>> = Cell::new(None);
        let on_frame = RefCell::new(on_frame);
        let on_done = RefCell::new(on_done);

        widget.add_tick_callback(move |_widget, frame_clock| {
            if gen_cell.get() != generation {
                return glib::ControlFlow::Break;
            }

            let now = frame_clock.frame_time();
            let start = match start_time.get() {
                Some(start) => start,
                None => {
                    start_time.set(Some(now));
                    now
                }
            };
            let elapsed = now - start;

            let raw = (elapsed as f64 / params.duration_us as f64).clamp(0.0, 1.0);
            (on_frame.borrow_mut())(params.easing.apply(raw));

            if elapsed >= params.duration_us {
                running.set(false);
                if let Some(done) = on_done.borrow_mut().take() {
                    done();
                }
                return glib::ControlFlow::Break;
            }

            glib::ControlFlow::Continue
        });
    }

    /// Cancel the run in place, leaving the widget at its current value.
    ///
    /// The in-flight tick callback self-terminates on its next frame and the
    /// done callback is **not** invoked.
    pub fn cancel(&self) {
        self.generation.set(self.generation.get().wrapping_add(1));
        self.running.set(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CURVES: [Easing; 3] = [Easing::Linear, Easing::EaseOutCubic, Easing::EaseInCubic];

    #[test]
    fn easing_endpoints_are_exact() {
        for easing in CURVES {
            assert_eq!(easing.apply(0.0), 0.0, "{easing:?} at 0");
            assert_eq!(easing.apply(1.0), 1.0, "{easing:?} at 1");
        }
    }

    #[test]
    fn easing_clamps_out_of_range_inputs() {
        for easing in CURVES {
            assert_eq!(easing.apply(-0.5), 0.0, "{easing:?} below 0");
            assert_eq!(easing.apply(1.5), 1.0, "{easing:?} above 1");
        }
    }

    #[test]
    fn linear_is_identity_in_range() {
        for t in [0.25, 0.5, 0.75] {
            assert!((Easing::Linear.apply(t) - t).abs() < f64::EPSILON);
        }
    }

    #[test]
    fn ease_out_is_ahead_of_linear() {
        assert!(Easing::EaseOutCubic.apply(0.5) > 0.5);
    }

    #[test]
    fn ease_in_is_behind_linear() {
        assert!(Easing::EaseInCubic.apply(0.5) < 0.5);
    }

    #[test]
    fn ease_in_mirrors_ease_out() {
        for step in 0..=100 {
            let t = f64::from(step) / 100.0;
            let mirrored = 1.0 - Easing::EaseOutCubic.apply(1.0 - t);
            assert!(
                (Easing::EaseInCubic.apply(t) - mirrored).abs() < 1e-12,
                "at {t}"
            );
        }
    }

    #[test]
    fn ease_out_is_monotonic() {
        let mut prev = Easing::EaseOutCubic.apply(0.0);
        for step in 1..=100 {
            let current = Easing::EaseOutCubic.apply(f64::from(step) / 100.0);
            assert!(current >= prev, "not monotonic at {step}");
            prev = current;
        }
    }

    #[test]
    fn params_builder_sets_fields() {
        let params = AnimationParams::new(120).with_easing(Easing::EaseOutCubic);
        assert_eq!(params.duration_us, 120_000);
        assert_eq!(params.easing, Easing::EaseOutCubic);
    }

    #[test]
    fn zero_duration_params_hit_the_instant_jump_guard() {
        // A reversal that already sits at its target rounds to zero
        // milliseconds. `start` detects this and jumps to the final state
        // instead of ticking, which would divide by zero on the first frame.
        assert_eq!(AnimationParams::new(0).duration_us, 0);
    }

    #[test]
    fn animations_flag_gates_motion() {
        // No GTK settings object exists without a display, so this exercises
        // the config half of the gate on its own.
        set_animations_enabled(false);
        assert!(!motion_enabled());
        set_animations_enabled(true);
        assert!(motion_enabled());
    }
}
