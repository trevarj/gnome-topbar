//! The crypto popover: prices, and the settings behind a gear.
//!
//! ```text
//! ┌──────────────────────────────────────────────┐
//! │ crypto                                   ⚙   │  title, gear
//! │  ₿   Bitcoin          $103,412     +2.4%     │  one row per entry
//! │  Ξ   Ethereum           $3,412     −1.3%     │
//! │  Ξ₿  ETH / BTC          ₿0.033     −3.6%     │
//! │ Updated 2m ago                               │
//! └──────────────────────────────────────────────┘
//! ```
//!
//! Two views in one popover rather than two surfaces: the settings are about
//! the very rows they replace, and a modal on its own layer — which is what
//! the weather's location picker needs, because it is a search — would put a
//! dimmed backdrop over the thing the user is editing.
//!
//! The gear crossfades to the settings view over 150ms in one animator run;
//! with motion off the run jumps straight to its end and the swap is instant.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::{Duration, SystemTime};

use gtk4::prelude::*;
use gtk4::{Align, Button, Image, Label, Orientation, glib, pango};
use topbar_services::{CryptoState, Entry, Services};

use crate::anim::{Animation, AnimationParams, Easing};
use crate::bridge::{self, BindingGuard};
use crate::style::classes;
use crate::surfaces::popovers::PopoverContent;
use crate::widgets::crypto::format::{self, Direction};
use crate::widgets::crypto::settings::Settings;
use crate::widgets::crypto::{emblem, is_overdue, refresh_now};
use crate::widgets::weather::forecast::age;

/// Logo size on a price row. A pair's two coins each take 5/8 of this.
const ROW_ICON: i32 = 24;
/// The gear that switches to the settings view, and the icon the empty state
/// points at with it.
const CONFIGURE_ICON: &str = "preferences-system-symbolic";
/// How long the crossfade between the two views takes.
const SWITCH_MS: u64 = 150;
/// How often the "Updated 2m ago" line is re-timed while the popover is open.
const AGE_TICK: Duration = Duration::from_secs(30);

/// The popover.
pub struct Prices {
    root: gtk4::Box,
    /// The price view.
    quotes: gtk4::Box,
    /// The settings view's holder, the price view's sibling.
    settings_view: gtk4::Box,
    /// Where the price rows live.
    list: gtk4::Box,
    rows: RefCell<Vec<Row>>,
    empty: gtk4::Box,
    updated: Label,
    /// The settings view itself.
    settings: Rc<Settings>,
    /// Which of the two is on screen.
    showing_settings: Cell<bool>,
    /// The crossfade between them.
    switch: Animation,
    /// The last snapshot, re-rendered whenever the popover reappears.
    state: RefCell<Rc<CryptoState>>,
    /// Re-times the footer while the popover is open, and only then.
    ticker: RefCell<Option<glib::SourceId>>,
    /// A handle on itself, for the callbacks that outlive one method call.
    me: Weak<Self>,
    /// How old prices may be before an open refetches them.
    interval: Duration,
    services: Services,
    _bindings: RefCell<Vec<BindingGuard>>,
}

