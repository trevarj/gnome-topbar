//! Album art: when to fetch it, where to keep it, and what to throw away.
//!
//! Three separable problems, and each is its own tested piece:
//!
//! - [`ArtDebounce`] decides *when* a URL is worth acting on. Chromium writes
//!   two or three temporary files during a track change — the first is usually
//!   the site's favicon — so acting on the first URL means flashing the wrong
//!   picture ~200ms before the right one. v1 learned this the hard way and
//!   settled on a 200ms grace period; so does v2.
//! - [`Lru`] decides *what to keep*: twenty entries, the least recently used
//!   one leaves when the twenty-first arrives.
//! - [`fetch`] does the I/O, off the runtime's worker threads.
//!
//! `file://` art is used where it lies — a local player has already put the
//! file on disk, and copying it would double the disk cost of every album.
//! Only downloads are cached.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tracing::{debug, warn};

use super::model::ArtRef;

/// How long a new art URL has to hold still before it is fetched.
pub(crate) const DEBOUNCE: Duration = Duration::from_millis(200);
/// How many downloaded images are kept on disk.
pub(crate) const CAPACITY: usize = 20;
/// How long a download may take before it is abandoned.
const FETCH_TIMEOUT: u64 = 10;
/// Largest image the panel will download. Album art is tens of kilobytes; a
/// player pointing at something enormous is a player to ignore.
const MAX_BYTES: usize = 8 * 1024 * 1024;

/// The waiting room between "the player says the art changed" and "fetch it".
#[derive(Debug, Default)]
pub(crate) struct ArtDebounce {
    /// The URL the player's view currently reflects.
    applied: Option<String>,
    /// The URL waiting for the grace period to run out.
    pending: Option<Pending>,
}

/// A URL and the moment it may be acted on.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Pending {
    url: Option<String>,
    due: Instant,
}

impl ArtDebounce {
    /// Record what the player is asking for now.
    ///
    /// Repeating the same request does **not** restart the clock: a player
    /// that re-announces unchanged metadata every second would otherwise never
    /// get its art fetched at all.
    pub(crate) fn want(&mut self, url: Option<&str>, now: Instant) {
        let url = url.map(str::to_string);
        if url == self.applied {
            // Back to what is already on screen: whatever was waiting is moot.
            self.pending = None;
            return;
        }
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.url == url)
        {
            return;
        }
        self.pending = Some(Pending {
            url,
            due: now + DEBOUNCE,
        });
    }

    /// When the next decision is due, if one is waiting.
    pub(crate) fn due(&self) -> Option<Instant> {
        self.pending.as_ref().map(|pending| pending.due)
    }

    /// Take the URL whose grace period has run out.
    ///
    /// `Some(None)` means "there is no art now", which is a real answer: a
    /// track without a cover clears the one before it.
    pub(crate) fn take_due(&mut self, now: Instant) -> Option<Option<String>> {
        let pending = self.pending.take_if(|pending| pending.due <= now)?;
        self.applied = pending.url.clone();
        Some(pending.url)
    }

    /// The URL the view is currently showing.
    #[cfg(test)]
    pub(crate) fn applied(&self) -> Option<&str> {
        self.applied.as_deref()
    }
}

/// A fixed-size most-recently-used list.
#[derive(Debug)]
pub(crate) struct Lru {
    capacity: usize,
    /// Most recently used first.
    keys: Vec<u64>,
}

impl Lru {
    /// An empty list holding at most `capacity` keys.
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity,
            keys: Vec::new(),
        }
    }

    /// Record a use of `key`, returning whatever no longer fits.
    pub(crate) fn touch(&mut self, key: u64) -> Vec<u64> {
        self.keys.retain(|existing| *existing != key);
        self.keys.insert(0, key);
        self.keys
            .split_off(self.capacity.max(1).min(self.keys.len()))
    }

    /// Whether `key` is in the list.
    #[cfg(test)]
    pub(crate) fn contains(&self, key: u64) -> bool {
        self.keys.contains(&key)
    }

    /// How many keys are held.
    pub(crate) fn len(&self) -> usize {
        self.keys.len()
    }
}

/// The downloaded-art directory and the list of what is in it.
#[derive(Debug)]
pub(crate) struct ArtCache {
    dir: Option<PathBuf>,
    lru: Lru,
}

impl ArtCache {
    /// Open `$XDG_CACHE_HOME/topbar/art`, seeding the list from what is
    /// already there and trimming it to [`CAPACITY`].
    ///
    /// Seeding matters: without it a restart every hour would grow the
    /// directory without bound, because nothing would remember the files.
    pub(crate) fn open() -> Self {
        match cache_dir() {
            Some(dir) => Self::at(dir),
            None => {
                warn!("no cache directory; album art will be fetched every time");
                Self::nowhere()
            }
        }
    }

