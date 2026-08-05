//! The press ripple.
//!
//! Ported from v1 `widgets/ripple.rs` and cut down to what the v2 palette
//! needs. A press starts a flat-opacity circle at the pointer and expands it
//! until it covers the whole control, fading as it grows — Material's touch
//! feedback, which GNOME Shell borrows for its own panel buttons.
//!
//! Two things it deliberately does not do:
//!
//! - It never decides its own colour. v1 measured the widget's text colour to
//!   guess at dark mode; v2 has one palette, and the ripple takes its tint from
//!   the `.ripple` rule in the generated stylesheet like everything else. That
//!   also keeps it away from the `.state-*` classes on the surfaces it is drawn
//!   inside, which would otherwise tint a press orange on a warning widget.
//! - It never runs when motion is off. `theme.ripple` and
//!   [`motion_enabled`](super::motion_enabled) both have to allow it; either
//!   one switched off means a press is a colour change and nothing more.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{DrawingArea, GestureClick, glib};

use crate::anim::motion_enabled;
use crate::style::classes;

/// How long a ripple takes to cover its control, in milliseconds.
///
/// The plan's ceiling for any motion in the panel; a ripple that outlives the
/// click that started it reads as lag.
const DURATION_MS: f64 = 300.0;

/// Fraction of the run spent at full strength before the fade starts.
///
/// The circle is still growing while it fades, which is what makes the ripple
/// read as a single gesture rather than as a shape that appears and then goes.
const HOLD: f64 = 0.4;

thread_local! {
    /// `theme.ripple`, cached for the lifetime of the process.
    static ENABLED: Cell<bool> = const { Cell::new(true) };
}

/// Record whether `theme.ripple` allows press ripples.
pub fn set_enabled(enabled: bool) {
    ENABLED.with(|cell| cell.set(enabled));
}

/// Whether a press should ripple at all.
///
/// Motion being off is enough on its own: a ripple is motion, and
/// `animations = false` means none of it anywhere.
pub fn enabled() -> bool {
    ENABLED.with(Cell::get) && motion_enabled()
}