impl Prices {
    /// Build the popover, bound to the crypto service.
    pub fn new(interval: Duration, services: &Services) -> Rc<Self> {
        let root = gtk4::Box::new(Orientation::Vertical, 0);
        root.add_css_class(classes::CRYPTO_POPOVER);

        // --- the price view -------------------------------------------------
        let quotes = gtk4::Box::new(Orientation::Vertical, 6);

        let header = gtk4::Box::new(Orientation::Horizontal, 8);
        header.add_css_class(classes::CRYPTO_HEADER);

        let title = Label::new(Some("crypto"));
        title.add_css_class(classes::CARD_TITLE);
        title.set_xalign(0.0);
        title.set_hexpand(true);

        let gear = Button::new();
        gear.add_css_class(classes::CRYPTO_CONFIGURE);
        gear.set_child(Some(&Image::from_icon_name(CONFIGURE_ICON)));
        gear.set_tooltip_text(Some("Choose which prices to show"));
        gear.set_valign(Align::Start);

        header.append(&title);
        header.append(&gear);
        quotes.append(&header);

        let list = gtk4::Box::new(Orientation::Vertical, 2);
        list.add_css_class(classes::CRYPTO_LIST);
        quotes.append(&list);

        let empty = gtk4::Box::new(Orientation::Vertical, 8);
        empty.add_css_class(classes::EMPTY_STATE);
        empty.set_valign(Align::Center);
        let empty_icon = Image::from_icon_name(CONFIGURE_ICON);
        empty_icon.add_css_class(classes::EMPTY_STATE_ICON);
        let empty_label = Label::new(Some("Nothing configured\nOpen settings to add a price"));
        empty_label.add_css_class(classes::EMPTY_STATE_LABEL);
        empty_label.set_justify(gtk4::Justification::Center);
        empty.append(&empty_icon);
        empty.append(&empty_label);
        quotes.append(&empty);

        let updated = Label::new(None);
        updated.add_css_class(classes::CRYPTO_UPDATED);
        updated.set_xalign(0.0);
        quotes.append(&updated);

        root.append(&quotes);

        // --- the settings view ----------------------------------------------
        let settings_view = gtk4::Box::new(Orientation::Vertical, 0);
        settings_view.set_visible(false);
        root.append(&settings_view);

        let prices = Rc::new_cyclic(|me: &Weak<Self>| {
            let settings = Settings::new(services, {
                let me = me.clone();
                move || {
                    if let Some(prices) = me.upgrade() {
                        prices.switch_to(false);
                    }
                }
            });
            settings_view.append(settings.root());

            Self {
                switch: Animation::new(&root),
                root,
                quotes,
                settings_view,
                list,
                rows: RefCell::new(Vec::new()),
                empty,
                updated,
                settings,
                showing_settings: Cell::new(false),
                state: RefCell::new(Rc::new(CryptoState::default())),
                ticker: RefCell::new(None),
                me: me.clone(),
                interval,
                services: services.clone(),
                _bindings: RefCell::new(Vec::new()),
            }
        });

        gear.connect_clicked({
            let me = Rc::downgrade(&prices);
            move |_| {
                if let Some(prices) = me.upgrade() {
                    prices.switch_to(true);
                }
            }
        });

        let binding = bridge::bind_state(&prices.root, services.crypto.state(), {
            let me = Rc::downgrade(&prices);
            move |_: &gtk4::Box, state: &CryptoState| {
                if let Some(prices) = me.upgrade() {
                    prices.render(state, SystemTime::now());
                }
            }
        });
        prices._bindings.borrow_mut().push(binding);

        prices
    }

    /// Switch to the settings view. What `TOPBAR_SMOKE_OPEN=crypto-settings`
    /// reaches, there being no pointer in the dev shell to press the gear with.
    pub fn show_settings(&self) {
        self.switch_to(true);
    }

    /// Crossfade between the two views.
    fn switch_to(&self, settings: bool) {
        if self.showing_settings.get() == settings {
            return;
        }
        self.showing_settings.set(settings);

        let quotes: gtk4::Widget = self.quotes.clone().upcast();
        let holder: gtk4::Widget = self.settings_view.clone().upcast();
        let (from, to) = if settings {
            (quotes, holder)
        } else {
            (holder, quotes)
        };

        // Whatever a superseded run left behind, the crossfade starts from a
        // known state: the outgoing view fully drawn, the incoming one gone.
        self.switch.cancel();
        from.set_visible(true);
        from.set_opacity(1.0);
        to.set_visible(false);
        to.set_opacity(0.0);

        self.switch.start(
            AnimationParams::new(SWITCH_MS).with_easing(Easing::Linear),
            Box::new(move |progress| {
                // One run, two halves: out, swap, in. Two chained runs would be
                // twice the duration and would need a done callback to survive
                // being superseded half way through.
                if progress < 0.5 {
                    from.set_opacity(1.0 - progress * 2.0);
                    return;
                }
                if from.is_visible() {
                    from.set_visible(false);
                    to.set_visible(true);
                }
                to.set_opacity((progress - 0.5) * 2.0);
            }),
            None,
        );
    }

    /// Put the price view back on screen with no animation at all.
    fn snap_to_quotes(&self) {
        self.showing_settings.set(false);
        self.switch.cancel();
        self.settings_view.set_visible(false);
        self.quotes.set_opacity(1.0);
        self.quotes.set_visible(true);
    }

    /// Draw `state`.
    fn render(&self, state: &CryptoState, now: SystemTime) {
        *self.state.borrow_mut() = Rc::new(state.clone());
        self.settings.render(&state.entries);

        self.sync_rows(&state.entries);
        let configured = !state.entries.is_empty();
        show(&self.empty, !configured);
        show(&self.list, configured);

        for row in self.rows.borrow().iter() {
            row.render(state);
        }

        match state.fetched_at {
            Some(fetched) => {
                show(&self.updated, true);
                set_text(&self.updated, &age(fetched, now));
            }
            // Nothing has ever landed, so there is no age to report; the rows
            // already say they have no numbers in them.
            None => show(&self.updated, false),
        }
    }

