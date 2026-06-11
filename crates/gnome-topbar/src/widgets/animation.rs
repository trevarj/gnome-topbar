//! Small shared animation framework for widget-local, frame-clock-driven motion.
//!
//! This is a deliberately tiny helper, not a general-purpose engine. It drives
//! fixed-duration animations from a GTK4 frame-clock tick callback, computing an
//! eased progress value in `0..=1` each frame and handing it to a per-frame
//! callback. It exists so the per-widget animations in this crate (workspace
//! pill grow-in, screen-sharing pulse, popover open/close, OSD entrance/exit,
//! and notification-toast enter/dismiss/reposition) can share one lifecycle and
//! easing implementation instead of each re-deriving frame-time math.
//!
//! # Design
//!
//! - **Frame-clock driven.** Progress is computed from
//!   [`gtk4::gdk::FrameClock::frame_time`] deltas (microseconds), not wall
//!   clock, so it stays in sync with the compositor's vsync.
//! - **Weak-ref guarded.** The driven widget is held weakly. If it is disposed
//!   mid-flight the tick callback self-terminates on the next frame, so there
//!   are no leaked callbacks and no use-after-dispose.
//! - **Animations toggle.** When `theme.animations` is disabled, the animation
//!   jumps straight to the final state (one `on_frame(1.0)` call plus the done
//!   callback) instead of ticking. Looping animations simply do not start.
//! - **Looping.** [`AnimationParams::repeat`] makes the animation restart from
//!   `0.0` after each cycle, which the screen-sharing pulse uses.
//!
//! # Usage
//!
//! ```ignore
//! let anim = Animation::new(&widget);
//! anim.start(
//!     AnimationParams::new(225).with_easing(Easing::EaseOutQuintic),
//!     {
//!         let widget = widget.clone();
//!         move |eased| widget.set_opacity(eased)
//!     },
//!     Some(Box::new(|| { /* finished */ })),
//! );
//! ```

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4::glib;
use gtk4::prelude::*;

use crate::services::config_manager::ConfigManager;

/// Easing curve applied to linear progress before it is handed to the
/// per-frame callback.
///
/// Curves match those already hand-rolled elsewhere in the crate so ported
/// animations keep their feel:
/// - [`Easing::Linear`] — workspace indicator width motion.
/// - [`Easing::EaseOutCubic`] — notification-toast slide / fade.
/// - [`Easing::EaseOutQuintic`] — popover open/close and OSD entrance/exit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Easing {
    /// No easing — progress passes through unchanged.
    Linear,
    /// Cubic ease-out: quick start, gentle settle (`1 - (1-t)^3`).
    EaseOutCubic,
    /// Quintic ease-out: snappier start, longer settle (`1 - (1-t)^5`).
    ///
    /// Drives the popover open/close fade (`layer_shell_popover.rs`) and the
    /// OSD entrance/exit (`osd.rs`). Approximates the Material Design
    /// `cubic-bezier(0.2, 0, 0, 1)` curve.
    EaseOutQuintic,
}

impl Easing {
    /// Apply the curve to a `0..=1` linear progress value.
    ///
    /// Input is clamped to `0..=1` so callers can pass slightly out-of-range
    /// values (e.g. from frame-time rounding) without producing overshoot.
    pub(crate) fn apply(self, progress: f64) -> f64 {
        let t = progress.clamp(0.0, 1.0);
        match self {
            Easing::Linear => t,
            Easing::EaseOutCubic => 1.0 - (1.0 - t).powi(3),
            Easing::EaseOutQuintic => 1.0 - (1.0 - t).powi(5),
        }
    }
}

/// Parameters for a single animation run.
#[derive(Debug, Clone, Copy)]
pub(crate) struct AnimationParams {
    /// Duration of one cycle, in microseconds.
    duration_us: i64,
    /// Easing curve applied to linear progress.
    easing: Easing,
    /// When `true`, restart from `0.0` after each cycle (loop forever until
    /// cancelled or the widget is disposed).
    repeat: bool,
}