    /// The same, for a caller that chooses the directory — tests, chiefly.
    pub(crate) fn at(dir: PathBuf) -> Self {
        if let Err(error) = std::fs::create_dir_all(&dir) {
            warn!("could not create {}: {error}", dir.display());
            return Self::nowhere();
        }

        let mut lru = Lru::new(CAPACITY);
        // Oldest first, so the newest file ends up at the front of the list.
        for (_, key) in existing_entries(&dir) {
            for evicted in lru.touch(key) {
                remove(&dir.join(evicted.to_string()));
            }
        }
        debug!("album art cache: {} of {CAPACITY} entries", lru.len());

        Self {
            dir: Some(dir),
            lru,
        }
    }

    /// A cache with nowhere to keep anything. Covers of type `file://` still
    /// work; downloads simply happen every time.
    fn nowhere() -> Self {
        Self {
            dir: None,
            lru: Lru::new(CAPACITY),
        }
    }

    /// Where `key` would be kept.
    pub(crate) fn path(&self, key: u64) -> Option<PathBuf> {
        self.dir.as_ref().map(|dir| dir.join(key.to_string()))
    }

    /// The cached file for `key`, if it has been downloaded already.
    pub(crate) fn hit(&mut self, key: u64) -> Option<PathBuf> {
        let path = self.path(key)?;
        if !path.is_file() {
            return None;
        }
        self.record(&ArtRef {
            key,
            path: path.clone(),
        });
        Some(path)
    }

    /// Note that a downloaded cover was just written.
    ///
    /// Art that lives outside the cache directory — every `file://` cover — is
    /// ignored: the cache must never evict a file it did not create.
    pub(crate) fn record(&mut self, art: &ArtRef) {
        let Some(dir) = self.dir.clone() else {
            return;
        };
        if art.path.parent() != Some(dir.as_path()) {
            return;
        }
        for evicted in self.lru.touch(art.key) {
            debug!("album art {evicted} evicted: the cache is full");
            remove(&dir.join(evicted.to_string()));
        }
    }

    /// Whether the cache is holding `key`.
    #[cfg(test)]
    pub(crate) fn holds(&self, key: u64) -> bool {
        self.lru.contains(key)
    }
}

/// The identity of an art URL: the cache file name and the panel's texture key.
pub(crate) fn key_for(url: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    url.hash(&mut hasher);
    hasher.finish()
}

/// Fetch `url` into `destination`, or read it where it lies.
///
/// Runs on the runtime's blocking pool: `minreq` is a blocking client and
/// reading a file is a blocking read, and neither belongs on a worker thread
/// that is also driving the bus.
pub(crate) async fn fetch(url: String, destination: Option<PathBuf>) -> Option<ArtRef> {
    let key = key_for(&url);
    tokio::task::spawn_blocking(move || {
        if let Some(path) = local_path(&url) {
            return path.is_file().then_some(ArtRef { key, path });
        }
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            warn!("album art URL `{url}` uses a scheme the panel cannot read");
            return None;
        }
        let path = destination?;
        download(&url, &path).then_some(ArtRef { key, path })
    })
    .await
    .ok()
    .flatten()
}

/// The file a `file://` URL points at.
fn local_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    // `file:///path` (empty host) is the only form a media player emits;
    // anything with a host in it is not a file this machine can read.
    let path = rest.strip_prefix('/').map(|path| format!("/{path}"))?;
    Some(PathBuf::from(percent_decoded(&path)))
}

/// Undo the percent-encoding a player applies to spaces and non-ASCII names.
fn percent_decoded(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
            if let Some(byte) = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Download `url` to `path`, atomically. Blocking.
fn download(url: &str, path: &Path) -> bool {
    let response = match minreq::get(url).with_timeout(FETCH_TIMEOUT).send() {
        Ok(response) => response,
        Err(error) => {
            debug!("could not fetch album art from {url}: {error}");
            return false;
        }
    };
    if !(200..300).contains(&response.status_code) {
        debug!(
            "album art at {url} answered {} {}",
            response.status_code, response.reason_phrase
        );
        return false;
    }

    let bytes = response.as_bytes();
    if bytes.is_empty() || bytes.len() > MAX_BYTES {
        debug!("album art at {url} is {} bytes; ignored", bytes.len());
        return false;
    }

    // Written beside the target and renamed, so a torn download is never
    // served: the panel would draw half an image and cache it forever.
    let temporary = path.with_extension("part");
    if let Err(error) = std::fs::write(&temporary, bytes) {
        warn!(
            "could not write album art to {}: {error}",
            temporary.display()
        );
        return false;
    }
    if let Err(error) = std::fs::rename(&temporary, path) {
        warn!("could not store album art at {}: {error}", path.display());
        remove(&temporary);
        return false;
    }
    true
}

/// Every cached file, oldest first, as `(modified, key)`.
fn existing_entries(dir: &Path) -> Vec<(std::time::SystemTime, u64)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<(std::time::SystemTime, u64)> = entries
        .flatten()
        .filter_map(|entry| {
            let key = entry.file_name().to_str()?.parse::<u64>().ok()?;
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, key))
        })
        .collect();
    found.sort_unstable();
    found
}

