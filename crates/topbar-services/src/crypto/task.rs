//! The one owner of the price cache.
//!
//! The same shape as [`crate::weather::task`], and deliberately so: the refresh
//! timer, the connectivity watch, and the commands the panel can send are the
//! only things that touch the snapshot, and requests are spawned rather than
//! awaited inline so a fifteen-second fetch cannot make a settings toggle wait.
//!
//! The one difference worth knowing about is that changing the entry list does
//! **not** refetch. Every fetch covers all three assets, so a new entry is
//! always already priced.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use tokio::sync::{mpsc, watch};
use tokio::time::Instant;
use tracing::{debug, info, warn};

use crate::connectivity::ConnectivityState;
use crate::crypto::api::{self, Endpoints};
use crate::crypto::model::{Asset, CryptoState, Entry, Phase, Quote};
use crate::error::SvcError;
use crate::refresh::Refresh;
use crate::state_store::StateStore;

/// What the panel can ask of the crypto service.
pub(crate) enum Command {
    /// Fetch now, whatever the schedule said.
    Refresh,
    /// Draw these entries from now on, and remember them.
    SetEntries(Vec<Entry>),
    /// The configuration changed under us.
    Configure(Duration),
}

/// Run the service until every handle is dropped.
pub(crate) async fn run(
    mut commands: mpsc::Receiver<Command>,
    publisher: watch::Sender<Arc<CryptoState>>,
    interval: Duration,
    endpoints: Endpoints,
    store: Option<StateStore>,
    mut connectivity: watch::Receiver<Arc<ConnectivityState>>,
    entries: Vec<Entry>,
) {
    let (answers, mut outcomes) = mpsc::channel(1);
    let online = connectivity.borrow_and_update().online;

    let mut task = Task {
        refresh: Refresh::new(interval),
        interval,
        endpoints,
        publisher,
        store,
        answers,
        entries,
        quotes: BTreeMap::new(),
        fetched_at: None,
        due: None,
        online,
        in_flight: false,
        deferred: false,
        phase: Phase::Loading,
    };

    task.publish();
    // A panel with no prices on it for half an hour is a panel with no crypto
    // widget, so the first fetch is immediate.
    task.due = Some(Instant::now());

    loop {
        let due = task.due;
        let timer = async move {
            match due {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending().await,
            }
        };
        tokio::pin!(timer);

        tokio::select! {
            command = commands.recv() => match command {
                Some(command) => task.command(command),
                None => break,
            },
            changed = connectivity.changed() => {
                // A watcher that has stopped is not evidence of being offline.
                let online = changed.map_or(true, |()| connectivity.borrow().online);
                task.set_online(online);
            },
            outcome = outcomes.recv() => {
                if let Some(outcome) = outcome {
                    task.settle(outcome);
                }
            },
            () = &mut timer => task.fetch(),
        }
    }

    debug!("the crypto service has no handles left; stopping");
}

/// Everything the loop owns.
struct Task {
    interval: Duration,
    endpoints: Endpoints,
    publisher: watch::Sender<Arc<CryptoState>>,
    store: Option<StateStore>,
    /// Where a spawned fetch sends what it found.
    answers: mpsc::Sender<Result<BTreeMap<Asset, Quote>, SvcError>>,
    /// The effective entry list.
    entries: Vec<Entry>,
    /// The last prices that arrived. Empty until the first success.
    quotes: BTreeMap<Asset, Quote>,
    /// When they arrived.
    fetched_at: Option<SystemTime>,
    /// Whether what is in hand is current.
    refresh: Refresh,
    /// When the next fetch is due. `None` while nothing is scheduled.
    due: Option<Instant>,
    online: bool,
    /// A request is out. Two in flight would race each other into the cache.
    in_flight: bool,
    /// A fetch came due while the machine was offline and is owed.
    deferred: bool,
    /// The phase the last publish claimed.
    phase: Phase,
}

