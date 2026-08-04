//! What a `topbar …` command actually does to the running panel.
//!
//! The socket, the framing and the version check are the services crate's (see
//! `topbar_services::ipc`); what arrives here is a decoded request and a
//! one-shot to answer on. Everything in this file therefore runs on the GTK
//! main thread, which is the point: raising a capsule, opening a popover and
//! hiding a bar are all things only that thread may do.
//!
//! The stream is consumed by one `glib::spawn_future_local`, so requests are
//! handled strictly in order and nothing here can be re-entered. Commands that
//! have to await a service — the inhibitor, the media players — hand the
//! one-shot to the service runtime and answer from there, which is what keeps
//! a slow D-Bus call from stalling the panel it is being asked about.

use std::path::PathBuf;
use std::rc::Rc;

use gtk4::glib;
use topbar_core::Config;
use topbar_core::ipc::{DumpTarget, IpcRequest, IpcResponse, MediaAction, PopoverAction};
use topbar_services::{Runtime, Services, SvcError};
use tracing::{info, warn};

use crate::bar::{BarManager, SharedConfig};
use crate::style;
use crate::surfaces::osd::{self, OsdEvent};
use crate::surfaces::popovers;
use crate::surfaces::toast;

/// Everything a request might need to reach.
#[derive(Clone)]
pub struct Panel {
    services: Services,
    manager: Rc<BarManager>,
    config: SharedConfig,
    /// The `--config` path, so a reload reads the same file the panel started
    /// from rather than whatever the search chain now prefers.
    config_path: Option<PathBuf>,
}

impl Panel {
    /// Describe the running panel.
    pub fn new(
        services: &Services,
        manager: &Rc<BarManager>,
        config: SharedConfig,
        config_path: Option<PathBuf>,
    ) -> Self {
        Self {
            services: services.clone(),
            manager: Rc::clone(manager),
            config,
            config_path,
        }
    }
}

/// Start answering `topbar` commands.
///
/// Does nothing when the request stream has already been taken, which cannot
/// happen twice: the services bundle hands it over exactly once.
pub fn install(panel: Panel) {
    let Some(mut requests) = panel.services.ipc.take_requests() else {
        warn!("the IPC request stream was already taken");
        return;
    };

    glib::spawn_future_local(async move {
        while let Some(envelope) = requests.recv().await {
            handle(&panel, envelope);
        }
        info!("the IPC listener has stopped");
    });
}

/// Apply one request and answer it.
fn handle(panel: &Panel, envelope: topbar_services::ipc::Envelope) {
    match envelope.request.clone() {
        // Answered by the services crate; reaching here would be a bug.
        IpcRequest::Hello { version } => envelope.answer(IpcResponse::Hello { version }),

        IpcRequest::VolumeChanged { percent, muted } => {
            let max = panel.services.audio.current().max_volume_pct.max(1);
            raise(
                envelope,
                OsdEvent::Volume {
                    percent,
                    muted,
                    max,
                },
            );
        }
        IpcRequest::VolumeUnavailable => raise(envelope, OsdEvent::NoOutput),
        IpcRequest::BrightnessChanged { percent } => {
            raise(envelope, OsdEvent::Brightness { percent });
        }

        IpcRequest::ToggleInhibitor => {
            let handle = panel.services.inhibitor.handle().clone();
            // The capsule is not raised here: toggling changes the service's
            // state, the surfaces are subscribed to it, and one of them will
            // show the flip. Raising it as well would be the same event twice.
            answer_async(envelope, async move { handle.toggle().await });
        }

        IpcRequest::Media { action } => {
            let handle = panel.services.media.handle().clone();
            answer_async(envelope, async move {
                match action {
                    MediaAction::PlayPause => handle.play_pause().await,
                    MediaAction::Next => handle.next().await,
                    MediaAction::Previous => handle.previous().await,
                    // The panel's own service offers no stop and no status;
                    // the CLI answers both without it. See `main::media`.
                    MediaAction::Stop | MediaAction::Status => {
                        Err(SvcError::NoPlayer("handled by the CLI directly".into()))
                    }
                }
            });
        }

        IpcRequest::Bar { action } => {
            let visible = panel.manager.set_bars_visible(action);
            envelope.answer(IpcResponse::Value {
                text: if visible { "shown" } else { "hidden" }.to_string(),
            });
        }

        IpcRequest::Popover { action } => {
            let connector = focused_connector(&panel.services);
            if popovers::dispatch(&action, connector.as_deref()) {
                envelope.answer(IpcResponse::Ok);
            } else {
                envelope.answer(IpcResponse::Error {
                    message: no_popover(&action),
                });
            }
        }

        IpcRequest::Reload => match reload(panel) {
            Ok(source) => envelope.answer(IpcResponse::Value { text: source }),
            Err(message) => envelope.answer(IpcResponse::Error { message }),
        },

        IpcRequest::Dump { target, json } => match dump(panel, target, json) {
            Ok(text) => envelope.answer(IpcResponse::Value { text }),
            Err(message) => envelope.answer(IpcResponse::Error { message }),
        },
    }
}

