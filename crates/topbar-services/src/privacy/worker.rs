//! The thread that talks to PipeWire.
//!
//! PipeWire's client library is built around a C main loop that owns the
//! connection and dispatches its callbacks; it is neither `Send` nor
//! integrable with tokio. So it gets a thread of its own and publishes through
//! a watch channel, which is exactly the arrangement the audio service uses for
//! libpulse's threaded main loop — one precedent, followed rather than
//! reinvented.
//!
//! **This replaces v1's `pw-dump` poller.** v1 ran a subprocess once a second,
//! parsed its JSON, and compared the answer to the last one: 86,400 process
//! spawns a day to notice something that happens twice. Here the registry tells
//! us, and the thread sleeps between events.
//!
//! The connection is **read-only** by construction: the registry is enumerated
//! and its `global` events are listened to, and nothing is ever created,
//! destroyed or configured. That matters more here than usual, because unlike
//! every other service in the crate this one talks to the *user's real session*
//! — there is no meaningful way to fake a PipeWire graph, and no reason to,
//! because reading the graph is the entire feature.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use pipewire::proxy::{Listener, ProxyT};
use pipewire::types::ObjectType;
use tokio::sync::watch;
use tracing::{debug, info, warn};

use super::PrivacyState;
use super::graph::{Graph, Link, Node};

/// Follow the node graph until the panel stops listening.
///
/// Runs on a thread of its own, started once. Returns when PipeWire's main loop
/// ends, which happens when the daemon goes away — the panel simply reports
/// nothing after that rather than reconnecting in a loop, because a session
/// whose PipeWire has died has larger problems than a missing dot.
pub(super) fn run(publisher: watch::Sender<Arc<PrivacyState>>) {
    // SAFETY: `pipewire::init` must be called once before anything else in the
    // library, and this thread is the only place the library is used.
    pipewire::init();

    if let Err(error) = follow(&publisher) {
        info!("privacy: not following PipeWire ({error}); the screen-share dot is off");
        // Whatever went wrong, the panel must not be left claiming a screen is
        // being shared.
        let _ = publisher.send(Arc::new(PrivacyState::default()));
    }
}

/// The link proxies and listeners the graph is keeping alive.
type Bound = Rc<RefCell<Vec<(Box<dyn ProxyT>, Box<dyn Listener>)>>>;

/// Connect, enumerate, and run until the loop ends.
fn follow(publisher: &watch::Sender<Arc<PrivacyState>>) -> Result<(), pipewire::Error> {
    let mainloop = pipewire::main_loop::MainLoopRc::new(None)?;
    let context = pipewire::context::ContextRc::new(&mainloop, None)?;
    let core = context.connect_rc(None)?;
    let registry = core.get_registry_rc()?;
    let registry_weak = registry.downgrade();

    let graph = Rc::new(RefCell::new(Graph::new()));
    // Link proxies and their listeners have to outlive the callback that made
    // them, or the `info` event carrying the link's state never arrives.
    let bound: Bound = Rc::new(RefCell::new(Vec::new()));
    let published = Rc::new(RefCell::new(false));

    let _listener = registry
        .add_listener_local()
        .global({
            let graph = Rc::clone(&graph);
            let bound = Rc::clone(&bound);
            let published = Rc::clone(&published);
            let publisher = publisher.clone();
            move |global| {
                let Some(properties) = &global.props else {
                    return;
                };
                match global.type_ {
                    ObjectType::Node => {
                        graph.borrow_mut().add_node(
                            global.id,
                            Node {
                                media_class: properties.get("media.class").map(str::to_string),
                                name: properties.get("node.name").map(str::to_string),
                                description: properties.get("node.description").map(str::to_string),
                                application: properties.get("application.name").map(str::to_string),
                            },
                        );
                    }
                    ObjectType::Link => {
                        let (Some(output), Some(input)) = (
                            properties.get("link.output.node").and_then(parse_id),
                            properties.get("link.input.node").and_then(parse_id),
                        ) else {
                            return;
                        };
                        // A link's *state* is not in its registry properties; it
                        // arrives on the proxy's `info` event, so the link is
                        // recorded inactive and bound.
                        graph.borrow_mut().add_link(
                            global.id,
                            Link {
                                output,
                                input,
                                active: false,
                            },
                        );
                        let Some(registry) = registry_weak.upgrade() else {
                            return;
                        };
                        let Ok(proxy) = registry.bind::<pipewire::link::Link, _>(global) else {
                            return;
                        };
                        let listener = proxy
                            .add_listener_local()
                            .info({
                                let graph = Rc::clone(&graph);
                                let published = Rc::clone(&published);
                                let publisher = publisher.clone();
                                let id = global.id;
                                move |info| {
                                    let active =
                                        matches!(info.state(), pipewire::link::LinkState::Active);
                                    graph.borrow_mut().set_link_active(id, active);
                                    publish(&publisher, &graph.borrow(), &published);
                                }
                            })
                            .register();
                        bound
                            .borrow_mut()
                            .push((Box::new(proxy), Box::new(listener)));
                    }
                    _ => {}
                }
                publish(&publisher, &graph.borrow(), &published);
            }
        })
        .global_remove({
            let graph = Rc::clone(&graph);
            let bound = Rc::clone(&bound);
            let published = Rc::clone(&published);
            let publisher = publisher.clone();
            move |id| {
                graph.borrow_mut().remove(id);
                // The proxy for a link that has gone is dead weight; dropping it
                // is what keeps a session of six hours from accumulating one per
                // stream that ever existed.
                bound
                    .borrow_mut()
                    .retain(|(proxy, _)| proxy.upcast_ref().id() != id);
                publish(&publisher, &graph.borrow(), &published);
            }
        })
        .register();

    debug!("privacy: following PipeWire's node graph");
    // Runs until the daemon goes away or the process ends. There is nothing to
    // poll and nothing to wake up for in between.
    mainloop.run();
    warn!("privacy: PipeWire's main loop ended");
    Ok(())
}

/// Publish the verdict, if it changed.
fn publish(
    publisher: &watch::Sender<Arc<PrivacyState>>,
    graph: &Graph,
    published: &Rc<RefCell<bool>>,
) {
    let sharing = graph.screen_sharing();
    if *published.borrow() == sharing {
        return;
    }
    *published.borrow_mut() = sharing;
    if sharing {
        info!(
            "privacy: a screen is being shared ({})",
            graph.describe().unwrap_or_else(|| "unknown".to_string())
        );
    } else {
        info!("privacy: the screen is no longer being shared");
    }
    let _ = publisher.send(Arc::new(PrivacyState {
        screen_sharing: sharing,
    }));
}

/// A node id out of a link's properties, which carry them as strings.
fn parse_id(value: &str) -> Option<u32> {
    value.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_names_its_ends_as_strings() {
        assert_eq!(parse_id("42"), Some(42));
        assert_eq!(parse_id(" 42 "), Some(42));
        assert_eq!(parse_id(""), None);
        assert_eq!(parse_id("-1"), None);
        assert_eq!(parse_id("not a number"), None);
    }
}
