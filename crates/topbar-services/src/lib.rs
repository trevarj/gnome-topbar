//! Async system services for gnome-topbar.
//!
//! Everything that talks to the outside world — D-Bus, niri, PulseAudio,
//! subprocesses, the network — lives here and runs on a tokio runtime. The
//! crate deliberately has **no** GTK dependency: that is what makes it
//! impossible for a service task to touch a widget. State leaves this crate as
//! `Send + Clone` handles and `Arc<Snapshot>` values; the GTK crate subscribes
//! to them from the main thread.

#![warn(missing_docs)]

pub mod error;
pub mod niri;
pub mod runtime;

pub use error::SvcError;
pub use niri::{KeyboardLayoutSnapshot, Niri, NiriHandle, WorkspaceView, WorkspacesSnapshot};
pub use runtime::{Runtime, Services};
