//! Counting pending updates on NixOS by re-locking a scratch copy.
//!
//! NixOS has no query for "how many updates are pending": `nix flake update`
//! answers it, but answers it by writing the lock file. (`--dry-run` used to
//! exist and was removed when the flake commands were reworked; nix 2.34
//! rejects the flag.) So the panel does what the dry run used to do, at arm's
//! length: copy `flake.nix` and `flake.lock` into a scratch directory, run the
//! real `nix flake update` against the copy, and count the inputs whose pin
//! moved. The system's own lock file is never opened for writing.
//!
//! The flake is assumed to live at [`DEFAULT_FLAKE_DIR`]; `[updates] flake`
//! points somewhere else for systems whose configuration lives in a dotfiles
//! checkout. A flake whose `flake.nix` reaches for sibling files at *lock*
//! time (a `path:./overlay` input, say) will fail against the two-file copy
//! and the card hides — the honest answer, and `update_count_command` remains
//! the escape hatch.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;
use tracing::debug;

use super::parse::Count;
use crate::proc::{self, CmdSpec};

/// Where a NixOS system flake canonically lives.
pub const DEFAULT_FLAKE_DIR: &str = "/etc/nixos";

/// Re-locking fetches the head of every input; a cold cache takes a while.
const RELOCK_TIMEOUT: Duration = Duration::from_secs(120);

/// How many changed inputs go in the card's subtitle.
const DETAIL_LINES: usize = 3;

/// Count the inputs of the flake at `dir` that would move if it were updated.
pub async fn count(dir: &Path) -> Count {
    count_with(dir, |scratch| {
        let argv: Vec<String> = vec![
            "nix".into(),
            "flake".into(),
            "update".into(),
            "--flake".into(),
            format!("path:{}", scratch.display()),
        ];
        CmdSpec::argv(argv).with_timeout(RELOCK_TIMEOUT)
    })
    .await
}

/// The same, with the re-lock command supplied — which is what the tests do,
/// handing in a script by absolute path instead of finding `nix` on `PATH`.
pub(crate) async fn count_with(dir: &Path, relock: impl Fn(&Path) -> CmdSpec) -> Count {
    let old_lock = match std::fs::read_to_string(dir.join("flake.lock")) {
        Ok(text) => text,
        Err(error) => {
            return Count::Unusable(format!(
                "no readable flake.lock in {} ({error}); set [updates] flake to \
                 where the system flake lives",
                dir.display()
            ));
        }
    };
    let scratch = match Scratch::holding(dir) {
        Ok(scratch) => scratch,
        Err(error) => return Count::Unusable(error),
    };

    let captured = match proc::capture(&relock(&scratch.path)).await {
        Ok(captured) => captured,
        Err(error) => return Count::Unusable(error.to_string()),
    };
    if captured.code != Some(0) {
        // nix's first stderr line names the problem better than a status code.
        let reason = captured
            .stderr
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("nix flake update failed")
            .trim()
            .to_string();
        return Count::Unusable(reason);
    }

    let new_lock = match std::fs::read_to_string(scratch.path.join("flake.lock")) {
        Ok(text) => text,
        Err(error) => return Count::Unusable(format!("the re-locked copy vanished ({error})")),
    };

    match diff_locks(&old_lock, &new_lock) {
        Ok((0, _)) => Count::UpToDate,
        Ok((count, mut lines)) => {
            debug!("updates: {count} flake input(s) would move");
            lines.truncate(DETAIL_LINES);
            Count::Found {
                count,
                detail: Some(lines.join("\n")),
            }
        }
        Err(reason) => Count::Unusable(reason),
    }
}

