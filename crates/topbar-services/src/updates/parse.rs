//! Turning a package manager's output into a number.
//!
//! Every one of these is a *contract with one program*, written down and
//! tested against real output, because the failure mode they all share is
//! silent: a parser that matched nothing reports zero pending updates, and
//! "zero" is indistinguishable from "up to date" on the card. So each counter
//! reports [`Count::Unusable`] rather than zero when it cannot make sense of
//! what it was given, and the card hides instead of lying.

use super::distro::Counter;
use crate::proc::Captured;

/// What a count came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Count {
    /// This many updates, and the first few lines to put in a subtitle.
    Found {
        /// How many packages.
        count: usize,
        /// The first lines of the listing, for the card's second line.
        detail: Option<String>,
    },
    /// The command ran and said there is nothing to do.
    UpToDate,
    /// The command could not be used here.
    ///
    /// A missing binary, an exit status the contract does not cover, output
    /// that does not look like what was expected. Different from `UpToDate` in
    /// exactly the way that matters: the card hides and the log says why,
    /// rather than the panel quietly claiming a machine is current.
    Unusable(String),
}

impl Count {
    /// How many, or zero.
    pub fn count(&self) -> usize {
        match self {
            Self::Found { count, .. } => *count,
            _ => 0,
        }
    }
}

/// How many lines of the listing go in the card's subtitle.
const DETAIL_LINES: usize = 3;

/// Read a captured command's output according to `counter`'s contract.
pub fn read(counter: Counter, captured: &Captured) -> Count {
    match counter {
        Counter::Guix => guix(captured),
        Counter::Debian => debian(captured),
        Counter::Arch => arch(captured),
        Counter::Fedora => fedora(captured),
        Counter::Silverblue => silverblue(captured),
    }
}

/// The user's own `update_count_command`.
///
/// v1's contract, kept exactly: **print a number, or one update per line**. A
/// bare integer is the count and there is no listing; anything else is counted
/// by its non-empty lines and those lines become the detail. Both forms are in
/// the wild — `pacman -Qu | wc -l` is the first, `pacman -Qu` is the second —
/// and a configuration that worked in v1 has to keep working.
pub fn override_output(captured: &Captured) -> Count {
    if !captured.ok() {
        return Count::Unusable(format!("the command exited {}", status(captured.code)));
    }
    let trimmed = captured.stdout.trim();
    if trimmed.is_empty() {
        return Count::UpToDate;
    }
    if let Ok(count) = trimmed.parse::<usize>() {
        return if count == 0 {
            Count::UpToDate
        } else {
            // A number is a number: there is nothing to list under it.
            Count::Found {
                count,
                detail: None,
            }
        };
    }
    lines_to_count(trimmed.lines())
}

/// `guix upgrade --dry-run`.
///
/// The listing sits under a "would be upgraded" heading, one indented entry per
/// package. A run that finds nothing prints "nothing to be done" and lists
/// nothing, which is [`Count::UpToDate`] rather than a parse failure.
fn guix(captured: &Captured) -> Count {
    if !captured.ok() {
        return Count::Unusable(format!("guix exited {}", status(captured.code)));
    }
    // Guix writes the listing to stderr on some versions and stdout on others,
    // so both are considered rather than guessing which.
    let text = format!("{}\n{}", captured.stdout, captured.stderr);
    let Some(start) = text
        .lines()
        .position(|line| line.contains("would be upgraded"))
    else {
        return Count::UpToDate;
    };
    let listing: Vec<&str> = text
        .lines()
        .skip(start + 1)
        // The entries are indented; the next unindented line ends the listing.
        .take_while(|line| line.starts_with(char::is_whitespace) && !line.trim().is_empty())
        .map(str::trim)
        .collect();
    lines_to_count(listing.into_iter())
}

/// `apt-get -s upgrade`.
///
/// The simulation prints one `Inst <package> …` line per package it would
/// install or upgrade, among a good deal of other chatter. Counting those lines
/// is apt's own documented way of answering this — it is what
/// `/etc/cron.daily/apt-compat` does.
fn debian(captured: &Captured) -> Count {
    if !captured.ok() {
        return Count::Unusable(format!("apt-get exited {}", status(captured.code)));
    }
    let listing: Vec<&str> = captured
        .stdout
        .lines()
        .filter_map(|line| line.strip_prefix("Inst "))
        .collect();
    if listing.is_empty() {
        return Count::UpToDate;
    }
    lines_to_count(listing.into_iter().map(package_name))
}

