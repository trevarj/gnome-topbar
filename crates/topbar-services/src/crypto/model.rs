//! What the panel reads its prices from.
//!
//! Three assets, two shapes of entry, and one snapshot published through one
//! watch channel. Everything here is pure: the ratio a pair is worth and the
//! 24-hour change that goes with it are worked out from the dollar prices the
//! service fetched, never asked for separately.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// An asset the widget can price.
///
/// A closed set on purpose. Every one of them costs a logo in `assets/crypto`,
/// a line in the settings view, and a name in the config schema, and the plan
/// is explicit that these three are the surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Asset {
    /// Bitcoin.
    Btc,
    /// Ethereum.
    Eth,
    /// Monero.
    Xmr,
}

impl Asset {
    /// Every asset, in the order the settings view lists them.
    pub const ALL: [Self; 3] = [Self::Btc, Self::Eth, Self::Xmr];

    /// What CoinGecko calls it.
    pub fn id(self) -> &'static str {
        match self {
            Self::Btc => "bitcoin",
            Self::Eth => "ethereum",
            Self::Xmr => "monero",
        }
    }

    /// The ticker, as the bar and the tooltip write it.
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Btc => "BTC",
            Self::Eth => "ETH",
            Self::Xmr => "XMR",
        }
    }

    /// The name a person would use.
    pub fn name(self) -> &'static str {
        match self {
            Self::Btc => "Bitcoin",
            Self::Eth => "Ethereum",
            Self::Xmr => "Monero",
        }
    }

    /// The currency sign a value denominated in this asset carries.
    ///
    /// The pair rows use it the way the user's shell script did: `₿0.033` says
    /// "this many bitcoin" without a second word of explanation.
    pub fn sign(self) -> &'static str {
        match self {
            Self::Btc => "₿",
            Self::Eth => "Ξ",
            Self::Xmr => "ɱ",
        }
    }

    /// The lowercase name a config entry or a state file writes.
    pub fn key(self) -> &'static str {
        match self {
            Self::Btc => "btc",
            Self::Eth => "eth",
            Self::Xmr => "xmr",
        }
    }
}

impl FromStr for Asset {
    type Err = EntryError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "btc" => Ok(Self::Btc),
            "eth" => Ok(Self::Eth),
            "xmr" => Ok(Self::Xmr),
            _ => Err(EntryError::UnknownAsset(value.trim().to_string())),
        }
    }
}

impl fmt::Display for Asset {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.key())
    }
}

/// Why a string is not an entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryError {
    /// The whole entry, or one side of a pair, was blank.
    Empty,
    /// A name that is not one of the three assets.
    UnknownAsset(String),
    /// `btc/btc`, which is the number one dressed up as a widget.
    SelfPair(Asset),
    /// More than one slash: `eth/btc/xmr` is not a thing.
    TooManyParts(String),
}

impl fmt::Display for EntryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(formatter, "an entry cannot be empty"),
            Self::UnknownAsset(name) => write!(formatter, "`{name}` is not one of btc, eth, xmr",),
            Self::SelfPair(asset) => write!(
                formatter,
                "`{asset}/{asset}` is always 1; pair two different assets",
            ),
            Self::TooManyParts(value) => {
                write!(formatter, "`{value}` has more than one `/`")
            }
        }
    }
}

impl std::error::Error for EntryError {}

/// One thing the widget draws: a price, or a ratio between two prices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Entry {
    /// One asset's dollar price.
    Single(Asset),
    /// How many of the second asset one of the first is worth.
    Pair(Asset, Asset),
}

impl Entry {
    /// The assets this entry needs a quote for.
    pub fn assets(self) -> Vec<Asset> {
        match self {
            Self::Single(asset) => vec![asset],
            Self::Pair(base, quote) => vec![base, quote],
        }
    }

    /// The asset whose logo leads the entry.
    pub fn leading(self) -> Asset {
        match self {
            Self::Single(asset) | Self::Pair(asset, _) => asset,
        }
    }

    /// The asset a pair is denominated in, if this is a pair.
    pub fn denominator(self) -> Option<Asset> {
        match self {
            Self::Single(_) => None,
            Self::Pair(_, quote) => Some(quote),
        }
    }

    /// How the entry is written on screen: `BTC`, or `ETH / BTC`.
    pub fn label(self) -> String {
        match self {
            Self::Single(asset) => asset.symbol().to_string(),
            Self::Pair(base, quote) => format!("{} / {}", base.symbol(), quote.symbol()),
        }
    }
}

