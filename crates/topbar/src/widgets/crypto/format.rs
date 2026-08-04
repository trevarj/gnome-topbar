//! Turning prices into the few characters a panel has room for.
//!
//! Every function here is pure and table-tested, because the whole widget is
//! four numbers and the only thing that can be wrong about it is how they are
//! written.
//!
//! Three registers:
//!
//! ```text
//!  compact   the bar          103.4k   3,412   235   0.033
//!  full      the popover      $103,412.44      $235.56
//!  chip      the change       +2.4%    −1.3%   —
//! ```

use topbar_services::{Entry, EntryQuote};

/// What stands in for a number that is not there. An em dash, not a hyphen.
pub const DASH: &str = "—";
/// The sign a rise carries in the tooltip.
const UP: char = '▲';
/// The sign a fall carries in the tooltip.
const DOWN: char = '▼';
/// The minus a change chip uses: U+2212, which lines up with `+` at the same
/// width. A hyphen next to a plus reads as a dash rather than a sign.
const MINUS: char = '−';
/// What separates a currency sign from the number it qualifies: U+2009.
///
/// `₿` is not in the panel's own font stack and arrives from whatever fallback
/// the system has; the one it lands on advances by less than the glyph is wide,
/// so `₿0.033` printed the sign straight through the leading zero. A thin space
/// costs nothing next to `$` and rescues the one that needs it.
const HAIR: char = '\u{2009}';

/// A price, in the fewest characters that still say what it is.
///
/// ```text
///  >= 10,000   103.4k     a bar has no room for six digits
///  >=  1,000   3,412      four digits, grouped
///  >=    100   235        cents on a three-figure price are noise
///  >=      1   23.45
///  <       1   0.4213
/// ```
pub fn compact(value: f64) -> String {
    if !value.is_finite() || value < 0.0 {
        return DASH.to_string();
    }
    if value >= 10_000.0 {
        return format!("{:.1}k", value / 1000.0);
    }
    // Rounded before it is classified, so 999.6 is grouped as the 1,000 it is
    // about to be written as rather than as the 999 it no longer is.
    let whole = value.round();
    if whole >= 1000.0 {
        return group(whole as u64);
    }
    if value >= 100.0 {
        return format!("{whole}");
    }
    if value >= 1.0 {
        return format!("{value:.2}");
    }
    format!("{value:.4}")
}

/// A ratio, at three decimals — the format the shell script this widget
/// replaces printed, and the one a pair is legible in.
///
/// A ratio of one or more is a price in disguise (BTC/ETH is about thirty) and
/// is written like one. A ratio so small that three decimals would show nothing
/// but zeros gets as many more as it takes, so a pair is never drawn as
/// `0.000`.
pub fn ratio(value: f64) -> String {
    if !value.is_finite() || value < 0.0 {
        return DASH.to_string();
    }
    if value >= 1.0 {
        return compact(value);
    }
    let mut decimals = 3;
    while decimals < 8 && value > 0.0 && value < 0.5 * 10_f64.powi(-decimals) {
        decimals += 1;
    }
    format!("{value:.*}", decimals as usize)
}

/// What one entry says on the bar.
pub fn bar_value(entry: Entry, quote: Option<EntryQuote>) -> String {
    let Some(quote) = quote else {
        return DASH.to_string();
    };
    match entry {
        Entry::Single(_) => compact(quote.value),
        Entry::Pair(..) => ratio(quote.value),
    }
}

/// What one entry says on a popover row: the value with its currency sign.
///
/// A single asset is priced in dollars; a pair is priced in whatever it is a
/// ratio *of*, which is what `₿ 0.033` means and why the sign is the
/// denominator's rather than always a dollar.
pub fn row_value(entry: Entry, quote: Option<EntryQuote>) -> String {
    let Some(quote) = quote else {
        return DASH.to_string();
    };
    match entry {
        Entry::Single(_) => format!("${}", full(quote.value)),
        Entry::Pair(_, denominator) => {
            format!("{}{HAIR}{}", denominator.sign(), ratio(quote.value))
        }
    }
}

