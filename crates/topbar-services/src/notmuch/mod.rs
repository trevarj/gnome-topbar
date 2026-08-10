//! Unread mail, as notmuch sees whatever fills the maildir.
//!
//! ```text
//!   cmd.rs      which notmuch runs, and with what arguments (pure)
//!   parse.rs    what it printed (pure)
//!   task.rs     the one owner: a timer, two subprocesses, a snapshot
//! ```
//!
//! The panel does not sync mail and does not read a message file. Something
//! else — lieer, mbsync, offlineimap, a systemd timer — fills the maildir and
//! runs `notmuch new`; this service asks the index that already exists how many
//! messages match a query, and what the newest conversations among them are.
//! Nothing here writes.
//!
//! ## Two commands, and why the cheap one runs far more often
//!
//! `notmuch count --lastmod` prints `count`, the database UUID and its
//! **revision**, tab separated, in single-digit milliseconds. The revision is
//! what makes the expensive command rare: `notmuch search --format=json` is
//! twice the cost and many times the output, so it runs only when the revision
//! has moved since the last list — or when the popover opens and wants one.
//!
//! ## Failure hides the widget
//!
//! No notmuch on `PATH`, a database that cannot be opened, output that does not
//! parse: all of them leave `available` false, and the widget draws nothing.
//! Not "0 unread" — "no new mail" and "I could not tell" look identical on a
//! panel, and only one of them is safe to guess. The same rule the updates card
//! is built on.

pub mod cmd;
pub mod parse;
mod task;

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::watch;
use topbar_core::config::NotmuchConfig;

use crate::error::SvcError;

pub use parse::Counted;

/// One conversation, as the popover draws it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailThread {
    /// Notmuch's thread id.
    pub thread: String,
    /// Who it is from: the first of the authors notmuch says matched.
    pub sender: String,
    /// What it is about.
    pub subject: String,
    /// When the newest message in it arrived, in notmuch's own words —
    /// `Today 09:14`, `Yest. 22:03`, `2026-03-14`. Taken rather than computed
    /// because notmuch has already done it and the panel has no better answer.
    pub when: String,
    /// How many messages in this conversation the query matched.
    pub matched: usize,
}

/// Everything the panel knows about unread mail right now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NotmuchState {
    /// Whether notmuch answered at all. False hides the widget outright.
    pub available: bool,
    /// How many **messages** match the query.
    pub unread: usize,
    /// The newest matching **conversations**, newest first.
    ///
    /// Capped at `[widgets.notmuch] max_items`, so this is very often shorter
    /// than the conversation the count implies — which is why the popover says
    /// so rather than letting the two numbers quietly disagree.
    pub threads: Vec<MailThread>,
    /// Whether the list was cut off by that cap.
    pub more: bool,
    /// Whether a count is running right now.
    pub checking: bool,
    /// The database revision the list was taken at.
    pub revision: Option<u64>,
}

impl NotmuchState {
    /// Whether the widget is on the bar at all.
    ///
    /// Nothing unread is nothing to say. An envelope that is permanently
    /// present and permanently empty is furniture, and the panel's rule is
    /// that a widget with nothing to say is invisible.
    pub fn shown(&self) -> bool {
        self.available && self.unread > 0
    }

    /// What the tooltip says.
    pub fn title(&self) -> String {
        match self.unread {
            1 => "1 unread message".to_string(),
            unread => format!("{unread} unread messages"),
        }
    }
}

/// The notmuch service.
#[derive(Clone)]
pub struct Notmuch {
    state: watch::Receiver<Arc<NotmuchState>>,
    commands: tokio::sync::mpsc::Sender<task::Command>,
    task: crate::lazy::Deferred,
}

impl Notmuch {
    /// Start counting mail.
    ///
    /// `program` is which notmuch to run; `None` means whatever `PATH` says,
    /// which is what the panel passes. The smoke run names one outright,
    /// because prepending a fake to `PATH` still leaves the developer's real
    /// notmuch — and their real mail — one directory behind it.
    /// `wanted` is whether a `notmuch` widget is on the bar.
    pub(crate) fn start(config: &NotmuchConfig, program: Option<PathBuf>, wanted: bool) -> Self {
        let (publisher, state) = watch::channel(Arc::new(NotmuchState::default()));
        let (commands, queue) = tokio::sync::mpsc::channel(2);
        let task = crate::lazy::Deferred::spawn(
            wanted,
            task::run(publisher, config.clone(), program, queue),
        );
        Self {
            state,
            commands,
            task,
        }
    }

    /// Start the task if it was held back. Returns whether this call did it.
    pub(crate) fn ensure_started(&self) -> bool {
        self.task.start()
    }

    /// Apply a changed `[widgets.notmuch]` section.
    ///
    /// The poll starts over rather than being edited: the query and the
    /// interval together decide what runs at all.
    pub async fn configure(&self, config: &NotmuchConfig) {
        let _ = self
            .commands
            .send(task::Command::Configure(Box::new(config.clone())))
            .await;
    }

    /// Count again now. Resume calls this; so does the popover opening.
    pub async fn recheck(&self) {
        let _ = self.commands.send(task::Command::Recheck).await;
    }

    /// A handle the widget can send commands through.
    pub fn handle(&self) -> NotmuchHandle {
        NotmuchHandle {
            commands: self.commands.clone(),
        }
    }

    /// Subscribe to mail state.
    pub fn state(&self) -> watch::Receiver<Arc<NotmuchState>> {
        self.state.clone()
    }

    /// The state as of right now.
    pub fn current(&self) -> Arc<NotmuchState> {
        self.state.borrow().clone()
    }
}

/// What the widget holds to ask the service for something.
#[derive(Clone)]
pub struct NotmuchHandle {
    commands: tokio::sync::mpsc::Sender<task::Command>,
}

impl NotmuchHandle {
    /// Count and list again, now.
    pub async fn refresh(&self) -> Result<(), SvcError> {
        self.commands
            .send(task::Command::Recheck)
            .await
            .map_err(|_| SvcError::ServiceStopped("notmuch"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_widget_with_nothing_to_report_is_not_drawn() {
        let nothing = NotmuchState::default();
        assert!(!nothing.shown(), "no notmuch, no envelope");

        let empty = NotmuchState {
            available: true,
            ..NotmuchState::default()
        };
        assert!(
            !empty.shown(),
            "an envelope permanently reading zero is furniture"
        );

        let waiting = NotmuchState {
            available: true,
            unread: 3,
            ..NotmuchState::default()
        };
        assert!(waiting.shown());
    }

    #[test]
    fn one_unread_message_is_not_one_unread_messages() {
        let one = NotmuchState {
            available: true,
            unread: 1,
            ..NotmuchState::default()
        };
        assert_eq!(one.title(), "1 unread message");

        let several = NotmuchState {
            available: true,
            unread: 12,
            ..NotmuchState::default()
        };
        assert_eq!(several.title(), "12 unread messages");
    }
}
