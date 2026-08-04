//! The crypto service against a real socket and, once, a real bus — both
//! private.
//!
//! The CoinGecko here is a listener on loopback answering with the recorded
//! fixture, and the NetworkManager is a stand-in on a `dbus-daemon` that lives
//! for one test. Nothing in this file touches the machine's real system bus or
//! the real internet.
//!
//! What is only covered here is the wiring: that a fetch reaches the snapshot,
//! that a rate limit degrades the way stale-while-revalidate says it should,
//! that being offline defers rather than fails, and that saving entries reaches
//! `state.json` in a form the next start resolves back.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::watch;

use super::*;
use crate::connectivity::ConnectivityState;
use crate::state_store::StateStore;

const PRICES: &str = include_str!("../../tests/fixtures/coingecko-prices.json");
const RATE_LIMIT: &str = include_str!("../../tests/fixtures/coingecko-rate-limit.json");

/// How long a test waits for the service to catch up before failing.
const PATIENCE: Duration = Duration::from_secs(10);
/// The interval the tests run the service at. Long enough that no test ever
/// races a scheduled refresh.
const INTERVAL: Duration = Duration::from_secs(1800);

/// What the stub answers with. Swappable, so one service can watch its endpoint
/// start rate limiting under it.
type Answer = Arc<std::sync::Mutex<(u16, String)>>;

/// A listener standing in for CoinGecko.
struct StubApi {
    endpoints: Endpoints,
    requests: Arc<AtomicUsize>,
    answer: Answer,
}

impl StubApi {
    /// Serve `body` with `status` to everything that connects.
    async fn start(status: u16, body: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback is available");
        let address = listener.local_addr().expect("a bound port");
        let requests = Arc::new(AtomicUsize::new(0));
        let answer: Answer = Arc::new(std::sync::Mutex::new((status, body.to_string())));

        let counter = Arc::clone(&requests);
        let served = Arc::clone(&answer);
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                counter.fetch_add(1, Ordering::SeqCst);
                let mut scratch = [0_u8; 2048];
                let _ = socket.read(&mut scratch).await;
                let (status, body) = served.lock().expect("the answer is not poisoned").clone();
                let reply = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n\
                     Content-Length: {length}\r\nConnection: close\r\n\r\n{body}",
                    length = body.len(),
                );
                let _ = socket.write_all(reply.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
        });

        Self {
            endpoints: Endpoints {
                prices: format!("http://{address}/prices"),
            },
            requests,
            answer,
        }
    }

    /// Answer differently from now on.
    fn answer_with(&self, status: u16, body: &str) {
        *self.answer.lock().expect("the answer is not poisoned") = (status, body.to_string());
    }

    /// How many requests it has answered.
    fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

