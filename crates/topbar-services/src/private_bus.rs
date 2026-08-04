//! A `dbus-daemon` that lives exactly as long as one test.
//!
//! Every bus test in this crate starts its own daemon and points both the
//! panel and its stand-in applications at it *by explicit address*. Nothing
//! here reads `$DBUS_SESSION_BUS_ADDRESS`, which is what makes it safe to run
//! `cargo test` on a live desktop: the developer's notification daemon is
//! never replaced and the music they are listening to is never touched.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};

use zbus::Connection;

/// A private session bus.
pub(crate) struct PrivateBus {
    child: Child,
    address: String,
}

impl PrivateBus {
    /// Start one, or `None` when this machine cannot run a bus.
    ///
    /// `dbus-daemon` needs a machine id, which a Nix build sandbox does not
    /// have, so these tests run in the dev shell and on a real desktop and sit
    /// out `nix flake check`. Everything they cover about *behaviour* is also
    /// covered by tests that need no bus at all; what is only covered here is
    /// the wire protocol itself.
    pub(crate) fn start() -> Option<Self> {
        let mut child = Command::new("dbus-daemon")
            .args(["--session", "--print-address", "--nofork"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;

        let stdout = child.stdout.take().expect("piped stdout");
        let mut address = String::new();
        let read = BufReader::new(stdout).read_line(&mut address);
        let address = address.trim().to_string();

        if read.is_err() || !address.starts_with("unix:") {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }

        Some(Self { child, address })
    }

    /// The address to hand to whatever should join this bus.
    pub(crate) fn address(&self) -> &str {
        &self.address
    }

    /// A fresh client connection to this bus.
    pub(crate) async fn connect(&self) -> Connection {
        zbus::connection::Builder::address(self.address.as_str())
            .expect("a well-formed private bus address")
            .build()
            .await
            .expect("the private bus accepts connections")
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Start a bus, or explain why the test is being skipped.
macro_rules! private_bus {
    () => {
        match $crate::private_bus::PrivateBus::start() {
            Some(bus) => bus,
            None => {
                eprintln!("skipping: no private bus available (dbus-daemon needs a machine id)");
                return;
            }
        }
    };
}

pub(crate) use private_bus;