impl FromStr for Entry {
    type Err = EntryError;

    /// Read `btc` or `eth/btc`, in any casing and with space around it.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = value.trim();
        if value.is_empty() {
            return Err(EntryError::Empty);
        }
        let mut parts = value.split('/');
        let first = parts.next().unwrap_or_default();
        let Some(second) = parts.next() else {
            return Ok(Self::Single(first.parse()?));
        };
        if parts.next().is_some() {
            return Err(EntryError::TooManyParts(value.to_string()));
        }
        if first.trim().is_empty() || second.trim().is_empty() {
            return Err(EntryError::Empty);
        }
        let base: Asset = first.parse()?;
        let quote: Asset = second.parse()?;
        if base == quote {
            return Err(EntryError::SelfPair(base));
        }
        Ok(Self::Pair(base, quote))
    }
}

impl fmt::Display for Entry {
    /// The form a config file and the state file write.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Single(asset) => formatter.write_str(asset.key()),
            Self::Pair(base, quote) => write!(formatter, "{}/{}", base.key(), quote.key()),
        }
    }
}

/// What one asset costs, and what it did in the last day.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quote {
    /// The price in US dollars.
    pub usd: f64,
    /// The change over 24 hours as a percentage, when the API reported one.
    /// `2.4` is "up 2.4%".
    pub change_24h: Option<f64>,
}

/// What one *entry* is worth, which for a pair is worked out rather than read.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntryQuote {
    /// Dollars for a single asset; a ratio for a pair.
    pub value: f64,
    /// The change over 24 hours as a percentage, when it could be worked out.
    pub change_24h: Option<f64>,
}

/// The 24-hour change of a ratio, from the changes of its two sides.
///
/// If A moved by `base` percent and B by `quote` percent, then the ratio A/B
/// moved by `(1 + base) / (1 + quote) - 1` — the dollars cancel, which is the
/// whole reason a pair needs no request of its own.
///
/// `None` when the arithmetic has nothing to say: a denominator that lost all
/// of its value 24 hours ago would divide by zero, and a non-finite figure from
/// the API must not turn into a non-finite figure on the panel.
pub fn pair_change(base: f64, quote: f64) -> Option<f64> {
    if !base.is_finite() || !quote.is_finite() {
        return None;
    }
    let denominator = 1.0 + quote / 100.0;
    if denominator.abs() < f64::EPSILON {
        return None;
    }
    let change = ((1.0 + base / 100.0) / denominator - 1.0) * 100.0;
    change.is_finite().then_some(change)
}

/// What the panel should be drawing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Phase {
    /// Nothing has arrived yet and nothing has failed. Only ever seen once per
    /// start: a refresh keeps whatever is on screen.
    #[default]
    Loading,
    /// Fresh prices.
    Ready,
    /// The last good prices, kept on screen because half-hour-old numbers beat
    /// an empty widget. [`CryptoState::fetched_at`] says how old they are.
    Stale,
    /// A fetch failed and none has ever succeeded, so there is nothing to keep.
    Unavailable,
}

/// The published price snapshot.
///
/// `quotes` always covers every asset the last fetch returned, not merely the
/// configured ones: turning Monero on in the settings view is then a redraw
/// rather than a round trip.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CryptoState {
    /// What to draw.
    pub phase: Phase,
    /// The price of each asset the last successful fetch covered.
    pub quotes: BTreeMap<Asset, Quote>,
    /// The entries to draw, resolved from the state file, the config, and the
    /// default — in that order. This is the *effective* list.
    pub entries: Vec<Entry>,
    /// When the quotes in hand were fetched.
    pub fetched_at: Option<SystemTime>,
}

impl CryptoState {
    /// What `entry` is worth, or `None` if a price it needs is missing.
    pub fn quote(&self, entry: Entry) -> Option<EntryQuote> {
        match entry {
            Entry::Single(asset) => {
                let quote = self.quotes.get(&asset)?;
                quote.usd.is_finite().then_some(EntryQuote {
                    value: quote.usd,
                    change_24h: quote.change_24h,
                })
            }
            Entry::Pair(base, quote) => {
                let base = self.quotes.get(&base)?;
                let quote = self.quotes.get(&quote)?;
                if !base.usd.is_finite() || !quote.usd.is_finite() || quote.usd == 0.0 {
                    return None;
                }
                let change = base
                    .change_24h
                    .zip(quote.change_24h)
                    .and_then(|(base, quote)| pair_change(base, quote));
                Some(EntryQuote {
                    value: base.usd / quote.usd,
                    change_24h: change,
                })
            }
        }
    }

