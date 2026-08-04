//! Application start-up: GTK, the stylesheet, and the bars.

use std::cell::RefCell;
use std::process::ExitCode;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Application, gdk, gio};
use topbar_core::Config;
use topbar_services::Services;
use tracing::{error, info};

use crate::anim;
use crate::bar::{BarManager, SharedConfig};
use crate::style;
use crate::surfaces;

/// The GApplication id.
const APP_ID: &str = "com.github.trevarj.gnome-topbar";

/// Run the panel until the last bar closes.
///
/// `services` is started before GTK so no widget can ever be built against a
/// service that does not exist yet.
pub fn run(config: Config, services: Services) -> ExitCode {
    force_wayland_backend();
    anim::set_animations_enabled(config.theme.animations);

    let config = SharedConfig::new(config);
    let app = Application::builder()
        .application_id(APP_ID)
        // Single-instance is enforced by a runtime lock file (M8), not by
        // D-Bus name ownership, so a second process must reach `main` and
        // report the conflict itself instead of silently activating the first.
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();

    let manager: Rc<RefCell<Option<Rc<BarManager>>>> = Rc::new(RefCell::new(None));
    app.connect_activate(move |app| {
        // GTK can activate more than once; the bars are built exactly once.
        if manager.borrow().is_some() {
            return;
        }
        if let Some(started) = start(app, &config, &services) {
            *manager.borrow_mut() = Some(started);
        } else {
            app.quit();
        }
    });

    // Our own CLI has already parsed argv; GTK must not see it again.
    let status = app.run_with_args::<&str>(&[]);
    if status == gtk4::glib::ExitCode::SUCCESS {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Build the stylesheet and the bars. `None` means the panel cannot run here.
fn start(app: &Application, config: &SharedConfig, services: &Services) -> Option<Rc<BarManager>> {
    let Some(display) = gdk::Display::default() else {
        error!("no display; is a Wayland compositor running?");
        return None;
    };
    if !gtk4_layer_shell::is_supported() {
        error!("the compositor does not support wlr-layer-shell; gnome-topbar needs it");
        return None;
    }

    style::apply(&display, &style::generate(&config.current()));

    let manager = BarManager::new(app, &display, config.clone(), services.clone());
    manager.sync();
    manager.watch_monitors();
    surfaces::popovers::install_smoke_hook();
    info!(
        "gnome-topbar is running (motion {})",
        if anim::motion_enabled() {
            "enabled"
        } else {
            "disabled"
        }
    );
    Some(manager)
}

/// Pin GDK to Wayland unless the user asked for something else.
///
/// The panel is layer-shell only; on a mixed session GDK would otherwise be
/// free to pick X11 and fail at `init_layer_shell`.
fn force_wayland_backend() {
    if std::env::var_os("GDK_BACKEND").is_some() {
        return;
    }
    // SAFETY: this runs before GTK is initialised and before any thread is
    // spawned, so no other thread can be reading the environment.
    unsafe { std::env::set_var("GDK_BACKEND", "wayland") };
}
