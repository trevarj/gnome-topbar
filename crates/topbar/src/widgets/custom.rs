//! `custom-*`: the widget the user writes themselves.
//!
//! ```text
//!  ₿ 103412 Ξ 3412        the live configuration's crypto script
//! ```
//!
//! An optional icon and a label, and the label is whatever a script printed.
//! Everything about *when* the script runs — the interval, the no-overlap
//! guard, the network gate, the last-good value it keeps through a failure —
//! belongs to [`topbar_services::custom`], which is why this file is short:
//! the widget subscribes to a snapshot and draws it, the way every other widget
//! in the panel does.
//!
//! There is one thing here that is not in any other widget. A left click runs
//! the configured `on_click` command and then asks for a refresh, because the
//! whole point of that key is a script whose output the command has just
//! changed — `on_click = "toggle-vpn"` beside a label that reads the VPN state.

use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Image, Label, pango};
use topbar_core::config::CustomWidgetConfig;
use topbar_services::CustomState;
use topbar_services::custom::{CustomClass, CustomExec};
use tracing::warn;

use crate::bar::BarContext;
use crate::bridge::{self, ActionScope, BindingGuard};
use crate::style::classes;
use crate::surfaces::tooltip::TooltipHandle;
use crate::widgets::shell::WidgetShell;
use crate::widgets::{ellipsize, install_click_commands, set_class};

/// What a failed click command is reported against.
const WIDGET_NAME: &str = "custom";

/// One `custom-*` widget.
pub struct CustomWidget {
    shell: WidgetShell,
    /// Holds the label, the icon and the tooltip the render closure touches.
    _inner: Rc<Inner>,
    /// Keeps the label subscribed to the script's snapshot.
    _binding: Option<BindingGuard>,
}

