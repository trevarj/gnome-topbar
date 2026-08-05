//! The crypto widget: a logo and a number per entry.
//!
//! ```text
//!  ₿ 103.4k  Ξ 3,412  Ξ₿ 0.033       click → the price popover
//! ```
//!
//! An entry is one asset priced in dollars, or a pair priced in the other —
//! `eth/btc` is drawn as the Ethereum logo with a small Bitcoin one on its
//! shoulder, which is what says the `0.033` is bitcoin rather than dollars.
//!
//! Everything it draws comes out of the one crypto service, including *which*
//! entries to draw: the settings view writes them into the snapshot, so the bar
//! and the popover can never disagree about what is configured. The widget owns
//! no timer and no cache.

mod format;
mod icons;
mod popover;
mod settings;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::SystemTime;

use gtk4::prelude::*;
use gtk4::{Align, Image, Label, Orientation, Overlay};
use topbar_core::config::CryptoConfig;
use topbar_services::crypto::Phase;
use topbar_services::{CryptoState, Entry, Services};

use crate::bar::BarContext;
use crate::bridge::{self, BindingGuard};
use crate::style::classes;
use crate::surfaces::popovers::{self, PopoverContent, PopoverHandle};
use crate::surfaces::tooltip::TooltipHandle;
use crate::widgets::crypto::popover::Prices;
use crate::widgets::shell::WidgetShell;
use crate::widgets::weather::forecast::age;

/// Widget name, for CSS classes and the popover registry.
const WIDGET_NAME: &str = "crypto";
/// Logo size on the bar, in pixels. A pair's two coins each take 5/8 of this.
const BAR_ICON: i32 = 16;
/// Shown while the first prices are on their way.
const LOADING_LABEL: &str = "…";
/// What stands in for the entries that did not fit in `max_chars`.
const ELLIPSIS: &str = "…";

/// The crypto widget.
pub struct CryptoWidget {
    shell: WidgetShell,
    /// Holds the rows, the tooltip, and the entries they were built from.
    _inner: Rc<Inner>,
    /// The popover's claim on the host.
    _popover: PopoverHandle,
    /// Keeps the rows subscribed to the service.
    _binding: BindingGuard,
}

impl CryptoWidget {
    /// Build the widget from `[widgets.crypto]`.
    pub fn new(config: &CryptoConfig, context: &BarContext) -> Self {
        let shell = WidgetShell::new(classes::CRYPTO);
        shell.make_interactive();

        let loading = Label::new(Some(LOADING_LABEL));
        shell.content().append(&loading);

        let ellipsis = Label::new(Some(ELLIPSIS));
        ellipsis.set_visible(false);
        shell.content().append(&ellipsis);

        let inner = Rc::new(Inner {
            wrapper: shell.root().clone(),
            content: shell.content().clone(),
            loading,
            ellipsis,
            rows: RefCell::new(Vec::new()),
            tooltip: shell.set_tooltip(&config.tooltip),
            max_chars: config.max_chars.map(|max| max as usize),
            fallback_tooltip: config.tooltip.clone(),
        });

        let binding = bridge::bind_state(shell.root(), context.services.crypto.state(), {
            let inner = Rc::downgrade(&inner);
            move |_: &gtk4::Box, state: &CryptoState| {
                if let Some(inner) = inner.upgrade() {
                    inner.render(state, SystemTime::now());
                }
            }
        });

        // The popover content is built lazily by the registry and kept for the
        // widget's lifetime; this is how the smoke hook gets hold of the same
        // instance without building a second one.
        let content: Rc<RefCell<Option<Rc<Prices>>>> = Rc::new(RefCell::new(None));
        let popover = {
            let services = context.services.clone();
            let interval = topbar_services::crypto::interval(config);
            let content = Rc::clone(&content);
            popovers::attach(context, WIDGET_NAME, shell.root(), move || {
                let prices = Prices::new(interval, &services);
                *content.borrow_mut() = Some(Rc::clone(&prices));
                prices as Rc<dyn PopoverContent>
            })
        };

        // `TOPBAR_SMOKE_OPEN=crypto-settings` opens the popover already switched
        // to its settings view. There is no synthetic pointer in the dev shell,
        // so a view only a click can reach could never be photographed. The
        // switch happens after the open, because opening refreshes the content
        // and a refresh deliberately lands on the prices.
        popovers::register_smoke_action(&format!("{WIDGET_NAME}-settings"), move || {
            popovers::dispatch(
                &topbar_core::ipc::PopoverAction::Show(WIDGET_NAME.to_string()),
                None,
            );
            if let Some(prices) = content.borrow().as_ref() {
                prices.show_settings();
            }
        });

        // `TOPBAR_SMOKE_OPEN=crypto-apply` does what switching Monero on in the
        // settings view does, so a run with no pointer in it can still prove
        // the whole write path: control → service → `state.json` → the bar
        // redrawing from the republished snapshot.
        popovers::register_smoke_action(&format!("{WIDGET_NAME}-apply"), {
            let services = context.services.clone();
            move || {
                let entries = services.crypto.state().borrow().entries.clone();
                let entries = settings::toggle_single(&entries, topbar_services::Asset::Xmr, true);
                let handle = services.crypto.handle().clone();
                bridge::act(
                    bridge::ActionScope::Toast {
                        widget: WIDGET_NAME,
                    },
                    async move { handle.set_entries(entries).await },
                );
            }
        });

        Self {
            shell,
            _inner: inner,
            _popover: popover,
            _binding: binding,
        }
    }

