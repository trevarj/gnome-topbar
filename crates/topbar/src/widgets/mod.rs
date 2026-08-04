//! Panel widgets and the shell they all share.

mod clock;
mod shell;

use std::any::Any;

use gtk4::prelude::*;
use topbar_core::Config;
use tracing::debug;

/// A widget that has been built and put in a bar section.
pub struct MountedWidget {
    /// The widget to append to a section.
    pub root: gtk4::Widget,
    /// Whatever must outlive this call — timers, subscriptions, the widget
    /// struct itself. Dropped when the bar is torn down.
    _keepalive: Box<dyn Any>,
}

impl MountedWidget {
    /// Pair a widget with the state that keeps it running.
    pub fn new(root: impl IsA<gtk4::Widget>, keepalive: impl Any) -> Self {
        Self {
            root: root.upcast(),
            _keepalive: Box::new(keepalive),
        }
    }
}

/// Build the widget named `name`, or `None` if there is nothing to build.
///
/// Unknown names are not an error here: config validation has already checked
/// the name against the supported set, so anything unhandled is simply a
/// widget from a later milestone.
pub fn mount(name: &str, config: &Config) -> Option<MountedWidget> {
    match name {
        "clock" => {
            let clock = clock::ClockWidget::new(&config.widgets.clock);
            Some(MountedWidget::new(clock.root(), clock))
        }
        other => {
            debug!("widget `{other}` is not implemented yet; skipping");
            None
        }
    }
}
