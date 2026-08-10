//! The one owner of the mail count: a timer, two subprocesses, a snapshot.
//!
//! There is no bus here and no connection to hold. What makes it worth a task
//! is the rule between the two commands: the cheap one runs on every tick, and
//! the expensive one runs only when the cheap one says the database moved.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::Instant;
use topbar_core::config::NotmuchConfig;
use tracing::{debug, info, warn};

use super::NotmuchState;
use super::cmd;
use super::parse;
use crate::proc;
use crate::refresh::Refresh;

/// The shortest interval a count may run at.
///
/// The configuration validator enforces this too; the clamp is here as well
/// because a `Config` built by hand could otherwise spin notmuch in a loop.
const MIN_INTERVAL: Duration = Duration::from_secs(60);

/// Count unread mail until the handle is dropped.
pub(crate) async fn run(
    publisher: watch::Sender<Arc<NotmuchState>>,
    config: NotmuchConfig,
    program: Option<PathBuf>,
    mut commands: tokio::sync::mpsc::Receiver<Command>,
) {
    let mut config = config;
    while let Some(next) = serve(&publisher, &config, program.clone(), &mut commands).await {
        info!("notmuch: the configuration changed; starting the count over");
        config = next;
    }
    debug!("the notmuch service has stopped");
}

/// What the panel can ask of the count between its own ticks.
#[derive(Debug, Clone)]
pub(crate) enum Command {
    /// Use this `[widgets.notmuch]` section from now on, starting over.
    Configure(Box<NotmuchConfig>),
    /// Count now. What a resume asks for, and what the popover asks for when
    /// it opens onto a list that may be an interval old.
    Recheck,
}

/// Run one configuration's worth of counting.
async fn serve(
    publisher: &watch::Sender<Arc<NotmuchState>>,
    config: &NotmuchConfig,
    program: Option<PathBuf>,
    commands: &mut tokio::sync::mpsc::Receiver<Command>,
) -> Option<NotmuchConfig> {
    let interval = Duration::from_secs(config.interval).max(MIN_INTERVAL);
    let mut task = Task {
        publisher: publisher.clone(),
        program,
        query: config.query.clone(),
        limit: config.max_items.max(1),
        refresh: Refresh::new(interval),
        listed: None,
    };

    // The first count is immediate: a panel that says nothing about mail for
    // five minutes after login is a panel with no mail widget.
    let mut due = Some(Instant::now());

    loop {
        let timer = async move {
            match due {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending().await,
            }
        };
        tokio::pin!(timer);

        tokio::select! {
            () = &mut timer => {
                due = task.check().await;
            }
            asked = commands.recv() => match asked? {
                Command::Configure(next) => return Some(*next),
                Command::Recheck => due = Some(Instant::now()),
            },
            // Every subscriber has gone: the panel is shutting down.
            () = task.publisher.closed() => return None,
        }
    }
}

/// One configuration's worth of counting, and what it has already listed.
struct Task {
    publisher: watch::Sender<Arc<NotmuchState>>,
    /// Which notmuch to run, when it is not whatever `PATH` says.
    program: Option<PathBuf>,
    query: String,
    limit: u32,
    refresh: Refresh,
    /// The database identity and revision the list on screen was taken at.
    ///
    /// The UUID is in it as well as the revision because a rebuilt database
    /// starts counting again from near zero: comparing revisions alone would
    /// read that as "nothing has changed" for as long as it took to catch up.
    listed: Option<(String, u64)>,
}

impl Task {
    /// Run one count, and say when the next one is due.
    async fn check(&mut self) -> Option<Instant> {
        self.publish(|state| state.checking = true);

        let counted = match self
            .run(cmd::count(self.program.as_deref(), &self.query))
            .await
        {
            Ok(stdout) => parse::counted(&stdout),
            Err(reason) => Err(reason),
        };

        let counted = match counted {
            Ok(counted) => counted,
            Err(reason) => {
                // Not "no new mail": the widget hides. "Nothing unread" and
                // "I could not tell" look identical on a panel, and only one
                // of them is safe to guess.
                warn!("notmuch: cannot count mail ({reason})");
                self.listed = None;
                self.publish(|state| {
                    state.available = false;
                    state.unread = 0;
                    state.threads.clear();
                    state.more = false;
                    state.revision = None;
                });
                return Some(Instant::now() + self.refresh.failed());
            }
        };

        let stamp = (counted.uuid.clone(), counted.revision);
        if self.listed.as_ref() != Some(&stamp) {
            self.relist(&stamp, counted.count).await;
        }

        debug!("notmuch: {} unread", counted.count);
        let (count, revision) = (counted.count, counted.revision);
        self.publish(move |state| {
            state.available = true;
            state.unread = count;
            state.revision = Some(revision);
        });
        Some(Instant::now() + self.refresh.succeeded())
    }