impl CustomWidget {
    /// Build the widget from `[widgets.custom-<name>]`.
    ///
    /// `name` is the full section name, `custom-crypto` and not `crypto`: it is
    /// how the script's runner is found and what the CSS class is made of, so
    /// a user can style one custom widget without styling the rest.
    pub fn new(name: &str, settings: &CustomWidgetConfig, context: &BarContext) -> Self {
        let shell = WidgetShell::new(name);

        let icon = Image::new();
        icon.add_css_class(classes::CUSTOM_ICON);
        match &settings.icon {
            Some(name) => icon.set_icon_name(Some(name)),
            None => icon.set_visible(false),
        }
        shell.content().append(&icon);

        let label = Label::new(None);
        label.add_css_class(classes::CUSTOM_LABEL);
        label.set_ellipsize(pango::EllipsizeMode::End);
        shell.content().append(&label);

        let inner = Rc::new(Inner {
            wrapper: shell.root().clone(),
            label,
            tooltip: shell.set_tooltip(settings.tooltip.as_deref().unwrap_or_default()),
            max_chars: settings.max_chars.map(|max| max as usize),
            configured_tooltip: settings.tooltip.clone(),
            has_icon: settings.icon.is_some(),
        });

        // The runner is keyed by the same name the section is, so this is only
        // ever `None` if the configuration and the service bundle were built
        // from different documents — a defect, not a state to design for.
        let exec = context.services.custom.get(name);
        if exec.is_none() {
            warn!("`{name}` has no script runner behind it");
        }
        let binding = exec.as_ref().map(|exec| {
            bridge::bind_state(shell.root(), exec.state(), {
                let inner = Rc::downgrade(&inner);
                move |_: &gtk4::Box, state: &CustomState| {
                    if let Some(inner) = inner.upgrade() {
                        inner.render(state);
                    }
                }
            })
        });

        if settings.on_click.is_some()
            || settings.on_click_right.is_some()
            || settings.on_click_middle.is_some()
        {
            shell.make_interactive();
        }
        install_left_click(shell.root(), settings.on_click.as_deref(), exec);
        install_click_commands(
            shell.root(),
            WIDGET_NAME,
            settings.on_click_right.as_deref(),
            settings.on_click_middle.as_deref(),
        );

        Self {
            shell,
            _inner: inner,
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
    /// The shell's outer box: what is hidden, and what carries the tint.
    wrapper: gtk4::Box,
    label: Label,
    tooltip: TooltipHandle,
    /// `max_chars`, the width the label is cut to.
    max_chars: Option<usize>,
    /// `tooltip`, shown when the script did not offer one of its own.
    configured_tooltip: Option<String>,
    /// Whether an icon was configured, so a text-free widget still shows it.
    has_icon: bool,
}

impl Inner {
    /// Draw `state`.
    fn render(&self, state: &CustomState) {
        let text = ellipsize(state.text(), self.max_chars);
        // An icon-only widget stays on the bar with nothing beside it; a
        // label-only one goes away when there is nothing to say.
        self.wrapper.set_visible(state.visible() || self.has_icon);
        self.label.set_visible(!text.is_empty());
        if self.label.text() != text {
            self.label.set_text(&text);
        }

        match state.tooltip(self.configured_tooltip.as_deref()) {
            Some(tooltip) => self.tooltip.set_text(&tooltip),
            None => self.tooltip.set_text(""),
        }

        for (class, wanted) in [
            (
                classes::STATE_SUCCESS,
                state.display.class == Some(CustomClass::Success),
            ),
            (
                classes::STATE_WARNING,
                state.display.class == Some(CustomClass::Warning),
            ),
            (
                classes::STATE_URGENT,
                state.display.class == Some(CustomClass::Urgent),
            ),
        ] {
            set_class(&self.wrapper, class, wanted);
        }
    }
}

/// Wire the left click: run the command, then re-read the script.
///
/// The refresh is asked for after the command has been *started* rather than
/// after it has finished, because [`proc::run`](topbar_services::proc::run)
/// deliberately detaches anything still alive after its grace period — a click
/// that opens a terminal has succeeded and must not be waited on. A script
/// whose state the command changes in under that grace period, which is what
/// `toggle-vpn` is, is read correctly; a slower one is read on its next tick.
fn install_left_click(anchor: &gtk4::Box, command: Option<&str>, exec: Option<CustomExec>) {
    let Some(command) = command.map(str::to_string) else {
        return;
    };
    let click = gtk4::GestureClick::new();
    click.set_button(gtk4::gdk::BUTTON_PRIMARY);
    click.connect_released(move |_, _, _, _| {
        let command = command.clone();
        let exec = exec.clone();
        bridge::act(
            ActionScope::Toast {
                widget: WIDGET_NAME,
            },
            async move {
                let outcome = topbar_services::proc::run(&command).await;
                if let Some(exec) = exec {
                    exec.refresh().await;
                }
                outcome
            },
        );
    });
    anchor.add_controller(click);
}

#[cfg(test)]
mod tests {
    use topbar_core::config::Config;
    use topbar_services::custom::{CustomDisplay, model};

    use super::*;

    /// The live configuration's own custom widget.
    const LIVE_CONFIG: &str = include_str!("../../../topbar-core/tests/fixtures/live-config.toml");

    fn live() -> CustomWidgetConfig {
        Config::parse(LIVE_CONFIG)
            .expect("the live config parses")
            .0
            .widgets
            .custom
            .remove("custom-crypto")
            .expect("the live config has a crypto script")
    }

    fn state(display: CustomDisplay) -> CustomState {
        CustomState {
            display,
            loading: false,
            failure: None,
        }
    }

    #[test]
    fn the_live_crypto_script_is_read_as_configured() {
        let config = live();
        assert!(config.exec.is_some());
        assert!(config.requires_network);
        assert_eq!(config.interval, 1800);
        assert_eq!(config.max_chars, Some(40));
        assert_eq!(config.tooltip.as_deref(), Some("Crypto prices"));
        assert!(config.icon.is_none(), "the script draws its own glyphs");
    }

    #[test]
    fn the_scripts_output_is_cut_to_the_configured_width() {
        // The live script prints something like " 103412  3412 ₿0.033",
        // which fits inside forty characters; a runaway one would not.
        let long = "x".repeat(60);
        assert_eq!(ellipsize(&long, Some(40)).chars().count(), 40);
        assert_eq!(ellipsize(" 103412  3412", Some(40)), " 103412  3412");
    }

    #[test]
    fn a_class_the_panel_does_not_paint_leaves_the_widget_untinted() {
        let display = model::display(r#"{"text":"x","class":"chartreuse"}"#, "", None);
        assert_eq!(state(display).display.class, None);
    }

    #[test]
    fn the_three_classes_the_panel_paints_reach_the_widget() {
        for (name, wanted) in [
            ("success", CustomClass::Success),
            ("warning", CustomClass::Warning),
            ("error", CustomClass::Urgent),
        ] {
            let raw = format!(r#"{{"text":"x","class":"{name}"}}"#);
            assert_eq!(model::display(&raw, "", None).class, Some(wanted));
        }
    }
}
