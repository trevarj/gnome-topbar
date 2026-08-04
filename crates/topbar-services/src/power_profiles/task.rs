//! The one owner of the power-profiles connection.
//!
//! Nothing else in the crate holds the proxy, so there is exactly one place
//! that decides which of the two bus names is answering and exactly one place
//! that writes `ActiveProfile`.

use std::collections::HashMap;
use std::sync::Arc;

use futures_util::StreamExt;
use tokio::sync::{mpsc, oneshot, watch};
use tracing::{debug, info};
use zbus::zvariant::OwnedValue;

use super::model::{PowerProfilesState, ProfileView};
use super::{ENDPOINTS, Endpoint};
use crate::error::SvcError;

/// A request to change the profile, and where to answer it.
pub(crate) struct Command {
    pub(crate) profile: String,
    pub(crate) reply: oneshot::Sender<Result<(), SvcError>>,
}

/// The property carrying the profile in force.
const ACTIVE: &str = "ActiveProfile";
/// The property listing what the daemon offers.
const PROFILES: &str = "Profiles";
/// The key naming a profile inside one entry of [`PROFILES`].
const PROFILE_KEY: &str = "Profile";

/// Follow the daemon until every handle is dropped.
pub(crate) async fn run(
    mut commands: mpsc::Receiver<Command>,
    publisher: watch::Sender<Arc<PowerProfilesState>>,
    address: Option<String>,
) {
    let connection = match crate::logind::connect(address.as_deref()).await {
        Ok(connection) => connection,
        Err(error) => {
            info!("no system bus ({error}); power profiles are unavailable");
            return drain(commands).await;
        }
    };
    let dbus = match zbus::fdo::DBusProxy::new(&connection).await {
        Ok(dbus) => dbus,
        Err(error) => {
            info!("cannot ask who is on the bus ({error}); power profiles are unavailable");
            return drain(commands).await;
        }
    };

    // Subscribed before the first look, so a daemon that starts between the
    // two is noticed rather than missed until the next restart.
    let mut owners = owner_changes(&dbus).await;

    loop {
        let running = match endpoint(&dbus).await {
            Some(endpoint) => {
                serve(
                    &connection,
                    endpoint,
                    &mut commands,
                    &publisher,
                    &mut owners,
                )
                .await
            }
            None => {
                publish(&publisher, PowerProfilesState::default());
                idle(&mut commands, &mut owners).await
            }
        };
        if !running {
            break;
        }
    }
}

/// Which of the two names is on the bus, newest first.
async fn endpoint(dbus: &zbus::fdo::DBusProxy<'_>) -> Option<Endpoint> {
    for endpoint in ENDPOINTS {
        let name = zbus::names::BusName::try_from(endpoint.name).ok()?;
        if dbus.name_has_owner(name).await.unwrap_or(false) {
            debug!("power profiles found at {}", endpoint.name);
            return Some(*endpoint);
        }
    }
    None
}

/// Serve one connected daemon. `false` means the panel is shutting down.
async fn serve(
    connection: &zbus::Connection,
    endpoint: Endpoint,
    commands: &mut mpsc::Receiver<Command>,
    publisher: &watch::Sender<Arc<PowerProfilesState>>,
    owners: &mut OwnerChanges,
) -> bool {
    let proxy = match build(connection, endpoint).await {
        Ok(proxy) => proxy,
        Err(error) => {
            debug!("{} did not answer ({error})", endpoint.name);
            publish(publisher, PowerProfilesState::default());
            return idle(commands, owners).await;
        }
    };

    let Some(mut active) = read_active(&proxy).await else {
        debug!("{} has no {ACTIVE}", endpoint.name);
        publish(publisher, PowerProfilesState::default());
        return idle(commands, owners).await;
    };
    let profiles = read_profiles(&proxy, &active).await;
    publish(publisher, snapshot(&active, &profiles));

    let mut changes = proxy.receive_property_changed::<String>(ACTIVE).await;

    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { return false };
                let previous = std::mem::replace(&mut active, command.profile.clone());
                // Optimistic: the radio row the user clicked marks itself
                // before the daemon has been asked, because a control that
                // waits for a round trip reads as a control that missed the
                // click.
                publish(publisher, snapshot(&active, &profiles));

                let answer = proxy
                    .set_property(ACTIVE, command.profile.as_str())
                    .await
                    .map_err(|error| SvcError::PowerProfile(error.to_string()));
                if answer.is_err() {
                    // ...and visibly back, which is the other half of the deal.
                    active = previous;
                    publish(publisher, snapshot(&active, &profiles));
                }
                let _ = command.reply.send(answer);
            }
            Some(change) = changes.next() => {
                match change.get().await {
                    Ok(profile) => {
                        active = profile;
                        publish(publisher, snapshot(&active, &profiles));
                    }
                    // The daemon went away mid-signal; start again.
                    Err(_) => return true,
                }
            }
            Some(()) = owners.next() => return true,
        }
    }
}

/// Wait for a daemon to appear, answering commands meanwhile.
///
/// `false` means the panel is shutting down.
async fn idle(commands: &mut mpsc::Receiver<Command>, owners: &mut OwnerChanges) -> bool {
    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { return false };
                let _ = command.reply.send(Err(SvcError::PowerProfile(
                    "no power-profiles daemon is running".into(),
                )));
            }
            Some(()) = owners.next() => return true,
        }
    }
}

