//! Hold to confirm.
//!
//! Shutting a machine down is the one thing in the panel that cannot be
//! undone, so it is the one thing that is not done on a click. The row fills
//! with accent under the pointer and fires when the fill reaches the end;
//! letting go before that cancels it, and nothing happens.
//!
//! The delay is the confirmation, which has one consequence worth stating:
//! **turning animations off does not turn the delay off**. With motion
//! disabled there is no fill to watch, so the row wears a static confirming
//! tint for the same [`HOLD_MS`] and then fires. A reduced-motion setting is a
//! statement about movement, not about how easily a laptop should power off.
//!
//! The state machine below has no GTK in it, which is why press, release,
//! key-repeat, timing and the reduced-motion path are all tested rather than
//! demonstrated.

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{gdk, glib};

use crate::anim::motion_enabled;
use crate::style::classes;

/// How long a row has to be held before it acts.
///
/// Long enough that a stray click cannot shut the machine down, short enough
/// that a deliberate press does not feel like it was ignored. v1 converged on
/// 800ms and it read as sluggish; GNOME's own press-and-hold affordances sit
/// around two thirds of a second.
pub const HOLD_MS: u64 = 650;

/// What the row should be showing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Indication {
    /// Fill the row to this fraction of its width.
    Fill(f64),
    /// Motion is off: tint the whole row instead, for the same duration.
    Static,
}

/// The press/release/timeout machine, with no widget in it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hold {
    duration_ms: u64,
    /// How long the current hold has run, or `None` when nothing is held.
    held_ms: Option<u64>,
    /// Whether the fill is drawn, or a static tint stands in for it.
    motion: bool,
}

impl Hold {
    /// A row that is not being held.
    pub const fn new(duration_ms: u64, motion: bool) -> Self {
        Self {
            duration_ms,
            held_ms: None,
            motion,
        }
    }

    /// Start holding. `false` when a hold was already in progress.
    ///
    /// Key repeat is why this answers rather than restarting: X and Wayland
    /// both send a stream of presses while Enter is down, and a machine that
    /// restarted the countdown on each of them would never reach the end.
    pub fn press(&mut self) -> bool {
        if self.held_ms.is_some() {
            return false;
        }
        self.held_ms = Some(0);
        true
    }

    /// Let go. `true` when that cancelled a hold in progress.
    pub fn release(&mut self) -> bool {
        self.held_ms.take().is_some()
    }

    /// Advance the hold by `elapsed_ms`, saying whether it has fired.
    ///
    /// Firing clears the hold, so a row cannot act twice on one press however
    /// many frames arrive afterwards.
    pub fn advance(&mut self, elapsed_ms: u64) -> bool {
        let Some(held) = self.held_ms else {
            return false;
        };
        let held = held.saturating_add(elapsed_ms);
        if held >= self.duration_ms {
            self.held_ms = None;
            return true;
        }
        self.held_ms = Some(held);
        false
    }

    /// Whether a hold is in progress.
    pub fn is_holding(self) -> bool {
        self.held_ms.is_some()
    }

    /// How far through the hold is, `0.0..=1.0`.
    pub fn progress(self) -> f64 {
        match self.held_ms {
            Some(held) if self.duration_ms > 0 => {
                (held as f64 / self.duration_ms as f64).clamp(0.0, 1.0)
            }
            Some(_) => 1.0,
            None => 0.0,
        }
    }

    /// What the row should draw right now.
    pub fn indication(self) -> Option<Indication> {
        if !self.is_holding() {
            return None;
        }
        if self.motion {
            Some(Indication::Fill(self.progress()))
        } else {
            Some(Indication::Static)
        }
    }
}

/// Whether a key press should start a hold.
///
/// Enter and space, the two keys GTK treats as "activate". A row that could be
/// shut down with the space bar and not with Enter would be a bug report.
pub fn is_confirm_key(key: gdk::Key) -> bool {
    matches!(key, gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::space)
}

/// A row wired for hold-to-confirm.
///
/// Keeps the machine, the frame-clock subscription and the widgets it paints,
/// so dropping it stops everything.
pub struct HoldRow {
    state: RefCell<Hold>,
    /// The box whose width is the fill.
    fill: gtk4::Box,
    /// The row the fill is measured against and tinted.
    row: gtk4::Widget,
    /// The running frame-clock callback, if a hold is in progress.
    tick: RefCell<Option<gtk4::TickCallbackId>>,
    /// The reduced-motion timer, which stands in for it when motion is off.
    ///
    /// Only ever one of the two is occupied; they are separate cells because
    /// they are separate types and cancelling has to reach either.
    timeout: RefCell<Option<glib::SourceId>>,
    /// Frame time the current hold started at, in microseconds.
    started_us: std::cell::Cell<i64>,
    /// What to do when a hold completes.
    confirmed: Box<dyn Fn()>,
}

