//! The pure half of the crypto service: the URL, the bodies, the entry list.

use super::*;
use crate::crypto::api::{parse_prices, prices_url};

const PRICES: &str = include_str!("../../tests/fixtures/coingecko-prices.json");
const PARTIAL: &str = include_str!("../../tests/fixtures/coingecko-prices-partial.json");
const RATE_LIMIT: &str = include_str!("../../tests/fixtures/coingecko-rate-limit.json");

/// Turn a list of literals into the owned strings config and state hand over.
fn owned(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

// ---------------------------------------------------------------------------
// The request
// ---------------------------------------------------------------------------

#[test]
fn the_request_always_asks_for_every_asset() {
    let url = prices_url("https://api.coingecko.com/api/v3/simple/price");
    assert!(
        url.contains("ids=bitcoin,ethereum,monero"),
        "one request covers all three, whatever is configured: {url}"
    );
    assert!(url.contains("vs_currencies=usd"));
    assert!(
        url.contains("include_24hr_change=true"),
        "the change chips have to come from somewhere: {url}"
    );
}

#[test]
fn the_endpoint_defaults_to_coingecko() {
    assert_eq!(
        Endpoints::default().prices,
        "https://api.coingecko.com/api/v3/simple/price"
    );
}

#[test]
fn a_bare_host_override_keeps_the_api_path() {
    // What the smoke run's stub is given: a host and a port, nothing else.
    temp_env("http://127.0.0.1:18081", |endpoints| {
        assert_eq!(
            endpoints.prices,
            "http://127.0.0.1:18081/api/v3/simple/price"
        );
    });
}

#[test]
fn an_override_with_a_path_of_its_own_is_taken_verbatim() {
    temp_env("http://127.0.0.1:18081/prices", |endpoints| {
        assert_eq!(endpoints.prices, "http://127.0.0.1:18081/prices");
    });
    temp_env("http://127.0.0.1:18081/prices/", |endpoints| {
        assert_eq!(endpoints.prices, "http://127.0.0.1:18081/prices");
    });
}

#[test]
fn a_blank_override_is_no_override() {
    temp_env("   ", |endpoints| {
        assert_eq!(endpoints, Endpoints::default());
    });
}

/// Run `check` with `TOPBAR_CRYPTO_API` set to `value`.
///
/// The whole crypto suite reads that one variable, and `cargo test` runs the
/// tests in one process, so this is serialised behind a mutex rather than left
/// to race.
fn temp_env(value: &str, check: impl FnOnce(Endpoints)) {
    use std::sync::Mutex;
    static LOCK: Mutex<()> = Mutex::new(());
    let guard = LOCK.lock().unwrap_or_else(|error| error.into_inner());

    // SAFETY: the mutex above makes this the only thread touching the
    // environment for the length of the call, which is what `set_var` asks for.
    unsafe { std::env::set_var("TOPBAR_CRYPTO_API", value) };
    let endpoints = Endpoints::from_env();
    unsafe { std::env::remove_var("TOPBAR_CRYPTO_API") };
    drop(guard);

    check(endpoints);
}

// ---------------------------------------------------------------------------
// The body
// ---------------------------------------------------------------------------

#[test]
fn a_full_answer_prices_all_three_assets() {
    let quotes = parse_prices(PRICES).expect("the fixture parses");
    assert_eq!(quotes.len(), 3);

    let btc = quotes.get(&Asset::Btc).expect("bitcoin");
    assert!((btc.usd - 103_412.44).abs() < 1e-6);
    assert_eq!(btc.change_24h, Some(2.4137));

    let eth = quotes.get(&Asset::Eth).expect("ethereum");
    assert!((eth.usd - 3_412.09).abs() < 1e-6);
    assert_eq!(eth.change_24h, Some(-1.2984));

    let xmr = quotes.get(&Asset::Xmr).expect("monero");
    assert!((xmr.usd - 234.56).abs() < 1e-6);
    assert_eq!(xmr.change_24h, Some(0.8321));
}

#[test]
fn a_missing_change_and_a_missing_asset_cost_only_themselves() {
    let quotes = parse_prices(PARTIAL).expect("a partial answer is still an answer");
    assert_eq!(quotes.len(), 2, "monero is simply absent");
    assert_eq!(
        quotes.get(&Asset::Btc).expect("bitcoin").change_24h,
        None,
        "a price with no change is still a price"
    );
    assert_eq!(quotes.get(&Asset::Xmr), None);
}

#[test]
fn an_answer_with_no_prices_in_it_is_an_error() {
    let error = parse_prices("{}").expect_err("that is not a price body");
    assert!(matches!(error, SvcError::Protocol(_)));
    let error =
        parse_prices(r#"{"dogecoin":{"usd":0.42}}"#).expect_err("none of the three were priced");
    assert!(matches!(error, SvcError::Protocol(_)));
}

#[test]
fn a_price_that_is_not_a_price_is_dropped() {
    let quotes =
        parse_prices(r#"{"bitcoin":{"usd":0},"ethereum":{"usd":3412.09},"monero":{"usd":null}}"#)
            .expect("ethereum still has a price");
    assert_eq!(quotes.len(), 1);
    assert_eq!(quotes.get(&Asset::Btc), None, "nothing costs zero dollars");
    assert_eq!(quotes.get(&Asset::Xmr), None);
}

#[test]
fn a_rate_limit_body_is_read_as_the_error_it_is() {
    let error = parse_prices(RATE_LIMIT).expect_err("that body has no prices in it");
    let text = error.to_string();
    assert!(
        text.contains("Rate Limit"),
        "the reason the service gave is what gets logged: {text}"
    );
}

#[test]
fn garbage_is_a_protocol_error_rather_than_a_panic() {
    for body in ["", "not json at all", "[1, 2, 3]", "{\"bitcoin\": 42}"] {
        let error = parse_prices(body).expect_err("garbage in");
        assert!(
            matches!(error, SvcError::Protocol(_)),
            "{body:?} produced {error}"
        );
    }
}

#[test]
fn a_rate_limit_says_so_rather_than_blaming_the_network() {
    let error = SvcError::RateLimited("You've exceeded the Rate Limit.".to_string());
    assert_eq!(error.user_message(), "Rate limited, retrying later");
}

// ---------------------------------------------------------------------------
// Which entries get drawn
// ---------------------------------------------------------------------------

#[test]
fn nothing_configured_and_nothing_saved_is_the_scripts_own_three() {
    assert_eq!(
        resolve_entries(None, &[]),
        vec![
            Entry::Single(Asset::Btc),
            Entry::Single(Asset::Eth),
            Entry::Pair(Asset::Eth, Asset::Btc),
        ]
    );
}

#[test]
fn the_config_beats_the_default() {
    assert_eq!(
        resolve_entries(None, &owned(&["xmr", "xmr/eth"])),
        vec![
            Entry::Single(Asset::Xmr),
            Entry::Pair(Asset::Xmr, Asset::Eth)
        ]
    );
}

#[test]
fn what_the_settings_view_saved_beats_the_config() {
    assert_eq!(
        resolve_entries(Some(&owned(&["btc"])), &owned(&["xmr", "xmr/eth"])),
        vec![Entry::Single(Asset::Btc)],
        "a choice made by hand is the whole point of the settings view"
    );
}

#[test]
fn a_saved_empty_list_means_the_user_turned_everything_off() {
    assert!(
        resolve_entries(Some(&[]), &owned(&["btc", "eth"])).is_empty(),
        "having saved nothing is different from never having saved"
    );
}

#[test]
fn one_unreadable_entry_costs_only_itself() {
    assert_eq!(
        resolve_entries(None, &owned(&["btc", "doge", "btc/btc", "eth/btc"])),
        vec![
            Entry::Single(Asset::Btc),
            Entry::Pair(Asset::Eth, Asset::Btc)
        ],
    );
}

#[test]
fn a_config_of_nothing_but_rubbish_falls_through_to_the_default() {
    assert_eq!(
        resolve_entries(None, &owned(&["doge", "sol/eth"])),
        resolve_entries(None, &[]),
        "an entirely invalid list is the same as no list"
    );
}

#[test]
fn the_same_entry_twice_is_drawn_once() {
    assert_eq!(
        resolve_entries(None, &owned(&["btc", "BTC", " btc "])),
        vec![Entry::Single(Asset::Btc)]
    );
}

#[test]
fn the_refresh_interval_comes_out_of_the_config_section() {
    assert_eq!(
        interval(&CryptoConfig::default()),
        Duration::from_secs(1800)
    );
    let config = CryptoConfig {
        interval: 60,
        ..CryptoConfig::default()
    };
    assert_eq!(
        interval(&config),
        Duration::from_secs(60),
        "the config minimum is passed through, not clamped again"
    );
}

#[test]
fn the_default_entries_are_the_ones_the_config_schema_advertises() {
    // `CryptoConfig::default()` is what `--print-example-config` shows and what
    // the config's own validation is written against; the service must agree
    // with it or a user reading the example gets something else.
    assert_eq!(
        resolve_entries(None, &CryptoConfig::default().entries),
        resolve_entries(None, &[]),
    );
    assert_eq!(CryptoConfig::default().entries, owned(&DEFAULT_ENTRIES));
}
