//! The one owner of the update check: a timer, a subprocess, a snapshot.
//!
//! There is no D-Bus here and no long-lived connection to keep: the whole
//! service is "run a program every hour and read what it printed". What makes
//! it worth a task of its own is everything around that — the check must not
//! run while the machine is offline (every one of these commands talks to a
//! mirror), must not overlap itself, and must not be believed when it fails.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::Instant;
use topbar_core::config::UpdatesConfig;
use tracing::{debug, info, warn};

use super::UpdatesState;
use super::distro::{Counter, Distro, detect_at};
use super::flake_count;
use super::parse::{Count, override_output, read};
use crate::connectivity::Connectivity;
use crate::proc::{self, CmdSpec};
use crate::refresh::Refresh;

/// The shortest interval a check may run at.
///
/// The configuration validator already enforces this; the clamp is here as
/// well because a `Config` built by hand could otherwise make the panel run
/// `apt-get` in a loop.
const MIN_INTERVAL: Duration = Duration::from_secs(60);

/// Follow pending updates until the handle is dropped.
pub(crate) async fn run(
    publisher: watch::Sender<Arc<UpdatesState>>,
    config: UpdatesConfig,
    connectivity: Connectivity,
    root: PathBuf,
) {
    let interval = Duration::from_secs(config.check_interval).max(MIN_INTERVAL);
    let Some(plan) = plan(&config, &root) else {
        // Nothing to run. The snapshot stays at its default — unavailable,
        // count zero — and the card never appears.
        return;
    };

    let mut connectivity = connectivity.state();
    let mut online = connectivity.borrow_and_update().online;

    let mut task = Task {
        publisher,
        plan,
        refresh: Refresh::new(interval),
        online,
        deferred: false,
    };

    // The first check is immediate: a panel that says nothing about updates for
    // an hour after login is a panel with no updates card.
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
            changed = connectivity.changed() => {
                // A watcher that has stopped is not evidence of being offline.
                online = changed.map_or(true, |()| connectivity.borrow().online);
                if let Some(at) = task.set_online(online) {
                    due = Some(at);
                }
            }
            () = &mut timer => {
                due = task.check().await;
            }
            // Every subscriber has gone: the panel is shutting down.
            () = task.publisher.closed() => break,
        }
    }

    debug!("the updates service has no subscribers left; stopping");
}

/// What the service is going to run, and how to read it.
enum Plan {
    /// The user's own command, through a shell.
    Override(CmdSpec),
    /// A command deduced from the distribution.
    Native {
        /// Which distribution it came from.
        distro: Distro,
        /// Which contract its output follows.
        counter: Counter,
    },
    /// NixOS: re-lock a scratch copy of the flake and count what moved.
    NixosFlake {
        /// Where `flake.nix` and `flake.lock` live.
        dir: PathBuf,
    },
}

/// Decide what to run, saying in the log why when the answer is "nothing".
///
/// The override wins outright: a user who has written a command has said what
/// they want counted, and second-guessing that with a package manager they may
/// not use would be worse than useless.
fn plan(config: &UpdatesConfig, root: &std::path::Path) -> Option<Plan> {
    if let Some(command) = config
        .update_count_command
        .as_deref()
        .map(str::trim)
        .filter(|command| !command.is_empty())
    {
        info!("updates: counting with the configured command");
        return Some(Plan::Override(CmdSpec::shell(command)));
    }

    let distro = detect_at(root);
    match distro.counter() {
        Some(counter) => {
            info!("updates: counting with {}'s own tools", distro.label());
            Some(Plan::Native { distro, counter })
        }
        // Not a single command but still counted: re-lock a copy of the
        // system flake and diff the pins. `[updates] flake` says where the
        // flake lives when it is not at the canonical path.
        None if distro == Distro::NixOS => {
            let dir = config
                .flake
                .as_deref()
                .map(str::trim)
                .filter(|dir| !dir.is_empty())
                .map_or_else(
                    || PathBuf::from(flake_count::DEFAULT_FLAKE_DIR),
                    expand_home,
                );
            info!(
                "updates: counting flake inputs that would move, against {}",
                dir.display()
            );
            Some(Plan::NixosFlake { dir })
        }
        None => {
            // Said plainly, once, because the alternative the user has is a
            // configuration key and they have to be told it exists.
            info!(
                "updates: no side-effect-free way to count updates on {}; \
                 set [updates] update_count_command to enable the card",
                distro.label()
            );
            None
        }
    }
}

/// `~/x` as an absolute path, because `[updates] flake` is written by a
/// person and people write `~`.
fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => {
            let home = std::env::var_os("HOME").map_or_else(std::env::temp_dir, PathBuf::from);
            home.join(rest)
        }
        None => PathBuf::from(path),
    }
}

/// Everything the loop owns.
struct Task {
    publisher: watch::Sender<Arc<UpdatesState>>,
    plan: Plan,
    refresh: Refresh,
    online: bool,
    /// Whether a check was skipped because the machine was offline.
    deferred: bool,
}