/// `checkupdates`.
///
/// One line per package: `name oldver -> newver`. Exit 2 is its documented
/// "no updates available"; anything else that is non-zero is a failure, and a
/// missing binary never gets here at all — `pacman-contrib` may not be
/// installed, and the runner reports that as an error rather than a status.
fn arch(captured: &Captured) -> Count {
    match captured.code {
        Some(0) => {}
        Some(2) => return Count::UpToDate,
        code => {
            return Count::Unusable(format!("checkupdates exited {}", status(code)));
        }
    }
    lines_to_count(captured.stdout.lines().map(package_name))
}

/// `dnf -q check-update`.
///
/// **The exit status is the contract**, and it is the reason this command needs
/// its own arm: `0` means nothing to do, `100` means there are updates, and
/// anything else is a failure. A parser that only looked at the output would
/// read a broken mirror's empty stdout as "up to date".
///
/// The listing is `name.arch  version  repository`, with blank lines and an
/// "Obsoleting Packages" section that is not part of the count.
fn fedora(captured: &Captured) -> Count {
    match captured.code {
        Some(0) => return Count::UpToDate,
        Some(100) => {}
        code => return Count::Unusable(format!("dnf exited {}", status(code))),
    }
    let listing: Vec<&str> = captured
        .stdout
        .lines()
        .take_while(|line| !line.starts_with("Obsoleting Packages"))
        .map(str::trim)
        .filter(|line| !line.is_empty())
        // A package line has three columns; anything shorter is a heading or a
        // continuation of a name too long for one line.
        .filter(|line| line.split_whitespace().count() >= 3)
        .collect();
    if listing.is_empty() {
        // Exit 100 with nothing readable under it is a contract the panel does
        // not understand, not a machine that is up to date.
        return Count::Unusable("dnf said there are updates but listed none".to_string());
    }
    lines_to_count(listing.into_iter().map(package_name))
}

/// `rpm-ostree upgrade --check`.
///
/// Exit `77` is its documented "no upgrade available". A deployment that *is*
/// available is announced as `AvailableUpdate:` followed by an indented block;
/// the useful number in it is the `Diff: N upgraded, M added` line, and where
/// there is no diff the panel counts the update as the one thing it is — a new
/// system image, not a set of packages.
fn silverblue(captured: &Captured) -> Count {
    match captured.code {
        Some(0) => {}
        Some(77) => return Count::UpToDate,
        code => {
            return Count::Unusable(format!("rpm-ostree exited {}", status(code)));
        }
    }
    let text = format!("{}\n{}", captured.stdout, captured.stderr);
    if !text.contains("AvailableUpdate") {
        return Count::UpToDate;
    }

    let detail = text
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("Version:"))
        .map(str::to_string);

    // `Diff: 12 upgraded, 3 added, 1 removed` — the leading number is the one
    // the card wants.
    let diff = text
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("Diff:"))
        .and_then(|diff| {
            diff.split(',')
                .filter_map(|part| part.split_whitespace().next()?.parse::<usize>().ok())
                .reduce(|a, b| a + b)
        });

    Count::Found {
        // No diff means rpm-ostree is offering a new deployment without saying
        // what changed in it. That is one update — the image — rather than a
        // count it declined to give.
        count: diff.filter(|count| *count > 0).unwrap_or(1),
        detail,
    }
}

/// Count non-empty lines and keep the first few for the card's subtitle.
fn lines_to_count<'a, I>(lines: I) -> Count
where
    I: Iterator<Item = &'a str>,
{
    let kept: Vec<&str> = lines
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if kept.is_empty() {
        return Count::UpToDate;
    }
    let detail = kept
        .iter()
        .take(DETAIL_LINES)
        .copied()
        .collect::<Vec<&str>>()
        .join(", ");
    Count::Found {
        count: kept.len(),
        detail: Some(detail),
    }
}

/// The package name out of a listing line.
///
/// The subtitle is a handful of names, not a table: "linux, firefox, mesa"
/// reads at a glance and three columns of versions does not.
fn package_name(line: &str) -> &str {
    line.split_whitespace().next().unwrap_or(line)
}