/// A price written out rather than abbreviated, for a surface with room.
pub fn full(value: f64) -> String {
    if !value.is_finite() || value < 0.0 {
        return DASH.to_string();
    }
    if value >= 1000.0 {
        return group(value.round() as u64);
    }
    if value >= 1.0 {
        return format!("{value:.2}");
    }
    format!("{value:.4}")
}

/// The 24-hour change as a chip: `+2.4%`, `−1.3%`, or a dash.
///
/// Rounded before its sign is decided, so a change of +0.02% is written `0.0%`
/// rather than `+0.0%`, which claims a rise the figure does not show.
pub fn change_chip(change: Option<f64>) -> String {
    match rounded(change) {
        None => DASH.to_string(),
        Some(value) if value > 0.0 => format!("+{value:.1}%"),
        Some(value) if value < 0.0 => format!("{MINUS}{:.1}%", value.abs()),
        Some(_) => "0.0%".to_string(),
    }
}

/// Which way a change chip should be tinted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Up: tinted with the success colour.
    Up,
    /// Down: tinted with the urgent colour.
    Down,
    /// Flat, or not reported: no tint at all.
    Flat,
}

/// Which way `change` went, once rounded to what the chip actually shows.
pub fn direction(change: Option<f64>) -> Direction {
    match rounded(change) {
        Some(value) if value > 0.0 => Direction::Up,
        Some(value) if value < 0.0 => Direction::Down,
        _ => Direction::Flat,
    }
}

/// One tooltip line: `BTC $103,412 ▲2.4%`.
pub fn tooltip_line(entry: Entry, quote: Option<EntryQuote>) -> String {
    let Some(quote) = quote else {
        return format!("{} {DASH}", entry.label());
    };
    let value = row_value(entry, Some(quote));
    match rounded(quote.change_24h) {
        None => format!("{} {value}", entry.label()),
        Some(change) if change > 0.0 => {
            format!("{} {value} {UP}{change:.1}%", entry.label())
        }
        Some(change) if change < 0.0 => {
            format!("{} {value} {DOWN}{:.1}%", entry.label(), change.abs())
        }
        Some(_) => format!("{} {value} 0.0%", entry.label()),
    }
}

/// A change rounded to the one decimal a chip shows, or `None`.
fn rounded(change: Option<f64>) -> Option<f64> {
    let change = change.filter(|change| change.is_finite())?;
    Some((change * 10.0).round() / 10.0)
}

/// Group an integer with commas: `103412` becomes `103,412`.
fn group(value: u64) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// How many of `values` fit in `max_chars`, drawn side by side.
///
/// `widgets.crypto.max_chars` is a promise about the *label*, so it is measured
/// the way the weather widget measures its own: in characters, in Rust, before
/// anything reaches Pango. The one difference is what happens when the promise
/// cannot be kept — a crypto label is a row of icons and numbers rather than
/// one string, so it cannot be cut mid-word. Whole entries are dropped from the
/// end instead and an ellipsis takes their place, which keeps every icon
/// standing next to the number it belongs to.
///
/// Returns how many entries to draw and whether to append an ellipsis.
pub fn fit(values: &[String], max_chars: Option<usize>) -> (usize, bool) {
    let Some(max_chars) = max_chars.filter(|max| *max > 0) else {
        return (values.len(), false);
    };
    if width(values, values.len(), false) <= max_chars {
        return (values.len(), false);
    }
    // One fewer at a time until what is left, plus the ellipsis that admits to
    // the rest, fits.
    for kept in (0..values.len()).rev() {
        if width(values, kept, true) <= max_chars {
            return (kept, true);
        }
    }
    (0, true)
}

/// How wide the first `kept` values are, joined by single spaces.
fn width(values: &[String], kept: usize, ellipsis: bool) -> usize {
    let mut parts: usize = 0;
    let mut chars: usize = 0;
    for value in values.iter().take(kept) {
        parts += 1;
        chars += value.chars().count();
    }
    if ellipsis {
        parts += 1;
        chars += 1;
    }
    chars + parts.saturating_sub(1)
}

#[cfg(test)]
mod tests {
    use topbar_services::Asset;

