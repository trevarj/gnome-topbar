//! The one owner of the backlight.
//!
//! Reads come from sysfs, which is always readable. Writes go through logind
//! when there is a session to write through, and fall back to sysfs when there
//! is not — the fallback works in a container and on a machine with the
//! backlight group opened up, and does nothing at all otherwise, which is the
//! honest outcome.
//!
//! Somebody else changing the brightness — a function key handled by the
//! firmware, `brightnessctl`, another panel — arrives as a udev event, so this
//! never polls. A machine whose udev cannot be watched keeps working: the
//! panel's own writes still update the snapshot, it simply stops noticing
//! other people's.

use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{mpsc, oneshot, watch};
use tracing::{debug, info, warn};

use crate::brightness::device::{self, Backlight};
use crate::brightness::model::BrightnessState;
use crate::brightness::throttle::Throttle;
use crate::change::{ChangeSource, Echoes, Field};
use crate::error::SvcError;
use crate::logind::{self, ManagerProxy, SessionProxy};

/// Who the panel says it is when it takes a session.
const WHO: &str = "topbar";

/// One thing the panel can ask of the backlight.
#[derive(Debug)]
pub(crate) enum Action {
    /// Set it to a percentage.
    Set(u32),
    /// Move it by a signed number of points.
    Step(i32),
}

/// A command, with who sent it and where to answer.
#[derive(Debug)]
pub(crate) struct Command {
    pub(crate) action: Action,
    pub(crate) source: ChangeSource,
    pub(crate) reply: oneshot::Sender<Result<(), SvcError>>,
}

/// Where a write goes.
enum Writer {
    /// Through logind, which is the privilege-safe path.
    Logind(SessionProxy<'static>),
    /// Straight to sysfs, for a machine with no logind session.
    Sysfs,
}

/// Follow the backlight until every handle is dropped.
pub(crate) async fn run(
    mut commands: mpsc::Receiver<Command>,
    publisher: watch::Sender<Arc<BrightnessState>>,
    address: Option<String>,
    root: Option<std::path::PathBuf>,
) {
    let discovered = match &root {
        Some(root) => device::discover_in(root),
        None => device::discover(),
    };
    let Some(backlight) = discovered else {
        info!("no backlight device; brightness control is unavailable");
        // The channel is still drained, so a command answers rather than
        // blocking a caller for ever.
        while let Some(command) = commands.recv().await {
            let _ = command.reply.send(Err(SvcError::NoBacklight));
        }
        return;
    };
    info!("backlight {} ({} steps)", backlight.name, backlight.max);

    let writer = writer(address.as_deref(), &backlight).await;
    let _ = publisher.send(Arc::new(BrightnessState {
        available: true,
        percent: backlight.read().unwrap_or(0),
        device: Some(backlight.name.clone()),
        change: None,
    }));

    let mut throttle = Throttle::new();
    let mut echoes = Echoes::new();
    // One slot: only one call is ever outstanding, and the sender is cloned
    // into each spawned write.
    let (finished, mut completions) = mpsc::channel::<()>(1);
    let mut monitor = Monitor::start();

    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { break };
                let target = match command.action {
                    Action::Set(percent) => percent.min(100),
                    Action::Step(delta) => step(publisher.borrow().percent, delta),
                };
                echoes.record(Field::Brightness, target, command.source, Instant::now());
                // Optimistic: the slider must not wait on a round trip. The
                // completion below re-reads sysfs and corrects it if the write
                // did not land where it was aimed.
                publish(&publisher, target, &backlight, &mut echoes);
                if let Some(percent) = throttle.request(target) {
                    write(&writer, &backlight, percent, finished.clone());
                }
                let _ = command.reply.send(Ok(()));
            }
            Some(()) = completions.recv() => {
                if let Some(percent) = throttle.finished() {
                    write(&writer, &backlight, percent, finished.clone());
                }
                let actual = backlight.read().unwrap_or_else(|| publisher.borrow().percent);
                publish(&publisher, actual, &backlight, &mut echoes);
            }
            Some(()) = monitor.changed() => {
                let Some(percent) = backlight.read() else { continue };
                publish(&publisher, percent, &backlight, &mut echoes);
            }
        }
    }
}