/// Wait until a published snapshot satisfies `predicate`.
async fn wait_for(
    state: &mut watch::Receiver<Arc<CryptoState>>,
    what: &str,
    predicate: impl Fn(&CryptoState) -> bool,
) -> Arc<CryptoState> {
    let wait = async {
        loop {
            // Cloned out before testing: a read guard held across an await
            // deadlocks against the task trying to publish the next value.
            let snapshot = state.borrow_and_update().clone();
            if predicate(&snapshot) {
                return snapshot;
            }
            state.changed().await.expect("the crypto service is alive");
        }
    };
    tokio::time::timeout(PATIENCE, wait)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

/// Wait until `predicate` holds, polling.
async fn wait_until(what: &str, predicate: impl Fn() -> bool) {
    let wait = async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    tokio::time::timeout(PATIENCE, wait)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}

/// A connectivity channel a test drives by hand.
fn manual_connectivity(
    online: bool,
) -> (
    watch::Sender<Arc<ConnectivityState>>,
    watch::Receiver<Arc<ConnectivityState>>,
) {
    watch::channel(Arc::new(ConnectivityState { online }))
}

fn default_entries() -> Vec<Entry> {
    resolve_entries(None, &[])
}

#[tokio::test]
async fn one_request_prices_every_entry_the_widget_draws() {
    let api = StubApi::start(200, PRICES).await;
    let (_online, connectivity) = manual_connectivity(true);

    let crypto = Crypto::start_with(
        INTERVAL,
        api.endpoints.clone(),
        None,
        connectivity,
        default_entries(),
    );
    let mut state = crypto.state();

    let ready = wait_for(&mut state, "prices", |state| state.phase == Phase::Ready).await;
    assert_eq!(api.requests(), 1, "one request, not one per entry");
    assert_eq!(ready.quotes.len(), 3, "all three, whatever is configured");

    let btc = ready
        .quote(Entry::Single(Asset::Btc))
        .expect("a bitcoin price");
    assert!((btc.value - 103_412.44).abs() < 1e-6);

    // The pair was never requested; it is arithmetic on what was.
    let pair = ready
        .quote(Entry::Pair(Asset::Eth, Asset::Btc))
        .expect("a ratio");
    assert!((pair.value - 0.032_995).abs() < 1e-5, "got {}", pair.value);
    assert!(pair.change_24h.expect("a derived change") < 0.0);
    assert!(ready.fetched_at.is_some());
}

#[tokio::test]
async fn a_rate_limit_keeps_the_last_prices_and_marks_them_stale() {
    let api = StubApi::start(200, PRICES).await;
    let (_online, connectivity) = manual_connectivity(true);

    let crypto = Crypto::start_with(
        INTERVAL,
        api.endpoints.clone(),
        None,
        connectivity,
        default_entries(),
    );
    let mut state = crypto.state();
    let ready = wait_for(&mut state, "prices", |state| state.phase == Phase::Ready).await;
    let good = ready.quotes.clone();

    api.answer_with(429, RATE_LIMIT);
    crypto
        .handle()
        .refresh_now()
        .await
        .expect("the service is alive");

    let stale = wait_for(&mut state, "the prices to go stale", |state| {
        state.phase == Phase::Stale
    })
    .await;
    assert_eq!(
        stale.quotes, good,
        "a rate limit keeps what was already on screen"
    );
    assert!(stale.stale_since().is_some(), "and admits how old it is");
}

#[tokio::test]
async fn a_first_fetch_that_is_rate_limited_has_nothing_to_fall_back_on() {
    let api = StubApi::start(429, RATE_LIMIT).await;
    let (_online, connectivity) = manual_connectivity(true);

    let crypto = Crypto::start_with(
        INTERVAL,
        api.endpoints.clone(),
        None,
        connectivity,
        default_entries(),
    );
    let mut state = crypto.state();

    let failed = wait_for(&mut state, "an unavailable widget", |state| {
        state.is_unavailable()
    })
    .await;
    assert!(failed.quotes.is_empty());
    assert_eq!(
        failed.entries.len(),
        3,
        "the widget still knows what it would draw"
    );
}

#[tokio::test]
async fn coming_back_online_fires_the_fetch_that_was_owed() {
    let api = StubApi::start(200, PRICES).await;
    let (online, connectivity) = manual_connectivity(false);

    let crypto = Crypto::start_with(
        INTERVAL,
        api.endpoints.clone(),
        None,
        connectivity,
        default_entries(),
    );
    let mut state = crypto.state();

    wait_for(&mut state, "the first frame", |state| state.is_loading()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(api.requests(), 0, "an offline panel must not dial out");
    assert!(
        !state.borrow().is_unavailable(),
        "being offline is not a failure"
    );

    online
        .send(Arc::new(ConnectivityState { online: true }))
        .ok();

    wait_for(&mut state, "prices", |state| state.phase == Phase::Ready).await;
    assert_eq!(api.requests(), 1, "exactly one request, not one per signal");
}

#[tokio::test]
async fn changing_the_entries_writes_them_down_without_asking_again() {
    let api = StubApi::start(200, PRICES).await;
    let (_online, connectivity) = manual_connectivity(true);

    let path = std::env::temp_dir()
        .join(format!("topbar-crypto-{}", std::process::id()))
        .join("state.json");
    let _ = std::fs::remove_dir_all(path.parent().expect("a parent"));
    let (persisted, store) = StateStore::open_at(path.clone());
    assert_eq!(persisted.crypto.entries, None);

    let crypto = Crypto::start_with(
        INTERVAL,
        api.endpoints.clone(),
        Some(store),
        connectivity,
        default_entries(),
    );
    let mut state = crypto.state();
    wait_for(&mut state, "prices", |state| state.phase == Phase::Ready).await;
    assert_eq!(api.requests(), 1);

    let wanted = vec![
        Entry::Single(Asset::Btc),
        Entry::Single(Asset::Xmr),
        Entry::Pair(Asset::Xmr, Asset::Btc),
    ];
    crypto
        .handle()
        .set_entries(wanted.clone())
        .await
        .expect("the service is alive");

    let changed = wait_for(&mut state, "the new entries", |state| {
        state.entries == wanted
    })
    .await;
    assert_eq!(
        api.requests(),
        1,
        "turning monero on must not cost a round trip"
    );
    assert!(
        changed.quote(Entry::Pair(Asset::Xmr, Asset::Btc)).is_some(),
        "the price was already in hand"
    );

    // And a panel started tomorrow draws what was chosen today.
    wait_until("the entries to reach the state file", || {
        std::fs::read_to_string(&path).is_ok_and(|contents| contents.contains("xmr/btc"))
    })
    .await;

    let (reloaded, _store) = StateStore::open_at(path.clone());
    let saved = reloaded.crypto.entries.expect("a saved list");
    assert_eq!(saved, vec!["btc", "xmr", "xmr/btc"]);
    assert_eq!(
        resolve_entries(Some(&saved), &owned_default()),
        wanted,
        "and it beats whatever the config still says"
    );
}

/// A config list that is deliberately different from what the test saves.
fn owned_default() -> Vec<String> {
    vec!["eth".to_string()]
}