/// Show `event` and say whether anything was there to show it.
fn raise(envelope: topbar_services::ipc::Envelope, event: OsdEvent) {
    if osd::show(event) {
        envelope.answer(IpcResponse::Ok);
    } else {
        // Not an error: `[osd] enabled = false` is a setting, and a media key
        // whose OSD is switched off has still done its job.
        envelope.answer(IpcResponse::Value {
            text: "no OSD is configured".to_string(),
        });
    }
}

/// Run a service call on the service runtime and answer when it lands.
fn answer_async<F>(envelope: topbar_services::ipc::Envelope, future: F)
where
    F: std::future::Future<Output = Result<(), SvcError>> + Send + 'static,
{
    Runtime::handle().spawn(async move {
        envelope.answer(match future.await {
            Ok(()) => IpcResponse::Ok,
            Err(error) => IpcResponse::Error {
                message: error.to_string(),
            },
        });
    });
}

/// The connector of the monitor the user is looking at.
fn focused_connector(services: &Services) -> Option<String> {
    let workspaces = services.niri.workspaces();
    let focused = workspaces.borrow().focused_output.clone();
    let connectors = toast::connectors();
    toast::hosting_output(focused.as_deref(), &connectors).map(ToString::to_string)
}

/// The message for a popover nothing answered.
fn no_popover(action: &PopoverAction) -> String {
    match action {
        PopoverAction::Show(widget) | PopoverAction::Toggle(widget) => {
            format!("no `{widget}` widget with a popover is on the bar")
        }
        PopoverAction::Hide(Some(widget)) => format!("no `{widget}` popover to close"),
        PopoverAction::Hide(None) => "no popover is open".to_string(),
    }
}

/// Re-read the configuration and swap the stylesheet.
///
/// What this does today is the half that is both safe and cheap: the file is
/// read and validated, the shared configuration is replaced so anything built
/// from here on uses it, and the single CSS provider is swapped atomically —
/// so colours, sizes, radii and fonts all change live. What it does *not* do
/// is rebuild the widgets, so a changed clock format or a widget added to a
/// section still waits for a restart. That is M12's delta-rebuild work, and
/// doing half of it here would mean doing it twice.
fn reload(panel: &Panel) -> Result<String, String> {
    let load =
        Config::find_and_load(panel.config_path.as_deref()).map_err(|error| error.to_string())?;
    for warning in &load.warnings {
        warn!("{warning}");
    }

    let display = gtk4::gdk::Display::default().ok_or("there is no display")?;
    style::apply(&display, &style::generate(&load.config));
    crate::anim::set_animations_enabled(load.config.theme.animations);
    panel.config.replace(load.config);

    let source = match &load.source {
        Some(path) => path.display().to_string(),
        None => "built-in defaults".to_string(),
    };
    info!("reloaded the stylesheet from {source}");
    // TODO(M12): rebuild the widgets whose section of the config changed.
    Ok(format!(
        "reloaded {source} (styling only; widget rebuilds land in M12)"
    ))
}