/// Eased radius at `progress`, as a fraction of the final radius.
///
/// A hard deceleration — the circle is most of the way out in the first third —
/// so the feedback lands with the press rather than trailing behind it.
fn radius_at(progress: f64) -> f64 {
    let t = progress.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Opacity at `progress`, as a fraction of the ripple's full strength.
///
/// Flat while the circle establishes itself, then out on an ease so the last
/// frames are the faintest rather than a cliff at the end of the run.
fn alpha_at(progress: f64) -> f64 {
    let t = progress.clamp(0.0, 1.0);
    if t <= HOLD {
        return 1.0;
    }
    let fade = (t - HOLD) / (1.0 - HOLD);
    (1.0 - fade).powi(2)
}

/// One ripple in flight.
struct Run {
    /// Centre, in the drawing area's coordinates.
    x: f64,
    y: f64,
    /// Distance from the centre to the farthest corner.
    radius: f64,
    /// Frame-clock time the run started, in microseconds.
    start: i64,
    /// Progress to draw regardless of the clock, for a still frame.
    #[cfg(debug_assertions)]
    frozen: Option<f64>,
}

impl Run {
    /// How far through the ripple is on the frame being drawn.
    fn progress(&self, now: i64) -> f64 {
        #[cfg(debug_assertions)]
        if let Some(frozen) = self.frozen {
            return frozen;
        }
        (now - self.start) as f64 / 1_000.0 / DURATION_MS
    }
}

/// A ripple surface, and the presses it has been asked to draw.
///
/// Cheap to clone: every clone drives the same drawing area.
#[derive(Clone)]
pub struct Ripple {
    area: DrawingArea,
    run: Rc<RefCell<Option<Run>>>,
    /// Bumped per press, so a second press ends the first one's tick callback
    /// instead of leaving two running against one another.
    generation: Rc<Cell<u64>>,
}

impl Default for Ripple {
    fn default() -> Self {
        Self::new()
    }
}

impl Ripple {
    /// Build a ripple surface. Add [`Ripple::area`] to whatever it draws on.
    pub fn new() -> Self {
        let area = DrawingArea::new();
        area.add_css_class(classes::RIPPLE);
        // Presses have to reach the control underneath, and the ripple must not
        // ask for any space of its own.
        area.set_can_target(false);
        area.set_hexpand(true);
        area.set_vexpand(true);

        let run: Rc<RefCell<Option<Run>>> = Rc::new(RefCell::new(None));
        area.set_draw_func({
            let run = Rc::clone(&run);
            move |area, cairo, _width, _height| {
                let run = run.borrow();
                let Some(run) = run.as_ref() else {
                    return;
                };
                let Some(clock) = area.frame_clock() else {
                    return;
                };
                let progress = run.progress(clock.frame_time());
                if progress > 1.0 {
                    return;
                }

                // The tint, alpha and all, comes from the stylesheet; the
                // ripple only decides how much of it is left.
                let colour = area.color();
                let alpha = f64::from(colour.alpha()) * alpha_at(progress);
                if alpha <= 0.001 {
                    return;
                }
                cairo.set_source_rgba(
                    f64::from(colour.red()),
                    f64::from(colour.green()),
                    f64::from(colour.blue()),
                    alpha,
                );
                cairo.arc(
                    run.x,
                    run.y,
                    run.radius * radius_at(progress),
                    0.0,
                    std::f64::consts::TAU,
                );
                // Clipping to the rounded shape is the parent's job: every
                // surface a ripple is drawn on already clips its own children.
                let _ = cairo.fill();
            }
        });

        Self {
            area,
            run,
            generation: Rc::new(Cell::new(0)),
        }
    }

    /// The widget to put on top of whatever is being pressed.
    pub fn area(&self) -> &DrawingArea {
        &self.area
    }

    /// Start a ripple centred on `(x, y)`, in the ripple surface's coordinates.
    ///
    /// The run always finishes, whether or not the button is still held: a
    /// ripple cut short at the moment of release reads as a dropped frame.
    pub fn start(&self, x: f64, y: f64) {
        if !enabled() {
            return;
        }
        // No frame clock means the surface is not on screen, and a run started
        // here would never be ticked and never cleared.
        let Some(start) = self.area.frame_clock().map(|clock| clock.frame_time()) else {
            return;
        };

        let width = f64::from(self.area.width());
        let height = f64::from(self.area.height());
        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);
        *self.run.borrow_mut() = Some(Run {
            x,
            y,
            radius: farthest_corner(x, y, width, height),
            start,
            #[cfg(debug_assertions)]
            frozen: None,
        });

        let run = Rc::clone(&self.run);
        let current = Rc::clone(&self.generation);
        self.area.add_tick_callback(move |area, clock| {
            if current.get() != generation {
                return glib::ControlFlow::Break;
            }
            let elapsed = run.borrow().as_ref().map_or(f64::MAX, |run| {
                (clock.frame_time() - run.start) as f64 / 1_000.0
            });

            area.queue_draw();
            if elapsed < DURATION_MS {
                return glib::ControlFlow::Continue;
            }
            // One last draw with nothing to draw, to clear the final frame.
            *run.borrow_mut() = None;
            area.queue_draw();
            glib::ControlFlow::Break
        });
    }

    /// Paint a ripple frozen part-way through, for a screenshot.
    ///
    /// A ripple is over in 300ms and the smoke helper waits for two identical
    /// frames, so a real one can never be photographed — and there is no
    /// synthetic pointer in the nested session to start one with anyway. This
    /// paints the frame a press would have produced at `progress` and leaves it
    /// there, with no tick callback to move it on. The same trick the power
    /// card's hold fill uses, for the same reason.
    #[cfg(debug_assertions)]
    pub fn paint(&self, progress: f64) {
        let width = f64::from(self.area.width());
        let height = f64::from(self.area.height());
        if width <= 0.0 || height <= 0.0 {
            return;
        }
        // A third of the way in, on the centre line: far enough from the middle
        // that the circle is visibly off-centre, which is the whole point of a
        // ripple starting where the pointer was.
        let (x, y) = (width / 3.0, height / 2.0);
        self.generation.set(self.generation.get().wrapping_add(1));
        *self.run.borrow_mut() = Some(Run {
            x,
            y,
            radius: farthest_corner(x, y, width, height),
            start: 0,
            frozen: Some(progress),
        });
        self.area.queue_draw();
    }

    /// Start a ripple where a gesture was pressed.
    ///
    /// The gesture reports coordinates in its own widget's space, which is
    /// rarely the space the ripple is drawn in.
    pub fn start_from(&self, gesture: &GestureClick, x: f64, y: f64) {
        let Some(widget) = gesture.widget() else {
            return;
        };
        let point = gtk4::graphene::Point::new(x as f32, y as f32);
        // No shared coordinate space means they are not on screen together,
        // and there is nothing to draw the ripple on.
        if let Some(point) = widget.compute_point(&self.area, &point) {
            self.start(f64::from(point.x()), f64::from(point.y()));
        }
    }
}

/// Distance from `(x, y)` to the farthest corner of a `width`×`height` box.
///
/// That is how far the circle has to travel to cover the control from wherever
/// it was pressed, which is what makes a ripple started in a corner grow bigger
/// than one started in the middle.
fn farthest_corner(x: f64, y: f64, width: f64, height: f64) -> f64 {
    let dx = x.max(width - x);
    let dy = y.max(height - y);
    dx.hypot(dy)
}