/// How many of `old`'s direct inputs are pinned differently in `new`,
/// and a "name: abc1234 → def5678" line for each.
pub(crate) fn diff_locks(old: &str, new: &str) -> Result<(usize, Vec<String>), String> {
    let old: Value =
        serde_json::from_str(old).map_err(|e| format!("unreadable flake.lock: {e}"))?;
    let new: Value = serde_json::from_str(new).map_err(|e| format!("unreadable re-lock: {e}"))?;

    let inputs = direct_inputs(&old)?;
    let mut lines = Vec::new();
    for name in &inputs {
        let before = pin_of(&old, name);
        let after = pin_of(&new, name);
        if before != after {
            lines.push(format!(
                "{name}: {} → {}",
                short(before.as_deref()),
                short(after.as_deref())
            ));
        }
    }
    Ok((lines.len(), lines))
}

/// The names of the root node's direct inputs, sorted for stable output.
fn direct_inputs(lock: &Value) -> Result<Vec<String>, String> {
    let root = lock["root"].as_str().unwrap_or("root");
    let inputs = lock["nodes"][root]["inputs"]
        .as_object()
        .ok_or("flake.lock has no root inputs")?;
    let mut names: Vec<String> = inputs.keys().cloned().collect();
    names.sort();
    Ok(names)
}

/// What `name` is pinned to: its locked `rev`, falling back to `narHash`.
///
/// The input reference is followed through `follows` indirections (an entry
/// whose value is a path array rather than a node key).
fn pin_of(lock: &Value, name: &str) -> Option<String> {
    let root = lock["root"].as_str().unwrap_or("root");
    let node = match &lock["nodes"][root]["inputs"][name] {
        // The common case: the entry names its node outright.
        Value::String(node) => node.clone(),
        // A follows entry is a path of input names walked from the root.
        Value::Array(path) => {
            let mut node = root.to_string();
            for step in path {
                let step = step.as_str()?;
                node = lock["nodes"][&node]["inputs"][step].as_str()?.to_string();
            }
            node
        }
        _ => return None,
    };
    let locked = &lock["nodes"][&node]["locked"];
    locked["rev"]
        .as_str()
        .or_else(|| locked["narHash"].as_str())
        .map(str::to_string)
}

/// A pin, cut to the seven characters a human compares.
fn short(pin: Option<&str>) -> String {
    match pin {
        Some(pin) => pin.chars().take(7).collect(),
        None => "?".into(),
    }
}

