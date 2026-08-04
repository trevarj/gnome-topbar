//! The power-profiles daemon: which of power-saver, balanced and performance
//! the machine is running, and how to change it.
//!
//! ```text
//!   model.rs   the published snapshot and the identifier→name/icon map (pure)
//!   task.rs    the one owner: the D-Bus client, commands in, readings out
//!   fake.rs    a daemon of a test's own, on both bus names
//! ```
//!
//! The daemon has been renamed once. `power-profiles-daemon` 0.20 moved from
//! `net.hadess.PowerProfiles` to `org.freedesktop.UPower.PowerProfiles` and
//! kept the old name working; both carry the same interface at a path of their
//! own. The panel therefore tries the new name first and falls back to the old
//! one, which is the difference between the Power Mode toggle appearing on a
//! current NixOS and appearing on a machine that has not updated in two years.
//!
//! A machine with neither publishes `available: false` and the toggle is not
//! drawn at all — a greyed-out Power Mode on a desktop that never had profiles
//! would be a row of dead space explaining an absence nobody asked about.

#[cfg(any(test, feature = "fake-power"))]
pub mod fake;
mod model;
mod task;

#[cfg(test)]
mod bus_tests;

use std::sync::Arc;

use tokio::sync::{mpsc, oneshot, watch};

use crate::error::SvcError;

pub use model::{PowerProfilesState, ProfileView, icon, label};

/// How many commands may be in flight before a sender waits.
const QUEUE: usize = 8;

/// Where the daemon can be found, newest name first.
///
/// The interface name is the bus name in both cases, which is what lets one
/// generic proxy serve both without a second `#[zbus::proxy]` trait.
pub(crate) const ENDPOINTS: &[Endpoint] = &[
    Endpoint {
        name: "org.freedesktop.UPower.PowerProfiles",
        path: "/org/freedesktop/UPower/PowerProfiles",
    },
    Endpoint {
        name: "net.hadess.PowerProfiles",
        path: "/net/hadess/PowerProfiles",
    },
];

/// One name the daemon may answer to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Endpoint {
    /// The well-known bus name, which is also the interface name.
    pub(crate) name: &'static str,
    /// The object the interface lives on.
    pub(crate) path: &'static str,
}

/// The power-profiles service.
///
/// Cloning is cheap — a channel sender and a watch subscription.
#[derive(Clone)]
pub struct PowerProfiles {
    handle: PowerProfilesHandle,
    state: watch::Receiver<Arc<PowerProfilesState>>,
}

impl PowerProfiles {
    /// Start following the daemon.
    ///
    /// `address` overrides the system bus, for the bus tests and for the smoke
    /// run's stand-in daemon.
    pub(crate) fn start(address: Option<String>) -> Self {
        let (commands, queue) = mpsc::channel(QUEUE);
        let (publisher, state) = watch::channel(Arc::new(PowerProfilesState::default()));
        tokio::spawn(task::run(queue, publisher, address));
        Self {
            handle: PowerProfilesHandle { commands },
            state,
        }
    }

    /// The handle commands are sent through.
    pub fn handle(&self) -> &PowerProfilesHandle {
        &self.handle
    }

    /// Subscribe to power-profile state.
    pub fn state(&self) -> watch::Receiver<Arc<PowerProfilesState>> {
        self.state.clone()
    }

    /// The state as of right now.
    pub fn current(&self) -> Arc<PowerProfilesState> {
        self.state.borrow().clone()
    }
}

/// What the panel may ask of the daemon.
#[derive(Clone)]
pub struct PowerProfilesHandle {
    commands: mpsc::Sender<task::Command>,
}

impl PowerProfilesHandle {
    /// Put the machine into `id`.
    ///
    /// Optimistic: the snapshot moves to the new profile before the call
    /// leaves, so the radio row the user clicked marks itself immediately. A
    /// refusal puts it back and returns the error, which the Quick Settings
    /// row renders under itself.
    pub async fn set_profile(&self, id: String) -> Result<(), SvcError> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(task::Command { profile: id, reply })
            .await
            .map_err(|_| SvcError::ServiceStopped("power profiles"))?;
        answer
            .await
            .map_err(|_| SvcError::ServiceStopped("power profiles"))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_new_bus_name_is_tried_before_the_old_one() {
        assert_eq!(ENDPOINTS[0].name, "org.freedesktop.UPower.PowerProfiles");
        assert_eq!(ENDPOINTS[1].name, "net.hadess.PowerProfiles");
    }

    #[test]
    fn every_endpoint_path_matches_its_name() {
        // The daemon serves each name at a path derived from it; a mismatched
        // pair would fail at run time on a machine nobody tests on.
        for endpoint in ENDPOINTS {
            let expected = format!("/{}", endpoint.name.replace('.', "/"));
            assert_eq!(
                endpoint.path, expected,
                "{} is served elsewhere",
                endpoint.name
            );
        }
    }

    #[tokio::test]
    async fn commands_against_a_stopped_service_report_it() {
        let (commands, queue) = mpsc::channel(1);
        drop(queue);
        let handle = PowerProfilesHandle { commands };
        let error = handle
            .set_profile("balanced".into())
            .await
            .expect_err("nothing is listening");
        assert!(matches!(error, SvcError::ServiceStopped("power profiles")));
    }
}
