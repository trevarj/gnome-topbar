//! Quick Settings: the aggregate menu, and the button that opens it.
//!
//! ```text
//!   model.rs     icon order, grid order, slider bounds, device marking (pure)
//!   button.rs    the bar button's row of status icons
//!   panel.rs     the panel itself, on the shared popover host
//!   expander.rs  one expandable section open at a time
//!   hold.rs      hold-to-confirm, for the power rows
//!   cards/       the blocks the panel is made of
//! ```
//!
//! Two rules run through all of it.
//!
//! **Every control sends [`ChangeSource::Ui`]**. That is the token the OSD
//! reads to tell "the user pressed a media key" from "the user is dragging the
//! slider they are looking at"; without it a capsule appears under the pointer
//! restating the number the control already shows.
//!
//! **Every failure lands inline.** Quick Settings is the first consumer of
//! [`ActionScope::Inline`](crate::bridge::ActionScope::Inline): a red caption
//! under the row that asked, cleared on the next attempt. A toast would be
//! redundant with a control the user is already looking at, and worse, it
//! would cover the panel that raised it.

mod button;
pub mod cards;
mod expander;
mod hold;
mod model;
mod panel;

use std::future::Future;
use std::rc::Rc;

use gtk4::prelude::*;
use topbar_core::Config;
use topbar_core::config::QuickSettingsConfig;
use topbar_services::{ChangeSource, SvcError};

use crate::bar::BarContext;
use crate::bridge::{self, ActionScope};
use crate::style::classes;
use crate::surfaces::inline;
use crate::surfaces::popovers::{self, PopoverContent, PopoverHandle};
use crate::surfaces::tooltip::TooltipHandle;
use crate::widgets::shell::WidgetShell;

/// Widget name, for CSS classes and the popover registry.
pub const WIDGET_NAME: &str = "quick_settings";

/// The Quick Settings widget.
pub struct QuickSettingsWidget {
    shell: WidgetShell,
    _indicators: Rc<button::IndicatorRow>,
    _popover: PopoverHandle,
    _tooltip: TooltipHandle,
}

impl QuickSettingsWidget {
    /// Build the widget from `[widgets.quick_settings]`.
    pub fn new(config: &Config, context: &BarContext) -> Self {
        let settings = config.widgets.quick_settings.clone();

        let shell = WidgetShell::new(classes::QUICK_SETTINGS);
        shell.make_interactive();

        let indicators =
            button::IndicatorRow::new(shell.content(), &context.services, settings.battery);
        let tooltip = shell.set_tooltip("Quick Settings");

        // The panel is still built lazily, on first open; this only remembers
        // *which* panel was built, so the smoke hooks can reach into it.
        let built: Rc<std::cell::RefCell<Option<Rc<panel::Panel>>>> =
            Rc::new(std::cell::RefCell::new(None));
        let popover = {
            let services = context.services.clone();
            let settings = settings.clone();
            let monitor = context.monitor.clone();
            let built = Rc::clone(&built);
            popovers::attach(context, WIDGET_NAME, shell.root(), move || {
                let panel = panel::Panel::new(&services, &settings, &monitor);
                *built.borrow_mut() = Some(Rc::clone(&panel));
                panel as Rc<dyn PopoverContent>
            })
        };

        install_scroll(shell.root(), context, &settings);
        install_click_commands(shell.root(), &settings);
        install_tooltip_refresh(shell.root(), &indicators, &tooltip);
        install_smoke_actions(&built, &context.services);

        Self {
            shell,
            _indicators: indicators,
            _popover: popover,
            _tooltip: tooltip,
        }
    }

    /// The widget to put in a bar section.
    pub fn root(&self) -> gtk4::Widget {
        self.shell.root().clone().upcast()
    }
}

/// Scrolling the button changes the output volume.
///
/// [`ChangeSource::Ui`] again, and for the same reason the sliders use it: the
/// icon on the button under the pointer changes as the wheel turns, which is
/// the feedback. A capsule as well would be one too many.
fn install_scroll(anchor: &gtk4::Box, context: &BarContext, settings: &QuickSettingsConfig) {
    let step = model::scroll_step(settings.audio_scroll_percentage);
    let scroll = gtk4::EventControllerScroll::new(gtk4::EventControllerScrollFlags::VERTICAL);
    scroll.connect_scroll({
        let audio = context.services.audio.handle().clone();
        move |_, _, delta| {
            let audio = audio.clone();
            // Up is louder, and GTK's positive delta points down.
            let up = delta < 0.0;
            bridge::act(
                ActionScope::Toast {
                    widget: WIDGET_NAME,
                },
                async move {
                    if up {
                        audio.inc_sink_volume(step, ChangeSource::Ui).await
                    } else {
                        audio.dec_sink_volume(step, ChangeSource::Ui).await
                    }
                },
            );
            gtk4::glib::Propagation::Stop
        }
    });
    anchor.add_controller(scroll);
}