impl HoldRow {
    /// Wire `row` so holding it runs `confirmed`.
    ///
    /// `fill` is drawn behind the row's content and grows to the row's width
    /// over [`HOLD_MS`]; it is the caller's job to have put it there.
    pub fn attach(
        row: &impl IsA<gtk4::Widget>,
        fill: &gtk4::Box,
        confirmed: impl Fn() + 'static,
    ) -> Rc<Self> {
        let held = Rc::new(Self {
            state: RefCell::new(Hold::new(HOLD_MS, motion_enabled())),
            fill: fill.clone(),
            row: row.as_ref().clone(),
            tick: RefCell::new(None),
            timeout: RefCell::new(None),
            started_us: std::cell::Cell::new(0),
            confirmed: Box::new(confirmed),
        });

        let click = gtk4::GestureClick::new();
        click.set_button(gdk::BUTTON_PRIMARY);
        click.connect_pressed({
            let held = Rc::downgrade(&held);
            move |gesture, _, _, _| {
                if let Some(held) = held.upgrade() {
                    held.begin();
                    gesture.set_state(gtk4::EventSequenceState::Claimed);
                }
            }
        });
        click.connect_released({
            let held = Rc::downgrade(&held);
            move |_, _, _, _| {
                if let Some(held) = held.upgrade() {
                    held.cancel();
                }
            }
        });
        click.connect_cancel({
            let held = Rc::downgrade(&held);
            move |_, _| {
                if let Some(held) = held.upgrade() {
                    held.cancel();
                }
            }
        });
        row.as_ref().add_controller(click);

        let keys = gtk4::EventControllerKey::new();
        keys.connect_key_pressed({
            let held = Rc::downgrade(&held);
            move |_, key, _, _| {
                if !is_confirm_key(key) {
                    return glib::Propagation::Proceed;
                }
                if let Some(held) = held.upgrade() {
                    held.begin();
                }
                glib::Propagation::Stop
            }
        });
        keys.connect_key_released({
            let held = Rc::downgrade(&held);
            move |_, key, _, _| {
                if is_confirm_key(key)
                    && let Some(held) = held.upgrade()
                {
                    held.cancel();
                }
            }
        });
        row.as_ref().add_controller(keys);

        // A pointer that leaves mid-hold is a hold that was abandoned.
        let motion = gtk4::EventControllerMotion::new();
        motion.connect_leave({
            let held = Rc::downgrade(&held);
            move |_| {
                if let Some(held) = held.upgrade() {
                    held.cancel();
                }
            }
        });
        row.as_ref().add_controller(motion);

        held
    }

    /// The machine's current state, for the smoke hook.
    #[cfg(debug_assertions)]
    pub fn state(&self) -> Hold {
        *self.state.borrow()
    }

    /// Start a hold, unless one is already running.
    pub fn begin(self: &Rc<Self>) {
        // The motion setting is read per hold rather than once at build time,
        // so a configuration reload takes effect on the next press.
        let motion = motion_enabled();
        {
            let mut state = self.state.borrow_mut();
            *state = Hold::new(HOLD_MS, motion);
            if !state.press() {
                return;
            }
        }

        self.started_us.set(0);
        self.draw();

        if !motion {
            // The static tint is the whole indication when there is no fill.
            // With motion on the fill says it by itself, and a tinted row
            // underneath it would be two answers to one question.
            self.row.add_css_class(classes::CONFIRMING);
            // No fill, but the same wait: the delay is the confirmation.
            let held = Rc::downgrade(self);
            let source = glib::timeout_add_local_once(
                std::time::Duration::from_millis(HOLD_MS),
                move || {
                    if let Some(held) = held.upgrade() {
                        held.finish();
                    }
                },
            );
            *self.timeout.borrow_mut() = Some(source);
            return;
        }

        let held = Rc::downgrade(self);
        let tick = self.row.add_tick_callback(move |_, clock| {
            let Some(held) = held.upgrade() else {
                return glib::ControlFlow::Break;
            };
            held.frame(clock.frame_time())
        });
        *self.tick.borrow_mut() = Some(tick);
    }

    /// Cancel a hold in progress, putting the row back.
    pub fn cancel(&self) {
        if !self.state.borrow_mut().release() {
            return;
        }
        self.stop();
        self.row.remove_css_class(classes::CONFIRMING);
        self.fill.set_size_request(0, -1);
    }

    /// One frame of the fill.
    fn frame(self: &Rc<Self>, now_us: i64) -> glib::ControlFlow {
        if self.started_us.get() == 0 {
            self.started_us.set(now_us);
        }
        let elapsed_ms = (now_us.saturating_sub(self.started_us.get()) / 1000).max(0) as u64;

        let fired = {
            let mut state = self.state.borrow_mut();
            if !state.is_holding() {
                return glib::ControlFlow::Break;
            }
            // Absolute rather than incremental: a frame the compositor skipped
            // must not make the hold take longer than it looks like it will.
            *state = Hold::new(HOLD_MS, true);
            state.press();
            state.advance(elapsed_ms)
        };

        if fired {
            self.finish();
            return glib::ControlFlow::Break;
        }
        self.draw();
        glib::ControlFlow::Continue
    }