/// Delete a file, saying so if it will not go.
fn remove(path: &Path) {
    if let Err(error) = std::fs::remove_file(path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        warn!("could not remove {}: {error}", path.display());
    }
}

/// `$XDG_CACHE_HOME/topbar/art`, or the default beneath `$HOME`.
fn cache_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))?;
    Some(base.join("topbar").join("art"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "file:///music/a.png";
    const B: &str = "https://example.invalid/b.png";

    #[test]
    fn a_new_url_waits_out_the_grace_period() {
        let now = Instant::now();
        let mut debounce = ArtDebounce::default();
        debounce.want(Some(A), now);

        assert_eq!(debounce.due(), Some(now + DEBOUNCE));
        assert_eq!(debounce.take_due(now + Duration::from_millis(199)), None);
        assert_eq!(debounce.take_due(now + DEBOUNCE), Some(Some(A.to_string())));
        assert_eq!(debounce.applied(), Some(A));
        assert_eq!(debounce.due(), None);
    }

    #[test]
    fn only_the_last_url_of_a_track_change_is_fetched() {
        // Chromium's favicon, then the real cover 150ms later.
        let now = Instant::now();
        let mut debounce = ArtDebounce::default();
        debounce.want(Some("file:///tmp/favicon.png"), now);
        debounce.want(Some(A), now + Duration::from_millis(150));

        assert_eq!(
            debounce.take_due(now + DEBOUNCE),
            None,
            "the clock restarted"
        );
        assert_eq!(
            debounce.take_due(now + Duration::from_millis(350)),
            Some(Some(A.to_string())),
            "the cover is what gets fetched, and the favicon never is"
        );
    }

    #[test]
    fn repeating_the_same_url_does_not_hold_the_fetch_off_forever() {
        let now = Instant::now();
        let mut debounce = ArtDebounce::default();
        debounce.want(Some(A), now);
        for step in 1..10 {
            debounce.want(Some(A), now + Duration::from_millis(step * 50));
        }
        assert_eq!(debounce.take_due(now + DEBOUNCE), Some(Some(A.to_string())));
    }

    #[test]
    fn art_that_goes_away_clears_after_the_same_grace_period() {
        let now = Instant::now();
        let mut debounce = ArtDebounce::default();
        debounce.want(Some(A), now);
        debounce.take_due(now + DEBOUNCE);

        let now = now + DEBOUNCE;
        debounce.want(None, now);
        assert_eq!(debounce.take_due(now + Duration::from_millis(100)), None);
        assert_eq!(debounce.take_due(now + DEBOUNCE), Some(None));
        assert_eq!(debounce.applied(), None);
    }

    #[test]
    fn a_url_that_flickers_and_comes_back_is_never_refetched() {
        let now = Instant::now();
        let mut debounce = ArtDebounce::default();
        debounce.want(Some(A), now);
        debounce.take_due(now + DEBOUNCE);

        // The player blanks its metadata mid-track-change and puts the same
        // cover back before the grace period runs out.
        let now = now + DEBOUNCE;
        debounce.want(None, now);
        debounce.want(Some(A), now + Duration::from_millis(50));
        assert_eq!(debounce.due(), None, "nothing left to decide");
        assert_eq!(debounce.take_due(now + Duration::from_secs(1)), None);
        assert_eq!(debounce.applied(), Some(A));
    }

    #[test]
    fn a_settled_url_never_fetches_twice() {
        let now = Instant::now();
        let mut debounce = ArtDebounce::default();
        debounce.want(Some(B), now);
        assert_eq!(debounce.take_due(now + DEBOUNCE), Some(Some(B.to_string())));
        debounce.want(Some(B), now + Duration::from_secs(5));
        assert_eq!(debounce.due(), None);
    }

    #[test]
    fn the_cache_holds_twenty_and_drops_the_oldest() {
        let mut lru = Lru::new(CAPACITY);
        for key in 0..CAPACITY as u64 {
            assert!(
                lru.touch(key).is_empty(),
                "nothing is evicted while there is room"
            );
        }
        assert_eq!(lru.len(), CAPACITY);

        assert_eq!(
            lru.touch(100),
            vec![0],
            "the least recently used one leaves"
        );
        assert_eq!(lru.len(), CAPACITY);
        assert!(!lru.contains(0));
        assert!(lru.contains(100));
    }

    #[test]
    fn using_an_entry_again_saves_it_from_eviction() {
        let mut lru = Lru::new(3);
        lru.touch(1);
        lru.touch(2);
        lru.touch(3);
        lru.touch(1);
        assert_eq!(
            lru.touch(4),
            vec![2],
            "1 was used again, so 2 is the oldest"
        );
        assert!(lru.contains(1));
    }

    #[test]
    fn touching_the_same_entry_never_grows_the_cache() {
        let mut lru = Lru::new(CAPACITY);
        for _ in 0..100 {
            assert!(lru.touch(7).is_empty());
        }
        assert_eq!(lru.len(), 1);
    }

    #[test]
    fn a_url_hashes_to_a_stable_cache_key() {
        assert_eq!(key_for(A), key_for(A));
        assert_ne!(key_for(A), key_for(B));
    }

    #[test]
    fn a_file_url_names_a_file_on_disk() {
        assert_eq!(
            local_path("file:///music/Aphex%20Twin/cover.png"),
            Some(PathBuf::from("/music/Aphex Twin/cover.png"))
        );
        assert_eq!(local_path("https://example.invalid/a.png"), None);
        assert_eq!(local_path("file://remote/share/a.png"), None);
    }

    #[test]
    fn percent_decoding_leaves_ordinary_text_alone() {
        assert_eq!(percent_decoded("/music/cover.png"), "/music/cover.png");
        assert_eq!(percent_decoded("100%"), "100%");
        assert_eq!(percent_decoded("%zz"), "%zz");
        assert_eq!(percent_decoded("a%2Fb"), "a/b");
    }

    /// A scratch cache directory, emptied first.
    fn scratch(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("topbar-art-cache-{}-{label}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// Pretend `key` was downloaded into `cache`.
    fn store(cache: &mut ArtCache, key: u64) {
        let path = cache.path(key).expect("a cache with a directory");
        std::fs::write(&path, b"cover").expect("write the cover");
        cache.record(&ArtRef { key, path });
    }

    #[test]
    fn evicting_an_entry_takes_its_file_off_the_disk() {
        let dir = scratch("evict");
        let mut cache = ArtCache::at(dir.clone());
        for key in 0..=CAPACITY as u64 {
            store(&mut cache, key);
        }

        assert!(!cache.holds(0), "the oldest entry was evicted");
        assert!(!dir.join("0").exists(), "and its file went with it");
        assert!(cache.holds(CAPACITY as u64));
        assert!(cache.hit(CAPACITY as u64).is_some());
        assert_eq!(
            std::fs::read_dir(&dir)
                .expect("the cache directory")
                .count(),
            CAPACITY
        );

        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[test]
    fn the_cache_never_takes_charge_of_a_file_it_did_not_download() {
        let dir = scratch("foreign");
        let mut cache = ArtCache::at(dir.clone());
        cache.record(&ArtRef {
            key: 7,
            path: PathBuf::from("/music/album/cover.png"),
        });
        assert!(
            !cache.holds(7),
            "a file:// cover belongs to the user, not to the cache"
        );

        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[test]
    fn reopening_the_cache_finds_what_is_there_and_trims_the_rest() {
        let dir = scratch("reopen");
        std::fs::create_dir_all(&dir).expect("the cache directory");
        for key in 0..30u64 {
            std::fs::write(dir.join(key.to_string()), b"cover").expect("write");
            // Distinct modification times, so "oldest" means something.
            std::thread::sleep(Duration::from_millis(2));
        }
        std::fs::write(dir.join("not-a-key"), b"stray").expect("write");

        let cache = ArtCache::at(dir.clone());
        assert_eq!(cache.lru.len(), CAPACITY);
        assert!(cache.holds(29), "the newest file survives");
        assert!(!cache.holds(0), "the oldest does not");
        assert!(!dir.join("0").exists());
        assert!(
            dir.join("not-a-key").exists(),
            "a file the cache did not write is left alone"
        );

        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[tokio::test]
    async fn art_that_is_not_there_is_no_art() {
        assert_eq!(fetch("file:///nowhere/at/all.png".into(), None).await, None);
        assert_eq!(fetch("data:image/png;base64,AAA".into(), None).await, None);
        assert_eq!(
            fetch("https://example.invalid/a.png".into(), None).await,
            None
        );
    }

    #[tokio::test]
    async fn a_local_file_is_used_where_it_lies() {
        let path = std::env::temp_dir().join(format!("topbar-art-{}.png", std::process::id()));
        std::fs::write(&path, b"not really a png").expect("write the art");

        let url = format!("file://{}", path.display());
        let art = fetch(url.clone(), None).await.expect("the file is there");
        assert_eq!(
            art.path, path,
            "a local file is never copied into the cache"
        );
        assert_eq!(art.key, key_for(&url));

        std::fs::remove_file(&path).expect("clean up");
    }
}
