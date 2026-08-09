//! What a widget is allowed to know about the bar it is being built into.
//!
//! Widgets are per-monitor, and some of them draw different things on
//! different monitors: the workspaces widget shows only the workspaces
//! belonging to *its* output. The connector name is the identity that survives
//! a hotplug, so it — not the `GdkMonitor` object — is what widgets key on.

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::gdk;
use topbar_core::Config;
use topbar_services::Services;

use crate::fonts::FontRendering;
use crate::surfaces::layer_popover::{self, LayerPopover};

/// The bar a widget is being mounted into.
#[derive(Clone)]
pub struct BarContext {
    /// Connector name of this bar's monitor, e.g. `eDP-1`.
    pub connector: String,
    /// The monitor itself, for geometry and scale.
    ///
    /// The popover host clamps itself to this monitor's geometry, and it has
    /// to be the same object the bar was built against rather than whichever
    /// monitor GDK thinks the pointer is on.
    pub monitor: gdk::Monitor,
    /// Handles to every running service.
    pub services: Services,
    /// This monitor's popover host — see [`BarContext::popovers`].
    host: Host,
}

/// A monitor's popover host, built the first time a widget asks for one.
///
/// A bar carrying no popover widget never creates the two layer surfaces at
/// all, and every widget that does carry one shares the same host — which is
/// how "exactly one popover open at a time" holds without any widget knowing
/// about the others.
#[derive(Clone)]
struct Host {
    top_margin: i32,
    /// The font settings for the popover glyph-clipping workaround, applied on
    /// every open — see [`LayerPopover::open`]. `None` when the feature is off.
    font_rendering: Option<FontRendering>,
    host: Rc<RefCell<Option<Rc<LayerPopover>>>>,
}

impl BarContext {
    /// Describe the bar on `monitor`.
    pub fn new(
        connector: &str,
        monitor: &gdk::Monitor,
        config: &Config,
        services: &Services,
    ) -> Self {
        Self {
            connector: connector.to_string(),
            monitor: monitor.clone(),
            services: services.clone(),
            host: Host {
                top_margin: layer_popover::window_top(config),
                font_rendering: FontRendering::from_config(config),
                host: Rc::new(RefCell::new(None)),
            },
        }
    }

    /// This monitor's popover host, created on first use.
    ///
    /// The returned handle is what keeps the host alive: it lives exactly as
    /// long as the widgets holding it, so a bar rebuild takes the surfaces
    /// down with the widgets that put them up.
    pub fn popovers(&self) -> Rc<LayerPopover> {
        if let Some(host) = self.host.host.borrow().as_ref() {
            return Rc::clone(host);
        }
        let host = LayerPopover::new(
            &self.monitor,
            self.host.top_margin,
            self.host.font_rendering.clone(),
        );
        *self.host.host.borrow_mut() = Some(Rc::clone(&host));
        host
    }
}