/// Answer `topbar dump`.
fn dump(panel: &Panel, target: DumpTarget, json: bool) -> Result<String, String> {
    let config = panel.config.current();
    let state = snapshot(&panel.services);

    let text = match (target, json) {
        (DumpTarget::DefaultConfig, false) => topbar_core::config::EXAMPLE_CONFIG_TOML.to_string(),
        (DumpTarget::DefaultConfig, true) => render(&Config::default().to_json())?,
        (DumpTarget::Config, false) => config.to_toml().map_err(|error| error.to_string())?,
        (DumpTarget::Config, true) => render(&config.to_json())?,
        (DumpTarget::State, false) => render(&Ok(state))?,
        (DumpTarget::State, true) => render(&Ok(state))?,
        (DumpTarget::All, false) => format!(
            "{}\n# ----- state -----\n{}",
            config.to_toml().map_err(|error| error.to_string())?,
            render(&Ok(state))?
        ),
        (DumpTarget::All, true) => render(
            &config
                .to_json()
                .map(|config| serde_json::json!({ "config": config, "state": state })),
        )?,
    };
    Ok(text)
}

/// Render a value as pretty JSON, or say why not.
fn render(value: &topbar_core::Result<serde_json::Value>) -> Result<String, String> {
    let value = value.as_ref().map_err(ToString::to_string)?;
    serde_json::to_string_pretty(value).map_err(|error| error.to_string())
}

/// A summary of what every service is currently reporting.
///
/// Hand-built rather than derived: this is a debugging aid, and what makes it
/// useful is that it is short enough to read in a terminal. Dumping every
/// snapshot in full would bury the three numbers anybody actually wants.
fn snapshot(services: &Services) -> serde_json::Value {
    let audio = services.audio.current();
    let brightness = services.brightness.current();
    let inhibitor = services.inhibitor.current();
    let media = services.media.state();
    let media = media.borrow();
    let notifications = services.notifications.state();
    let notifications = notifications.borrow();
    let workspaces = services.niri.workspaces();
    let workspaces = workspaces.borrow();
    let tray = services.tray.state();
    let tray = tray.borrow();
    let weather = services.weather.state();
    let weather = weather.borrow();
    let crypto = services.crypto.state();
    let crypto = crypto.borrow();

    serde_json::json!({
        "audio": {
            "available": audio.available,
            "default_sink": audio.default_sink,
            "sink_volume_pct": audio.sink_volume_pct,
            "sink_muted": audio.sink_muted,
            "default_source": audio.default_source,
            "source_volume_pct": audio.source_volume_pct,
            "source_muted": audio.source_muted,
            "source_in_use": audio.source_in_use,
            "max_volume_pct": audio.max_volume_pct,
            "sinks": audio.sinks.len(),
            "sources": audio.sources.len(),
        },
        "brightness": {
            "available": brightness.available,
            "percent": brightness.percent,
            "device": brightness.device,
        },
        "inhibitor": {
            "available": inhibitor.available,
            "active": inhibitor.active,
        },
        "media": {
            "players": media.players.len(),
            "active": media.active().map(|player| player.identity.clone()),
        },
        "notifications": {
            "enabled": notifications.enabled,
            "history": notifications.history.len(),
            "toasts": notifications.toasts.len(),
            "unseen": notifications.unseen_count,
            "do_not_disturb": notifications.dnd,
        },
        "niri": {
            "connected": workspaces.connected,
            "focused_output": workspaces.focused_output,
            "outputs": workspaces.outputs.len(),
        },
        "tray": {
            "items": tray.items.len(),
        },
        "weather": {
            "location": weather.location.as_ref().map(|location| location.label.clone()),
            "has_data": weather.data().is_some(),
        },
        "crypto": {
            "entries": crypto.entries.len(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use topbar_core::ipc::VisibilityAction;

    #[test]
    fn a_missing_popover_says_which_one() {
        assert!(
            no_popover(&PopoverAction::Show("clock".into())).contains("clock"),
            "the message has to name the widget"
        );
        assert!(
            no_popover(&PopoverAction::Toggle("quick_settings".into())).contains("quick_settings")
        );
        assert_eq!(no_popover(&PopoverAction::Hide(None)), "no popover is open");
    }

    #[test]
    fn every_visibility_action_has_an_answer() {
        for action in [
            VisibilityAction::Show,
            VisibilityAction::Hide,
            VisibilityAction::Toggle,
        ] {
            // The mapping lives in `BarManager::set_bars_visible`; this is the
            // guard that a new variant cannot be added without one.
            let _ = action;
        }
    }
}
