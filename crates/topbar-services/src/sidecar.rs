//! What every fake sidecar does when its bus goes away.
//!
//! The `topbar-fake-*` binaries exist for the nested-niri smoke runs: each one
//! takes a bus name on the run's **private** `dbus-daemon` and serves a
//! NetworkManager, a BlueZ, a UPower, a media player or a tray item that the
//! panel under test can talk to without anything on the developer's own
//! session hearing about it.
//!
//! A private bus lives exactly as long as the run. A sidecar parked on
//! `pending()` does not: when the run is interrupted — `Ctrl-C`, a driver that
//! died, a `timeout` that fired — the daemon goes and the fakes stay, and an
//! interrupted afternoon leaves a pile of them alive. The smoke scripts have
//! `pkill` traps for exactly this, but a trap only runs if the shell that set
//! it is still there to run it.
//!
//! So the fakes end themselves. Everything they do is on one connection, and
//! when that connection closes there is nobody left to serve.

use std::future::Future;

use zbus::Connection;

/// Wait for `stop`, or for `connection` to close, whichever comes first.
///
/// `what` names the sidecar in the line it prints on the way out, which is the
/// only trace it leaves in the run's log.
pub async fn park(connection: &Connection, what: &str, stop: impl Future<Output = ()>) {
    tokio::select! {
        () = stop => {}
        () = connection.closed() => {
            eprintln!("topbar-fake-{what}: the bus closed; exiting");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::private_bus::private_bus;

    #[tokio::test]
    async fn a_sidecar_outlives_its_bus_by_no_more_than_a_moment() {
        // The whole point, as a test: the bus goes, the sidecar goes. One fake
        // stands for all of them — they share this function and nothing else
        // about the shape differs.
        let bus = private_bus!();
        let connection = zbus::connection::Builder::address(bus.address())
            .expect("a well-formed private bus address")
            .build()
            .await
            .expect("the connection is made");

        let parked = tokio::spawn({
            let connection = connection.clone();
            async move {
                // A `stop` that never comes, which is what every fake's park
                // future is until something asks it to quit.
                park(&connection, "test", std::future::pending::<()>()).await;
            }
        });

        // Kill the daemon the way an interrupted run does.
        drop(bus);

        tokio::time::timeout(Duration::from_secs(2), parked)
            .await
            .expect("the sidecar has two seconds to notice the bus is gone")
            .expect("the task did not panic");
    }

    #[tokio::test]
    async fn being_told_to_stop_still_works_while_the_bus_is_up() {
        let bus = private_bus!();
        let connection = zbus::connection::Builder::address(bus.address())
            .expect("a well-formed private bus address")
            .build()
            .await
            .expect("the connection is made");

        let (quit, wait) = tokio::sync::oneshot::channel::<()>();
        let parked = tokio::spawn({
            let connection = connection.clone();
            async move {
                park(&connection, "test", async {
                    let _ = wait.await;
                })
                .await;
            }
        });

        quit.send(()).expect("the sidecar is listening");
        tokio::time::timeout(Duration::from_secs(2), parked)
            .await
            .expect("a sidecar told to quit quits")
            .expect("the task did not panic");
        assert!(!connection.is_closed(), "and not because the bus went away");
    }
}
