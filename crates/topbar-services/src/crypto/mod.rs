//! Crypto prices: one request, every asset, two surfaces.
//!
//! ```text
//!   model.rs   the published snapshot and what an entry is   (pure)
//!   api.rs     the one CoinGecko URL and its body            (pure + I/O)
//!   task.rs    the one owner of all of it
//! ```
//!
//! The bar widget and its popover — both the price rows and the settings view
//! — render from the single [`CryptoState`] this service publishes. Every fetch
//! asks CoinGecko for all three assets whatever the user configured, so turning
//! one on in the settings view is a redraw and not a round trip.
//!
//! When to fetch next is [`crate::refresh`], shared with the weather: the
//! configured interval with ±10% of jitter, a doubling backoff after a failure,
//! and no scheduling at all while the machine is offline — the moment
//! [`Connectivity`](crate::connectivity) says the network is back, the owed
//! fetch runs. A failure keeps the last prices on screen with their age rather
//! than blanking the widget.

mod api;
mod model;
mod task;

#[cfg(test)]
mod bus_tests;
#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use topbar_core::config::CryptoConfig;
use tracing::warn;

use crate::connectivity::Connectivity;
use crate::error::SvcError;
use crate::lazy::Deferred;
use crate::state_store::StateStore;

pub use api::Endpoints;
pub use model::{
    Asset, CryptoState, Entry, EntryError, EntryQuote, PersistedCrypto, Phase, Quote, pair_change,
};

use task::Command;

/// How many commands may be in flight before a sender waits.
const QUEUE: usize = 8;

/// The entries a panel that has never been told otherwise draws.
///
/// Bitcoin, Ethereum, and the ratio between them — which is exactly what the
/// shell script this widget replaces printed.
pub const DEFAULT_ENTRIES: [&str; 3] = ["btc", "eth", "eth/btc"];

/// How long between refreshes, out of `[widgets.crypto]`.
///
/// Nothing is clamped here: config validation rejects anything under a minute
/// outright rather than quietly correcting it, so a value that reaches this
/// point is one the user meant. The popover reads the same figure to decide
/// whether opening it is worth a request.
pub fn interval(config: &CryptoConfig) -> Duration {
    Duration::from_secs(config.interval)
}

/// The crypto price service.
///
/// Cloning is cheap — a channel sender and a watch subscription — so the bar
/// widget and its popover each hold their own copy of the same one cache.
#[derive(Clone)]
pub struct Crypto {
    handle: CryptoHandle,
    state: watch::Receiver<Arc<CryptoState>>,
    task: Deferred,
}

impl Crypto {
    /// Start the service from the configuration and what was remembered.
    ///
    /// `wanted` is whether a `crypto` widget is on the bar. A panel without one
    /// gets the handles and an empty snapshot but asks CoinGecko for nothing —
    /// see [`crate::lazy`] — until a reload places the widget.
    pub(crate) fn start(
        config: &CryptoConfig,
        persisted: PersistedCrypto,
        store: StateStore,
        connectivity: &Connectivity,
        wanted: bool,
    ) -> Self {
        Self::spawn(
            interval(config),
            Endpoints::from_env(),
            Some(store),
            connectivity.state(),
            resolve_entries(persisted.entries.as_deref(), &config.entries),
            wanted,
        )
    }

    /// Start the task if it was held back. Returns whether this call did it.
    pub(crate) fn ensure_started(&self) -> bool {
        self.task.start()
    }

    /// The same, with everything named explicitly. Tests use this to point the
    /// service at a local listener and drive connectivity by hand.
    #[cfg(test)]
    pub(crate) fn start_with(
        interval: Duration,
        endpoints: Endpoints,
        store: Option<StateStore>,
        connectivity: watch::Receiver<Arc<crate::connectivity::ConnectivityState>>,
        entries: Vec<Entry>,
    ) -> Self {
        Self::spawn(interval, endpoints, store, connectivity, entries, true)
    }

