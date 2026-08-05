//! The blocks that make up the Quick Settings panel.
//!
//! One module per block, each owning its own subscriptions and its own inline
//! error slots, so the panel itself is a layout and nothing more.

pub mod battery;
pub mod bluetooth;
pub mod header;
pub mod network;
pub mod power;
pub mod resources;
pub mod sliders;
pub mod toggles;
pub mod updates;