/// The configured right- and middle-click commands.
///
/// The live configuration binds right-click to `loginctl lock-session`, which
/// is the shortest path from "I am leaving my desk" to a locked screen: no
/// panel to open, no row to find.
fn install_click_commands(anchor: &gtk4::Box, settings: &QuickSettingsConfig) {
    for (button, command) in [
        (gtk4::gdk::BUTTON_SECONDARY, settings.on_click_right.clone()),
        (gtk4::gdk::BUTTON_MIDDLE, settings.on_click_middle.clone()),
    ] {
        let Some(command) = command else { continue };
        let click = gtk4::GestureClick::new();
        click.set_button(button);
        click.connect_released(move |_, _, _, _| {
            let command = command.clone();
            // A toast rather than an inline slot: the panel is shut, so there
            // is no row under the pointer for a caption to belong to.
            bridge::act(
                ActionScope::Toast {
                    widget: WIDGET_NAME,
                },
                async move { topbar_services::proc::run(&command).await },
            );
        });
        anchor.add_controller(click);
    }
}

/// Keep the tooltip's text current while the pointer is over the button.
fn install_tooltip_refresh(
    anchor: &gtk4::Box,
    indicators: &Rc<button::IndicatorRow>,
    tooltip: &TooltipHandle,
) {
    let motion = gtk4::EventControllerMotion::new();
    motion.connect_enter({
        let indicators = Rc::downgrade(indicators);
        let tooltip = tooltip.clone();
        move |_, _, _| {
            if let Some(indicators) = indicators.upgrade() {
                tooltip.set_text(&indicators.tooltip());
            }
        }
    });
    anchor.add_controller(motion);
}

/// How long the smoke hook waits for the panel to lay out before reaching in.
///
/// The nested session renders in software and is throttled by the host
/// compositor, so a widget's allocation is not final the instant it is
/// mapped — and the painted fill is a fraction of the row's *width*.
#[cfg(debug_assertions)]
const SMOKE_SETTLE: std::time::Duration = std::time::Duration::from_millis(1200);

/// How far across the Shut Down row the smoke hook paints its fill.
#[cfg(debug_assertions)]
const SMOKE_FILL: f64 = 0.55;

/// The volume the smoke hook drags the output slider to.
#[cfg(debug_assertions)]
const SMOKE_VOLUME: u32 = 70;

/// The password the smoke hook answers a Wi-Fi prompt with.
///
/// Distinctive on purpose: the run greps `panel.log` and every process's
/// command line for it afterwards, and a string like "test" would match
/// something by accident.
#[cfg(debug_assertions)]
const SMOKE_PASSWORD: &str = "topbar-smoke-psk-9f3a";

