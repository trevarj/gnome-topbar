//! Pending updates: how many, on whatever distribution this is.
//!
//! ```text
//!   distro.rs   which distribution, and what counts updates here (pure)
//!   parse.rs    what each of those commands' output means (pure)
//!   task.rs     the one owner: a timer, a subprocess, a snapshot
//! ```
//!
//! ## Auto-detection, and its two honest gaps
//!
//! v1 had one package manager — Guix — and a configuration key the user had to
//! fill in by hand. v2 reads `/etc/os-release` and deduces a **read-only**
//! counting command for Guix, Debian, Arch, Fedora and Fedora's image-based
//! editions. Nothing it runs syncs a package database, takes a lock or
//! downloads anything.
//!
//! **NixOS** is counted differently: no single command answers "how many
//! updates are pending" without writing the lock file, so the service does
//! what `nix flake update --dry-run` used to do before the flag was removed —
//! it re-locks a scratch *copy* of the system flake and diffs the pins (see
//! [`flake_count`]). `[updates] flake` says where the flake lives when it is
//! not at `/etc/nixos`. **Anything unrecognised** has no counter at all: a
//! card reporting "0 updates" on a machine the panel cannot read would be
//! worse than no card, so the service logs what to put in
//! `update_count_command` and the card stays hidden.
//!
//! ## The override
//!
//! `[updates] update_count_command` still wins, and still runs through a shell
//! — the one documented exception to the argv rule in [`crate::proc`], because
//! that key has been a shell command line since v1 and pipelines are the normal
//! way to write one. Its contract is v1's, unchanged: **print a number, or one
//! update per line**.
//!
//! ## Failure hides the card
//!
//! A command that could not run, an exit status the contract does not cover,
//! output that does not look like what was expected — all of them hide the card
//! rather than reporting zero. "Up to date" and "I could not tell" look
//! identical on a panel, and only one of them is safe to guess.

pub mod distro;
pub mod flake_count;
pub mod parse;
mod task;

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::watch;
use topbar_core::config::UpdatesConfig;

use crate::connectivity::Connectivity;

pub use distro::{Counter, Distro};
pub use parse::Count;

/// Everything the panel knows about pending updates.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpdatesState {
    /// Whether there is any way to count updates on this machine.
    ///
    /// False on NixOS with no override, on an unrecognised distribution, and
    /// wherever the counting command turned out not to work.
    pub available: bool,
    /// How many updates are pending.
    pub count: usize,
    /// The first few package names, for the card's second line.
    pub detail: Option<String>,
    /// Whether a check is running right now.
    pub checking: bool,
    /// Which distribution the count came from, for the tooltip and the log.
    pub source: Option<&'static str>,
}

impl UpdatesState {
    /// Whether the card is drawn at all.
    ///
    /// Nothing pending is nothing to say: the plan's rule, and GNOME's. A card
    /// permanently reading "0 updates" is a row of furniture.
    pub fn shown(&self) -> bool {
        self.available && self.count > 0
    }

    /// What the card is titled.
    pub fn title(&self) -> String {
        match self.count {
            1 => "1 update".to_string(),
            count => format!("{count} updates"),
        }
    }
}

/// The updates service.
#[derive(Clone)]
pub struct Updates {
    state: watch::Receiver<Arc<UpdatesState>>,
    commands: tokio::sync::mpsc::Sender<UpdatesConfig>,
    task: crate::lazy::Deferred,
}

impl Updates {
    /// Start checking for updates, reading `/etc/os-release` under `root`.
    ///
    /// `root` is `/` in the panel and a fixture directory in the tests — the
    /// same seam the state store uses, and for the same reason: the alternative
    /// is a test that can only check the distribution the developer happens to
    /// be running.
    /// `wanted` is whether the Quick Settings menu — the only thing with an
    /// updates card in it — is on the bar.
    pub(crate) fn with_root(
        config: &UpdatesConfig,
        connectivity: &Connectivity,
        root: PathBuf,
        wanted: bool,
    ) -> Self {
        let (publisher, state) = watch::channel(Arc::new(UpdatesState::default()));
        let (commands, queue) = tokio::sync::mpsc::channel(2);
        let task = crate::lazy::Deferred::spawn(
            wanted,
            task::run(publisher, config.clone(), connectivity.clone(), root, queue),
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

    /// Apply a changed `[updates]` section. Hot reload is what calls this.
    ///
    /// The check starts over rather than being edited: which command runs at
    /// all is decided from the whole section at once.
    pub async fn configure(&self, config: &UpdatesConfig) {
        let _ = self.commands.send(config.clone()).await;
    }

    /// Subscribe to update state.
    pub fn state(&self) -> watch::Receiver<Arc<UpdatesState>> {
        self.state.clone()
    }

    /// The state as of right now.
    pub fn current(&self) -> Arc<UpdatesState> {
        self.state.borrow().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_card_with_nothing_to_report_is_not_drawn() {
        let nothing = UpdatesState::default();
        assert!(!nothing.shown(), "no counter, no card");

        let current = UpdatesState {
            available: true,
            ..UpdatesState::default()
        };
        assert!(
            !current.shown(),
            "a card permanently reading '0 updates' is furniture"
        );

        let pending = UpdatesState {
            available: true,
            count: 7,
            ..UpdatesState::default()
        };
        assert!(pending.shown());
    }

    #[test]
    fn one_update_is_not_one_updates() {
        assert_eq!(
            UpdatesState {
                count: 1,
                ..UpdatesState::default()
            }
            .title(),
            "1 update"
        );
        assert_eq!(
            UpdatesState {
                count: 7,
                ..UpdatesState::default()
            }
            .title(),
            "7 updates"
        );
    }
}