impl AnimationParams {
    /// Create parameters for a one-shot, linear animation of `duration_ms`.
    pub(crate) fn new(duration_ms: u64) -> Self {
        Self {
            duration_us: (duration_ms as i64) * 1_000,
            easing: Easing::Linear,
            repeat: false,
        }
    }

    /// Set the easing curve.
    pub(crate) fn with_easing(mut self, easing: Easing) -> Self {
        self.easing = easing;
        self
    }

    /// Make the animation loop, restarting from `0.0` after each cycle.
    pub(crate) fn repeating(mut self) -> Self {
        self.repeat = true;
        self
    }
}

/// Per-frame callback: receives eased progress in `0..=1`.
type FrameFn = Box<dyn FnMut(f64)>;
/// Optional callback invoked once when a (non-repeating) animation finishes
/// or is cancelled-to-finish.
type DoneFn = Box<dyn FnOnce()>;

/// Shared, restartable animation state bound to a single widget.
///
/// Holds the driven widget weakly and tracks a generation counter so that
/// restarting (or cancelling) makes any in-flight tick callback self-terminate
/// on its next frame.
#[derive(Clone)]
pub(crate) struct Animation {
    widget: glib::WeakRef<gtk4::Widget>,
    /// Bumped on every `start`/`cancel` so stale tick callbacks exit.
    generation: Rc<Cell<u64>>,
    running: Rc<Cell<bool>>,
}

impl Animation {
    /// Create an animation handle bound to `widget`.
    ///
    /// The widget is held weakly; the animation never keeps it alive.
    pub(crate) fn new(widget: &impl IsA<gtk4::Widget>) -> Self {
        let weak = glib::WeakRef::new();
        weak.set(Some(widget.as_ref()));
        Self {
            widget: weak,
            generation: Rc::new(Cell::new(0)),
            running: Rc::new(Cell::new(false)),
        }
    }

    /// Whether an animation is currently running.
    pub(crate) fn is_running(&self) -> bool {
        self.running.get()
    }

    /// Start (or restart) the animation.
    ///
    /// Any previously running animation on this handle is cancelled first
    /// (its tick callback exits on the next frame via the generation bump).
    ///
    /// `on_frame` is called every frame with eased progress in `0..=1`.
    /// `on_done` is called once when a non-repeating animation reaches `1.0`.
    /// For repeating animations `on_done` is never called (use `cancel`).
    ///
    /// When `theme.animations` is disabled this jumps straight to the final
    /// state: `on_frame(1.0)` then `on_done()` are invoked synchronously for
    /// one-shot animations, and repeating animations do not start at all.
    pub(crate) fn start(
        &self,
        params: AnimationParams,
        on_frame: FrameFn,
        on_done: Option<DoneFn>,
    ) {
        // Supersede any in-flight tick callback.
        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);

        let Some(widget) = self.widget.upgrade() else {
            return;
        };

        // Jump straight to the final state when animations are disabled, or when
        // the run has no positive duration (e.g. a reversal whose start already
        // sits at the target, so the proportional duration rounds to zero). A
        // ticking run with a zero duration would divide by zero on its first
        // frame, so this guard is load-bearing, not just an optimisation.
        if !ConfigManager::global().animations_enabled()
            || (!params.repeat && params.duration_us <= 0)
        {
            self.running.set(false);
            if !params.repeat {
                let mut on_frame = on_frame;
                on_frame(1.0);
                if let Some(done) = on_done {
                    done();
                }
            }
            return;
        }

        self.running.set(true);

        let gen_cell = Rc::clone(&self.generation);
        let running = Rc::clone(&self.running);
        // Start time is captured on the first tick (we don't have a frame clock
        // reference here), so reversals and restarts begin cleanly at t=0.
        let start_time: Cell<Option<i64>> = Cell::new(None);
        let on_frame = RefCell::new(on_frame);
        let on_done = RefCell::new(on_done);

