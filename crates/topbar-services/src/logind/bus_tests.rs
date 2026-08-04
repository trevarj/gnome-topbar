//! A systemd-logind that lives exactly as long as one test.
//!
//! Serves the three methods the panel calls, on a private bus of the test's
//! own: `Inhibit`, which hands back a real pipe descriptor so the fd's lifetime
//! can be *observed* rather than assumed; `GetSessionByPID`, so the brightness
//! service finds a session; and `Session.SetBrightness`, which records every
//! call so the throttle's effect is a number a test can read.
//!
//! Nothing here touches the system bus. Taking a real inhibitor lock on the
//! developer's machine — or setting their screen brightness — during
//! `cargo test` would be unforgivable.

use std::os::fd::{FromRawFd, OwnedFd as StdOwnedFd};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use zbus::zvariant::{OwnedFd, OwnedObjectPath};

use crate::private_bus::PrivateBus;

/// Where logind's manager lives.
pub(crate) const MANAGER_PATH: &str = "/org/freedesktop/login1";
/// The session the fake reports for every process.
pub(crate) const SESSION_PATH: &str = "/org/freedesktop/login1/session/c1";
/// How long a bus test waits for something before giving up.
pub(crate) const PATIENCE: Duration = Duration::from_secs(10);

/// One `Inhibit` call, as the fake saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InhibitCall {
    pub(crate) what: String,
    pub(crate) who: String,
    pub(crate) why: String,
    pub(crate) mode: String,
}

/// What the fake has been asked to do, shared across restarts.
#[derive(Debug, Default)]
pub(crate) struct Journal {
    pub(crate) inhibits: Vec<InhibitCall>,
    /// The read end of every lock handed out, kept so the test can watch it.
    pub(crate) locks: Vec<StdOwnedFd>,
    pub(crate) brightness: Vec<(String, String, u32)>,
    /// Every power action asked for: the method name and its `interactive`
    /// flag. A test that expects *nothing* to have happened reads this too.
    pub(crate) power: Vec<(String, bool)>,
}

/// A shared journal.
pub(crate) type Log = Arc<Mutex<Journal>>;

/// Lock through poisoning: the journal is plain data.
pub(crate) fn journal(log: &Log) -> std::sync::MutexGuard<'_, Journal> {
    log.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The manager half of the fake.
pub(crate) struct FakeManager {
    log: Log,
}

#[zbus::interface(name = "org.freedesktop.login1.Manager")]
impl FakeManager {
    /// Hand back one end of a fresh pipe, and keep the other.
    ///
    /// The client's end is the lock: while it is open the pipe stays open, and
    /// the moment it is dropped the end this keeps reads end-of-file. That is
    /// how a test tells "released" from "still held" without asking the panel.
    fn inhibit(&self, what: &str, who: &str, why: &str, mode: &str) -> zbus::fdo::Result<OwnedFd> {
        let (read, write) = pipe().map_err(|error| {
            zbus::fdo::Error::Failed(format!("could not make an inhibitor pipe: {error}"))
        })?;

        let mut journal = journal(&self.log);
        journal.inhibits.push(InhibitCall {
            what: what.to_string(),
            who: who.to_string(),
            why: why.to_string(),
            mode: mode.to_string(),
        });
        journal.locks.push(read);
        Ok(OwnedFd::from(write))
    }

    #[zbus(name = "GetSessionByPID")]
    fn get_session_by_pid(&self, _pid: u32) -> zbus::fdo::Result<OwnedObjectPath> {
        OwnedObjectPath::try_from(SESSION_PATH)
            .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))
    }

    fn list_sessions(&self) -> Vec<(String, u32, String, String, OwnedObjectPath)> {
        Vec::new()
    }

    #[zbus(name = "PowerOff")]
    fn power_off(&self, interactive: bool) {
        self.record("PowerOff", interactive);
    }

    fn reboot(&self, interactive: bool) {
        self.record("Reboot", interactive);
    }

    fn suspend(&self, interactive: bool) {
        self.record("Suspend", interactive);
    }

    #[zbus(name = "CanPowerOff")]
    fn can_power_off(&self) -> String {
        "yes".to_string()
    }

    fn can_reboot(&self) -> String {
        "challenge".to_string()
    }

    /// Deliberately unavailable, so the disabled-row path has something real
    /// to be driven by.
    fn can_suspend(&self) -> String {
        "na".to_string()
    }
}