    /// Rebuild the price rows if they are not the ones `entries` names.
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
            self.list.remove(&row.root);
        }
        for entry in entries {
            let row = Row::new(*entry);
            self.list.append(&row.root);
            rows.push(row);
        }
    }

    /// Re-time the footer, without waiting for the service to publish.
    fn retime(&self) {
        let state = Rc::clone(&self.state.borrow());
        self.render(&state, SystemTime::now());
    }
}

impl PopoverContent for Prices {
    fn root(&self) -> gtk4::Widget {
        self.root.clone().upcast()
    }

    fn refresh(&self) {
        // Every open lands on the prices: the settings are a detour, and
        // reopening into them would hide the numbers the click asked for.
        self.snap_to_quotes();

        let state = Rc::clone(&self.state.borrow());
        let now = SystemTime::now();
        self.render(&state, now);

        // Prices older than the schedule intended are worth one request; an
        // open popover is the only moment the panel knows somebody is looking.
        if is_overdue(&state, self.interval, now) {
            refresh_now(&self.services);
        }

        // "Updated 2m ago" stops being true while the popover is open, and the
        // service publishes nothing in between. The tick exists only while
        // somebody can see the line it re-times.
        let mut ticker = self.ticker.borrow_mut();
        if ticker.is_some() {
            return;
        }
        let me = self.me.clone();
        *ticker = Some(glib::timeout_add_local(AGE_TICK, move || {
            match me.upgrade() {
                Some(prices) => {
                    prices.retime();
                    glib::ControlFlow::Continue
                }
                None => glib::ControlFlow::Break,
            }
        }));
    }

    fn closed(&self) {
        if let Some(ticker) = self.ticker.borrow_mut().take() {
            ticker.remove();
        }
    }
}

/// One price row.
struct Row {
    entry: Entry,
    root: gtk4::Box,
    value: Label,
    change: Label,
}

impl Row {
    fn new(entry: Entry) -> Self {
        let root = gtk4::Box::new(Orientation::Horizontal, 10);
        root.add_css_class(classes::CRYPTO_ROW);

        root.append(&emblem(entry, ROW_ICON));

        let name = Label::new(Some(&name_of(entry)));
        name.add_css_class(classes::CRYPTO_NAME);
        name.set_xalign(0.0);
        name.set_hexpand(true);
        name.set_ellipsize(pango::EllipsizeMode::End);

        let value = Label::new(None);
        value.add_css_class(classes::CRYPTO_ROW_VALUE);
        value.set_xalign(1.0);

        let change = Label::new(None);
        change.add_css_class(classes::CRYPTO_CHANGE);

        root.append(&name);
        root.append(&value);
        root.append(&change);

        Self {
            entry,
            root,
            value,
            change,
        }
    }

    /// Draw this row's price and change out of `state`.
    fn render(&self, state: &CryptoState) {
        let quote = state.quote(self.entry);
        set_text(&self.value, &format::row_value(self.entry, quote));

        let change = quote.and_then(|quote| quote.change_24h);
        set_text(&self.change, &format::change_chip(change));
        self.change.remove_css_class(classes::CRYPTO_CHANGE_UP);
        self.change.remove_css_class(classes::CRYPTO_CHANGE_DOWN);
        match format::direction(change) {
            Direction::Up => self.change.add_css_class(classes::CRYPTO_CHANGE_UP),
            Direction::Down => self.change.add_css_class(classes::CRYPTO_CHANGE_DOWN),
            // A flat change wears no tint: green and red are for movement, and
            // a market that did nothing did nothing good or bad.
            Direction::Flat => {}
        }
    }
}

/// What a row calls its entry: an asset's name, or the pair written out.
fn name_of(entry: Entry) -> String {
    match entry {
        Entry::Single(asset) => asset.name().to_string(),
        Entry::Pair(..) => entry.label(),
    }
}

/// Set a label only when the text changed.
fn set_text(label: &Label, text: &str) {
    if label.text() != text {
        label.set_text(text);
    }
}

/// Show or hide, only when it is a change.
fn show(widget: &impl IsA<gtk4::Widget>, visible: bool) {
    let widget = widget.as_ref();
    if widget.is_visible() != visible {
        widget.set_visible(visible);
    }
}

#[cfg(test)]
mod tests {
    use topbar_services::Asset;

    use super::*;

    #[test]
    fn a_row_is_named_for_the_asset_or_written_as_a_pair() {
        assert_eq!(name_of(Entry::Single(Asset::Btc)), "Bitcoin");
        assert_eq!(name_of(Entry::Single(Asset::Xmr)), "Monero");
        assert_eq!(name_of(Entry::Pair(Asset::Eth, Asset::Btc)), "ETH / BTC");
    }
}