    /// The widget to put in a bar section.
    pub fn root(&self) -> gtk4::Widget {
        self.shell.root().clone().upcast()
    }
}

/// Everything the render closure touches.
struct Inner {
    /// The shell's outer box, which is what `.disconnected` dims.
    wrapper: gtk4::Box,
    /// Where the entry rows are appended.
    content: gtk4::Box,
    /// The single "…" the very first fetch shows.
    loading: Label,
    /// The "…" standing in for entries `max_chars` left no room for.
    ellipsis: Label,
    /// One per entry, rebuilt only when the entry list itself changes.
    rows: RefCell<Vec<Row>>,
    tooltip: TooltipHandle,
    /// `widgets.crypto.max_chars`.
    max_chars: Option<usize>,
    /// `widgets.crypto.tooltip`, shown until there is something better.
    fallback_tooltip: String,
}

impl Inner {
    /// Draw `state`.
    fn render(&self, state: &CryptoState, now: SystemTime) {
        self.sync_rows(&state.entries);

        if state.entries.is_empty() {
            // Nothing to price, but the widget must stay on the bar and stay
            // clickable — the settings view is the only way back.
            self.loading.set_visible(false);
            self.ellipsis.set_visible(false);
            self.set_disconnected(true);
            self.tooltip
                .set_text("No prices configured\nClick to choose some");
            return;
        }

        if state.is_loading() {
            self.loading.set_visible(true);
            self.ellipsis.set_visible(false);
            for row in self.rows.borrow().iter() {
                row.root.set_visible(false);
            }
            self.set_disconnected(false);
            self.tooltip.set_text(&self.fallback_tooltip);
            return;
        }
        self.loading.set_visible(false);

        // With nothing ever fetched there are no numbers to draw, so the logos
        // stand alone and dimmed: the widget says which prices it would show
        // and admits it has none of them.
        let unavailable = state.is_unavailable();
        self.set_disconnected(unavailable);

        let values: Vec<String> = state
            .entries
            .iter()
            .map(|entry| format::bar_value(*entry, state.quote(*entry)))
            .collect();
        let (kept, ellipsized) = if unavailable {
            (state.entries.len(), false)
        } else {
            format::fit(&values, self.max_chars)
        };

        for (index, row) in self.rows.borrow().iter().enumerate() {
            let visible = index < kept || unavailable;
            row.root.set_visible(visible);
            if !visible {
                continue;
            }
            row.value.set_visible(!unavailable);
            if !unavailable {
                set_text(&row.value, &values[index]);
            }
        }
        self.ellipsis.set_visible(ellipsized);

        self.tooltip.set_text(&self.tooltip_text(state, now));
    }

    /// The tooltip: one line per entry, plus the age of a stale reading.
    fn tooltip_text(&self, state: &CryptoState, now: SystemTime) -> String {
        if state.is_unavailable() {
            return "Prices are unavailable".to_string();
        }
        let mut lines: Vec<String> = state
            .entries
            .iter()
            .map(|entry| format::tooltip_line(*entry, state.quote(*entry)))
            .collect();
        // A stale price looks exactly like a fresh one on a panel this narrow,
        // so the tooltip is where its age is admitted to.
        if let Some(since) = state.stale_since() {
            lines.push(age(since, now));
        }
        lines.join("\n")
    }

    /// Rebuild the rows if `entries` is not what they were built from.
    ///
    /// Rare — a settings change or a config reload — so a rebuild is cheaper
    /// than the machinery that would avoid one, and it keeps the widget's shape
    /// a pure function of the entry list.
    fn sync_rows(&self, entries: &[Entry]) {
        {
            let rows = self.rows.borrow();
            if rows.len() == entries.len()
                && rows
                    .iter()
                    .zip(entries)
                    .all(|(row, entry)| row.entry == *entry)
            {
                return;
            }
        }

        let mut rows = self.rows.borrow_mut();
        for row in rows.drain(..) {
            self.content.remove(&row.root);
        }
        // The ellipsis always trails the rows it stands in for, so it is taken
        // off and put back rather than inserted around.
        self.content.remove(&self.ellipsis);
        for entry in entries {
            let row = Row::new(*entry);
            self.content.append(&row.root);
            rows.push(row);
        }
        self.content.append(&self.ellipsis);
    }

