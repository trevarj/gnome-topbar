//! Keeping the machine awake, by holding a file descriptor.
//!
//! `org.freedesktop.login1.Manager.Inhibit("idle", …, "block")` returns a
//! descriptor, and the lock exists for exactly as long as that descriptor
//! does. That is the whole mechanism, and it is the reason this is the right
//! way to do it: there is no lock to leak. A panel that crashes, is killed, or
//! is `SIGKILL`ed has its descriptors closed by the kernel and the machine goes
//! back to sleeping on schedule — which is not true of any protocol where
//! "release" is a message you have to remember to send.
//!
//! It is the same call `systemd-inhibit(1)` makes, so it works whatever is
//! actually watching for idleness: swayidle, hypridle, GNOME's own daemon.
//!
//! logind restarting takes the descriptor with it. The service notices through
//! `NameOwnerChanged` and takes the lock again if it was holding one, because a
//! caffeine toggle that silently stopped caffeinating is worse than one that
//! was never there.

use std::sync::Arc;

use futures_util::StreamExt;
use tokio::sync::{mpsc, oneshot, watch};
use tracing::{debug, info, warn};
use zbus::zvariant::OwnedFd;

use crate::error::SvcError;
use crate::logind::{self, ManagerProxy};

/// What the lock is taken against.
const WHAT: &str = "idle";
/// Who is asking, as `systemd-inhibit --list` will show it.
const WHO: &str = "topbar";
/// Why, as the same listing will show it.
const WHY: &str = "The user asked to keep this session awake";
/// `block` rather than `delay`: this is a refusal, not a request for warning.
const MODE: &str = "block";

/// How many commands may be in flight before a sender waits.
const QUEUE: usize = 8;

/// Whether the machine is being kept awake.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub struct InhibitorState {
    /// Whether logind is there to take a lock from.
    pub available: bool,
    /// Whether a lock is held right now.
    pub active: bool,
}

/// One thing the panel can ask.
#[derive(Debug)]
enum Action {
    /// Take or release the lock.
    Set(bool),
    /// Flip it.
    Toggle,
}

/// A command and where to answer it.
#[derive(Debug)]
struct Command {
    action: Action,
    reply: oneshot::Sender<Result<(), SvcError>>,
}

/// The idle-inhibitor service.
#[derive(Clone)]
pub struct Inhibitor {
    handle: InhibitorHandle,
    state: watch::Receiver<Arc<InhibitorState>>,
}

impl Inhibitor {
    /// Start watching logind.
    ///
    /// `address` overrides the system bus for the bus tests.
    pub(crate) fn start(address: Option<String>) -> Self {
        let (commands, queue) = mpsc::channel(QUEUE);
        let (publisher, state) = watch::channel(Arc::new(InhibitorState::default()));
        tokio::spawn(run(queue, publisher, address));
        Self {
            handle: InhibitorHandle { commands },
            state,
        }
    }

    /// The handle commands are sent through.
    pub fn handle(&self) -> &InhibitorHandle {
        &self.handle
    }

    /// Subscribe to inhibitor state.
    pub fn state(&self) -> watch::Receiver<Arc<InhibitorState>> {
        self.state.clone()
    }

    /// The state as of right now.
    pub fn current(&self) -> Arc<InhibitorState> {
        self.state.borrow().clone()
    }
}

/// What the panel may ask of the inhibitor.
#[derive(Clone)]
pub struct InhibitorHandle {
    commands: mpsc::Sender<Command>,
}

impl InhibitorHandle {
    /// Take the lock, or release it.
    pub async fn set_active(&self, active: bool) -> Result<(), SvcError> {
        self.send(Action::Set(active)).await
    }

    /// Flip it.
    pub async fn toggle(&self) -> Result<(), SvcError> {
        self.send(Action::Toggle).await
    }

    /// Post a command and wait for the task to answer it.
    async fn send(&self, action: Action) -> Result<(), SvcError> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(Command { action, reply })
            .await
            .map_err(|_| SvcError::ServiceStopped("inhibitor"))?;
        answer
            .await
            .map_err(|_| SvcError::ServiceStopped("inhibitor"))?
    }
}

