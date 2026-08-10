//! What notmuch printed.
//!
//! Both readers are pure and both refuse rather than guess: output that does
//! not look like what was asked for produces an error, and an error hides the
//! widget. The alternative — reading a truncated or reshaped answer as "no new
//! mail" — is the one failure mode a mail indicator must not have.

use serde::Deserialize;

use super::MailThread;

/// Shown for a message whose author notmuch could not name.
const UNKNOWN_SENDER: &str = "Unknown sender";
/// Shown for one with an empty `Subject:`.
const NO_SUBJECT: &str = "(no subject)";

/// What `notmuch count --lastmod` answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Counted {
    /// How many messages matched.
    pub count: usize,
    /// The database revision the count was taken at.
    ///
    /// Only meaningful next to the UUID it came with, which is why a database
    /// that has been rebuilt — new UUID, revision back near zero — is treated
    /// as a change rather than compared numerically.
    pub revision: u64,
    /// The database's UUID.
    pub uuid: String,
}

/// Read `count \t uuid \t revision`.
///
/// That is one line and exactly three fields. Anything else is notmuch
/// answering a question this code did not ask.
pub fn counted(stdout: &str) -> Result<Counted, String> {
    let line = stdout
        .lines()
        .next()
        .ok_or_else(|| "notmuch printed nothing".to_string())?;

    let mut fields = line.split('\t');
    let (Some(count), Some(uuid), Some(revision), None) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return Err(format!("expected three tab-separated fields, got {line:?}"));
    };

    Ok(Counted {
        count: count
            .trim()
            .parse()
            .map_err(|_| format!("{count:?} is not a count"))?,
        revision: revision
            .trim()
            .parse()
            .map_err(|_| format!("{revision:?} is not a revision"))?,
        uuid: uuid.trim().to_string(),
    })
}

/// One element of `notmuch search --format=json --output=summary`.
///
/// Everything but the thread id is defaulted: a thread with no subject and no
/// author is odd but not a reason to drop the whole list.
#[derive(Debug, Deserialize)]
struct Summary {
    thread: String,
    #[serde(default)]
    authors: String,
    #[serde(default)]
    subject: String,
    #[serde(default)]
    date_relative: String,
    #[serde(default)]
    matched: usize,
}

/// Read the conversation list.
pub fn threads(stdout: &str) -> Result<Vec<MailThread>, String> {
    let summaries: Vec<Summary> = serde_json::from_str(stdout)
        .map_err(|error| format!("unreadable search output: {error}"))?;

    Ok(summaries
        .into_iter()
        .map(|summary| MailThread {
            thread: summary.thread,
            sender: sender(&summary.authors),
            subject: subject(&summary.subject),
            when: summary.date_relative,
            matched: summary.matched,
        })
        .collect())
}

/// The one name to put on a row.
///
/// Notmuch writes the authors of a thread as the ones whose messages *matched*
/// the query, a `|`, then the ones whose did not. For unread mail that
/// distinction is the whole point: the people in the second list are the ones
/// who wrote the parts already read.
fn sender(authors: &str) -> String {
    let (matched, rest) = authors.split_once('|').unwrap_or((authors, ""));
    let first = |names: &str| {
        names
            .split(',')
            .map(str::trim)
            .find(|name| !name.is_empty())
            .map(str::to_string)
    };
    first(matched)
        .or_else(|| first(rest))
        .unwrap_or_else(|| UNKNOWN_SENDER.to_string())
}

/// The subject, or a stand-in for a message that has none.
fn subject(subject: &str) -> String {
    let subject = subject.trim();
    if subject.is_empty() {
        NO_SUBJECT.to_string()
    } else {
        subject.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape `notmuch search --format=json --output=summary` really has,
    /// down to the `|` in `authors` and the fields this code ignores.
    const SEARCH: &str = r#"[
      {"thread": "0000000000000691",
       "timestamp": 1786328986,
       "date_relative": "Today 02:29",
       "matched": 18,
       "total": 103,
       "authors": "Jean Louis, Cecilio Pardo| tomas@tuxteam.de, Eli Zaretskii",
       "subject": "AGENTS.md",
       "tags": ["inbox", "unread"]},
      {"thread": "00000000000004d2",
       "timestamp": 1786300000,
       "date_relative": "Yest. 18:26",
       "matched": 1,
       "total": 1,
       "authors": "Someone Else",
       "subject": "",
       "tags": ["inbox", "unread"]}
    ]"#;

    #[test]
    fn a_lastmod_line_yields_the_count_and_the_revision() {
        let counted = counted("352\t61f82ad2-8b7d-4d25-a23e-45b8e7d71d6d\t37939\n").unwrap();
        assert_eq!(counted.count, 352);
        assert_eq!(counted.revision, 37939);
        assert_eq!(counted.uuid, "61f82ad2-8b7d-4d25-a23e-45b8e7d71d6d");
    }

    #[test]
    fn an_empty_inbox_is_a_zero_and_not_a_failure() {
        // The one case that must not be confused with a broken database.
        let counted = counted("0\t61f82ad2\t37939\n").unwrap();
        assert_eq!(counted.count, 0);
    }

    #[test]
    fn output_that_is_not_three_fields_is_refused_rather_than_read_as_zero() {
        for output in [
            "",
            "352\n",
            "352\t61f82ad2\n",
            "352\ta\t1\textra\n",
            "lots\ta\t1\n",
        ] {
            assert!(counted(output).is_err(), "{output:?} was accepted");
        }
    }

    #[test]
    fn a_thread_summary_becomes_a_row() {
        let threads = threads(SEARCH).unwrap();
        assert_eq!(threads.len(), 2);
        assert_eq!(threads[0].thread, "0000000000000691");
        assert_eq!(threads[0].subject, "AGENTS.md");
        assert_eq!(threads[0].when, "Today 02:29");
        assert_eq!(threads[0].matched, 18);
    }

    #[test]
    fn the_sender_is_the_first_author_whose_message_actually_matched() {
        // Everything after the `|` wrote the part that has already been read.
        assert_eq!(
            sender("Jean Louis, Cecilio Pardo| tomas@tuxteam.de"),
            "Jean Louis"
        );
        assert_eq!(sender("Stefan Monnier, Andreas Schwab"), "Stefan Monnier");
    }

    #[test]
    fn a_thread_nobody_matched_falls_back_to_whoever_is_left() {
        assert_eq!(
            sender("| tomas@tuxteam.de, Eli Zaretskii"),
            "tomas@tuxteam.de"
        );
        assert_eq!(sender(""), UNKNOWN_SENDER);
        assert_eq!(sender("|"), UNKNOWN_SENDER);
    }

    #[test]
    fn a_message_with_no_subject_still_says_something() {
        let threads = threads(SEARCH).unwrap();
        assert_eq!(threads[1].subject, NO_SUBJECT);
        assert_eq!(threads[1].sender, "Someone Else");
    }

    #[test]
    fn an_empty_result_is_an_empty_list_and_not_an_error() {
        assert_eq!(threads("[]").unwrap(), Vec::new());
    }

    #[test]
    fn output_that_is_not_json_is_refused_rather_than_read_as_empty() {
        for output in ["", "notmuch: unknown option", "{}", "[{\"no\": 1}]"] {
            assert!(threads(output).is_err(), "{output:?} was accepted");
        }
    }
}
