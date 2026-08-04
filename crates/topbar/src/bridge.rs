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

use std::cell::RefCell;
use std::future::Future;
use std::sync::Arc;

use gtk4::glib;
use gtk4::prelude::*;
use topbar_services::{NotificationsHandle, Runtime, SvcError, watch};
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

/// Run a service call that answers with something, and use the answer.
///
/// The sibling of [`act`], for the calls a widget cannot simply fire and
/// forget: the tray has to *have* a menu before it can draw one. The future
/// runs on the service runtime and `then` runs on the main thread with what it
/// produced; a failure takes the same single reporting path a failed [`act`]
/// does, so a menu that never arrives says so rather than opening blank.
pub fn request<T, F>(scope: ActionScope, future: F, then: impl FnOnce(T) + 'static)
where
    T: Send + 'static,
    F: Future<Output = Result<T, SvcError>> + Send + 'static,
{
    let answer = Runtime::handle().spawn(future);
    glib::spawn_future_local(async move {
        match answer.await {
            Ok(Ok(value)) => then(value),
            Ok(Err(error)) => report(scope, &error),
            // The runtime dropped the task, which only happens on shutdown.
            Err(_) => report(scope, &SvcError::ServiceStopped(scope.widget())),
        }
    });
}

thread_local! {
    /// The daemon [`report`] raises its own banners through.
    ///
    /// A thread-local rather than a parameter because `report` is reached from
    /// every click handler in the panel, and threading a handle through all of
    /// them would be a worse cost than one cell set once at start-up. It is
    /// only ever touched from the main thread.
    static REPORTER: RefCell<Option<NotificationsHandle>> = const { RefCell::new(None) };
}

/// Give [`report`] somewhere to put its banners. Called once, from start-up.
pub fn install_reporter(handle: NotificationsHandle) {
    REPORTER.with_borrow_mut(|reporter| *reporter = Some(handle));
}

/// The notification daemon, for the widgets that talk to it directly.
pub fn notifications() -> Option<NotificationsHandle> {
    REPORTER.with_borrow(Clone::clone)
}

/// Surface a failed action. Runs on the main thread.
///
/// Still one function, and still the only one: a failure is logged with its
/// full detail and shown to the user as the one short sentence
/// [`SvcError::user_message`] carries. The banner goes through the very same
/// daemon an application's would, so it queues, stacks, and expires like
/// anything else — it is simply marked internal, which keeps it out of the
/// history and past Do Not Disturb.
fn report(scope: ActionScope, error: &SvcError) {
    warn!("{}: {} ({error})", scope.widget(), error.user_message());

    // Inline reporting lands in M9 with the Quick Settings rows that need it;
    // until then the log line above is what an inline failure gets, because a
    // toast would be redundant with the control the user is looking at.
    let ActionScope::Toast { .. } = scope else {
        return;
    };
    let Some(handle) = notifications() else {
        return;
    };

    let summary = error.user_message().to_string();
    let detail = error.to_string();
    // Deliberately not through `act`: a failure to report a failure must not
    // try to report itself.
    Runtime::handle().spawn(async move {
        if let Err(error) = handle.report(summary, detail).await {
            warn!("could not raise a banner for a failed action: {error}");
        }
    });
}
