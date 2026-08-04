//! The two things the panel asks systemd-logind for.
//!
//! **Brightness**, because `Session.SetBrightness` writes to a sysfs file the
//! user has no permission on, without a setuid helper and without shelling out
//! to `brightnessctl`; and **inhibitors**, because `Manager.Inhibit` hands back
//! a file descriptor whose mere existence keeps the machine awake, and whose
//! closing releases it — including when the panel is killed, since the kernel
//! closes it for us. Neither has an alternative worth having.
//!
//! Both live on the *system* bus. Nothing here connects to it eagerly: a
//! machine without logind is a machine where the brightness falls back to
//! sysfs and the inhibitor toggle is simply absent.

#[cfg(test)]
pub(crate) mod bus_tests;

use zbus::Connection;
use zbus::zvariant::{OwnedFd, OwnedObjectPath};

/// The bus name logind owns.
pub(crate) const BUS_NAME: &str = "org.freedesktop.login1";

/// The manager, trimmed to what the panel calls.
#[zbus::proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1"
)]
pub(crate) trait Manager {
    /// Take an inhibitor lock; holding the returned descriptor holds the lock.
    fn inhibit(&self, what: &str, who: &str, why: &str, mode: &str) -> zbus::Result<OwnedFd>;

    /// The session a process belongs to.
    #[zbus(name = "GetSessionByPID")]
    fn get_session_by_pid(&self, pid: u32) -> zbus::Result<OwnedObjectPath>;

    /// Every session: id, uid, user, seat, object path.
    fn list_sessions(&self) -> zbus::Result<Vec<(String, u32, String, String, OwnedObjectPath)>>;
}

/// A session, trimmed to the one method the panel calls on it.
#[zbus::proxy(
    interface = "org.freedesktop.login1.Session",
    default_service = "org.freedesktop.login1"
)]
pub(crate) trait Session {
    /// Set a sysfs-backed device's brightness, in the device's own units.
    fn set_brightness(&self, subsystem: &str, name: &str, brightness: u32) -> zbus::Result<()>;
}

/// Connect to the system bus, or to the address a test handed us.
pub(crate) async fn connect(address: Option<&str>) -> zbus::Result<Connection> {
    match address {
        Some(address) => zbus::connection::Builder::address(address)?.build().await,
        None => Connection::system().await,
    }
}

/// Find the session whose brightness this panel is allowed to set.
///
/// Asking by process id is the right answer and usually works. It does not
/// when the panel was started outside a session scope — a user service on some
/// systemd configurations — so the fallback is the graphical seat, and after
/// that any session at all. Guessing wrong costs one refused call and a
/// fallback to sysfs; not guessing costs the feature.
pub(crate) async fn session_path(manager: &ManagerProxy<'_>) -> Option<OwnedObjectPath> {
    if let Ok(path) = manager.get_session_by_pid(std::process::id()).await {
        return Some(path);
    }

    let sessions = manager.list_sessions().await.ok()?;
    pick_session(&sessions)
}

/// Pick a session from what `ListSessions` returned.
///
/// Split out from the call so the preference order is testable without a bus.
pub(crate) fn pick_session(
    sessions: &[(String, u32, String, String, OwnedObjectPath)],
) -> Option<OwnedObjectPath> {
    sessions
        .iter()
        .find(|(_, _, _, seat, _)| seat == "seat0")
        .or_else(|| sessions.first())
        .map(|(_, _, _, _, path)| path.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, seat: &str) -> (String, u32, String, String, OwnedObjectPath) {
        (
            id.to_string(),
            1000,
            "trev".to_string(),
            seat.to_string(),
            OwnedObjectPath::try_from(format!("/org/freedesktop/login1/session/{id}"))
                .expect("a well-formed object path"),
        )
    }

    #[test]
    fn the_graphical_seat_is_preferred() {
        let sessions = [session("c1", ""), session("c2", "seat0")];
        assert_eq!(
            pick_session(&sessions).map(|path| path.to_string()),
            Some("/org/freedesktop/login1/session/c2".to_string())
        );
    }

    #[test]
    fn any_session_beats_no_session() {
        let sessions = [session("c7", "")];
        assert_eq!(
            pick_session(&sessions).map(|path| path.to_string()),
            Some("/org/freedesktop/login1/session/c7".to_string())
        );
    }

    #[test]
    fn no_sessions_means_no_answer() {
        assert_eq!(pick_session(&[]), None);
    }
}
