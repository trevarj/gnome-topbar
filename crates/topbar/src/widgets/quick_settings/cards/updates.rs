//! The updates card: how many, and nothing else.
//!
//! ```text
//! ┌───────────────────────────────────────┐
//! │ ⭯  7 updates                          │
//! │    linux, mesa, firefox               │
//! └───────────────────────────────────────┘
//! ```
//!
//! A statement, not a control. v1's card had a Refresh button, a scrolling
//! table of every pending package grouped by repository, a last-checked line
//! and a toggle that opened a terminal and ran `guix pull && guix package
//! --upgrade` in it. All of that is gone.
//!
//! The reason is what a panel is for. "Are there updates?" is a question a
//! glance answers; "which 340 packages?" is a question a package manager
//! answers, in a window, with a keyboard. And a panel that launches a terminal
//! and starts a system upgrade in it is a panel that can break a machine from a
//! stray click — the plan drops it outright.
//!
//! **The card is absent when there is nothing to say.** Not greyed out, not
//! reading "Up to date": absent. That covers three cases the user cannot
//! distinguish and should not have to — no updates, no way to count them on
//! this distribution, and a counting command that failed — and in every one of
//! them the honest panel is a quiet one.

use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Align, Image, Label, Orientation, Spinner};
use topbar_services::{Services, UpdatesState};

use crate::bridge::{self, BindingGuard};
use crate::style::classes;
use crate::widgets::quick_settings::set_text;

/// The icon Adwaita uses for "there is something to install".
const ICON: &str = "software-update-available-symbolic";

/// Space between the icon, the text and the spinner.
const GAP: i32 = 12;

/// The updates card.
pub struct UpdatesCard {
    root: gtk4::Box,
    title: Label,
    detail: Label,
    spinner: Spinner,
    services: Services,
    bindings: std::cell::RefCell<Vec<BindingGuard>>,
}

impl UpdatesCard {
    /// Build the card, hidden.
    pub fn new(services: &Services) -> Rc<Self> {
        let root = gtk4::Box::new(Orientation::Horizontal, GAP);
        root.add_css_class(classes::QS_CARD);
        root.add_css_class(classes::QS_UPDATES);
        root.set_visible(false);

        let icon = Image::from_icon_name(ICON);
        icon.add_css_class(classes::QS_ICON);
        icon.set_valign(Align::Center);
        root.append(&icon);

        let text = gtk4::Box::new(Orientation::Vertical, 0);
        text.set_valign(Align::Center);
        text.set_hexpand(true);

        let title = Label::new(None);
        title.add_css_class(classes::QS_CARD_TITLE);
        title.set_xalign(0.0);
        text.append(&title);

        // The package names, not a table of versions: "linux, mesa, firefox"
        // reads at a glance and three columns does not.
        let detail = Label::new(None);
        detail.add_css_class(classes::QS_CARD_LINE);
        detail.set_xalign(0.0);
        detail.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        detail.set_visible(false);
        text.append(&detail);

        root.append(&text);

        let spinner = Spinner::new();
        spinner.set_valign(Align::Center);
        spinner.set_visible(false);
        root.append(&spinner);

        let card = Rc::new(Self {
            root,
            title,
            detail,
            spinner,
            services: services.clone(),
            bindings: std::cell::RefCell::new(Vec::new()),
        });

        let binding = bridge::bind_state(&card.root, services.updates.state(), {
            let card = Rc::downgrade(&card);
            move |_: &gtk4::Box, state: &UpdatesState| {
                if let Some(card) = card.upgrade() {
                    card.render(state);
                }
            }
        });
        card.bindings.borrow_mut().push(binding);

        card
    }

    /// The widget to put in the panel.
    pub fn root(&self) -> &gtk4::Box {
        &self.root
    }

    /// Re-render from current state.
    pub fn refresh(&self) {
        self.render(&self.services.updates.current());
    }

    /// Draw the card.
    fn render(&self, state: &UpdatesState) {
        // A check that finds nothing leaves the card hidden and the spinner
        // with nowhere to be, which is right: a spinner on a card that is about
        // to disappear is a flicker nobody asked for.
        let shown = state.shown();
        self.root.set_visible(shown);
        if !shown {
            self.spinner.stop();
            return;
        }

        set_text(&self.title, &state.title());
        match &state.detail {
            Some(detail) if !detail.is_empty() => {
                set_text(&self.detail, detail);
                self.detail.set_visible(true);
            }
            _ => self.detail.set_visible(false),
        }

        // Only while re-checking a count that is already on screen: the first
        // check of a session has no card to spin on.
        self.spinner.set_visible(state.checking);
        if state.checking {
            self.spinner.start();
        } else {
            self.spinner.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_icon_is_the_symbolic_one_adwaita_ships() {
        assert!(ICON.ends_with("-symbolic"));
    }

    #[test]
    fn the_card_is_absent_for_all_three_reasons_a_user_cannot_tell_apart() {
        // No counter on this distribution...
        assert!(!UpdatesState::default().shown());
        // ...a counting command that failed...
        assert!(
            !UpdatesState {
                available: false,
                count: 7,
                ..UpdatesState::default()
            }
            .shown(),
            "a stale count from before a failure is not something to show"
        );
        // ...and a machine that is simply up to date.
        assert!(
            !UpdatesState {
                available: true,
                count: 0,
                ..UpdatesState::default()
            }
            .shown()
        );
    }
}