    use super::*;

    fn quote(value: f64, change: Option<f64>) -> Option<EntryQuote> {
        Some(EntryQuote {
            value,
            change_24h: change,
        })
    }

    #[test]
    fn a_price_is_written_in_the_register_its_size_calls_for() {
        let table = [
            (103_412.44, "103.4k"),
            (10_000.0, "10.0k"),
            (9_999.4, "9,999"),
            (3_412.09, "3,412"),
            (1_000.0, "1,000"),
            (999.6, "1,000"),
            (234.56, "235"),
            (100.0, "100"),
            (99.994, "99.99"),
            (23.456, "23.46"),
            (1.0, "1.00"),
            (0.4213, "0.4213"),
            (0.0, "0.0000"),
        ];
        for (value, expected) in table {
            assert_eq!(compact(value), expected, "compact({value})");
        }
    }

    #[test]
    fn a_price_that_is_not_a_price_is_a_dash() {
        assert_eq!(compact(f64::NAN), DASH);
        assert_eq!(compact(f64::INFINITY), DASH);
        assert_eq!(compact(-1.0), DASH);
        assert_eq!(full(f64::NAN), DASH);
        assert_eq!(ratio(f64::NAN), DASH);
    }

    #[test]
    fn a_ratio_is_three_decimals() {
        let table = [
            (0.032_995, "0.033"),
            (0.052, "0.052"),
            (0.002_268, "0.002"),
            (0.5, "0.500"),
            (0.999_9, "1.000"),
        ];
        for (value, expected) in table {
            assert_eq!(ratio(value), expected, "ratio({value})");
        }
    }

    #[test]
    fn a_ratio_too_small_for_three_decimals_gets_more() {
        // BTC priced in a hypothetical asset worth thousands of bitcoin: three
        // decimals would say "0.000", which is not a number anyone wants.
        assert_eq!(ratio(0.000_023), "0.00002");
        assert_eq!(ratio(0.000_4), "0.0004");
        assert_eq!(ratio(0.000_000_000_1), "0.00000000");
    }

    #[test]
    fn a_ratio_of_more_than_one_is_a_price_in_disguise() {
        // BTC/ETH is about thirty; three decimals on it would be absurd.
        assert_eq!(ratio(30.3), "30.30");
        assert_eq!(ratio(1.0), "1.00");
    }

    #[test]
    fn a_price_written_in_full_keeps_its_digits() {
        assert_eq!(full(103_412.44), "103,412");
        assert_eq!(full(1_234_567.0), "1,234,567");
        assert_eq!(full(234.56), "234.56");
        assert_eq!(full(0.4213), "0.4213");
    }

    #[test]
    fn the_bar_writes_singles_and_pairs_differently() {
        assert_eq!(
            bar_value(Entry::Single(Asset::Btc), quote(103_412.44, None)),
            "103.4k"
        );
        assert_eq!(
            bar_value(Entry::Pair(Asset::Eth, Asset::Btc), quote(0.032_995, None)),
            "0.033"
        );
        assert_eq!(bar_value(Entry::Single(Asset::Btc), None), DASH);
    }

    #[test]
    fn a_popover_row_carries_the_sign_of_what_it_is_priced_in() {
        assert_eq!(
            row_value(Entry::Single(Asset::Btc), quote(103_412.44, None)),
            "$103,412"
        );
        assert_eq!(
            row_value(Entry::Pair(Asset::Eth, Asset::Btc), quote(0.032_995, None)),
            "₿\u{2009}0.033"
        );
        assert_eq!(
            row_value(Entry::Pair(Asset::Xmr, Asset::Eth), quote(0.068_74, None)),
            "Ξ\u{2009}0.069"
        );
        assert_eq!(row_value(Entry::Single(Asset::Btc), None), DASH);
    }