    /// Paint the current state.
    fn draw(&self) {
        let Some(indication) = self.state.borrow().indication() else {
            self.fill.set_size_request(0, -1);
            return;
        };
        match indication {
            Indication::Fill(progress) => {
                let width = (f64::from(self.row.width()) * progress).round() as i32;
                self.fill.set_size_request(width, -1);
            }
            // The static tint is the `.confirming` class on the row; the fill
            // box stays out of it entirely.
            Indication::Static => self.fill.set_size_request(0, -1),
        }
    }

    /// The hold completed: put the row back and do the thing.
    fn finish(&self) {
        self.stop();
        *self.state.borrow_mut() = Hold::new(HOLD_MS, motion_enabled());
        self.row.remove_css_class(classes::CONFIRMING);
        self.fill.set_size_request(0, -1);
        (self.confirmed)();
    }

    /// Drop whichever timer is running.
    fn stop(&self) {
        if let Some(tick) = self.tick.borrow_mut().take() {
            tick.remove();
        }
        if let Some(source) = self.timeout.borrow_mut().take() {
            source.remove();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hold_that_runs_its_course_fires_exactly_once() {
        let mut hold = Hold::new(650, true);
        assert!(hold.press());
        assert!(!hold.advance(300), "not yet");
        assert!(hold.advance(350), "650ms is 650ms");
        assert!(!hold.is_holding(), "the hold is spent");
        assert!(!hold.advance(1000), "and cannot fire again");
    }

    #[test]
    fn letting_go_early_cancels_and_nothing_happens() {
        let mut hold = Hold::new(650, true);
        hold.press();
        hold.advance(400);
        assert!(hold.release(), "a hold was cancelled");
        assert!(!hold.is_holding());
        assert_eq!(hold.progress(), 0.0);
        assert!(!hold.advance(10_000), "a released row never fires");
    }

    #[test]
    fn releasing_when_nothing_is_held_is_not_a_cancellation() {
        let mut hold = Hold::new(650, true);
        assert!(!hold.release());
    }

    #[test]
    fn key_repeat_does_not_restart_the_countdown() {
        let mut hold = Hold::new(650, true);
        assert!(hold.press());
        hold.advance(600);
        // Wayland sends a press every 30ms or so while the key is down.
        assert!(!hold.press(), "a repeat is not a new hold");
        assert!(hold.advance(50), "and the original countdown finished");
    }

    #[test]
    fn progress_tracks_the_hold_and_stays_in_range() {
        let mut hold = Hold::new(1000, true);
        assert_eq!(hold.progress(), 0.0);
        hold.press();
        hold.advance(250);
        assert!((hold.progress() - 0.25).abs() < f64::EPSILON);
        hold.advance(250);
        assert!((hold.progress() - 0.5).abs() < f64::EPSILON);
        hold.advance(10_000);
        assert_eq!(hold.progress(), 0.0, "firing clears it");
    }

    #[test]
    fn with_motion_the_row_fills() {
        let mut hold = Hold::new(1000, true);
        assert_eq!(hold.indication(), None, "an idle row shows nothing");
        hold.press();
        hold.advance(400);
        assert_eq!(hold.indication(), Some(Indication::Fill(0.4)));
    }

    #[test]
    fn without_motion_the_delay_stays_and_the_fill_does_not() {
        let mut hold = Hold::new(1000, false);
        hold.press();
        hold.advance(400);
        assert_eq!(
            hold.indication(),
            Some(Indication::Static),
            "reduced motion means no fill, not no confirmation"
        );
        assert!(!hold.advance(400), "and the wait is the same wait");
        assert!(hold.advance(200), "650…1000ms later, it fires");
    }

    #[test]
    fn a_zero_length_hold_still_needs_a_press() {
        let mut hold = Hold::new(0, true);
        assert!(!hold.advance(1), "nothing is held");
        hold.press();
        assert!(hold.advance(0), "and then it fires at once");
    }

    #[test]
    fn only_enter_and_space_confirm() {
        assert!(is_confirm_key(gdk::Key::Return));
        assert!(is_confirm_key(gdk::Key::KP_Enter));
        assert!(is_confirm_key(gdk::Key::space));
        assert!(!is_confirm_key(gdk::Key::Escape));
        assert!(!is_confirm_key(gdk::Key::a));
        assert!(!is_confirm_key(gdk::Key::Tab));
    }

    #[test]
    fn the_hold_is_long_enough_to_be_deliberate() {
        const {
            assert!(HOLD_MS >= 500, "shorter than this is an accident waiting");
            assert!(HOLD_MS <= 800, "longer than this reads as unresponsive");
        }
    }
}