/// Hold the lock — or not — until every handle is dropped.
async fn run(
    mut commands: mpsc::Receiver<Command>,
    publisher: watch::Sender<Arc<InhibitorState>>,
    address: Option<String>,
) {
    let connection = match logind::connect(address.as_deref()).await {
        Ok(connection) => connection,
        Err(error) => {
            info!("no system bus ({error}); the idle inhibitor is unavailable");
            return drain(commands).await;
        }
    };
    let manager = match ManagerProxy::new(&connection).await {
        Ok(manager) => manager,
        Err(error) => {
            info!("no logind ({error}); the idle inhibitor is unavailable");
            return drain(commands).await;
        }
    };

    // Subscribed before the first call, so a restart landing between the two
    // is queued rather than lost.
    let mut owners = match owner_changes(&connection).await {
        Ok(owners) => owners,
        Err(error) => {
            debug!("cannot watch logind's bus name ({error}); restarts go unnoticed");
            Box::pin(futures_util::stream::pending())
        }
    };

    publish(&publisher, true, false);

    // The lock itself. Dropping it is what releases the machine.
    let mut lock: Option<OwnedFd> = None;

    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { break };
                let wanted = match command.action {
                    Action::Set(active) => active,
                    Action::Toggle => lock.is_none(),
                };
                let answer = apply(&manager, &mut lock, wanted).await;
                if answer.is_ok() {
                    publish(&publisher, true, lock.is_some());
                }
                let _ = command.reply.send(answer);
            }
            Some(()) = owners.next() => {
                // logind came back. Anything we were holding died with the old
                // process, so it is taken again rather than assumed.
                if lock.is_some() {
                    info!("logind restarted; taking the idle inhibitor again");
                    lock = None;
                    match take(&manager).await {
                        Ok(fresh) => lock = Some(fresh),
                        Err(error) => warn!("could not retake the idle inhibitor: {error}"),
                    }
                    publish(&publisher, true, lock.is_some());
                }
            }
        }
    }
}

/// Answer every command with "unavailable" rather than blocking a caller.
async fn drain(mut commands: mpsc::Receiver<Command>) {
    while let Some(command) = commands.recv().await {
        let _ = command
            .reply
            .send(Err(SvcError::Inhibitor("logind is not available".into())));
    }
}

/// Take or release the lock so that its presence matches `wanted`.
async fn apply(
    manager: &ManagerProxy<'_>,
    lock: &mut Option<OwnedFd>,
    wanted: bool,
) -> Result<(), SvcError> {
    match (wanted, lock.is_some()) {
        (true, false) => {
            *lock = Some(take(manager).await?);
            debug!("idle inhibitor taken");
        }
        (false, true) => {
            // Dropping the descriptor *is* the release; there is nothing to
            // call and nothing that can fail.
            *lock = None;
            debug!("idle inhibitor released");
        }
        _ => {}
    }
    Ok(())
}

/// Ask logind for an inhibitor descriptor.
async fn take(manager: &ManagerProxy<'_>) -> Result<OwnedFd, SvcError> {
    manager
        .inhibit(WHAT, WHO, WHY, MODE)
        .await
        .map_err(|error| SvcError::Inhibitor(error.to_string()))
}

/// A stream that yields whenever logind's bus name gets a new owner.
async fn owner_changes(
    connection: &zbus::Connection,
) -> zbus::Result<std::pin::Pin<Box<dyn futures_util::Stream<Item = ()> + Send>>> {
    let dbus = zbus::fdo::DBusProxy::new(connection).await?;
    let stream = dbus
        .receive_name_owner_changed_with_args(&[(0, logind::BUS_NAME)])
        .await?;
    Ok(Box::pin(stream.filter_map(|signal| async move {
        let args = signal.args().ok()?;
        // An empty new owner is logind going away; a non-empty one is the
        // replacement that our descriptor does not belong to.
        args.new_owner().as_ref().map(|_| ())
    })))
}

/// Publish a state, if it is not the one already published.
fn publish(publisher: &watch::Sender<Arc<InhibitorState>>, available: bool, active: bool) {
    let next = InhibitorState { available, active };
    publisher.send_if_modified(|current| {
        if **current == next {
            false
        } else {
            *current = Arc::new(next);
            true
        }
    });
}

/// Acquiring, releasing and re-acquiring against a logind of the test's own.
#[cfg(test)]
mod bus_tests {
    use std::time::Duration;

    use super::*;
    use crate::logind::bus_tests::{InhibitCall, Log, journal, serve_logind, wait_for};
    use crate::private_bus::private_bus;

    /// Wait for the panel's idea of the inhibitor to be `wanted`.
    async fn wait_for_active(inhibitor: &Inhibitor, wanted: bool) {
        wait_for(if wanted { "the lock" } else { "the release" }, || {
            inhibitor.current().active == wanted
        })
        .await;
    }