/// Where a percentage lands after a relative change.
fn step(current: u32, delta: i32) -> u32 {
    if delta >= 0 {
        current.saturating_add(delta.unsigned_abs()).min(100)
    } else {
        current.saturating_sub(delta.unsigned_abs())
    }
}

/// Publish a percentage, attributing it if it moved.
fn publish(
    publisher: &watch::Sender<Arc<BrightnessState>>,
    percent: u32,
    backlight: &Backlight,
    echoes: &mut Echoes,
) {
    let previous = publisher.borrow().clone();
    if previous.percent == percent && previous.available {
        return;
    }
    let change = Some(echoes.attribute(Field::Brightness, percent, Instant::now()));
    let _ = publisher.send(Arc::new(BrightnessState {
        available: true,
        percent,
        device: Some(backlight.name.clone()),
        change,
    }));
}

/// Send one brightness value, telling `finished` when the call lands.
fn write(writer: &Writer, backlight: &Backlight, percent: u32, finished: mpsc::Sender<()>) {
    let raw = backlight.raw(percent);
    match writer {
        Writer::Logind(session) => {
            let session = session.clone();
            let name = backlight.name.clone();
            tokio::spawn(async move {
                if let Err(error) = session.set_brightness(device::SUBSYSTEM, &name, raw).await {
                    warn!("logind refused a brightness of {raw}: {error}");
                }
                let _ = finished.send(()).await;
            });
        }
        Writer::Sysfs => {
            if let Err(error) = backlight.write(raw) {
                warn!("could not write the backlight directly: {error}");
            }
            tokio::spawn(async move {
                let _ = finished.send(()).await;
            });
        }
    }
}

/// Decide how writes will be made, once, at start-up.
async fn writer(address: Option<&str>, backlight: &Backlight) -> Writer {
    let Ok(connection) = logind::connect(address).await else {
        debug!("no system bus; the backlight is written directly");
        return Writer::Sysfs;
    };
    let Ok(manager) = ManagerProxy::new(&connection).await else {
        debug!("no logind on the system bus; the backlight is written directly");
        return Writer::Sysfs;
    };
    let Some(path) = logind::session_path(&manager).await else {
        debug!("no logind session for {WHO}; the backlight is written directly");
        return Writer::Sysfs;
    };
    let built = match SessionProxy::builder(&connection).path(path.clone()) {
        Ok(builder) => builder.build().await,
        Err(error) => Err(error),
    };
    match built {
        Ok(session) => {
            debug!("setting {} through logind session {path}", backlight.name);
            Writer::Logind(session)
        }
        Err(error) => {
            debug!("logind session {path} is not usable ({error}); writing directly");
            Writer::Sysfs
        }
    }
}

/// The udev watch, or nothing at all on a machine that has none.
///
/// `udev::MonitorSocket` is not `Send`, so it cannot live inside a tokio task
/// — and it has no async interface either. It therefore gets a thread of its
/// own that blocks in `poll(2)` and forwards "something changed" down a
/// channel. The thread ends when the receiver is dropped, which is when the
/// service stops.
struct Monitor {
    events: Option<mpsc::UnboundedReceiver<()>>,
}

impl Monitor {
    /// Start watching the backlight subsystem.
    ///
    /// The socket is built *inside* the thread: it is neither `Send` nor
    /// `Sync`, so it cannot be created here and moved over.
    fn start() -> Self {
        let (sender, events) = mpsc::unbounded_channel();
        let spawned = std::thread::Builder::new()
            .name("topbar-udev".to_string())
            .spawn(move || watch_udev(&sender));
        if let Err(error) = spawned {
            warn!("could not start the udev thread ({error}); external changes go unnoticed");
            return Self { events: None };
        }
        Self {
            events: Some(events),
        }
    }

