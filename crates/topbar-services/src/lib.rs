//! Async system services for gnome-topbar.
//!
//! Everything that talks to the outside world — D-Bus, niri, PulseAudio,
//! subprocesses, the network — lives here and runs on a tokio runtime. The
//! crate deliberately has **no** GTK dependency: that is what makes it
//! impossible for a service task to touch a widget. State leaves this crate as
//! `Send + Clone` handles and `Arc<Snapshot>` values; the GTK crate subscribes
//! to them from the main thread.
//!
//! M0 only establishes the crate and its toolchain (tokio + zbus link cleanly);
//! the services themselves land from M2 onward.

#![warn(missing_docs)]

pub mod runtime;

pub use runtime::Runtime;
