//! The niri compositor service: one event stream in, actions out.

mod requests;
mod snapshot;
mod stream;

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::watch;
use tracing::error;

pub use requests::NiriHandle;
pub use snapshot::{KeyboardLayoutSnapshot, WorkspaceView, WorkspacesSnapshot};

/// Everything the panel needs from niri.
///
/// Cloning is cheap: the handle is reference-counted and the receivers are
/// watch subscriptions, so every bar and widget can hold its own copy.
#[derive(Clone)]
pub struct Niri {
    handle: NiriHandle,
    workspaces: watch::Receiver<Arc<WorkspacesSnapshot>>,
    keyboard_layout: watch::Receiver<Arc<KeyboardLayoutSnapshot>>,
}

impl Niri {
    /// Connect to niri at `socket` and start the event-stream task.
    ///
    /// Must be called from inside the service runtime. `None` — no
    /// `$NIRI_SOCKET` — is not fatal: the snapshots stay disconnected and the
    /// widgets that need them dim, which is also what a compositor restart
    /// looks like.
    pub(crate) fn start(socket: Option<PathBuf>) -> Self {
        let (workspaces_tx, workspaces) = watch::channel(Arc::new(WorkspacesSnapshot::default()));
        let (keyboard_tx, keyboard_layout) =
            watch::channel(Arc::new(KeyboardLayoutSnapshot::default()));

        match socket.clone() {
            Some(path) => {
                tokio::spawn(stream::run(
                    path,
                    stream::Publishers {
                        workspaces: workspaces_tx,
                        keyboard_layout: keyboard_tx,
                    },
                ));
            }
            None => {
                error!("no niri socket; the workspace and keyboard-layout widgets will stay dimmed")
            }
        }

        Self {
            handle: NiriHandle::new(socket),
            workspaces,
            keyboard_layout,
        }
    }

    /// The handle actions are sent through.
    pub fn handle(&self) -> &NiriHandle {
        &self.handle
    }

    /// Subscribe to workspace state.
    pub fn workspaces(&self) -> watch::Receiver<Arc<WorkspacesSnapshot>> {
        self.workspaces.clone()
    }

    /// Subscribe to keyboard-layout state.
    pub fn keyboard_layout(&self) -> watch::Receiver<Arc<KeyboardLayoutSnapshot>> {
        self.keyboard_layout.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Running outside niri — a TTY login, a CI runner, `cargo test` — has to
    /// produce a quiet, disconnected service rather than a failure.
    #[tokio::test]
    async fn without_a_socket_the_service_starts_disconnected() {
        let niri = Niri::start(None);

        let workspaces = niri.workspaces();
        assert!(!workspaces.borrow().connected);
        assert!(workspaces.borrow().outputs.is_empty());
        assert_eq!(workspaces.borrow().focused_output, None);

        let layouts = niri.keyboard_layout();
        assert!(!layouts.borrow().connected);
        assert!(!layouts.borrow().is_switchable());
        assert_eq!(layouts.borrow().current(), None);

        assert!(niri.handle().focus_workspace(1).await.is_err());
    }
}
