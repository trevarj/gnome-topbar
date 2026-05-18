//! Compositor backend abstraction for workspace, keyboard layout, and focus tracking.
//!
//! This module provides Niri workspace, keyboard layout, and focus tracking.
//!
//! The backend trait feeds workspace, keyboard layout, and focused-output consumers.
//!
//! # Usage
//!
//! Services should use `CompositorManager::global()` to get a shared backend instance,
//! then register callbacks via `register_workspace_callback` and `register_window_callback`.

mod manager;
mod niri;
pub mod types;
pub mod xkb_names;

pub use manager::CompositorManager;
pub use niri::NiriBackend;
pub use types::*; // Includes KeyboardLayoutInfo, KeyboardLayoutCallback
