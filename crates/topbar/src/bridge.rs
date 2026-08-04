//! The one crossing between service state and widgets.
//!
//! Services run on tokio worker threads; widgets exist only on the GTK main
//! thread. Exactly two functions cross that line, and every widget uses them:
//!
//! - [`bind_state`] — subscribe a widget to a snapshot channel. The render
//!   closure runs on the main context, fires once immediately so a widget is
//!   never blank on its first frame, and stops when the widget goes away.
//! - [`act`] — run a mutating service call. It is the single place a
//!   `Result<(), SvcError>` may be discarded, which is what stops failures
//!   from disappearing into a click handler.
//!
//! Both are deliberately narrow. There is no way to get a widget reference
//! into a service, and no way to block the main thread on one.

use std::future::Future;
use std::sync::Arc;

use gtk4::glib;
use gtk4::prelude::*;
use topbar_services::{Runtime, SvcError, watch};
use tracing::warn;

/// Keeps a [`bind_state`] subscription alive.
///
/// Dropping it aborts the subscription, so a widget's bindings die with the
/// widget rather than rendering into a disposed tree.
pub struct BindingGuard {
    handle: Option<glib::JoinHandle<()>>,
}

impl Drop for BindingGuard {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}

/// Render `widget` from `receiver`, now and on every change.
///
/// The widget is held weakly inside the subscription: the guard usually lives
/// in the widget's own keep-alive box, and a strong reference there would be a
/// cycle that never collects. If the widget is disposed before the guard is
/// dropped, the next change ends the subscription instead of touching it.
pub fn bind_state<W, S, F>(
    widget: &W,
    mut receiver: watch::Receiver<Arc<S>>,
    render: F,
) -> BindingGuard
where
    W: IsA<gtk4::Widget>,
    S: Send + Sync + 'static,
    F: Fn(&W, &S) + 'static,
{
    // Render before the first await so the widget has content on the frame it
    // is first drawn, not one main-loop turn later.
    render(widget, &receiver.borrow_and_update().clone());

    let weak = widget.downgrade();
    let handle = glib::spawn_future_local(async move {
        while receiver.changed().await.is_ok() {
            let Some(widget) = weak.upgrade() else {
                break;
            };
            let state = receiver.borrow_and_update().clone();
            render(&widget, &state);
        }
    });

    BindingGuard {
        handle: Some(handle),
    }
}

/// Where a failed action is reported.
#[derive(Debug, Clone, Copy)]
pub enum ActionScope {
    /// Announce the failure in a toast. The default for panel-button actions.
    Toast {
        /// Widget that started the action, for the log line.
        widget: &'static str,
    },
    /// Announce it inside the row that started it — Quick Settings (M9), where
    /// a toast would be redundant with the control that just reverted.
    #[allow(dead_code)]
    Inline {
        /// Widget that started the action, for the log line.
        widget: &'static str,
    },
}

impl ActionScope {
    fn widget(self) -> &'static str {
        match self {
            Self::Toast { widget } | Self::Inline { widget } => widget,
        }
    }
}

/// Run a mutating service call, reporting failure rather than dropping it.
///
/// The future runs on the service runtime; the report hops back to the main
/// thread, because that is where anything the user can see lives.
pub fn act<F>(scope: ActionScope, future: F)
where
    F: Future<Output = Result<(), SvcError>> + Send + 'static,
{
    Runtime::handle().spawn(async move {
        if let Err(error) = future.await {
            glib::idle_add_once(move || report(scope, &error));
        }
    });
}

/// Surface a failed action. Runs on the main thread.
fn report(scope: ActionScope, error: &SvcError) {
    // TODO(M4): render `ActionScope::Toast` as a toast and `Inline` in the row
    // that started the action. Until the toast surface exists the log is the
    // only place a failure can go — but it goes through here either way, so
    // wiring it up is a change to this function alone.
    warn!("{}: {} ({error})", scope.widget(), error.user_message());
}
