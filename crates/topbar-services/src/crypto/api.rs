//! Talking to CoinGecko: one URL, one body.
//!
//! The request always asks for **all three** assets, whatever the user has
//! configured. It costs nothing extra — one row of JSON each — and it is what
//! makes the settings view instant: turning Monero on redraws from prices that
//! are already in hand instead of waiting out a round trip.
//!
//! As in [`crate::weather::api`], the split is pure-versus-I/O: [`prices_url`]
//! and [`parse_prices`] are fixture-tested, and [`fetch`] is the only thing
//! that touches the network. `minreq` is blocking, so every request runs inside
//! [`tokio::task::spawn_blocking`].
//!
//! # Environment
//!
//! `TOPBAR_CRYPTO_API` replaces the base URL. It exists for the visual smoke
//! run and the tests, which point it at a local listener serving the recorded
//! fixture so a screenshot of populated prices does not depend on somebody
//! else's rate limit. Unset — which is every real run — it is CoinGecko itself.

use std::collections::BTreeMap;

use serde::Deserialize;
use tracing::debug;

use crate::crypto::model::{Asset, Quote};
use crate::error::SvcError;

/// CoinGecko's public API root.
const API_BASE: &str = "https://api.coingecko.com";
/// Overrides [`API_BASE`].
const API_ENV: &str = "TOPBAR_CRYPTO_API";
/// The path under the base that quotes simple prices.
const PRICES_PATH: &str = "/api/v3/simple/price";

/// How long a request may take before it is abandoned. The weather's fifteen
/// seconds, for the same reason: the panel retries on a backoff, so a slow
/// answer is worth less than a prompt failure.
const TIMEOUT_SECS: u64 = 15;
/// Header budget for one answer; past this it is a captive portal.
const MAX_HEADERS: usize = 16 * 1024;

/// Where the price request goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    /// Base URL of the price endpoint, path included.
    pub prices: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            prices: format!("{API_BASE}{PRICES_PATH}"),
        }
    }
}

impl Endpoints {
    /// The real endpoint, unless the environment names another.
    ///
    /// `TOPBAR_CRYPTO_API` names a *base*, so a stub only has to be told which
    /// host and port it is on; the API path is appended unless the override
    /// already carries a path of its own, which is what lets a stub serving one
    /// route be pointed at directly.
    pub fn from_env() -> Self {
        let Some(base) = override_from(API_ENV) else {
            return Self::default();
        };
        let trimmed = base.trim_end_matches('/');
        let has_path = trimmed
            .split_once("://")
            .map(|(_, rest)| rest.contains('/'))
            .unwrap_or_else(|| trimmed.contains('/'));
        Self {
            prices: if has_path {
                trimmed.to_string()
            } else {
                format!("{trimmed}{PRICES_PATH}")
            },
        }
    }
}

/// A non-empty base URL from `name`.
fn override_from(name: &str) -> Option<String> {
    let value = std::env::var(name).ok()?;
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    debug!("{name} redirects price requests to {value}");
    Some(value.to_string())
}

/// The price request, which always covers every supported asset.
pub fn prices_url(base: &str) -> String {
    let ids = Asset::ALL
        .iter()
        .map(|asset| asset.id())
        .collect::<Vec<_>>()
        .join(",");
    format!("{base}?ids={ids}&vs_currencies=usd&include_24hr_change=true")
}

/// Fetch `url`, or say why not.
pub async fn fetch(url: String) -> Result<String, SvcError> {
    tokio::task::spawn_blocking(move || blocking_fetch(&url))
        .await
        .map_err(|error| SvcError::Http(format!("the request task failed: {error}")))?
}

/// The request itself, on a blocking thread.
fn blocking_fetch(url: &str) -> Result<String, SvcError> {
    let response = minreq::get(url)
        .with_timeout(TIMEOUT_SECS)
        .with_max_headers_size(MAX_HEADERS)
        .send()
        .map_err(|error| SvcError::Http(error.to_string()))?;

    let status = response.status_code;
    let body = response
        .as_str()
        .map_err(|error| SvcError::Http(format!("the answer was not text: {error}")))?;

    // A rate limit is its own error rather than a generic HTTP failure: it is
    // the one failure a free CoinGecko key hits routinely, the panel's answer
    // to it is "wait longer", and the user is owed those words rather than
    // "could not reach the service" for a service that answered fine.
    if status == 429 {
        return Err(SvcError::RateLimited(rate_limit_reason(body)));
    }
    if !(200..300).contains(&status) {
        return Err(SvcError::Http(format!("the service answered {status}")));
    }
    Ok(body.to_string())
}

/// CoinGecko's error envelope: `{"status": {"error_code", "error_message"}}`.
#[derive(Debug, Deserialize)]
struct ErrorBody {
    status: ErrorStatus,
}

#[derive(Debug, Deserialize)]
struct ErrorStatus {
    error_message: Option<String>,
}

/// The explanation out of a 429 body, or a stand-in when there is none.
fn rate_limit_reason(body: &str) -> String {
    serde_json::from_str::<ErrorBody>(body)
        .ok()
        .and_then(|error| error.status.error_message)
        .filter(|message| !message.trim().is_empty())
        .unwrap_or_else(|| "the price service is rate limiting this panel".to_string())
}

/// One asset's row in the price body.
#[derive(Debug, Deserialize)]
struct PriceRow {
    usd: Option<f64>,
    /// CoinGecko omits this for an asset it has no 24-hour history for, and
    /// drops it entirely when `include_24hr_change` was not asked for.
    usd_24h_change: Option<f64>,
}

/// Read a price body into a quote per asset.
///
/// Missing assets are simply absent from the map rather than an error: an
/// answer covering two of the three still draws two of the three entries, and
/// blanking the widget because Monero was briefly unlisted would be worse than
/// the gap. An answer covering *none* of them is an error, because that is not
/// a price body at all.
pub fn parse_prices(body: &str) -> Result<BTreeMap<Asset, Quote>, SvcError> {
    if let Ok(error) = serde_json::from_str::<ErrorBody>(body) {
        return Err(SvcError::Http(error.status.error_message.unwrap_or_else(
            || "the price service refused the request".to_string(),
        )));
    }

    let rows: BTreeMap<String, PriceRow> = serde_json::from_str(body)
        .map_err(|error| SvcError::Protocol(format!("unreadable prices: {error}")))?;

    let quotes: BTreeMap<Asset, Quote> = Asset::ALL
        .iter()
        .filter_map(|asset| {
            let row = rows.get(asset.id())?;
            let usd = row.usd.filter(|usd| usd.is_finite() && *usd > 0.0)?;
            Some((
                *asset,
                Quote {
                    usd,
                    change_24h: row.usd_24h_change.filter(|change| change.is_finite()),
                },
            ))
        })
        .collect();

    if quotes.is_empty() {
        return Err(SvcError::Protocol(
            "the price answer had no prices in it".to_string(),
        ));
    }
    Ok(quotes)
}