    /// When the prices on screen were taken, if they are no longer current.
    pub fn stale_since(&self) -> Option<SystemTime> {
        match self.phase {
            Phase::Stale => self.fetched_at,
            _ => None,
        }
    }

    /// Whether there is nothing at all to show.
    pub fn is_unavailable(&self) -> bool {
        matches!(self.phase, Phase::Unavailable)
    }

    /// Whether the panel is waiting for its very first prices.
    pub fn is_loading(&self) -> bool {
        matches!(self.phase, Phase::Loading)
    }
}

/// The entries the settings view last saved, as `state.json` keeps them.
///
/// Stored as the same strings the config file uses rather than as a typed list:
/// a state file written by a build that knew about a fourth asset then loads
/// here without taking the whole document down with it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PersistedCrypto {
    /// `None` means the user has never touched the settings view, which is what
    /// lets `[widgets.crypto] entries` still be the seed. An empty list is a
    /// deliberate "show nothing" and is honoured.
    pub entries: Option<Vec<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_asset_parses_in_any_casing() {
        assert_eq!("btc".parse::<Entry>(), Ok(Entry::Single(Asset::Btc)));
        assert_eq!("ETH".parse::<Entry>(), Ok(Entry::Single(Asset::Eth)));
        assert_eq!("  Xmr ".parse::<Entry>(), Ok(Entry::Single(Asset::Xmr)));
    }

    #[test]
    fn a_pair_parses_in_any_casing() {
        assert_eq!(
            "eth/btc".parse::<Entry>(),
            Ok(Entry::Pair(Asset::Eth, Asset::Btc))
        );
        assert_eq!(
            "XMR / ETH".parse::<Entry>(),
            Ok(Entry::Pair(Asset::Xmr, Asset::Eth))
        );
    }

    #[test]
    fn an_unknown_asset_is_refused_by_name() {
        assert_eq!(
            "doge".parse::<Entry>(),
            Err(EntryError::UnknownAsset("doge".to_string()))
        );
        assert_eq!(
            "eth/doge".parse::<Entry>(),
            Err(EntryError::UnknownAsset("doge".to_string()))
        );
        assert!(
            "doge"
                .parse::<Entry>()
                .unwrap_err()
                .to_string()
                .contains("doge"),
            "the message has to name the entry that was wrong"
        );
    }

    #[test]
    fn a_pair_of_one_asset_with_itself_is_refused() {
        assert_eq!(
            "btc/btc".parse::<Entry>(),
            Err(EntryError::SelfPair(Asset::Btc))
        );
    }

    #[test]
    fn nothing_and_half_a_pair_are_refused() {
        assert_eq!("".parse::<Entry>(), Err(EntryError::Empty));
        assert_eq!("   ".parse::<Entry>(), Err(EntryError::Empty));
        assert_eq!("btc/".parse::<Entry>(), Err(EntryError::Empty));
        assert_eq!("/btc".parse::<Entry>(), Err(EntryError::Empty));
    }

    #[test]
    fn a_ratio_of_ratios_is_refused() {
        assert_eq!(
            "eth/btc/xmr".parse::<Entry>(),
            Err(EntryError::TooManyParts("eth/btc/xmr".to_string()))
        );
    }

    #[test]
    fn an_entry_round_trips_through_its_written_form() {
        for text in ["btc", "eth", "xmr", "eth/btc", "xmr/eth"] {
            let entry: Entry = text.parse().expect("a valid entry");
            assert_eq!(entry.to_string(), text);
        }
    }

    #[test]
    fn a_pair_is_labelled_with_both_tickers() {
        assert_eq!(Entry::Single(Asset::Btc).label(), "BTC");
        assert_eq!(Entry::Pair(Asset::Eth, Asset::Btc).label(), "ETH / BTC");
    }

    #[test]
    fn an_entry_knows_which_prices_it_needs() {
        assert_eq!(Entry::Single(Asset::Btc).assets(), vec![Asset::Btc]);
        assert_eq!(
            Entry::Pair(Asset::Eth, Asset::Btc).assets(),
            vec![Asset::Eth, Asset::Btc]
        );
        assert_eq!(Entry::Pair(Asset::Eth, Asset::Btc).leading(), Asset::Eth);
        assert_eq!(
            Entry::Pair(Asset::Eth, Asset::Btc).denominator(),
            Some(Asset::Btc)
        );
        assert_eq!(Entry::Single(Asset::Btc).denominator(), None);
    }

    /// The identity the derivation rests on: if both sides moved by the same
    /// percentage, their ratio did not move at all.
    #[test]
    fn a_ratio_whose_sides_moved_together_did_not_move() {
        let change = pair_change(5.0, 5.0).expect("a figure");
        assert!(change.abs() < 1e-9, "{change} should be zero");
    }

    #[test]
    fn a_ratio_change_is_derived_from_both_sides() {
        // ETH -1.2984%, BTC +2.4137% => 0.987016 / 1.024137 - 1.
        let change = pair_change(-1.2984, 2.4137).expect("a figure");
        assert!((change - -3.624_667).abs() < 1e-4, "got {change}");

        // The other way round is the mirror image, near enough.
        let back = pair_change(2.4137, -1.2984).expect("a figure");
        assert!((back - 3.761_015).abs() < 1e-4, "got {back}");
    }

    #[test]
    fn a_denominator_that_lost_everything_has_no_ratio_change() {
        assert_eq!(pair_change(1.0, -100.0), None);
        assert_eq!(pair_change(f64::NAN, 1.0), None);
        assert_eq!(pair_change(1.0, f64::INFINITY), None);
    }

    fn state() -> CryptoState {
        CryptoState {
            phase: Phase::Ready,
            quotes: BTreeMap::from([
                (
                    Asset::Btc,
                    Quote {
                        usd: 103_412.44,
                        change_24h: Some(2.4137),
                    },
                ),
                (
                    Asset::Eth,
                    Quote {
                        usd: 3_412.09,
                        change_24h: Some(-1.2984),
                    },
                ),
            ]),
            entries: Vec::new(),
            fetched_at: None,
        }
    }

    #[test]
    fn a_single_entry_is_quoted_in_dollars() {
        let quote = state().quote(Entry::Single(Asset::Btc)).expect("a price");
        assert!((quote.value - 103_412.44).abs() < 1e-6);
        assert_eq!(quote.change_24h, Some(2.4137));
    }

    #[test]
    fn a_pair_is_quoted_as_a_ratio_of_the_two_prices() {
        let quote = state()
            .quote(Entry::Pair(Asset::Eth, Asset::Btc))
            .expect("a ratio");
        assert!(
            (quote.value - 0.032_995).abs() < 1e-5,
            "got {}",
            quote.value
        );
        let change = quote.change_24h.expect("a derived change");
        assert!((change - -3.624_667).abs() < 1e-4, "got {change}");
    }

    #[test]
    fn an_entry_whose_price_is_missing_has_no_quote() {
        let state = state();
        assert_eq!(state.quote(Entry::Single(Asset::Xmr)), None);
        assert_eq!(state.quote(Entry::Pair(Asset::Xmr, Asset::Btc)), None);
        assert_eq!(state.quote(Entry::Pair(Asset::Btc, Asset::Xmr)), None);
    }

    #[test]
    fn a_pair_with_a_missing_change_still_has_a_ratio() {
        let mut state = state();
        state.quotes.insert(
            Asset::Btc,
            Quote {
                usd: 103_412.44,
                change_24h: None,
            },
        );
        let quote = state
            .quote(Entry::Pair(Asset::Eth, Asset::Btc))
            .expect("a ratio");
        assert!(quote.value > 0.0);
        assert_eq!(quote.change_24h, None, "half a change is no change");
    }

    #[test]
    fn a_worthless_denominator_has_no_ratio_at_all() {
        let mut state = state();
        state.quotes.insert(
            Asset::Btc,
            Quote {
                usd: 0.0,
                change_24h: None,
            },
        );
        assert_eq!(state.quote(Entry::Pair(Asset::Eth, Asset::Btc)), None);
    }

    #[test]
    fn only_a_stale_snapshot_reports_an_age() {
        let mut state = state();
        state.fetched_at = Some(SystemTime::UNIX_EPOCH);
        assert_eq!(state.stale_since(), None);
        assert!(!state.is_unavailable());
        assert!(!state.is_loading());

        state.phase = Phase::Stale;
        assert_eq!(state.stale_since(), Some(SystemTime::UNIX_EPOCH));

        state.phase = Phase::Unavailable;
        assert_eq!(state.stale_since(), None);
        assert!(state.is_unavailable());
    }

    #[test]
    fn a_panel_that_has_fetched_nothing_yet_is_loading() {
        assert_eq!(CryptoState::default().phase, Phase::Loading);
        assert!(CryptoState::default().is_loading());
    }
}
