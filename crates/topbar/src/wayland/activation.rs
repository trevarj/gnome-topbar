//! xdg-activation tokens, so an app can raise itself when its notification is
//! acted on.
//!
//! Ported from v1 `services/activation_token.rs`. Clicking "Reply" on a banner
//! is meant to bring the chat window forward, and on Wayland an application may
//! only raise itself when it can show the compositor a token proving a user
//! asked for it. The panel is the one holding that proof, so it asks the
//! compositor for a token and hands it over in the `ActivationToken` signal
//! immediately before `ActionInvoked`.
//!
//! Everything here is best effort: a compositor without `xdg_activation_v1`
//! simply produces `None`, the action still fires, and the app just does not
//! get focus.

use gdk4_wayland::prelude::*;
use gtk4::gdk;
use tracing::{debug, warn};
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle};
use wayland_protocols::xdg::activation::v1::client::{
    xdg_activation_token_v1::{self, XdgActivationTokenV1},
    xdg_activation_v1::{self, XdgActivationV1},
};

/// How many round trips to wait for the compositor to mint a token.
const MAX_ROUNDTRIPS: usize = 3;

/// What the token request collects as the compositor answers.
#[derive(Default)]
struct State {
    manager: Option<XdgActivationV1>,
    token: Option<String>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _data: &(),
        _conn: &Connection,
        queue: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
            && interface == "xdg_activation_v1"
        {
            state.manager = Some(registry.bind(name, version.min(1), queue, ()));
        }
    }
}

impl Dispatch<XdgActivationV1, ()> for State {
    fn event(
        _state: &mut Self,
        _proxy: &XdgActivationV1,
        _event: xdg_activation_v1::Event,
        _data: &(),
        _conn: &Connection,
        _queue: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<XdgActivationTokenV1, ()> for State {
    fn event(
        state: &mut Self,
        proxy: &XdgActivationTokenV1,
        event: xdg_activation_token_v1::Event,
        _data: &(),
        _conn: &Connection,
        _queue: &QueueHandle<Self>,
    ) {
        if let xdg_activation_token_v1::Event::Done { token } = event {
            state.token = Some(token);
            proxy.destroy();
        }
    }
}

/// Ask the compositor for a token letting `app_id` raise itself.
///
/// `surface` is the panel surface the click landed on, which is what makes the
/// request a *user* activation as far as the compositor is concerned.
pub fn token(app_id: Option<&str>, surface: Option<&gdk::Surface>) -> Option<String> {
    let display = gdk::Display::default()?
        .downcast::<gdk4_wayland::WaylandDisplay>()
        .ok()?;
    if !display.query_registry("xdg_activation_v1") {
        debug!("the compositor does not offer xdg_activation_v1; actions cannot raise windows");
        return None;
    }

    let connection = connection(&display)?;
    let mut queue: EventQueue<State> = connection.new_event_queue();
    let handle = queue.handle();
    let _registry = connection.display().get_registry(&handle, ());

    let mut state = State::default();
    if let Err(error) = queue.roundtrip(&mut state) {
        warn!("could not enumerate Wayland globals: {error}");
        return None;
    }

    let manager = state.manager.as_ref()?;
    let token = manager.get_activation_token(&handle, ());
    if let Some(app_id) = app_id.map(str::trim).filter(|id| !id.is_empty()) {
        token.set_app_id(app_id.to_string());
    }
    match surface
        .and_then(|surface| surface.downcast_ref::<gdk4_wayland::WaylandSurface>())
        .and_then(gdk4_wayland::WaylandSurface::wl_surface)
    {
        Some(surface) => token.set_surface(&surface),
        None => debug!("no Wayland surface for the activation token; it may be refused"),
    }
    token.commit();

    for _ in 0..MAX_ROUNDTRIPS {
        if state.token.is_some() {
            break;
        }
        if let Err(error) = queue.roundtrip(&mut state) {
            warn!("could not obtain an activation token: {error}");
            return None;
        }
    }
    state.token
}

/// Borrow GTK's own Wayland connection rather than opening a second one.
fn connection(display: &gdk4_wayland::WaylandDisplay) -> Option<Connection> {
    use wayland_client::Proxy;

    let backend = display.wl_display()?.backend().upgrade()?;
    Some(Connection::from_backend(backend))
}
