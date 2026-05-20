//! XDG activation token helper.
//!
//! Notification action activation on Wayland needs an activation token so the
//! receiving application can ask the compositor to focus/raise its window.

use gdk4_wayland::prelude::*;
use tracing::{debug, warn};
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle};
use wayland_protocols::xdg::activation::v1::client::{
    xdg_activation_token_v1::{self, XdgActivationTokenV1},
    xdg_activation_v1::{self, XdgActivationV1},
};

const MAX_TOKEN_ROUNDTRIPS: usize = 3;

struct ActivationTokenState {
    manager: Option<XdgActivationV1>,
    token: Option<String>,
}

impl ActivationTokenState {
    fn new() -> Self {
        Self {
            manager: None,
            token: None,
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for ActivationTokenState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
            && interface == "xdg_activation_v1"
        {
            debug!("Found xdg_activation_v1 v{version}");
            state.manager = Some(registry.bind(name, version.min(1), qh, ()));
        }
    }
}

impl Dispatch<XdgActivationV1, ()> for ActivationTokenState {
    fn event(
        _state: &mut Self,
        _proxy: &XdgActivationV1,
        _event: xdg_activation_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<XdgActivationTokenV1, ()> for ActivationTokenState {
    fn event(
        state: &mut Self,
        proxy: &XdgActivationTokenV1,
        event: xdg_activation_token_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let xdg_activation_token_v1::Event::Done { token } = event {
            debug!("Received xdg activation token");
            state.token = Some(token);
            proxy.destroy();
        }
    }
}

/// Request a Wayland XDG activation token for passing to an application.
pub fn request_activation_token(
    app_id: Option<&str>,
    surface: Option<&gtk4::gdk::Surface>,
) -> Option<String> {
    let gdk_display = gtk4::gdk::Display::default()?;
    let wayland_display = gdk_display
        .downcast::<gdk4_wayland::WaylandDisplay>()
        .ok()?;

    if !wayland_display.query_registry("xdg_activation_v1") {
        debug!("Compositor does not advertise xdg_activation_v1");
        return None;
    }

    let connection = connection_from_gdk_display(&wayland_display)?;
    let mut event_queue: EventQueue<ActivationTokenState> = connection.new_event_queue();
    let qh = event_queue.handle();

    let display = connection.display();
    let _registry = display.get_registry(&qh, ());

    let mut state = ActivationTokenState::new();
    if let Err(e) = event_queue.roundtrip(&mut state) {
        warn!("Failed xdg activation registry roundtrip: {e}");
        return None;
    }

    let Some(manager) = state.manager.as_ref() else {
        debug!("xdg_activation_v1 not bound after registry roundtrip");
        return None;
    };

    let token = manager.get_activation_token(&qh, ());
    if let Some(app_id) = app_id.filter(|value| !value.is_empty()) {
        token.set_app_id(app_id.to_string());
    }
    if let Some(surface) = surface
        .and_then(|surface| surface.downcast_ref::<gdk4_wayland::WaylandSurface>())
        .and_then(|surface| surface.wl_surface())
    {
        token.set_surface(&surface);
    } else {
        debug!("No Wayland source surface available for xdg activation token");
    }
    token.commit();

    for _ in 0..MAX_TOKEN_ROUNDTRIPS {
        if state.token.is_some() {
            break;
        }
        if let Err(e) = event_queue.roundtrip(&mut state) {
            warn!("Failed xdg activation token roundtrip: {e}");
            return None;
        }
    }

    state.token
}

fn connection_from_gdk_display(
    wayland_display: &gdk4_wayland::WaylandDisplay,
) -> Option<Connection> {
    use wayland_client::Proxy;

    let wl_display = wayland_display.wl_display()?;
    let backend = wl_display.backend().upgrade()?;
    Some(Connection::from_backend(backend))
}
