//! The list of what has arrived.
//!
//! ```text
//! ┌──────────────────────────────────────┐
//! │ Unread mail                       12 │
//! │ Eli Zaretskii                Today   │
//! │ Re: bug#79231: seq-uniq       09:14  │
//! │ …                                    │
//! │ and more                             │
//! └──────────────────────────────────────┘
//! ```
//!
//! Rows are **conversations**, not messages, because that is what
//! `notmuch search` counts; the header's number is messages, because that is
//! what the tooltip says and the two must not quietly disagree. A conversation
//! holding more than one unread message says so on its own row.

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Align, Image, Label, Orientation, pango};
use topbar_services::{MailThread, NotmuchState, Services};

use crate::bridge::{self, ActionScope, BindingGuard};
use crate::style::{classes, icons};
use crate::surfaces::popovers::PopoverContent;

/// Where this popover's failures are reported.
const SCOPE: ActionScope = ActionScope::Toast { widget: "notmuch" };
/// Shown when the query matches nothing.
const EMPTY: &str = "No Unread Mail";
/// The line under a list that had to stop somewhere.
const MORE: &str = "and more";

/// The popover.
pub struct Inbox {
    root: gtk4::Box,
    count: Label,
    list: gtk4::Box,
    empty: gtk4::Box,
    more: Label,
    /// The rows on screen, so an unchanged snapshot does not rebuild them.
    shown: RefCell<Vec<MailThread>>,
    services: Services,
    _binding: RefCell<Option<BindingGuard>>,
}

impl Inbox {
    /// Build the popover, bound to the notmuch service.
    pub fn new(services: &Services) -> Rc<Self> {
        let root = gtk4::Box::new(Orientation::Vertical, 8);
        root.add_css_class(classes::NOTMUCH_POPOVER);

        let title = Label::new(Some("Unread mail"));
        title.add_css_class(classes::CARD_TITLE);
        title.set_xalign(0.0);
        title.set_hexpand(true);

        let count = Label::new(None);
        count.add_css_class(classes::NOTMUCH_COUNT);

        let header = gtk4::Box::new(Orientation::Horizontal, 8);
        header.add_css_class(classes::NOTMUCH_HEADER);
        header.append(&title);
        header.append(&count);

        let list = gtk4::Box::new(Orientation::Vertical, 4);
        list.add_css_class(classes::NOTMUCH_LIST);

        let more = Label::new(Some(MORE));
        more.add_css_class(classes::NOTMUCH_MORE);
        more.set_xalign(0.0);
        more.set_visible(false);

        // The same empty-state vocabulary the notification history uses.
        let empty = gtk4::Box::new(Orientation::Vertical, 12);
        empty.add_css_class(classes::EMPTY_STATE);
        empty.set_halign(Align::Center);
        empty.set_valign(Align::Center);
        let placeholder = Image::from_icon_name(icons::MAIL_UNREAD);
        placeholder.add_css_class(classes::EMPTY_STATE_ICON);
        empty.append(&placeholder);
        let caption = Label::new(Some(EMPTY));
        caption.add_css_class(classes::EMPTY_STATE_LABEL);
        empty.append(&caption);
        empty.set_visible(false);

        root.append(&header);
        root.append(&list);
        root.append(&more);
        root.append(&empty);

        let inbox = Rc::new(Self {
            root,
            count,
            list,
            empty,
            more,
            shown: RefCell::new(Vec::new()),
            services: services.clone(),
            _binding: RefCell::new(None),
        });

        let binding = bridge::bind_state(&inbox.root, services.notmuch.state(), {
            let inbox = Rc::downgrade(&inbox);
            move |_, state: &NotmuchState| {
                if let Some(inbox) = inbox.upgrade() {
                    inbox.render(state);
                }
            }
        });
        *inbox._binding.borrow_mut() = Some(binding);

        inbox
    }

    /// Draw `state`.
    fn render(&self, state: &NotmuchState) {
        self.count.set_text(&state.unread.to_string());
        self.count.set_visible(state.unread > 0);

        let empty = state.threads.is_empty();
        self.empty.set_visible(empty);
        self.list.set_visible(!empty);
        self.more.set_visible(state.more);

        // Rebuilt only when the conversations themselves change: the count
        // moves on its own and must not throw the list away under the pointer.
        if *self.shown.borrow() == state.threads {
            return;
        }
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        for thread in &state.threads {
            self.list.append(&row(thread));
        }
        self.shown.replace(state.threads.clone());
    }
}

impl PopoverContent for Inbox {
    fn root(&self) -> gtk4::Widget {
        self.root.clone().upcast()
    }

    /// Count again on every open.
    ///
    /// The poll runs every few minutes; a popover opened in between would
    /// otherwise show a list that is exactly as old as the last tick. The
    /// service only re-runs the expensive search if the database actually
    /// moved, so this is one eight-millisecond count in the common case.
    fn refresh(&self) {
        let handle = self.services.notmuch.handle();
        bridge::act(SCOPE, async move { handle.refresh().await });
    }
}

/// One conversation.
fn row(thread: &MailThread) -> gtk4::Box {
    let sender = Label::new(Some(&thread.sender));
    sender.add_css_class(classes::NOTMUCH_SENDER);
    sender.set_xalign(0.0);
    sender.set_hexpand(true);
    sender.set_single_line_mode(true);
    sender.set_ellipsize(pango::EllipsizeMode::End);

    let when = Label::new(Some(&thread.when));
    when.add_css_class(classes::NOTMUCH_TIME);

    // Baselines, not boxes: the timestamp is drawn a size smaller than the
    // sender beside it, and aligning the two boxes at the top leaves it
    // sitting visibly high on the line.
    let top = gtk4::Box::new(Orientation::Horizontal, 6);
    top.set_valign(Align::Baseline);
    top.append(&sender);
    top.append(&when);

    // A conversation holding more than one unread message says so, which is
    // how the header's message count and this list's length explain each
    // other rather than looking like a bug.
    if thread.matched > 1 {
        let matched = Label::new(Some(&thread.matched.to_string()));
        matched.add_css_class(classes::NOTMUCH_MATCHED);
        top.append(&matched);
    }

    let subject = Label::new(Some(&thread.subject));
    subject.add_css_class(classes::NOTMUCH_SUBJECT);
    subject.set_xalign(0.0);
    subject.set_single_line_mode(true);
    subject.set_ellipsize(pango::EllipsizeMode::End);

    let row = gtk4::Box::new(Orientation::Vertical, 2);
    row.add_css_class(classes::NOTMUCH_ROW);
    row.append(&top);
    row.append(&subject);
    row
}