    /// Wear the panel's has-no-data treatment, or take it off.
    fn set_disconnected(&self, disconnected: bool) {
        if disconnected {
            self.wrapper.add_css_class(classes::DISCONNECTED);
        } else {
            self.wrapper.remove_css_class(classes::DISCONNECTED);
        }
    }
}

/// One entry on the bar: its logo (or two) and its number.
struct Row {
    entry: Entry,
    root: gtk4::Box,
    value: Label,
}

impl Row {
    fn new(entry: Entry) -> Self {
        let root = gtk4::Box::new(Orientation::Horizontal, 0);
        root.add_css_class(classes::CRYPTO_ENTRY);

        root.append(&emblem(entry, BAR_ICON));

        let value = Label::new(None);
        value.add_css_class(classes::CRYPTO_VALUE);
        root.append(&value);

        Self { entry, root, value }
    }
}

/// The logo (or the pair of logos) that leads an entry.
///
/// A pair is two coins on a diagonal: the numerator leads from the top-left
/// and overlaps the denominator sitting behind it on the lower-right — the
/// two-coin lockup every exchange draws for a trading pair. Each coin is 5/8
/// of the slot, which leaves them overlapping by about 40%: enough that they
/// read as one mark, not two entries.
pub fn emblem(entry: Entry, size: i32) -> gtk4::Widget {
    let Some(denominator) = entry.denominator() else {
        return icon(entry.leading(), size).upcast();
    };

    let coin = size * 5 / 8;
    let overlay = Overlay::new();
    // The overlay takes the whole slot; the coins place themselves inside it.
    overlay.set_size_request(size, size);

    let behind = icon(denominator, coin);
    behind.set_halign(Align::End);
    behind.set_valign(Align::End);
    overlay.set_child(Some(&behind));

    let front = icon(entry.leading(), coin);
    front.set_halign(Align::Start);
    front.set_valign(Align::Start);
    // The ring keeps the front coin's edge legible over the one behind it.
    front.add_css_class(classes::CRYPTO_BADGE);
    overlay.add_overlay(&front);
    overlay.upcast()
}

/// One asset's logo at `size` pixels.
fn icon(asset: topbar_services::Asset, size: i32) -> Image {
    let image = icons::image(asset, size);
    image.add_css_class(classes::CRYPTO_ICON);
    image
}

/// Set a label only when the text changed, which costs a bar relayout.
fn set_text(label: &Label, text: &str) {
    if label.text() != text {
        label.set_text(text);
    }
}

/// Ask the service for fresh prices.
fn refresh_now(services: &Services) {
    let handle = services.crypto.handle().clone();
    bridge::act(
        bridge::ActionScope::Toast {
            widget: WIDGET_NAME,
        },
        async move { handle.refresh_now().await },
    );
}

/// How old prices have to be before opening the popover refetches them.
///
/// The interval itself: anything younger is what the schedule intended to be on
/// screen, and asking again would only spend somebody else's rate limit.
fn is_overdue(state: &CryptoState, interval: std::time::Duration, now: SystemTime) -> bool {
    match (state.phase, state.fetched_at) {
        (Phase::Loading, _) => false,
        (Phase::Unavailable, _) | (_, None) => true,
        (_, Some(fetched)) => now
            .duration_since(fetched)
            .is_ok_and(|elapsed| elapsed >= interval),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use topbar_services::Asset;

    use super::*;

    const INTERVAL: Duration = Duration::from_secs(1800);

    fn state(phase: Phase, fetched_at: Option<SystemTime>) -> CryptoState {
        CryptoState {
            phase,
            quotes: Default::default(),
            entries: vec![Entry::Single(Asset::Btc)],
            fetched_at,
        }
    }

    #[test]
    fn prices_older_than_the_interval_are_refetched_when_the_popover_opens() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000_000);
        let fresh = state(Phase::Ready, Some(now - Duration::from_secs(60)));
        assert!(!is_overdue(&fresh, INTERVAL, now));

        let old = state(Phase::Ready, Some(now - INTERVAL));
        assert!(is_overdue(&old, INTERVAL, now));
    }

    #[test]
    fn a_widget_with_nothing_on_it_always_refetches() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000_000);
        assert!(is_overdue(&state(Phase::Unavailable, None), INTERVAL, now));
        assert!(is_overdue(&state(Phase::Ready, None), INTERVAL, now));
    }

    #[test]
    fn a_first_fetch_already_out_is_not_asked_for_twice() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000_000);
        assert!(
            !is_overdue(&state(Phase::Loading, None), INTERVAL, now),
            "the request the panel started at launch is still coming"
        );
    }
}
