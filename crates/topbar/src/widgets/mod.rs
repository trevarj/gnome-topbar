//! Panel widgets and the shell they all share.

mod clock;
mod control_panel;
mod keyboard_layout;
mod shell;
mod workspaces;

use std::any::Any;

use gtk4::prelude::*;
use topbar_core::Config;
use tracing::debug;

use crate::bar::BarContext;

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
/// `context` carries the identity of the bar being built — chiefly its
/// connector name, which is how a per-monitor widget knows which monitor it is
/// on — and the service handles. Unknown names are not an error here: config
/// validation has already checked the name against the supported set, so
/// anything unhandled is simply a widget from a later milestone.
pub fn mount(name: &str, config: &Config, context: &BarContext) -> Option<MountedWidget> {
    match name {
        "clock" => {
            let clock = clock::ClockWidget::new(&config.widgets.clock, context);
            Some(MountedWidget::new(clock.root(), clock))
        }
        "workspaces" => {
            let workspaces = workspaces::WorkspacesWidget::new(config, context);
            Some(MountedWidget::new(workspaces.root(), workspaces))
        }
        "keyboard_layout" => {
            let layout = keyboard_layout::KeyboardLayoutWidget::new(
                &config.widgets.keyboard_layout,
                context,
            );
            Some(MountedWidget::new(layout.root(), layout))
        }
        other => {
            debug!("widget `{other}` is not implemented yet; skipping");
            None
        }
    }
}
