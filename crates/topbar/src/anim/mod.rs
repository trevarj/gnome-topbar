//! Frame-clock driven motion.
//!
//! Every animation in the panel runs through [`Animation`]: a fixed-duration
//! run driven by the widget's GTK frame clock, so motion stays in sync with
//! the compositor's vsync instead of a wall-clock timer. CSS `transition`
//! rules are deliberately not used anywhere in the generated stylesheet — GTK4
//! transitions on containers with nested children leak memory, and a single
//! Rust-side implementation is easier to reason about (and to switch off).

mod animator;
mod scale_box;
pub mod watchdog;

pub use animator::{Animation, AnimationParams, Easing, motion_enabled, set_animations_enabled};
pub use scale_box::ScaleBox;
