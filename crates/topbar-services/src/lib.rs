//! Async system services for topbar.
//!
//! Everything that talks to the outside world — D-Bus, niri, PulseAudio,
//! subprocesses, the network — lives here and runs on a tokio runtime. The
//! crate deliberately has **no** GTK dependency: that is what makes it
//! impossible for a service task to touch a widget. State leaves this crate as
//! `Send + Clone` handles and `Arc<Snapshot>` values; the GTK crate subscribes
//! to them from the main thread.

#![warn(missing_docs)]

pub mod connectivity;
pub mod crypto;
pub mod error;
pub mod media;
pub mod niri;
pub mod notifications;
pub mod refresh;
pub mod runtime;
pub mod state_store;
pub mod tray;
pub mod weather;

#[cfg(test)]
mod private_bus;

/// The channel type every service publishes its state through.
///
/// Re-exported so the GTK crate can name a subscription without depending on
/// tokio itself — the less of the async stack it can see, the harder it is to
/// accidentally do async work on the main thread.
pub use tokio::sync::watch;

pub use connectivity::{Connectivity, ConnectivityState};
pub use crypto::{Asset, Crypto, CryptoHandle, CryptoState, Entry, EntryQuote, Quote};
pub use error::SvcError;
pub use media::{ArtRef, Media, MediaHandle, MediaState, PlaybackStatus, PlayerView};
pub use niri::{KeyboardLayoutSnapshot, Niri, NiriHandle, WorkspaceView, WorkspacesSnapshot};
pub use notifications::{
    Action, CloseReason, GroupView, IconSource, ImageData, NotifState, NotificationView,
    Notifications, NotificationsHandle, ToastView, Urgency,
};
pub use runtime::{Runtime, Services};
pub use state_store::StateStore;
pub use tray::{
    IconView, ItemView, MenuEvent, MenuKind, MenuNode, Pixmap, ScrollAxis, Status as TrayStatus,
    ToggleKind, ToggleState, Tray, TrayHandle, TrayState,
};
pub use weather::{
    CurrentWeather, DailyWeather, GeocodeResult, LocationView, Phase, TemperatureUnit, Weather,
    WeatherData, WeatherHandle, WeatherState,
};
