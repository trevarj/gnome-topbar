//! The blocks that make up the Quick Settings panel.
//!
//! One module per block, each owning its own subscriptions and its own inline
//! error slots, so the panel itself is a layout and nothing more. M9b adds
//! network and VPN here; M9c adds bluetooth, updates, resources and privacy.

pub mod battery;
pub mod header;
pub mod network;
pub mod power;
pub mod sliders;
pub mod toggles;