    #[test]
    fn a_change_chip_signs_itself_after_it_is_rounded() {
        assert_eq!(change_chip(Some(2.4137)), "+2.4%");
        assert_eq!(change_chip(Some(-1.2984)), "−1.3%");
        assert_eq!(change_chip(Some(0.0)), "0.0%");
        assert_eq!(
            change_chip(Some(0.02)),
            "0.0%",
            "a rise too small to show must not claim a sign"
        );
        assert_eq!(change_chip(Some(-0.02)), "0.0%");
        assert_eq!(change_chip(None), DASH);
        assert_eq!(change_chip(Some(f64::NAN)), DASH);
    }

    #[test]
    fn a_chip_is_tinted_by_what_it_ended_up_showing() {
        assert_eq!(direction(Some(2.4137)), Direction::Up);
        assert_eq!(direction(Some(-1.2984)), Direction::Down);
        assert_eq!(direction(Some(0.0)), Direction::Flat);
        assert_eq!(direction(Some(0.02)), Direction::Flat);
        assert_eq!(direction(None), Direction::Flat);
    }

    #[test]
    fn a_tooltip_line_is_the_entry_its_value_and_which_way_it_went() {
        assert_eq!(
            tooltip_line(Entry::Single(Asset::Btc), quote(103_412.44, Some(2.4137))),
            "BTC $103,412 ▲2.4%"
        );
        assert_eq!(
            tooltip_line(Entry::Single(Asset::Eth), quote(3_412.09, Some(-1.2984))),
            "ETH $3,412 ▼1.3%"
        );
        assert_eq!(
            tooltip_line(
                Entry::Pair(Asset::Eth, Asset::Btc),
                quote(0.032_995, Some(-3.6247))
            ),
            "ETH / BTC ₿\u{2009}0.033 ▼3.6%"
        );
    }

    #[test]
    fn a_tooltip_line_with_no_change_simply_omits_it() {
        assert_eq!(
            tooltip_line(Entry::Single(Asset::Xmr), quote(234.56, None)),
            "XMR $234.56"
        );
    }

    #[test]
    fn a_tooltip_line_for_a_price_that_never_arrived_says_so() {
        assert_eq!(tooltip_line(Entry::Single(Asset::Xmr), None), "XMR —");
    }

    fn values() -> Vec<String> {
        ["103.4k", "3,412", "0.033"]
            .iter()
            .map(|value| (*value).to_string())
            .collect()
    }

    #[test]
    fn a_label_that_fits_keeps_every_entry() {
        // "103.4k 3,412 0.033" is 18 characters.
        assert_eq!(fit(&values(), Some(18)), (3, false));
        assert_eq!(fit(&values(), Some(40)), (3, false));
        assert_eq!(fit(&values(), None), (3, false));
        assert_eq!(fit(&values(), Some(0)), (3, false), "zero means unset");
    }

    #[test]
    fn a_label_that_does_not_fit_drops_whole_entries_from_the_end() {
        // "103.4k 3,412 …" is 14; "103.4k …" is 8; "…" is 1.
        assert_eq!(fit(&values(), Some(17)), (2, true));
        assert_eq!(fit(&values(), Some(14)), (2, true));
        assert_eq!(fit(&values(), Some(13)), (1, true));
        assert_eq!(fit(&values(), Some(8)), (1, true));
        assert_eq!(fit(&values(), Some(7)), (0, true));
        assert_eq!(fit(&values(), Some(1)), (0, true));
    }

    #[test]
    fn the_cut_counts_characters_rather_than_bytes() {
        // An em dash is three bytes and one character, which is the whole
        // point: counting bytes would call this eleven wide instead of nine.
        let wide = vec!["—".to_string(), "103.4k".to_string(), "—".to_string()];
        // "— 103.4k —" is ten characters and fourteen bytes.
        assert_eq!(fit(&wide, Some(10)), (3, false));
        // Two of them plus the ellipsis is "— 103.4k …", which is ten again, so
        // nine leaves room for one entry and the ellipsis: "— …".
        assert_eq!(fit(&wide, Some(9)), (1, true));
        assert_eq!(fit(&wide, Some(2)), (0, true));
    }

    #[test]
    fn an_empty_list_is_never_ellipsized() {
        assert_eq!(fit(&[], Some(10)), (0, false));
        assert_eq!(fit(&[], None), (0, false));
    }
}