/// A scratch directory holding copies of `flake.nix` and `flake.lock`,
/// removed when dropped.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn holding(dir: &Path) -> Result<Self, String> {
        let path = std::env::temp_dir().join(format!(
            "topbar-relock-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        // A leftover from a killed run is stale, not precious.
        std::fs::remove_dir_all(&path).ok();
        std::fs::create_dir_all(&path).map_err(|e| format!("no scratch directory: {e}"))?;
        for file in ["flake.nix", "flake.lock"] {
            std::fs::copy(dir.join(file), path.join(file))
                .map_err(|e| format!("could not copy {file} out of {} ({e})", dir.display()))?;
        }
        Ok(Self { path })
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lock file with the given pins for inputs `nixpkgs` and `crane`,
    /// `crane` following the root's own `nixpkgs`for its `nixpkgs` input —
    /// the shape `nix flake lock` writes for this repository itself.
    fn lock(nixpkgs: &str, crane: &str) -> String {
        format!(
            r#"{{
  "nodes": {{
    "crane": {{
      "inputs": {{ "nixpkgs": ["nixpkgs"] }},
      "locked": {{ "rev": "{crane}", "narHash": "sha256-c" }}
    }},
    "nixpkgs": {{
      "locked": {{ "rev": "{nixpkgs}", "narHash": "sha256-n" }}
    }},
    "root": {{
      "inputs": {{ "crane": "crane", "nixpkgs": "nixpkgs" }}
    }}
  }},
  "root": "root",
  "version": 7
}}"#
        )
    }

    #[test]
    fn identical_locks_count_zero() {
        let a = lock("aaaaaaa1", "ccccccc1");
        let (count, lines) = diff_locks(&a, &a).expect("well-formed");
        assert_eq!(count, 0);
        assert!(lines.is_empty());
    }

    #[test]
    fn a_moved_pin_is_counted_and_named() {
        let old = lock("aaaaaaa1", "ccccccc1");
        let new = lock("bbbbbbb2", "ccccccc1");
        let (count, lines) = diff_locks(&old, &new).expect("well-formed");
        assert_eq!(count, 1);
        assert_eq!(lines, ["nixpkgs: aaaaaaa → bbbbbbb"]);
    }

    #[test]
    fn every_moved_input_counts_once() {
        let old = lock("aaaaaaa1", "ccccccc1");
        let new = lock("bbbbbbb2", "ddddddd2");
        let (count, lines) = diff_locks(&old, &new).expect("well-formed");
        assert_eq!(count, 2);
        // Sorted by name, so the subtitle is stable run to run.
        assert_eq!(
            lines,
            ["crane: ccccccc → ddddddd", "nixpkgs: aaaaaaa → bbbbbbb"]
        );
    }

    #[test]
    fn an_input_that_appears_in_the_relock_is_a_change() {
        let old = r#"{"nodes":{"root":{"inputs":{"nixpkgs":"nixpkgs"}},
            "nixpkgs":{"locked":{"rev":"aaaaaaa1"}}},"root":"root","version":7}"#;
        let new = lock("aaaaaaa1", "ccccccc1");
        // The old lock has no `crane`; the diff walks the OLD inputs, so a
        // brand-new input is invisible — updating cannot add inputs, only
        // `flake.nix` edits can, and those re-lock on rebuild anyway.
        let (count, _) = diff_locks(old, &new).expect("well-formed");
        assert_eq!(count, 0);
    }

    #[test]
    fn garbage_is_unusable_rather_than_zero() {
        assert!(diff_locks("not json", "{}").is_err());
        assert!(diff_locks("{}", "{}").is_err(), "no root inputs");
    }

    #[tokio::test]
    async fn a_missing_lock_file_names_the_configuration_key() {
        let dir = std::env::temp_dir().join(format!("topbar-noflake-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let count = count(&dir).await;
        let Count::Unusable(reason) = count else {
            panic!("a directory with no flake must be unusable, got {count:?}");
        };
        assert!(reason.contains("[updates] flake"), "{reason}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn the_relock_runs_against_a_copy_and_the_diff_is_read_back() {
        let dir = std::env::temp_dir().join(format!(
            "topbar-flake-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("flake.nix"), "{ outputs = _: { }; }").unwrap();
        let original = lock("aaaaaaa1", "ccccccc1");
        std::fs::write(dir.join("flake.lock"), &original).unwrap();

        // Stands in for `nix flake update`: bump nixpkgs in the scratch copy.
        let script = dir.join("relock.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\ncat > \"$1/flake.lock\" <<'EOF'\n{}\nEOF\n",
                lock("bbbbbbb2", "ccccccc1")
            ),
        )
        .unwrap();

        let count = count_with(&dir, |scratch| {
            let argv: Vec<String> = vec![
                "sh".into(),
                script.display().to_string(),
                scratch.display().to_string(),
            ];
            CmdSpec::argv(argv)
        })
        .await;

        assert_eq!(
            count,
            Count::Found {
                count: 1,
                detail: Some("nixpkgs: aaaaaaa → bbbbbbb".into())
            }
        );
        // The real lock file was never touched.
        assert_eq!(
            std::fs::read_to_string(dir.join("flake.lock")).unwrap(),
            original
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_failing_relock_reports_nix_own_words() {
        let dir = std::env::temp_dir().join(format!("topbar-flakefail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("flake.nix"), "{ }").unwrap();
        std::fs::write(dir.join("flake.lock"), lock("aaaaaaa1", "ccccccc1")).unwrap();

        let count = count_with(&dir, |_| {
            CmdSpec::argv(["sh", "-c", "echo 'error: cannot fetch input' >&2; exit 1"])
        })
        .await;

        assert_eq!(count, Count::Unusable("error: cannot fetch input".into()));
        std::fs::remove_dir_all(&dir).ok();
    }
}
