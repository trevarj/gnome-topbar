//! Which notmuch runs, and with what arguments.
//!
//! Pure, so the two things that matter about these commands can be asserted
//! without a mail store: that the user's query reaches notmuch as **one
//! argument** and never as shell text, and that the JSON asked for is a format
//! version this code has actually been written against.

use std::path::Path;
use std::time::Duration;

use crate::proc::CmdSpec;

/// The program, when the configuration does not name one.
pub const PROGRAM: &str = "notmuch";

/// How long either command may take.
///
/// A count against this developer's 7,000-message database is eight
/// milliseconds. Two seconds is not a budget, it is a tripwire: past it the
/// database is locked or the disk is asleep, and the right answer is to hide
/// the widget and try again on the next tick rather than to hold a task open.
const TIMEOUT: Duration = Duration::from_secs(2);

/// The JSON schema version this code parses.
///
/// Pinned, not left to default: notmuch bumps it, and an unpinned reader is a
/// reader that changes meaning when the machine is updated. notmuch 0.40
/// accepts 1 through 5.
pub const FORMAT_VERSION: u32 = 5;

/// Which notmuch to run.
fn program(program: Option<&Path>) -> String {
    program.map_or_else(
        || PROGRAM.to_string(),
        |path| path.to_string_lossy().into_owned(),
    )
}

/// Count the messages matching `query`, and say what revision the database is
/// at while it is there.
///
/// `--lastmod` is what makes the expensive command rare: the revision only
/// moves when something was indexed, so an unchanged one means the list on
/// screen is still the right list.
pub fn count(notmuch: Option<&Path>, query: &str) -> CmdSpec {
    CmdSpec::argv([
        program(notmuch),
        "count".to_string(),
        "--lastmod".to_string(),
        // Everything the panel decides for itself goes in as its own argument.
        // The query is the user's own text and must never see a shell.
        query.to_string(),
    ])
    .with_timeout(TIMEOUT)
}

/// List the newest `limit` conversations matching `query`.
pub fn search(notmuch: Option<&Path>, query: &str, limit: u32) -> CmdSpec {
    CmdSpec::argv([
        program(notmuch),
        "search".to_string(),
        "--format=json".to_string(),
        format!("--format-version={FORMAT_VERSION}"),
        "--output=summary".to_string(),
        "--sort=newest-first".to_string(),
        format!("--limit={limit}"),
        query.to_string(),
    ])
    .with_timeout(TIMEOUT)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn the_count_asks_for_the_revision_as_well_as_the_number() {
        let spec = count(None, "tag:unread and tag:inbox");
        assert_eq!(
            spec.argv,
            ["notmuch", "count", "--lastmod", "tag:unread and tag:inbox"]
        );
    }

    #[test]
    fn the_search_pins_the_json_version_it_was_written_against() {
        let spec = search(None, "tag:unread", 10);
        assert!(
            spec.argv.contains(&"--format-version=5".to_string()),
            "{:?}",
            spec.argv
        );
        assert!(spec.argv.contains(&"--limit=10".to_string()));
        assert!(spec.argv.contains(&"--sort=newest-first".to_string()));
    }

    #[test]
    fn a_query_is_one_argument_however_it_is_written() {
        // The query is the user's own text out of their own file. If it ever
        // reached a shell this would be two commands instead of one.
        let hostile = "tag:unread; rm -rf ~/Mail";
        for spec in [count(None, hostile), search(None, hostile, 10)] {
            assert!(
                spec.argv.iter().any(|argument| argument == hostile),
                "the query was split up: {:?}",
                spec.argv
            );
            assert!(
                !spec.argv.iter().any(|argument| argument == "sh"),
                "the query reached a shell: {:?}",
                spec.argv
            );
        }
    }

    #[test]
    fn a_named_program_is_run_instead_of_whatever_path_says() {
        // The smoke run's belt: prepending a fake to PATH still leaves the
        // real notmuch, and the real mail, one directory behind it.
        let fake = PathBuf::from("/tmp/fake/notmuch");
        let spec = count(Some(&fake), "tag:unread");
        assert_eq!(spec.argv[0], "/tmp/fake/notmuch");
    }

    #[test]
    fn neither_command_may_sit_there_for_the_default_forty_five_seconds() {
        assert_eq!(count(None, "tag:unread").timeout, TIMEOUT);
        assert_eq!(search(None, "tag:unread", 1).timeout, TIMEOUT);
    }
}