impl FakeManager {
    /// Note a power action rather than carrying one out.
    fn record(&self, method: &str, interactive: bool) {
        journal(&self.log)
            .power
            .push((method.to_string(), interactive));
    }
}

/// The session half.
pub(crate) struct FakeSession {
    log: Log,
    /// How long each call takes to answer.
    ///
    /// A test that wants to *see* the throttle coalesce needs the call to take
    /// longer than the burst does to arrive; zero is the default everywhere
    /// else.
    delay: Duration,
    /// The sysfs tree to write through to, as the real logind does.
    ///
    /// Without it the service would re-read the file after every call and find
    /// the value it started with, which is a fake that behaves differently from
    /// the thing it stands in for.
    sysfs: Option<std::path::PathBuf>,
}

#[zbus::interface(name = "org.freedesktop.login1.Session")]
impl FakeSession {
    async fn set_brightness(&self, subsystem: &str, name: &str, brightness: u32) {
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        if let Some(root) = &self.sysfs {
            let _ = std::fs::write(root.join(name).join("brightness"), brightness.to_string());
        }
        journal(&self.log)
            .brightness
            .push((subsystem.to_string(), name.to_string(), brightness));
    }
}

/// Serve a logind on `bus`, recording into `log`.
pub(crate) async fn serve_logind(
    bus: &PrivateBus,
    log: &Log,
    delay: Duration,
    sysfs: Option<std::path::PathBuf>,
) -> zbus::Connection {
    zbus::connection::Builder::address(bus.address())
        .expect("a well-formed private bus address")
        .name(super::BUS_NAME)
        .expect("a well-formed bus name")
        .serve_at(
            MANAGER_PATH,
            FakeManager {
                log: Arc::clone(log),
            },
        )
        .expect("the manager path is free")
        .serve_at(
            SESSION_PATH,
            FakeSession {
                log: Arc::clone(log),
                delay,
                sysfs,
            },
        )
        .expect("the session path is free")
        .build()
        .await
        .expect("the fake logind starts")
}

/// A non-blocking pipe: `(read, write)`.
fn pipe() -> std::io::Result<(StdOwnedFd, StdOwnedFd)> {
    let mut ends = [0; 2];
    // SAFETY: a two-element array of the size `pipe2` writes into, and flags it
    // documents. The descriptors are handed straight to `OwnedFd`, which closes
    // them.
    let made = unsafe { libc::pipe2(ends.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) };
    if made != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: both descriptors were just created by `pipe2` and are owned here.
    unsafe {
        Ok((
            StdOwnedFd::from_raw_fd(ends[0]),
            StdOwnedFd::from_raw_fd(ends[1]),
        ))
    }
}

/// Whether the other end of `lock` has been closed.
///
/// A non-blocking read of a pipe nobody has written to answers `EAGAIN` while
/// the far end is open, and zero — end of file — once it is not.
pub(crate) fn is_released(lock: &StdOwnedFd) -> bool {
    use std::os::fd::AsRawFd;

    let mut byte = 0u8;
    // SAFETY: a one-byte buffer and a descriptor owned by `lock`.
    let read = unsafe { libc::read(lock.as_raw_fd(), std::ptr::from_mut(&mut byte).cast(), 1) };
    read == 0
}

/// Wait for `predicate`, or fail saying what was being waited for.
pub(crate) async fn wait_for(what: &str, mut predicate: impl FnMut() -> bool) {
    let wait = async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    tokio::time::timeout(PATIENCE, wait)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}