    /// Wait for the backlight to change under us.
    ///
    /// With no monitor — or once the thread has ended — this never resolves,
    /// which in a `select!` arm is exactly "this input does not exist". The
    /// receiver is dropped rather than left closed on purpose: a closed
    /// channel answers `None` immediately, every time, which would turn the
    /// surrounding `select!` into a spin.
    async fn changed(&mut self) -> Option<()> {
        let Some(events) = self.events.as_mut() else {
            return std::future::pending().await;
        };
        match events.recv().await {
            Some(()) => Some(()),
            None => {
                self.events = None;
                std::future::pending().await
            }
        }
    }
}

/// Build the udev monitor and block on it, forwarding every backlight change.
fn watch_udev(sender: &mpsc::UnboundedSender<()>) {
    use std::os::fd::AsRawFd;

    let socket = match udev::MonitorBuilder::new()
        .and_then(|builder| builder.match_subsystem(device::SUBSYSTEM))
        .and_then(udev::MonitorBuilder::listen)
    {
        Ok(socket) => socket,
        Err(error) => {
            info!("no udev backlight monitor ({error}); external changes go unnoticed");
            return;
        }
    };

    loop {
        let mut poll = libc::pollfd {
            fd: socket.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one well-formed pollfd, an infinite timeout, and a file
        // descriptor owned by `socket`, which outlives the call.
        let ready = unsafe { libc::poll(&raw mut poll, 1, -1) };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            warn!("the udev socket stopped answering: {error}");
            return;
        }

        // Draining is required whether or not anything is forwarded: an unread
        // socket stays readable and `poll` would spin.
        let changed = socket
            .iter()
            .any(|event| event.event_type() == udev::EventType::Change);
        if changed && sender.send(()).is_err() {
            // The service has stopped; so does this thread.
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relative_change_stays_inside_the_range() {
        assert_eq!(step(50, 5), 55);
        assert_eq!(step(98, 5), 100);
        assert_eq!(step(100, 5), 100);
        assert_eq!(step(50, -5), 45);
        assert_eq!(step(2, -5), 0);
        assert_eq!(step(0, -5), 0);
        assert_eq!(step(50, 0), 50);
    }

    #[test]
    fn a_step_larger_than_the_range_still_lands_inside_it() {
        assert_eq!(step(50, i32::MAX), 100);
        assert_eq!(step(50, i32::MIN), 0);
    }

    #[test]
    fn publishing_the_same_value_twice_is_not_a_change() {
        let backlight = Backlight {
            name: "intel_backlight".into(),
            path: std::path::PathBuf::from("/nonexistent"),
            max: 100,
        };
        let (publisher, _receiver) = watch::channel(Arc::new(BrightnessState::default()));
        let mut echoes = Echoes::new();

        publish(&publisher, 40, &backlight, &mut echoes);
        let first = publisher.borrow().change.expect("a first change");

        publish(&publisher, 40, &backlight, &mut echoes);
        assert_eq!(
            publisher.borrow().change.expect("carried forward").serial,
            first.serial,
            "a re-read that moved nothing must not raise the OSD again"
        );
    }

    #[test]
    fn a_change_the_panel_asked_for_carries_its_source() {
        let backlight = Backlight {
            name: "intel_backlight".into(),
            path: std::path::PathBuf::from("/nonexistent"),
            max: 100,
        };
        let (publisher, _receiver) = watch::channel(Arc::new(BrightnessState::default()));
        let mut echoes = Echoes::new();

        echoes.record(Field::Brightness, 40, ChangeSource::Ui, Instant::now());
        publish(&publisher, 40, &backlight, &mut echoes);
        assert_eq!(
            publisher.borrow().change.map(|change| change.source),
            Some(ChangeSource::Ui)
        );

        publish(&publisher, 70, &backlight, &mut echoes);
        assert_eq!(
            publisher.borrow().change.map(|change| change.source),
            Some(ChangeSource::External),
            "a value nobody asked for came from somewhere else"
        );
    }
}