    fn spawn(
        interval: Duration,
        endpoints: Endpoints,
        store: Option<StateStore>,
        connectivity: watch::Receiver<Arc<crate::connectivity::ConnectivityState>>,
        entries: Vec<Entry>,
        wanted: bool,
    ) -> Self {
        let (commands, queue) = mpsc::channel(QUEUE);
        let (publisher, state) = watch::channel(Arc::new(CryptoState {
            entries: entries.clone(),
            ..CryptoState::default()
        }));
        let task = Deferred::spawn(
            wanted,
            task::run(
                queue,
                publisher,
                interval,
                endpoints,
                store,
                connectivity,
                entries,
            ),
        );
        Self {
            handle: CryptoHandle { commands },
            state,
            task,
        }
    }

    /// The handle commands are sent through.
    pub fn handle(&self) -> &CryptoHandle {
        &self.handle
    }

    /// Subscribe to the prices.
    pub fn state(&self) -> watch::Receiver<Arc<CryptoState>> {
        self.state.clone()
    }
}

/// What the panel may ask of the crypto service.
#[derive(Clone)]
pub struct CryptoHandle {
    commands: mpsc::Sender<Command>,
}

impl CryptoHandle {
    /// Fetch now, whatever the schedule said.
    ///
    /// The popover asks for this when it opens onto prices older than the
    /// interval, and — once M12's lifecycle service exists — so does
    /// `lifecycle.on_resume`, a laptop opened after a night asleep having the
    /// most obviously wrong prices on it.
    // M12: lifecycle.on_resume -> refresh_now
    pub async fn refresh_now(&self) -> Result<(), SvcError> {
        self.send(Command::Refresh).await
    }

    /// Draw these entries from now on, and remember them.
    ///
    /// Written to `state.json` rather than to the user's `config.toml` — the
    /// panel never edits their config — and from then on they override the
    /// `[widgets.crypto] entries` seed. No refetch happens: the prices in hand
    /// already cover every supported asset.
    pub async fn set_entries(&self, entries: Vec<Entry>) -> Result<(), SvcError> {
        self.send(Command::SetEntries(entries)).await
    }

    /// Apply a changed `[widgets.crypto]` interval. M12's hot reload calls it.
    pub async fn configure(&self, interval: Duration) -> Result<(), SvcError> {
        self.send(Command::Configure(interval)).await
    }

    /// Post a command, or report that the service has stopped.
    async fn send(&self, command: Command) -> Result<(), SvcError> {
        self.commands
            .send(command)
            .await
            .map_err(|_| SvcError::ServiceStopped("crypto"))
    }
}

/// Resolve the entries the widget draws, in priority order.
///
/// 1. What the settings view last saved. A choice the user made by hand beats
///    everything, which is what makes the popover able to override the config
///    without ever writing to it. An empty saved list is a deliberate "show
///    nothing" and is honoured; *never having saved* is `None`, not empty.
/// 2. `[widgets.crypto] entries`, for the entries that parse. One unreadable
///    entry costs that entry and nothing else — config validation has already
///    told the user about it by name.
/// 3. [`DEFAULT_ENTRIES`], when neither of the above named anything usable.
pub fn resolve_entries(saved: Option<&[String]>, configured: &[String]) -> Vec<Entry> {
    if let Some(saved) = saved {
        return parse_entries(saved, "state.json");
    }
    let configured = parse_entries(configured, "widgets.crypto.entries");
    if !configured.is_empty() {
        return configured;
    }
    parse_entries(
        &DEFAULT_ENTRIES.map(str::to_string),
        "the built-in defaults",
    )
}

/// Read a list of written entries, dropping and logging the ones that are not.
fn parse_entries(values: &[String], source: &str) -> Vec<Entry> {
    let mut entries = Vec::with_capacity(values.len());
    for value in values {
        match value.parse::<Entry>() {
            // Duplicates would draw the same number twice; the settings view
            // cannot produce them and a hand-edited file should not either.
            Ok(entry) if !entries.contains(&entry) => entries.push(entry),
            Ok(_) => warn!("{source}: `{value}` is listed twice; keeping the first"),
            Err(error) => warn!("{source}: {error}"),
        }
    }
    entries
}