/// Give `button` a ripple, wrapping its child to hold the drawing area.
///
/// Called after the child is set. Buttons that should not ripple — a text link,
/// a hold-to-confirm row that draws its own fill — simply do not call this.
pub fn install(button: &gtk4::Button) {
    let Some(child) = button.child() else {
        return;
    };

    // Taken off the button before it is given to the wrapper. A widget that
    // still has a parent cannot be given another one: GTK refuses, warns, and
    // leaves the button with nothing in it — which is a pill with no label on
    // it and no clue as to why.
    button.set_child(gtk4::Widget::NONE);

    let overlay = gtk4::Overlay::new();
    overlay.set_child(Some(&child));
    // The wrapper inherits the button's border radius, and clipping to it is
    // what holds the circle inside the button's rounded shape.
    overlay.add_css_class(classes::RIPPLE_CLIP);
    overlay.set_overflow(gtk4::Overflow::Hidden);

    let ripple = Ripple::new();
    overlay.add_overlay(ripple.area());
    button.set_child(Some(&overlay));

    // Capture phase: the ripple belongs to the press, whatever the button's own
    // handlers go on to do with it.
    let gesture = GestureClick::new();
    gesture.set_propagation_phase(gtk4::PropagationPhase::Capture);
    gesture.connect_pressed(move |gesture, _presses, x, y| ripple.start_from(gesture, x, y));
    button.add_controller(gesture);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_circle_starts_at_nothing_and_ends_covering_everything() {
        assert_eq!(radius_at(0.0), 0.0);
        assert_eq!(radius_at(1.0), 1.0);
    }

    #[test]
    fn the_circle_is_most_of_the_way_out_early() {
        // The point of the curve: at a third of the run the ripple already
        // reads as having arrived.
        assert!(radius_at(1.0 / 3.0) > 0.7);
    }

    #[test]
    fn the_circle_only_ever_grows() {
        let mut previous = 0.0;
        for step in 0..=100 {
            let radius = radius_at(f64::from(step) / 100.0);
            assert!(radius >= previous, "shrank at {step}");
            previous = radius;
        }
    }

    #[test]
    fn the_tint_holds_before_it_fades() {
        assert_eq!(alpha_at(0.0), 1.0);
        assert_eq!(alpha_at(HOLD), 1.0);
        assert!(alpha_at(HOLD + 0.01) < 1.0);
    }

    #[test]
    fn the_tint_is_gone_by_the_end() {
        assert_eq!(alpha_at(1.0), 0.0);
        let mut previous = 1.0;
        for step in 40..=100 {
            let alpha = alpha_at(f64::from(step) / 100.0);
            assert!(alpha <= previous, "brightened at {step}");
            previous = alpha;
        }
    }

    #[test]
    fn both_curves_ignore_progress_outside_the_run() {
        assert_eq!(radius_at(-1.0), 0.0);
        assert_eq!(radius_at(2.0), 1.0);
        assert_eq!(alpha_at(-1.0), 1.0);
        assert_eq!(alpha_at(2.0), 0.0);
    }

    #[test]
    fn a_press_in_the_middle_reaches_every_corner() {
        // 40x20 pressed dead centre: half the diagonal.
        let radius = farthest_corner(20.0, 10.0, 40.0, 20.0);
        assert!((radius - 500.0_f64.sqrt()).abs() < 1e-9);
    }

    #[test]
    fn a_press_in_a_corner_has_to_cross_the_whole_control() {
        let radius = farthest_corner(0.0, 0.0, 30.0, 40.0);
        assert!((radius - 50.0).abs() < 1e-9, "3-4-5 triangle");

        // Wherever the press lands, the finished circle reaches all four
        // corners — which is what "the ripple covers the control" means.
        let (width, height) = (30.0_f64, 40.0_f64);
        for (x, y) in [
            (0.0, 0.0),
            (30.0, 0.0),
            (0.0, 40.0),
            (30.0, 40.0),
            (7.0, 9.0),
        ] {
            let radius = farthest_corner(x, y, width, height);
            for (cx, cy) in [(0.0, 0.0), (width, 0.0), (0.0, height), (width, height)] {
                let corner: f64 = (cx - x).hypot(cy - y);
                assert!(corner <= radius + 1e-9, "({x},{y}) misses ({cx},{cy})");
            }
        }
    }

    #[test]
    fn the_ripple_config_flag_gates_it_on_its_own() {
        set_enabled(false);
        assert!(!enabled());
        set_enabled(true);
        assert!(enabled(), "motion is enabled by default without a display");
    }

    #[test]
    fn switching_motion_off_switches_the_ripple_off_too() {
        super::super::set_animations_enabled(false);
        set_enabled(true);
        assert!(!enabled(), "a ripple is motion like any other");
        super::super::set_animations_enabled(true);
    }
}