impl Task {
    /// Publish the snapshot the current state implies, if it changed.
    fn publish(&self) {
        let state = CryptoState {
            phase: self.phase,
            quotes: self.quotes.clone(),
            entries: self.entries.clone(),
            fetched_at: self.fetched_at,
        };
        if **self.publisher.borrow() == state {
            return;
        }
        let _ = self.publisher.send(Arc::new(state));
    }

    /// Start a price request, or explain to itself why not.
    fn fetch(&mut self) {
        self.due = None;

        if !self.online {
            // Not a failure: no backoff, no stale timestamp, nothing on screen
            // changes. The request is simply owed until the network is back.
            debug!("the machine is offline; the price refresh is deferred");
            self.deferred = true;
            return;
        }
        if self.in_flight {
            return;
        }

        self.deferred = false;
        self.in_flight = true;

        let url = api::prices_url(&self.endpoints.prices);
        let answers = self.answers.clone();
        tokio::spawn(async move {
            let outcome = match api::fetch(url).await {
                Ok(body) => api::parse_prices(&body),
                Err(error) => Err(error),
            };
            let _ = answers.send(outcome).await;
        });
    }

    /// A request came back.
    fn settle(&mut self, outcome: Result<BTreeMap<Asset, Quote>, SvcError>) {
        self.in_flight = false;
        match outcome {
            Ok(quotes) => {
                debug!("prices updated: {} asset(s)", quotes.len());
                self.quotes = quotes;
                self.fetched_at = Some(SystemTime::now());
                self.phase = Phase::Ready;
                self.publish();
                let wait = self.refresh.succeeded();
                self.due = Some(Instant::now() + wait);
            }
            Err(error) => {
                warn!("the prices could not be refreshed: {error}");
                // Stale-while-revalidate: prices half an hour old with their
                // age admitted to beat an empty widget every time.
                self.phase = if self.quotes.is_empty() {
                    Phase::Unavailable
                } else {
                    Phase::Stale
                };
                self.publish();
                let wait = self.refresh.failed();
                info!("retrying the prices in {}s", wait.as_secs());
                self.due = Some(Instant::now() + wait);
            }
        }
    }

    /// The network came or went.
    fn set_online(&mut self, online: bool) {
        if self.online == online {
            return;
        }
        self.online = online;
        if online && self.deferred {
            info!("the machine is back online; refreshing the prices");
            self.fetch();
        }
    }

    /// Apply one command.
    fn command(&mut self, command: Command) {
        match command {
            Command::Refresh => self.fetch(),
            Command::SetEntries(entries) => self.set_entries(entries),
            Command::Configure(interval) => self.configure(interval),
        }
    }

    /// Draw a different list from now on, and write it down.
    ///
    /// No fetch: every request covers all three assets, so whatever the new
    /// list names is already priced. That is what makes the settings view feel
    /// instant, and it is why the entries live in the snapshot rather than
    /// being read from the config by the widget.
    fn set_entries(&mut self, entries: Vec<Entry>) {
        if self.entries == entries {
            return;
        }
        debug!("the crypto entries are now {entries:?}");
        self.entries = entries;
        if let Some(store) = &self.store {
            let saved: Vec<String> = self.entries.iter().map(Entry::to_string).collect();
            store.update(move |state| state.crypto.entries = Some(saved));
        }
        self.publish();
    }

    /// The configuration changed. M12's hot reload is what calls this.
    fn configure(&mut self, interval: Duration) {
        if self.interval == interval {
            return;
        }
        debug!("the crypto refresh interval is now {interval:?}");
        self.interval = interval;
        // The prices in hand are still prices. Unlike the weather — where a
        // changed unit makes every temperature in the cache wrong — this is one
        // of the two places nothing is thrown away, because a dollar is a
        // dollar however often it is asked for.
        self.refresh = Refresh::new(interval);
        if self.quotes.is_empty() {
            // Except that a widget with nothing on it must not sit out a whole
            // fresh interval before trying again. The backoff ladder has just
            // been reset, so this is also the user's edit acting as a retry.
            self.fetch();
            return;
        }
        self.due = Some(Instant::now() + self.refresh.succeeded());
    }
}