        widget.add_tick_callback(move |_w, frame_clock| {
            // Superseded by a newer start()/cancel() — stop without firing
            // on_done (the new run owns the final state).
            if gen_cell.get() != generation {
                return glib::ControlFlow::Break;
            }

            let now = frame_clock.frame_time();
            let start = match start_time.get() {
                Some(t) => t,
                None => {
                    start_time.set(Some(now));
                    now
                }
            };

            let elapsed = now - start;

            if params.repeat {
                // Loop: fract() of cycles, but keep exact 1.0 unreachable so
                // the curve sweeps the full range each period.
                let cycles = elapsed as f64 / params.duration_us as f64;
                let phase = cycles.fract();
                (on_frame.borrow_mut())(params.easing.apply(phase));
                return glib::ControlFlow::Continue;
            }

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

    /// Cancel the animation in place (leaves the widget at its current value).
    ///
    /// The in-flight tick callback self-terminates on its next frame. The done
    /// callback is **not** invoked.
    pub(crate) fn cancel(&self) {
        self.generation.set(self.generation.get().wrapping_add(1));
        self.running.set(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easing_endpoints_are_exact() {
        for easing in [Easing::Linear, Easing::EaseOutCubic, Easing::EaseOutQuintic] {
            assert_eq!(easing.apply(0.0), 0.0, "{easing:?} at 0");
            assert_eq!(easing.apply(1.0), 1.0, "{easing:?} at 1");
        }
    }

    #[test]
    fn easing_clamps_out_of_range_inputs() {
        for easing in [Easing::Linear, Easing::EaseOutCubic, Easing::EaseOutQuintic] {
            assert_eq!(easing.apply(-0.5), 0.0, "{easing:?} below 0");
            assert_eq!(easing.apply(1.5), 1.0, "{easing:?} above 1");
        }
    }

    #[test]
    fn linear_is_identity_in_range() {
        assert!((Easing::Linear.apply(0.25) - 0.25).abs() < f64::EPSILON);
        assert!((Easing::Linear.apply(0.5) - 0.5).abs() < f64::EPSILON);
        assert!((Easing::Linear.apply(0.75) - 0.75).abs() < f64::EPSILON);
    }

    #[test]
    fn ease_out_curves_are_ahead_of_linear() {
        // Ease-out front-loads progress, so at the midpoint it is past 0.5,
        // and quintic (steeper) is ahead of cubic.
        let cubic = Easing::EaseOutCubic.apply(0.5);
        let quintic = Easing::EaseOutQuintic.apply(0.5);
        assert!(cubic > 0.5);
        assert!(quintic > cubic);
    }

    #[test]
    fn ease_out_quintic_matches_popover_formula() {
        // Mirrors layer_shell_popover.rs: eased = 1 - (1 - t)^5.
        for t in [0.0_f64, 0.1, 0.37, 0.5, 0.83, 1.0] {
            let expected = 1.0 - (1.0 - t).powi(5);
            assert!((Easing::EaseOutQuintic.apply(t) - expected).abs() < 1e-12);
        }
    }

    #[test]
    fn ease_out_curves_are_monotonic() {
        for easing in [Easing::EaseOutCubic, Easing::EaseOutQuintic] {
            let mut prev = easing.apply(0.0);
            for i in 1..=100 {
                let cur = easing.apply(i as f64 / 100.0);
                assert!(cur >= prev, "{easing:?} not monotonic at {i}");
                prev = cur;
            }
        }
    }

    #[test]
    fn params_builder_sets_fields() {
        let p = AnimationParams::new(225)
            .with_easing(Easing::EaseOutQuintic)
            .repeating();
        assert_eq!(p.duration_us, 225_000);
        assert_eq!(p.easing, Easing::EaseOutQuintic);
        assert!(p.repeat);
    }

    #[test]
    fn zero_duration_params_trigger_instant_jump_guard() {
        // A reversal whose start already sits at the target rounds to a
        // zero-millisecond duration. `start` detects this (`duration_us <= 0`)
        // and jumps straight to the final state instead of ticking, which would
        // otherwise divide by zero on the first frame.
        let p = AnimationParams::new(0);
        assert_eq!(p.duration_us, 0);
        assert!(!p.repeat);
    }
}