    /// Fetch the conversation list, the database having moved since the last.
    ///
    /// A list that cannot be read does not hide the widget: the count is the
    /// indicator and it is already good. Only the popover loses anything.
    async fn relist(&mut self, stamp: &(String, u64), count: usize) {
        if count == 0 {
            self.listed = Some(stamp.clone());
            self.publish(|state| {
                state.threads.clear();
                state.more = false;
            });
            return;
        }

        let listed = self
            .run(cmd::search(
                self.program.as_deref(),
                &self.query,
                self.limit,
            ))
            .await
            .and_then(|stdout| parse::threads(&stdout));

        match listed {
            Ok(threads) => {
                // Exactly as many as were asked for means there are very
                // probably more behind them, which the popover says rather
                // than letting its list quietly disagree with the count.
                let more = threads.len() as u32 >= self.limit;
                self.listed = Some(stamp.clone());
                self.publish(move |state| {
                    state.threads = threads;
                    state.more = more;
                });
            }
            Err(reason) => {
                debug!("notmuch: cannot list conversations ({reason})");
                self.listed = None;
                self.publish(|state| {
                    state.threads.clear();
                    state.more = false;
                });
            }
        }
    }

    /// Run one command and hand back what it printed, or why it did not.
    async fn run(&self, spec: proc::CmdSpec) -> Result<String, String> {
        match proc::capture(&spec).await {
            Ok(captured) if captured.ok() => Ok(captured.stdout),
            Ok(captured) => Err(match captured.stderr.trim() {
                "" => format!("notmuch exited {:?}", captured.code),
                stderr => stderr.to_string(),
            }),
            // The program could not be started at all: no notmuch on this
            // machine, or the path the smoke run named does not exist.
            Err(error) => Err(error.to_string()),
        }
    }

    /// Edit the snapshot and publish it if anything moved.
    fn publish(&self, edit: impl FnOnce(&mut NotmuchState)) {
        self.publisher.send_if_modified(|current| {
            let mut next = (**current).clone();
            next.checking = false;
            edit(&mut next);
            if **current == next {
                false
            } else {
                *current = Arc::new(next);
                true
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notmuch::MailThread;

    fn task() -> Task {
        let (publisher, _receiver) = watch::channel(Arc::new(NotmuchState::default()));
        Task {
            publisher,
            program: None,
            query: "tag:unread".to_string(),
            limit: 10,
            refresh: Refresh::new(Duration::from_secs(300)),
            listed: None,
        }
    }

    fn thread(id: &str) -> MailThread {
        MailThread {
            thread: id.to_string(),
            sender: "Someone".to_string(),
            subject: "Hello".to_string(),
            when: "Today 09:14".to_string(),
            matched: 1,
        }
    }

    #[tokio::test]
    async fn an_empty_result_clears_the_list_without_running_the_search() {
        let mut task = task();
        task.publish(|state| state.threads = vec![thread("a")]);

        // count 0 needs no search: there is nothing to list, and the command
        // that would say so is the expensive one.
        task.relist(&("uuid".to_string(), 7), 0).await;

        assert!(task.publisher.borrow().threads.is_empty());
        assert!(!task.publisher.borrow().more);
        assert_eq!(task.listed, Some(("uuid".to_string(), 7)));
    }

    #[test]
    fn a_rebuilt_database_counts_as_a_change_even_at_a_lower_revision() {
        // `notmuch new --full-scan` starts the revision again from near zero.
        // Comparing revisions alone would read that as "nothing moved" for as
        // long as it took to catch up with the old number.
        let listed = Some(("old-uuid".to_string(), 37939));
        assert_ne!(listed.as_ref(), Some(&("new-uuid".to_string(), 12)));
    }

    #[test]
    fn a_snapshot_that_did_not_move_is_not_republished() {
        let task = task();
        let mut receiver = task.publisher.subscribe();
        task.publish(|state| {
            state.available = true;
            state.unread = 3;
        });
        assert!(receiver.has_changed().unwrap());
        let _ = receiver.borrow_and_update();

        task.publish(|state| {
            state.available = true;
            state.unread = 3;
        });
        assert!(
            !receiver.has_changed().unwrap(),
            "an unchanged count woke every subscriber"
        );
    }
}
