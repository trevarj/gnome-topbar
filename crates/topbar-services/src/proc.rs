//! The one place the panel starts a process.
//!
//! Everything the user can put a shell command in — `on_click`,
//! `on_click_right`, `on_click_middle`, and from M10 the custom-\* widgets —
//! comes through here, for two reasons.
//!
//! **Nothing is left behind.** A child that is spawned and forgotten becomes a
//! zombie the moment it exits, because nobody waited on it; a panel that runs
//! a click command sixty times an hour would accumulate sixty entries in the
//! process table. Every child started here is handed to a task whose only job
//! is to wait on it, so it is reaped whenever it ends, however it ends.
//!
//! **A command that fails says so.** `sh -c` starts successfully whatever
//! nonsense it is given, and reports the truth in an exit status a moment
//! later — 127 for a command that does not exist. So the runner gives the
//! child [`GRACE`] to fall over: if it does, the caller gets an error it can
//! show, and if it is still running it is detached and left alone, because a
//! click that opened a terminal has succeeded and must not be waited on.

use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;
use tracing::{debug, warn};

use crate::error::SvcError;

/// How long a command is watched before it is assumed to have started.
///
/// Long enough for a shell to report "command not found", short enough that a
/// click never feels like it hung. Everything slower than this is a program
/// that is running, which is what the user asked for.
const GRACE: Duration = Duration::from_millis(250);

/// How much of a failing command's stderr is quoted back.
const STDERR_CAP: usize = 200;

/// Run `command` through a shell, reaping it whatever it does.
///
/// Returns as soon as the command has either failed or been running for
/// [`GRACE`]. An error carries the reason a row can show; the log line carries
/// the rest.
pub async fn run(command: &str) -> Result<(), SvcError> {
    if command.trim().is_empty() {
        return Err(SvcError::Command {
            command: command.to_string(),
            reason: "the command is empty".to_string(),
        });
    }

    debug!("running `{command}`");
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        // No stdin: a command that asks a question the user cannot see would
        // hang for ever holding a reaper task open.
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| SvcError::Command {
            command: command.to_string(),
            reason: error.to_string(),
        })?;

    match tokio::time::timeout(GRACE, child.wait()).await {
        // It fell over inside the grace period: that is a failure the user
        // asked for feedback on.
        Ok(Ok(status)) if !status.success() => {
            let detail = stderr(&mut child).await;
            Err(SvcError::Command {
                command: command.to_string(),
                reason: describe(status.code(), &detail),
            })
        }
        // It finished, successfully, before the grace period was up.
        Ok(Ok(_)) => Ok(()),
        Ok(Err(error)) => Err(SvcError::Command {
            command: command.to_string(),
            reason: error.to_string(),
        }),
        // Still running, which is the normal case: hand it to a reaper and
        // stop caring.
        Err(_) => {
            let name = command.to_string();
            tokio::spawn(async move {
                match child.wait().await {
                    Ok(status) if !status.success() => {
                        warn!("`{name}` ended with {status}");
                    }
                    Ok(_) => debug!("`{name}` finished"),
                    Err(error) => warn!("could not wait for `{name}`: {error}"),
                }
            });
            Ok(())
        }
    }
}

/// Whatever the child managed to say before it died, capped.
async fn stderr(child: &mut tokio::process::Child) -> String {
    use tokio::io::AsyncReadExt;

    let Some(mut pipe) = child.stderr.take() else {
        return String::new();
    };
    let mut buffer = Vec::new();
    let _ = pipe.read_to_end(&mut buffer).await;
    let text = String::from_utf8_lossy(&buffer);
    let trimmed = text.trim();
    if trimmed.chars().count() <= STDERR_CAP {
        return trimmed.to_string();
    }
    trimmed.chars().take(STDERR_CAP).collect::<String>() + "…"
}

/// Turn an exit status and its complaint into one line.
fn describe(code: Option<i32>, detail: &str) -> String {
    let status = match code {
        // What a shell says when the program is not on `PATH`.
        Some(127) => "command not found".to_string(),
        Some(126) => "command not executable".to_string(),
        Some(code) => format!("exited with status {code}"),
        None => "killed by a signal".to_string(),
    };
    if detail.is_empty() {
        status
    } else {
        format!("{status}: {detail}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_program_is_named_as_one() {
        assert_eq!(describe(Some(127), ""), "command not found");
        assert_eq!(
            describe(Some(127), "sh: nope: not found"),
            "command not found: sh: nope: not found"
        );
        assert_eq!(describe(Some(2), ""), "exited with status 2");
        assert_eq!(describe(None, ""), "killed by a signal");
        assert_eq!(describe(Some(126), ""), "command not executable");
    }

    #[tokio::test]
    async fn an_empty_command_is_refused_rather_than_run() {
        let error = run("   ").await.expect_err("nothing to run");
        assert_eq!(error.user_message(), "That command could not be run");
    }

    #[tokio::test]
    async fn a_command_that_works_reports_success() {
        run("true").await.expect("`true` is true");
    }

    #[tokio::test]
    async fn a_command_that_does_not_exist_is_caught_within_the_grace_period() {
        let error = run("topbar-no-such-program-exists")
            .await
            .expect_err("there is no such program");
        assert!(
            error.to_string().contains("not found"),
            "unhelpful message: {error}"
        );
        assert_eq!(error.user_message(), "That command could not be run");
    }

    #[tokio::test]
    async fn a_command_that_fails_quickly_says_what_it_said() {
        let error = run("echo trouble >&2; exit 3")
            .await
            .expect_err("exit 3 is a failure");
        let text = error.to_string();
        assert!(text.contains("status 3"), "{text}");
        assert!(text.contains("trouble"), "{text}");
    }

    #[tokio::test]
    async fn a_long_running_command_is_detached_rather_than_waited_on() {
        let started = std::time::Instant::now();
        run("sleep 30").await.expect("it started");
        assert!(
            started.elapsed() < GRACE * 4,
            "a click that opens an application must not block on it"
        );
    }

    #[tokio::test]
    async fn a_detached_command_is_still_reaped() {
        // Short enough to finish while the test is still running, long enough
        // to outlive the grace period — so it takes the reaper path rather
        // than being waited on inline.
        run("sleep 0.4").await.expect("it started");
        tokio::time::sleep(Duration::from_millis(900)).await;
        assert_eq!(
            zombie_children(),
            0,
            "a click command must not leave a defunct process behind"
        );
    }

    /// How many of this process's children the kernel is still holding.
    ///
    /// Reads `/proc` directly rather than shelling out to `ps`, so the test
    /// does not depend on a tool being installed to prove a tool is reaped.
    fn zombie_children() -> usize {
        let ours = std::process::id();
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return 0;
        };
        entries
            .flatten()
            .filter(|entry| {
                let Ok(status) = std::fs::read_to_string(entry.path().join("status")) else {
                    return false;
                };
                let zombie = status
                    .lines()
                    .any(|line| line.starts_with("State:") && line.contains('Z'));
                let child = status
                    .lines()
                    .find_map(|line| line.strip_prefix("PPid:"))
                    .and_then(|pid| pid.trim().parse::<u32>().ok())
                    == Some(ours);
                zombie && child
            })
            .count()
    }
}
