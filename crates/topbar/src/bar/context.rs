//! What a widget is allowed to know about the bar it is being built into.
//!
//! Widgets are per-monitor, and some of them draw different things on
//! different monitors: the workspaces widget shows only the workspaces
//! belonging to *its* output. The connector name is the identity that survives
//! a hotplug, so it — not the `GdkMonitor` object — is what widgets key on.

use gtk4::gdk;
use topbar_services::Services;

/// The bar a widget is being mounted into.
#[derive(Clone)]
pub struct BarContext {
    /// Connector name of this bar's monitor, e.g. `eDP-1`.
    pub connector: String,
    /// The monitor itself, for geometry and scale.
    ///
    /// Nothing reads it until M3: the popover host clamps itself to the
    /// monitor's work area, and it needs the same object the bar was built
    /// against rather than whichever monitor GDK thinks the pointer is on.
    #[allow(dead_code)]
    pub monitor: gdk::Monitor,
    /// Handles to every running service.
    pub services: Services,
}
