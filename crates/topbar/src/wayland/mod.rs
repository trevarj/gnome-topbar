//! Wayland protocols the panel speaks directly, past GTK.
//!
//! GTK does not expose xdg-activation (M4) or the background-effect blur
//! protocol (M11), so both are spoken over GTK's *own* Wayland connection,
//! borrowed through `gdk4-wayland`. Opening a second connection would work but
//! would put the panel's surfaces on a display object the compositor does not
//! associate with them.

pub mod activation;
pub mod blur;