/// An exit status, as words.
fn status(code: Option<i32>) -> String {
    code.map_or_else(|| "on a signal".to_string(), |code| code.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ran(code: i32, stdout: &str) -> Captured {
        Captured {
            code: Some(code),
            stdout: stdout.to_string(),
            stderr: String::new(),
        }
    }

    fn ran_stderr(code: i32, stderr: &str) -> Captured {
        Captured {
            code: Some(code),
            stdout: String::new(),
            stderr: stderr.to_string(),
        }
    }

    #[test]
    fn arch_counts_one_line_per_package() {
        let output = "\
linux 6.12.4.arch1-1 -> 6.12.5.arch1-1
mesa 1:24.3.1-1 -> 1:24.3.2-1
firefox 133.0-1 -> 133.0.3-1
sqlite 3.47.1-1 -> 3.47.2-1
";
        let Count::Found { count, detail } = read(Counter::Arch, &ran(0, output)) else {
            panic!("expected a count");
        };
        assert_eq!(count, 4);
        assert_eq!(
            detail.as_deref(),
            Some("linux, mesa, firefox"),
            "the subtitle is names, not a table of versions"
        );
    }

    #[test]
    fn arch_reads_its_documented_no_updates_status() {
        // `checkupdates` exits 2 with nothing on stdout when there is nothing
        // to do, which a plain line count would read as zero either way — but
        // exit 1 is a *failure*, and that distinction is the whole point.
        assert_eq!(read(Counter::Arch, &ran(2, "")), Count::UpToDate);
        assert!(matches!(
            read(Counter::Arch, &ran(1, "")),
            Count::Unusable(_)
        ));
    }

    #[test]
    fn debian_counts_the_lines_apt_marks_as_installs() {
        // Real `apt-get -s upgrade` output, trimmed: the `Inst` lines are the
        // count and everything around them is not.
        let output = "\
NOTE: This is only a simulation!
      apt-get needs root privileges for real execution.
Reading package lists...
Building dependency tree...
Reading state information...
Calculating upgrade...
The following packages will be upgraded:
  base-files libc6 libssl3
3 upgraded, 0 newly installed, 0 to remove and 0 not upgraded.
Inst base-files [12.4+deb12u5] (12.4+deb12u7 Debian:12.8/stable [amd64])
Conf base-files (12.4+deb12u7 Debian:12.8/stable [amd64])
Inst libc6 [2.36-9+deb12u7] (2.36-9+deb12u9 Debian:12.8/stable [amd64])
Conf libc6 (2.36-9+deb12u9 Debian:12.8/stable [amd64])
Inst libssl3 [3.0.14-1~deb12u2] (3.0.15-1~deb12u1 Debian:12.8/stable [amd64])
Conf libssl3 (3.0.15-1~deb12u1 Debian:12.8/stable [amd64])
";
        let Count::Found { count, detail } = read(Counter::Debian, &ran(0, output)) else {
            panic!("expected a count");
        };
        assert_eq!(count, 3, "Conf lines are not installs");
        assert_eq!(detail.as_deref(), Some("base-files, libc6, libssl3"));
    }

    #[test]
    fn debian_with_nothing_to_do_says_so() {
        let output = "\
Reading package lists...
Building dependency tree...
Calculating upgrade...
0 upgraded, 0 newly installed, 0 to remove and 0 not upgraded.
";
        assert_eq!(read(Counter::Debian, &ran(0, output)), Count::UpToDate);
    }

    #[test]
    fn fedora_reads_the_exit_status_rather_than_the_output() {
        // The contract that makes dnf different from everything else here: an
        // empty stdout with status 0 is "up to date", and status 100 means
        // there is something to install.
        assert_eq!(read(Counter::Fedora, &ran(0, "")), Count::UpToDate);

        let output = "
kernel.x86_64                          6.11.10-300.fc41            updates
kernel-core.x86_64                     6.11.10-300.fc41            updates
firefox.x86_64                         133.0-1.fc41                updates

Obsoleting Packages
python3-foo.noarch                     1.2-3.fc41                  updates
";
        let Count::Found { count, detail } = read(Counter::Fedora, &ran(100, output)) else {
            panic!("expected a count");
        };
        assert_eq!(count, 3, "obsoleted packages are not pending updates");
        assert_eq!(
            detail.as_deref(),
            Some("kernel.x86_64, kernel-core.x86_64, firefox.x86_64")
        );
    }

    #[test]
    fn a_dnf_that_failed_is_not_a_machine_that_is_up_to_date() {
        // A broken mirror exits 1 with an empty stdout, which a parser that
        // only read the output would call "no updates".
        assert!(matches!(
            read(Counter::Fedora, &ran(1, "")),
            Count::Unusable(_)
        ));
        // And 100 with nothing readable under it is a contract change, not a
        // count of zero.
        assert!(matches!(
            read(Counter::Fedora, &ran(100, "\n\n")),
            Count::Unusable(_)
        ));
    }

    #[test]
    fn silverblue_counts_the_diff_rpm_ostree_offers() {
        let output = "\
AvailableUpdate:
        Version: 41.20241205.0 (2024-12-05T00:53:12Z)
         Commit: 9f3aabbccddeeff00112233445566778899aabbccddeeff001122334455667788
   GPGSignature: Valid signature by ABC
           Diff: 12 upgraded, 3 added, 1 removed
";
        let Count::Found { count, detail } = read(Counter::Silverblue, &ran(0, output)) else {
            panic!("expected a count");
        };
        assert_eq!(count, 16, "everything the deployment changes");
        assert!(detail.as_deref().is_some_and(|d| d.contains("41.2024")));
    }

    #[test]
    fn silverblue_offering_an_image_with_no_diff_counts_it_as_one_update() {
        let output = "\
AvailableUpdate:
        Version: 41.20241205.0 (2024-12-05T00:53:12Z)
";
        assert_eq!(read(Counter::Silverblue, &ran(0, output)).count(), 1);
    }

    #[test]
    fn silverblue_reads_its_documented_no_upgrade_status() {
        assert_eq!(read(Counter::Silverblue, &ran(77, "")), Count::UpToDate);
        assert!(matches!(
            read(Counter::Silverblue, &ran(1, "")),
            Count::Unusable(_)
        ));
    }

    #[test]
    fn guix_counts_the_listing_under_the_heading() {
        let output = "\
The following packages would be upgraded:
   emacs	29.4 → 30.1
   hello	2.12 → 2.12.1
   icecat	128.4.0 → 128.5.0

";
        let Count::Found { count, detail } = read(Counter::Guix, &ran(0, output)) else {
            panic!("expected a count");
        };
        assert_eq!(count, 3);
        assert!(detail.as_deref().is_some_and(|d| d.contains("emacs")));
    }

    #[test]
    fn guix_writing_its_listing_to_stderr_is_read_just_the_same() {
        let output = "The following packages would be upgraded:\n   emacs\t29.4 → 30.1\n";
        assert_eq!(read(Counter::Guix, &ran_stderr(0, output)).count(), 1);
    }

    #[test]
    fn guix_with_nothing_to_do_is_up_to_date() {
        assert_eq!(
            read(Counter::Guix, &ran(0, "nothing to be done\n")),
            Count::UpToDate
        );
    }

    #[test]
    fn the_users_own_command_may_print_a_number_or_a_list() {
        // v1's contract, both halves, because a configuration that worked then
        // has to keep working.
        assert_eq!(
            override_output(&ran(0, "12\n")),
            Count::Found {
                count: 12,
                detail: None
            }
        );
        let Count::Found { count, detail } = override_output(&ran(
            0,
            "linux-libre     6.15.1          6.15.2\nicecat          128.10.0        128.11.0\n",
        )) else {
            panic!("expected a count");
        };
        assert_eq!(count, 2);
        assert!(detail.as_deref().is_some_and(|d| d.contains("linux-libre")));
    }

    #[test]
    fn an_override_printing_zero_or_nothing_is_up_to_date() {
        assert_eq!(override_output(&ran(0, "0\n")), Count::UpToDate);
        assert_eq!(override_output(&ran(0, "")), Count::UpToDate);
        assert_eq!(override_output(&ran(0, "   \n\n")), Count::UpToDate);
    }

    #[test]
    fn an_override_that_failed_hides_the_card_rather_than_reporting_zero() {
        // The live configuration's command names `guix`, and this machine has
        // no Guix any more — so this is the path the user will actually see
        // until they change that key, and it must not claim they are current.
        assert!(matches!(override_output(&ran(127, "")), Count::Unusable(_)));
        assert!(matches!(
            override_output(&ran(1, "3\n")),
            Count::Unusable(_)
        ));
    }

    #[test]
    fn something_that_is_not_a_count_is_counted_as_a_line() {
        // "3 updates" parses as neither an integer nor nothing, so v1 counted
        // it as one line and so does this. Documented rather than clever.
        assert_eq!(override_output(&ran(0, "3 updates\n")).count(), 1);
    }

    #[test]
    fn a_count_of_nothing_is_zero_whatever_shape_it_came_in() {
        assert_eq!(Count::UpToDate.count(), 0);
        assert_eq!(Count::Unusable("boom".into()).count(), 0);
    }

    #[test]
    fn the_subtitle_is_a_handful_of_names_rather_than_a_table() {
        let output = "a 1 -> 2\nb 1 -> 2\nc 1 -> 2\nd 1 -> 2\ne 1 -> 2\n";
        let Count::Found { count, detail } = read(Counter::Arch, &ran(0, output)) else {
            panic!("expected a count");
        };
        assert_eq!(count, 5);
        let detail = detail.expect("a subtitle");
        assert_eq!(detail.matches(',').count(), 2, "three names, two commas");
    }
}
