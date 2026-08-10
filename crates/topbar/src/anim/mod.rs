//! Frame-clock driven motion.
//!
//! Every animation in the panel runs through [`Animation`]: a fixed-duration
//! run driven by the widget's GTK frame clock, so motion stays in sync with
//! the compositor's vsync instead of a wall-clock timer. CSS `transition`
//! rules are deliberately not used anywhere in the generated stylesheet — GTK4
//! transitions on containers with nested children leak memory, and a single
//! Rust-side implementation is easier to reason about (and to switch off).
//!
//! # Every piece of motion in the panel
//!
//! The whole inventory, so a new animation has something to be consistent with
//! and a changed duration has something to be checked against. Every constant
//! named here is the only place its number appears.
//!
//! | Element | Duration | Easing | Reversible mid-flight |
//! |---|---|---|---|
//! | Widget hover in / out | `shell::FADE_IN_MS` 120 / `FADE_OUT_MS` 200 | ease-out | yes, from the live opacity |
//! | Widget press ripple | `ripple::DURATION_MS` 300 | hard ease-out, fades from 40% | no — a press always finishes |
//! | Popover open / close | `layer_popover::OPEN_MS` 200 / `CLOSE_MS` 150 | ease-out / ease-in | yes, `Motion` pays only for the distance left |
//! | Banner arrive / leave | `toast::SLIDE_MS` 150 | ease-out / ease-in | yes, from the live reveal |
//! | OSD in / out | `osd::ENTER_MS` 150 / `LEAVE_MS` 200 | ease-out / ease-in | yes, from the live opacity |
//! | OSD fill retarget | `osd_bar::RETARGET_MS` 100 | ease-out | yes; the first raise jumps instead |
//! | Workspace pill transfer | `strip::TRANSFER_MS` 200 | ease-out | yes, from the interpolated layout |
//! | Workspace slot appear | `strip::APPEAR_MS` 150 | ease-out | retargetable |
//! | Workspace urgent pulse | `strip::PULSE_MS` 500 × `PULSE_CYCLES` 2 | cosine | restart only; **ends steady** |
//! | Tray attention pulse | `tray::PULSE_MS` 1200, 2 cycles | cosine | restart only; **ends steady** |
//! | Screen-share dot | `button::PULSE_MS` 2000 per breath | cosine | — |
//! | System monitor fade-in | `system_monitor::FADE_MS` 150 | ease-out | hiding is instant |
//! | Section reveal | `expander::REVEAL_MS` 200 | ease-out / ease-in | yes, from the live reveal |
//! | QS row reveal | `expander::ROW_REVEAL_MS` 150 | ease-out / ease-in | yes |
//! | Chevron turn | `expander::REVEAL_MS` 200, `CHEVRON_TURN` 180° | ease-out / ease-in | yes, from the live angle |
//! | Crypto view crossfade | `crypto::popover::SWITCH_MS` 150 | linear | superseded-safe |
//! | Media card arrive / leave | `media::CARD_REVEAL_MS` 200 | ease-out / ease-in | yes, from the live reveal |
//! | Media art crossfade | `media::ART_FADE_MS` 150 | linear | restart only |
//! | Media track text swap | `media::TEXT_FADE_MS` 150 | linear | restart only |
//! | Media play/pause glyph | `media::ICON_FADE_MS` 120 | ease-out | restart only |
//! | Keyboard layout switch | `keyboard_layout::SWITCH_FADE_MS` 150 | ease-out | restart only |
//! | Hold to confirm | `hold::HOLD_MS` 650 | linear fill | cancelled by releasing |
//!
//! Two rules the table has to keep obeying. Nothing runs longer than 300ms;
//! and the screen-share dot is the *only* unbounded loop in the panel, which
//! is why it is the only row above with no end state — it stops when the dot
//! is hidden, and it never starts when motion is off.
//!
//! # `animations = false`
//!
//! [`motion_enabled`] answers to `theme.animations` **and** GTK's
//! `gtk-enable-animations`, and [`Animation::start`] jumps a run that cannot
//! move straight to its final state: one `on_frame(1.0)`, the done callback,
//! and no tick callback at all. Everything above therefore has an
//! instant-and-correct end state rather than a disabled one — a banner still
//! leaves, a section still becomes invisible, a chevron still points the right
//! way. Three places check the flag themselves before starting, because "jump
//! to the end" is the wrong answer for a pulse: the workspace and tray pulses
//! pin their tint on, and the screen-share dot simply never breathes.
//!
//! One thing deliberately keeps its delay: hold-to-confirm still takes its full
//! 650ms with motion off, with a static fill instead of a growing one. The
//! delay is there to make an accidental press hard, and that reason has nothing
//! to do with animation.

mod animator;
pub mod ripple;
mod rotate_box;
mod scale_box;
mod slide_box;
pub mod watchdog;

pub use animator::{Animation, AnimationParams, Easing, motion_enabled, set_animations_enabled};
pub use ripple::Ripple;
pub use rotate_box::RotateBox;
pub use scale_box::ScaleBox;
pub use slide_box::SlideBox;