impl Task {
    /// Run one check, and say when the next one is due.
    async fn check(&mut self) -> Option<Instant> {
        if !self.online {
            // Every one of these commands talks to a mirror. Nothing is
            // scheduled: coming back online is what starts the next one.
            debug!("updates: offline; the check waits");
            self.deferred = true;
            return None;
        }

        self.publish(|state| state.checking = true);
        let count = match &self.plan {
            Plan::NixosFlake { dir } => flake_count::count(dir).await,
            Plan::Override(spec) => match proc::capture(spec).await {
                Ok(captured) => override_output(&captured),
                Err(error) => Count::Unusable(error.to_string()),
            },
            Plan::Native { counter, .. } => match proc::capture(&counter.spec()).await {
                Ok(captured) => read(*counter, &captured),
                // The command could not be started at all: `checkupdates` on a
                // machine without pacman-contrib, or a package manager that is
                // no longer installed.
                Err(error) => Count::Unusable(error.to_string()),
            },
        };

        let source = match &self.plan {
            Plan::Native { distro, .. } => Some(distro.label()),
            Plan::Override(_) => Some("the configured command"),
            Plan::NixosFlake { .. } => Some("NixOS"),
        };

        let wait = match &count {
            Count::Found { count, detail } => {
                debug!("updates: {count} pending");
                let (count, detail) = (*count, detail.clone());
                self.publish(move |state| {
                    state.available = true;
                    state.count = count;
                    state.detail = detail;
                    state.source = source;
                });
                self.refresh.succeeded()
            }
            Count::UpToDate => {
                debug!("updates: nothing pending");
                self.publish(move |state| {
                    state.available = true;
                    state.count = 0;
                    state.detail = None;
                    state.source = source;
                });
                self.refresh.succeeded()
            }
            Count::Unusable(reason) => {
                // Not "zero updates": the card hides, because "up to date" and
                // "I could not tell" look identical on a panel and only one of
                // them is safe to guess.
                warn!("updates: cannot count on this machine ({reason})");
                self.publish(|state| {
                    state.available = false;
                    state.count = 0;
                    state.detail = None;
                    state.source = None;
                });
                self.refresh.failed()
            }
        };
        Some(Instant::now() + wait)
    }

    /// The machine came online or went offline.
    fn set_online(&mut self, online: bool) -> Option<Instant> {
        if self.online == online {
            return None;
        }
        self.online = online;
        if online && self.deferred {
            info!("the machine is back online; checking for updates");
            self.deferred = false;
            return Some(Instant::now());
        }
        None
    }

    /// Edit the snapshot and publish it if anything moved.
    fn publish(&self, edit: impl FnOnce(&mut UpdatesState)) {
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

    fn config(command: Option<&str>) -> UpdatesConfig {
        UpdatesConfig {
            check_interval: 3600,
            update_count_command: command.map(str::to_string),
            flake: None,
        }
    }

    /// A fixture root whose `/etc/os-release` says `id`.
    fn root_saying(id: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "topbar-updates-{}-{id}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(path.join("etc")).expect("a fixture root");
        std::fs::write(path.join("etc/os-release"), format!("ID={id}\n")).expect("write");
        path
    }

    #[test]
    fn the_users_own_command_beats_whatever_the_machine_is() {
        // Somebody who wrote a command has said what they want counted, and
        // second-guessing it with a package manager they may not use would be
        // worse than useless.
        let root = root_saying("arch");
        let plan = plan(&config(Some("pacman -Qu | wc -l")), &root).expect("a plan");
        let Plan::Override(spec) = plan else {
            panic!("the configured command should win");
        };
        assert_eq!(spec.argv, ["sh", "-c", "pacman -Qu | wc -l"]);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_blank_command_is_no_command_at_all() {
        let root = root_saying("arch");
        for blank in [Some("   "), Some(""), None] {
            let plan = plan(&config(blank), &root).expect("a plan");
            assert!(
                matches!(plan, Plan::Native { .. }),
                "an empty key must not shadow the distribution's own tools"
            );
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn nixos_relocks_the_canonical_flake_unless_told_otherwise() {
        let root = root_saying("nixos");
        let plan = plan(&config(None), &root).expect("NixOS counts by re-locking a copy");
        let Plan::NixosFlake { dir } = plan else {
            panic!("expected the flake plan");
        };
        assert_eq!(dir, PathBuf::from(flake_count::DEFAULT_FLAKE_DIR));
        // The override still wins outright, like everywhere else.
        assert!(matches!(
            super::plan(&config(Some("true")), &root),
            Some(Plan::Override(_))
        ));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn the_flake_key_moves_the_relock_and_expands_a_tilde() {
        let root = root_saying("nixos");
        let with_flake = UpdatesConfig {
            flake: Some("~/dotfiles/nixos".into()),
            ..config(None)
        };
        let Some(Plan::NixosFlake { dir }) = plan(&with_flake, &root) else {
            panic!("expected the flake plan");
        };
        assert!(!dir.to_string_lossy().contains('~'), "{}", dir.display());
        assert!(dir.ends_with("dotfiles/nixos"), "{}", dir.display());
        // Blank means unset, matching update_count_command's own rule.
        let blank = UpdatesConfig {
            flake: Some("  ".into()),
            ..config(None)
        };
        let Some(Plan::NixosFlake { dir }) = plan(&blank, &root) else {
            panic!("expected the flake plan");
        };
        assert_eq!(dir, PathBuf::from(flake_count::DEFAULT_FLAKE_DIR));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn an_unrecognised_distribution_is_treated_the_same_way() {
        let root = root_saying("alpine");
        assert!(plan(&config(None), &root).is_none());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn the_check_never_runs_more_often_than_once_a_minute() {
        // Validation already enforces this; the clamp is what stops a `Config`
        // built by hand running `apt-get` in a loop.
        assert_eq!(MIN_INTERVAL, Duration::from_secs(60));
        assert_eq!(Duration::from_secs(0).max(MIN_INTERVAL), MIN_INTERVAL);
        assert_eq!(
            Duration::from_secs(3600).max(MIN_INTERVAL),
            Duration::from_secs(3600)
        );
    }
}
