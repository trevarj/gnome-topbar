//! Compositor backend abstraction for workspace, keyboard layout, and focus tracking.
//!
//! This module provides a pluggable backend system for different Wayland compositors:
//! - MangoWC / DWL (via `mmsg` CLI tool)
//! - Niri (via socket IPC with JSON protocol)
//! - Hyprland (via socket IPC with JSON protocol)
//! - Sway / Miracle WM / Scroll (via i3 IPC binary protocol over Unix socket)
//!
//! The backend trait feeds workspace, keyboard layout, and focused-output consumers.
//!
//! # Usage
//!
//! Services should use `CompositorManager::global()` to get a shared backend instance,
//! then register callbacks via `register_workspace_callback` and `register_window_callback`.

pub mod dwl_ipc;
mod factory;
mod hyprland;
mod manager;
mod mango;
mod niri;
mod sway;
pub mod types;
pub mod xkb_names;

pub use factory::BackendKind;
pub use hyprland::HyprlandBackend;
pub use manager::CompositorManager;
pub use mango::MangoBackend;
pub use niri::NiriBackend;
pub use sway::SwayBackend;
pub use types::*; // Includes KeyboardLayoutInfo, KeyboardLayoutCallback
