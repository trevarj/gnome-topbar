//! The bar: layer-shell windows, their layout, and their lifecycle.

mod context;
mod manager;
mod section_layout;
mod window;

pub use context::BarContext;
pub use manager::{BarManager, SharedConfig};
pub use section_layout::{Section, SectionClip, SectionedBar};