/// Answer every command with "unavailable" rather than blocking a caller.
async fn drain(mut commands: mpsc::Receiver<Command>) {
    while let Some(command) = commands.recv().await {
        let _ = command.reply.send(Err(SvcError::PowerProfile(
            "no power-profiles daemon is running".into(),
        )));
    }
}

/// A proxy for one endpoint. The interface name is the bus name.
async fn build(
    connection: &zbus::Connection,
    endpoint: Endpoint,
) -> zbus::Result<zbus::Proxy<'static>> {
    zbus::proxy::Builder::new(connection)
        .destination(endpoint.name)?
        .path(endpoint.path)?
        .interface(endpoint.name)?
        .cache_properties(zbus::proxy::CacheProperties::Yes)
        .build()
        .await
}

/// The profile in force, or `None` when the interface will not say.
async fn read_active(proxy: &zbus::Proxy<'_>) -> Option<String> {
    proxy.get_property::<String>(ACTIVE).await.ok()
}

/// Exactly the profiles the daemon reports, in its own order.
///
/// A daemon that reports none still has an active profile, and a Power Mode
/// menu with one row in it is more use than an empty one.
async fn read_profiles(proxy: &zbus::Proxy<'_>, active: &str) -> Vec<String> {
    let entries = proxy
        .get_property::<Vec<HashMap<String, OwnedValue>>>(PROFILES)
        .await
        .unwrap_or_default();

    let profiles: Vec<String> = entries.iter().filter_map(profile_name).collect();
    if profiles.is_empty() {
        vec![active.to_string()]
    } else {
        profiles
    }
}

/// The `Profile` key of one entry, when it is a string.
fn profile_name(entry: &HashMap<String, OwnedValue>) -> Option<String> {
    let value = entry.get(PROFILE_KEY)?;
    String::try_from(value.try_clone().ok()?).ok()
}

/// Build a snapshot from the identifiers the daemon uses.
fn snapshot(active: &str, profiles: &[String]) -> PowerProfilesState {
    PowerProfilesState {
        available: true,
        active: Some(ProfileView::new(active)),
        profiles: profiles.iter().map(|id| ProfileView::new(id)).collect(),
    }
}

/// Publish a state, if it is not the one already published.
fn publish(publisher: &watch::Sender<Arc<PowerProfilesState>>, next: PowerProfilesState) {
    publisher.send_if_modified(|current| {
        if **current == next {
            false
        } else {
            *current = Arc::new(next);
            true
        }
    });
}

/// A stream that yields whenever either bus name changes hands.
type OwnerChanges = std::pin::Pin<Box<dyn futures_util::Stream<Item = ()> + Send>>;

/// Watch both names, so a daemon starting or stopping is noticed either way.
async fn owner_changes(dbus: &zbus::fdo::DBusProxy<'_>) -> OwnerChanges {
    let mut merged: Vec<_> = Vec::new();
    for endpoint in ENDPOINTS {
        match dbus
            .receive_name_owner_changed_with_args(&[(0, endpoint.name)])
            .await
        {
            Ok(stream) => merged.push(stream.map(|_| ()).boxed()),
            Err(error) => debug!("cannot watch {} ({error})", endpoint.name),
        }
    }
    if merged.is_empty() {
        return Box::pin(futures_util::stream::pending());
    }
    Box::pin(futures_util::stream::select_all(merged))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(profile: &str) -> HashMap<String, OwnedValue> {
        let mut entry = HashMap::new();
        entry.insert(
            PROFILE_KEY.to_string(),
            OwnedValue::try_from(zbus::zvariant::Value::from(profile)).expect("a string value"),
        );
        entry.insert(
            "Driver".to_string(),
            OwnedValue::try_from(zbus::zvariant::Value::from("placeholder")).expect("a string"),
        );
        entry
    }

    #[test]
    fn a_profile_entry_is_read_by_its_profile_key() {
        assert_eq!(profile_name(&entry("balanced")), Some("balanced".into()));
    }

    #[test]
    fn an_entry_without_a_profile_key_is_skipped() {
        let mut without = HashMap::new();
        without.insert(
            "Driver".to_string(),
            OwnedValue::try_from(zbus::zvariant::Value::from("intel_pstate")).expect("a string"),
        );
        assert_eq!(profile_name(&without), None);
    }

    #[test]
    fn a_snapshot_carries_the_daemons_own_order() {
        let state = snapshot(
            "balanced",
            &[
                "power-saver".into(),
                "balanced".into(),
                "performance".into(),
            ],
        );
        assert!(state.available);
        assert_eq!(state.active_id(), Some("balanced"));
        assert_eq!(
            state
                .profiles
                .iter()
                .map(|profile| profile.id.as_str())
                .collect::<Vec<_>>(),
            ["power-saver", "balanced", "performance"]
        );
    }

    #[test]
    fn publishing_the_same_state_twice_does_not_wake_a_subscriber() {
        let (publisher, mut receiver) = watch::channel(Arc::new(PowerProfilesState::default()));
        receiver.mark_unchanged();

        publish(&publisher, snapshot("balanced", &["balanced".into()]));
        assert!(receiver.has_changed().expect("the channel is alive"));
        receiver.mark_unchanged();

        publish(&publisher, snapshot("balanced", &["balanced".into()]));
        assert!(!receiver.has_changed().expect("the channel is alive"));
    }
}
