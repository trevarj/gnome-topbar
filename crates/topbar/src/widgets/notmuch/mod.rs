//! Unread mail: an envelope, and mostly nothing at all.
//!
//! ```text
//!  ✉
//! ```
//!
//! Invisible unless something matches the query, which on a read inbox is most
//! of the time — and invisible too when notmuch cannot be run at all, because
//! "nothing unread" and "I could not tell" look identical on a panel and only
//! one of them is safe to guess. See [`topbar_services::notmuch`].
//!
//! The count is in the tooltip rather than on the bar. An envelope is the same
//! width whether it is three messages or three hundred, and a widget that
//! changes width every time mail arrives moves everything beside it.
//!
//! Left click opens the list; see [`popover`].

mod popover;

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::Image;
use gtk4::prelude::*;
use topbar_core::config::NotmuchConfig;
use topbar_services::NotmuchState;

use crate::bar::BarContext;
use crate::bridge::{self, BindingGuard};
use crate::style::{classes, icons};
use crate::surfaces::popovers::{self, PopoverContent, PopoverHandle};
use crate::surfaces::tooltip::TooltipHandle;
use crate::widgets::install_click_commands;
use crate::widgets::shell::WidgetShell;

use popover::Inbox;

/// Widget name, for CSS classes and the popover registry.
const WIDGET_NAME: &str = "notmuch";

/// The unread-mail widget.
pub struct NotmuchWidget {
    shell: WidgetShell,
    /// Holds the icon and the tooltip the render closure touches.
    _inner: Rc<Inner>,
    /// The popover's claim on the host.
    _popover: PopoverHandle,
    /// Keeps them subscribed to the service.
    _binding: BindingGuard,
}

impl NotmuchWidget {
    /// Build the widget from `[widgets.notmuch]`.
    pub fn new(config: &NotmuchConfig, context: &BarContext) -> Self {
        let shell = WidgetShell::new(classes::NOTMUCH);
        shell.make_interactive();
        // Nothing has been counted yet, and an envelope that appears and then
        // vanishes a moment later is worse than one that arrives once.
        shell.root().set_visible(false);

        let icon = Image::from_icon_name(icons::MAIL_UNREAD);
        icon.add_css_class(classes::NOTMUCH_ICON);
        shell.content().append(&icon);

        let inner = Rc::new(Inner {
            tooltip: shell.set_tooltip(&config.tooltip),
        });

        let binding = bridge::bind_state(shell.root(), context.services.notmuch.state(), {
            let inner = Rc::downgrade(&inner);
            move |root: &gtk4::Box, state: &NotmuchState| {
                let Some(inner) = inner.upgrade() else {
                    return;
                };
                if state.shown() {
                    inner.tooltip.set_text(&state.title());
                }
                // Zero width rather than an empty pill: a widget with nothing
                // to say should not take up room saying it.
                root.set_visible(state.shown());
            }
        });

        let content: Rc<RefCell<Option<Rc<Inbox>>>> = Rc::new(RefCell::new(None));
        let popover = {
            let services = context.services.clone();
            let content = Rc::clone(&content);
            popovers::attach(context, WIDGET_NAME, shell.root(), move || {
                let inbox = Inbox::new(&services);
                *content.borrow_mut() = Some(Rc::clone(&inbox));
                inbox as Rc<dyn PopoverContent>
            })
        };

        install_click_commands(
            shell.root(),
            WIDGET_NAME,
            config.on_click_right.as_deref(),
            config.on_click_middle.as_deref(),
        );

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
    tooltip: TooltipHandle,
}