    #[tokio::test]
    async fn a_toggle_takes_a_real_descriptor_and_dropping_it_releases_the_machine() {
        let bus = private_bus!();
        let log = Log::default();
        let _logind = serve_logind(&bus, &log, Duration::ZERO, None).await;

        let inhibitor = Inhibitor::start(Some(bus.address().to_string()));
        wait_for("logind to answer", || inhibitor.current().available).await;
        assert!(!inhibitor.current().active, "nothing is held at start-up");

        inhibitor.handle().toggle().await.expect("logind is there");
        wait_for_active(&inhibitor, true).await;

        {
            let journal = journal(&log);
            assert_eq!(
                journal.inhibits,
                vec![InhibitCall {
                    what: WHAT.to_string(),
                    who: WHO.to_string(),
                    why: WHY.to_string(),
                    mode: MODE.to_string(),
                }],
                "the panel asks logind for exactly what systemd-inhibit does"
            );
            assert!(
                !crate::logind::bus_tests::is_released(&journal.locks[0]),
                "the descriptor is still open, so the machine is still awake"
            );
        }

        inhibitor.handle().toggle().await.expect("logind is there");
        wait_for_active(&inhibitor, false).await;

        // Releasing is dropping, and dropping is observable from the far end of
        // the pipe — no second call to logind, and nothing to leak.
        wait_for("the descriptor to close", || {
            crate::logind::bus_tests::is_released(&journal(&log).locks[0])
        })
        .await;
        assert_eq!(journal(&log).inhibits.len(), 1, "releasing calls nothing");
    }

    #[tokio::test]
    async fn setting_a_state_it_is_already_in_does_nothing() {
        let bus = private_bus!();
        let log = Log::default();
        let _logind = serve_logind(&bus, &log, Duration::ZERO, None).await;

        let inhibitor = Inhibitor::start(Some(bus.address().to_string()));
        wait_for("logind to answer", || inhibitor.current().available).await;

        inhibitor.handle().set_active(true).await.expect("held");
        wait_for_active(&inhibitor, true).await;
        inhibitor
            .handle()
            .set_active(true)
            .await
            .expect("still held");
        assert_eq!(
            journal(&log).inhibits.len(),
            1,
            "asking twice must not take two locks"
        );

        inhibitor
            .handle()
            .set_active(false)
            .await
            .expect("released");
        wait_for_active(&inhibitor, false).await;
        inhibitor
            .handle()
            .set_active(false)
            .await
            .expect("still free");
        assert_eq!(journal(&log).inhibits.len(), 1);
    }

    #[tokio::test]
    async fn a_logind_restart_gets_the_lock_taken_again() {
        let bus = private_bus!();
        let log = Log::default();
        let logind = serve_logind(&bus, &log, Duration::ZERO, None).await;

        let inhibitor = Inhibitor::start(Some(bus.address().to_string()));
        wait_for("logind to answer", || inhibitor.current().available).await;
        inhibitor.handle().toggle().await.expect("held");
        wait_for_active(&inhibitor, true).await;
        assert_eq!(journal(&log).inhibits.len(), 1);

        // logind goes away, taking the descriptor's far end with it, and comes
        // back as a new owner of the same name.
        drop(logind);
        let _restarted = serve_logind(&bus, &log, Duration::ZERO, None).await;

        wait_for("the lock to be taken again", || {
            journal(&log).inhibits.len() == 2
        })
        .await;
        assert!(
            inhibitor.current().active,
            "a caffeine toggle that stopped caffeinating would be worse than none"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_inhibited_before_logind_answers() {
        let state = InhibitorState::default();
        assert!(!state.available);
        assert!(!state.active);
    }

    #[test]
    fn publishing_the_same_state_twice_does_not_wake_a_subscriber() {
        let (publisher, mut receiver) = watch::channel(Arc::new(InhibitorState::default()));
        receiver.mark_unchanged();

        publish(&publisher, true, false);
        assert!(receiver.has_changed().expect("the channel is alive"));
        receiver.mark_unchanged();

        publish(&publisher, true, false);
        assert!(!receiver.has_changed().expect("the channel is alive"));
    }

    #[tokio::test]
    async fn commands_against_a_stopped_service_report_it() {
        let (commands, queue) = mpsc::channel(1);
        drop(queue);
        let handle = InhibitorHandle { commands };
        let error = handle.toggle().await.expect_err("nothing is listening");
        assert!(matches!(error, SvcError::ServiceStopped("inhibitor")));
    }

    #[tokio::test]
    async fn a_machine_with_no_logind_answers_rather_than_hanging() {
        let (commands, queue) = mpsc::channel(1);
        let handle = InhibitorHandle { commands };
        tokio::spawn(drain(queue));
        let error = handle.toggle().await.expect_err("there is nothing to hold");
        assert!(matches!(error, SvcError::Inhibitor(_)));
        assert_eq!(error.user_message(), "Could not change the idle inhibitor");
    }
}