/// Ways for the smoke run to open things a pointer would.
///
/// `TOPBAR_SMOKE_OPEN=quick_settings` already opens the panel through the
/// popover registry; these reach the blocks inside it, which nothing else can.
/// Debug builds only.
#[cfg(debug_assertions)]
fn install_smoke_actions(
    built: &Rc<std::cell::RefCell<Option<Rc<panel::Panel>>>>,
    services: &topbar_services::Services,
) {
    use panel::Block;

    // A volume the *panel* asked for, which is the whole point: a `topbar
    // volume set` would carry ChangeSource::Cli and raise a capsule, and the
    // thing being proved is that a Quick Settings change does not.
    popovers::register_smoke_action("quick-settings-volume", {
        let audio = services.audio.handle().clone();
        move || {
            popovers::dispatch(
                &topbar_core::ipc::PopoverAction::Show(WIDGET_NAME.to_string()),
                None,
            );
            let audio = audio.clone();
            gtk4::glib::timeout_add_local_once(SMOKE_SETTLE, move || {
                attempt(inline::names::VOLUME, async move {
                    audio.set_sink_volume(SMOKE_VOLUME, ChangeSource::Ui).await
                });
            });
        }
    });

    // The charge limit, likewise: the buttons in the card cannot be clicked
    // without a pointer, and what matters is that the write reaches the
    // kernel's own files.
    popovers::register_smoke_action("quick-settings-limit", {
        let battery = services.battery.handle().clone();
        let built = Rc::clone(built);
        move || {
            popovers::dispatch(
                &topbar_core::ipc::PopoverAction::Show(WIDGET_NAME.to_string()),
                None,
            );
            let battery = battery.clone();
            let built = Rc::clone(&built);
            gtk4::glib::timeout_add_local_once(SMOKE_SETTLE, move || {
                if let Some(panel) = built.borrow().clone() {
                    panel.expand(Block::BatteryHealth);
                }
                let battery = battery.clone();
                attempt(inline::names::BATTERY, async move {
                    battery
                        .set_thresholds(
                            topbar_services::battery::LIMIT_PRESET.0,
                            topbar_services::battery::LIMIT_PRESET.1,
                        )
                        .await
                });
            });
        }
    });

    // The password row cannot be typed into without a keyboard, so the answer
    // goes in the way the row's own Connect button would send it — through the
    // service handle, wrapped, never through a command line. The fake
    // NetworkManager records what came back out of its `GetSecrets` call, which
    // is how the run proves the key travelled on the bus and nowhere else.
    popovers::register_smoke_action("quick-settings-wifi-password", {
        let network = services.network.handle().clone();
        move || {
            let network = network.clone();
            gtk4::glib::timeout_add_local_once(SMOKE_SETTLE, move || {
                attempt(inline::names::WIFI, async move {
                    network
                        .submit_secret(topbar_services::Secret::new(SMOKE_PASSWORD.to_string()))
                        .await
                });
            });
        }
    });

    for (suffix, block) in [
        ("battery", Block::BatteryHealth),
        ("power", Block::Power),
        ("mode", Block::PowerMode),
        ("wifi", Block::WiFi),
        ("vpn", Block::Vpn),
    ] {
        popovers::register_smoke_action(&format!("quick-settings-{suffix}"), {
            let built = Rc::clone(built);
            move || {
                // Through the registry, so the panel opens exactly the way
                // `topbar popover show quick-settings` opens it.
                popovers::dispatch(
                    &topbar_core::ipc::PopoverAction::Show(WIDGET_NAME.to_string()),
                    None,
                );
                let built = Rc::clone(&built);
                gtk4::glib::timeout_add_local_once(SMOKE_SETTLE, move || {
                    let Some(panel) = built.borrow().clone() else {
                        return;
                    };
                    panel.expand(block);

                    if block != Block::Power {
                        return;
                    }
                    // A second beat, so the rows have their final width before
                    // a fraction of it is painted.
                    gtk4::glib::timeout_add_local_once(SMOKE_SETTLE, move || {
                        // A real press and release first: the state machine
                        // runs, the row cancels, and nothing is called. The
                        // log line is the proof, and panel.log carries it.
                        let power = panel.power();
                        power.begin_hold(cards::power::Row::Suspend);
                        let progress = power.hold_progress(cards::power::Row::Suspend);
                        power.cancel_holds();
                        tracing::info!(
                            "smoke: held Suspend to {:.0}% and released it; nothing fired",
                            progress * 100.0
                        );
                        // Then a still frame of the fill, which a moving one
                        // could never give the screenshot helper.
                        power.paint_hold(cards::power::Row::ShutDown, SMOKE_FILL);
                    });
                });
            }
        });
    }
}

/// Nothing to install in a packaged build.
#[cfg(not(debug_assertions))]
fn install_smoke_actions(
    _built: &Rc<std::cell::RefCell<Option<Rc<panel::Panel>>>>,
    _services: &topbar_services::Services,
) {
}

/// Run a service call, reporting failure under the row that asked.
///
/// The clear is the other half of the contract: a caption from the last
/// attempt still sitting there while a new one is in flight would be a lie
/// about what just happened.
pub(crate) fn attempt<F>(slot: &'static str, future: F)
where
    F: Future<Output = Result<(), SvcError>> + Send + 'static,
{
    inline::clear(slot);
    bridge::act(ActionScope::Inline { widget: slot }, future);
}

/// The same, with something to do once it has worked.
///
/// One caller: the VPN row, which closes the panel on a successful connect when
/// `vpn_close_on_connect` asks it to. The follow-up runs on the main thread and
/// only on success — a tunnel that failed leaves the panel open with the error
/// under the row that raised it, which is the whole point of the inline scope.
pub(crate) fn attempt_then<F>(slot: &'static str, future: F, then: impl FnOnce() + 'static)
where
    F: Future<Output = Result<(), SvcError>> + Send + 'static,
{
    inline::clear(slot);
    bridge::request(ActionScope::Inline { widget: slot }, future, |()| then());
}

/// Set an icon only when it changed.
pub(crate) fn set_icon(image: &gtk4::Image, name: &str) {
    if image.icon_name().as_deref() != Some(name) {
        image.set_icon_name(Some(name));
    }
}

/// Set a label only when the text changed, which costs a relayout.
pub(crate) fn set_text(label: &gtk4::Label, text: &str) {
    if label.text() != text {
        label.set_text(text);
    }
}
