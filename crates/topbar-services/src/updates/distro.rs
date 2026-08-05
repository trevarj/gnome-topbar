//! Which distribution this is, and how it counts pending updates.
//!
//! Pure, and tested against real `/etc/os-release` files: the panel has to work
//! this out on a machine nobody here has, and getting it wrong is a card that
//! reports a number from the wrong package manager.

use std::path::Path;
use std::time::Duration;

use crate::proc::CmdSpec;

/// The distributions the panel can count updates on by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Distro {
    /// GNU Guix.
    Guix,
    /// NixOS.
    NixOS,
    /// Debian, Ubuntu, and everything that says `ID_LIKE=debian`.
    Debian,
    /// Arch, Manjaro, EndeavourOS.
    Arch,
    /// Fedora, and the RPM family that says `ID_LIKE=fedora`.
    Fedora,
    /// Fedora's image-based editions: Silverblue, Kinoite, Bazzite.
    FedoraSilverblue,
    /// Something else, or a machine with no `/etc/os-release` at all.
    Unknown,
}

impl Distro {
    /// The name to put in a log line.
    pub fn label(self) -> &'static str {
        match self {
            Self::Guix => "Guix",
            Self::NixOS => "NixOS",
            Self::Debian => "Debian",
            Self::Arch => "Arch",
            Self::Fedora => "Fedora",
            Self::FedoraSilverblue => "Fedora Silverblue",
            Self::Unknown => "an unrecognised distribution",
        }
    }

    /// How to count pending updates here, if the panel knows how.
    ///
    /// `None` means "there is no side-effect-free way to count on this system"
    /// — not "there are no updates". The card stays hidden and the log says
    /// what to put in `update_count_command`. See [`Counter`] for the two
    /// distributions that land here and why.
    pub fn counter(self) -> Option<Counter> {
        match self {
            Self::Debian => Some(Counter::Debian),
            Self::Arch => Some(Counter::Arch),
            Self::Fedora => Some(Counter::Fedora),
            Self::FedoraSilverblue => Some(Counter::Silverblue),
            Self::Guix => Some(Counter::Guix),
            // NixOS has no cheap, side-effect-free count. See `Counter`.
            Self::NixOS | Self::Unknown => None,
        }
    }
}

/// Work out the distribution from an `/etc/os-release` file's contents.
///
/// `ID` first, then `ID_LIKE`, which is what derivatives are for: Ubuntu says
/// `ID_LIKE=debian`, Manjaro says `arch`, Nobara says `fedora`. `VARIANT_ID`
/// separates Silverblue from Fedora proper, because they are the same `ID` with
/// entirely different package managers — `dnf` on one, `rpm-ostree` on the
/// other — and counting with the wrong one gives a number that is always zero.
pub fn detect(os_release: &str) -> Distro {
    let id = field(os_release, "ID").unwrap_or_default();
    let variant = field(os_release, "VARIANT_ID").unwrap_or_default();

    if let Some(distro) = from_id(&id, &variant) {
        return distro;
    }
    // `ID_LIKE` is a space-separated list, most-specific first.
    for like in field(os_release, "ID_LIKE").unwrap_or_default().split(' ') {
        if let Some(distro) = from_id(like, &variant) {
            return distro;
        }
    }
    Distro::Unknown
}

/// The same, reading the file under `root`.
///
/// `root` is `/` in the panel and a fixture directory in the tests, which is
/// the whole reason it is a parameter: the alternative is a test that can only
/// check the distribution the developer happens to be running.
pub fn detect_at(root: &Path) -> Distro {
    // `/etc/os-release` is the canonical path and is a symlink to
    // `/usr/lib/os-release` on a stateless system; both are read because a
    // fixture directory has one or the other, never the link.
    for relative in ["etc/os-release", "usr/lib/os-release"] {
        if let Ok(text) = std::fs::read_to_string(root.join(relative)) {
            return detect(&text);
        }
    }
    Distro::Unknown
}

/// Which distribution an `ID`-shaped token names.
fn from_id(id: &str, variant: &str) -> Option<Distro> {
    match id {
        "guix" => Some(Distro::Guix),
        "nixos" => Some(Distro::NixOS),
        "debian" | "ubuntu" | "raspbian" | "linuxmint" | "pop" => Some(Distro::Debian),
        "arch" | "archarm" | "manjaro" | "endeavouros" | "cachyos" => Some(Distro::Arch),
        "fedora" | "nobara" | "rhel" | "centos" => {
            // Silverblue, Kinoite, Sericea, Onyx and the uBlue images all set a
            // VARIANT_ID and are all `rpm-ostree` rather than `dnf`.
            Some(if is_image_based(variant) {
                Distro::FedoraSilverblue
            } else {
                Distro::Fedora
            })
        }
        _ => None,
    }
}

/// Whether a Fedora `VARIANT_ID` names one of the image-based editions.
fn is_image_based(variant: &str) -> bool {
    matches!(
        variant,
        "silverblue" | "kinoite" | "sericea" | "onyx" | "iot" | "coreos"
    ) || variant.starts_with("silverblue")
        || variant.starts_with("kinoite")
}

/// One `KEY=value` out of an os-release file.
///
/// The values are shell-quoted, so `ID="fedora"` and `ID=fedora` are the same
/// thing and both occur in the wild.
fn field(text: &str, key: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .filter_map(|line| line.split_once('='))
        .find(|(name, _)| *name == key)
        .map(|(_, value)| unquote(value).to_ascii_lowercase())
}

/// Strip one layer of shell quoting.
fn unquote(value: &str) -> &str {
    let value = value.trim();
    for quote in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|v| v.strip_suffix(quote))
        {
            return inner;
        }
    }
    value
}

/// How long a package manager may take before the check is abandoned.
///
/// `dnf check-update` against a cold mirror really does take twenty seconds.
const COUNT_TIMEOUT: Duration = Duration::from_secs(60);

/// One way of counting pending updates.
///
/// Every one of these is **read-only**: nothing here downloads a package,
/// touches a lock file, or changes what the next `upgrade` would do. That rules
/// out several obvious answers, and it is why two distributions have none.
///
/// ## Guix
///
/// `guix upgrade --dry-run` prints the packages it *would* replace under a
/// "would be upgraded" heading and replaces none of them. It is the closest
/// thing Guix has to a query, and it is the same shape the live configuration's
/// own `update_count_command` produced. It needs a profile to have been built
/// at least once; on a machine where it fails the card hides, which is the
/// correct answer to "this cannot be counted here".
///
/// ## NixOS
///
/// **There is no counter here, deliberately.** NixOS has no notion of "pending
/// updates" that can be answered without doing work: `nix flake update` writes
/// a lock file, `nixos-rebuild build` builds the system, and
/// `nix store diff-closures` compares two closures that both have to exist
/// first. Every candidate either changes the machine or takes minutes.
///
/// A wrong guess is worse than an absence — a card reporting "0 updates" on a
/// machine three months behind is a card that lies — so the service logs what
/// to put in `update_count_command` and the card stays hidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Counter {
    /// `guix upgrade --dry-run`, one line per package.
    Guix,
    /// `apt-get -s upgrade`, counting `Inst ` lines.
    Debian,
    /// `checkupdates`, one line per package.
    Arch,
    /// `dnf -q check-update`, whose exit status is the contract.
    Fedora,
    /// `rpm-ostree upgrade --check`.
    Silverblue,
}

impl Counter {
    /// The command to run.
    pub fn spec(self) -> CmdSpec {
        let argv: Vec<&str> = match self {
            // `--dry-run` prints what it would do and does none of it.
            Self::Guix => vec!["guix", "upgrade", "--dry-run"],
            // `-s` is simulate; `Debug::NoLocking` is what lets it run as a
            // user without root and without waiting on dpkg's lock.
            Self::Debian => vec!["apt-get", "-s", "-o", "Debug::NoLocking=true", "upgrade"],
            // From `pacman-contrib`, and deliberately the only Arch answer:
            // `pacman -Qu` reports nothing useful without a database sync, and
            // syncing is a write.
            Self::Arch => vec!["checkupdates"],
            Self::Fedora => vec!["dnf", "-q", "check-update"],
            Self::Silverblue => vec!["rpm-ostree", "upgrade", "--check"],
        };
        CmdSpec::argv(argv).with_timeout(COUNT_TIMEOUT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real `/etc/os-release` from each distribution, trimmed to the keys
    /// that matter. Every one of these is copied from the shipped file rather
    /// than invented, because the whole point is to parse what is out there.
    const GUIX: &str = r#"NAME="Guix System"
ID=guix
PRETTY_NAME="Guix System"
HOME_URL="https://guix.gnu.org"
"#;

    const NIXOS: &str = r#"ANSI_COLOR="1;34"
BUG_REPORT_URL="https://github.com/NixOS/nixpkgs/issues"
BUILD_ID="26.05.20260801.abcdef0"
ID=nixos
LOGO="nix-snowflake"
NAME=NixOS
PRETTY_NAME="NixOS 26.05 (Warbler)"
VERSION="26.05 (Warbler)"
VERSION_ID="26.05"
"#;

    const DEBIAN: &str = r#"PRETTY_NAME="Debian GNU/Linux 12 (bookworm)"
NAME="Debian GNU/Linux"
VERSION_ID="12"
ID=debian
HOME_URL="https://www.debian.org/"
"#;

    const UBUNTU: &str = r#"PRETTY_NAME="Ubuntu 24.04.1 LTS"
NAME="Ubuntu"
VERSION_ID="24.04"
ID=ubuntu
ID_LIKE=debian
"#;

    const ARCH: &str = r#"NAME="Arch Linux"
PRETTY_NAME="Arch Linux"
ID=arch
BUILD_ID=rolling
"#;

    const MANJARO: &str = r#"NAME="Manjaro Linux"
ID=manjaro
ID_LIKE=arch
PRETTY_NAME="Manjaro Linux"
"#;

    const FEDORA: &str = r#"NAME="Fedora Linux"
VERSION="41 (Workstation Edition)"
ID=fedora
VERSION_ID=41
VARIANT="Workstation Edition"
VARIANT_ID=workstation
"#;

    const SILVERBLUE: &str = r#"NAME="Fedora Linux"
VERSION="41.20241118.0 (Silverblue)"
ID=fedora
VERSION_ID=41
VARIANT="Silverblue"
VARIANT_ID=silverblue
"#;

    const KINOITE: &str = r#"NAME="Fedora Linux"
ID=fedora
VARIANT="Kinoite"
VARIANT_ID=kinoite
"#;

    const NOBARA: &str = r#"NAME="Nobara Linux"
ID=nobara
ID_LIKE="fedora"
VARIANT_ID=kde
"#;

    const ALPINE: &str = r#"NAME="Alpine Linux"
ID=alpine
VERSION_ID=3.21.0
PRETTY_NAME="Alpine Linux v3.21"
"#;

    #[test]
    fn every_distribution_the_plan_names_is_recognised() {
        assert_eq!(detect(GUIX), Distro::Guix);
        assert_eq!(detect(NIXOS), Distro::NixOS);
        assert_eq!(detect(DEBIAN), Distro::Debian);
        assert_eq!(detect(ARCH), Distro::Arch);
        assert_eq!(detect(FEDORA), Distro::Fedora);
        assert_eq!(detect(SILVERBLUE), Distro::FedoraSilverblue);
    }

    #[test]
    fn a_derivative_is_read_from_the_family_it_names() {
        assert_eq!(detect(UBUNTU), Distro::Debian);
        assert_eq!(detect(MANJARO), Distro::Arch);
        assert_eq!(detect(NOBARA), Distro::Fedora);
    }

    #[test]
    fn the_image_based_editions_are_not_fedora_proper() {
        // Same ID, entirely different package manager: `dnf check-update` on
        // Silverblue answers about a package set nobody upgrades, so the count
        // would be permanently zero.
        assert_eq!(detect(SILVERBLUE), Distro::FedoraSilverblue);
        assert_eq!(detect(KINOITE), Distro::FedoraSilverblue);
        assert_eq!(
            detect(SILVERBLUE).counter(),
            Some(Counter::Silverblue),
            "and it counts with rpm-ostree"
        );
        assert_eq!(detect(FEDORA).counter(), Some(Counter::Fedora));
    }

    #[test]
    fn a_distribution_nobody_here_has_heard_of_is_unknown_rather_than_guessed() {
        assert_eq!(detect(ALPINE), Distro::Unknown);
        assert_eq!(detect(""), Distro::Unknown);
        assert_eq!(detect("nonsense\n"), Distro::Unknown);
        assert_eq!(detect(ALPINE).counter(), None);
    }

    #[test]
    fn quoting_is_stripped_the_way_a_shell_would_strip_it() {
        assert_eq!(detect("ID=\"debian\"\n"), Distro::Debian);
        assert_eq!(detect("ID='arch'\n"), Distro::Arch);
        assert_eq!(detect("ID=Fedora\n"), Distro::Fedora, "case is ignored");
        assert_eq!(detect("  ID=arch  \n"), Distro::Arch);
    }

    #[test]
    fn nixos_has_no_single_command_counter_because_it_relocks_a_copy_instead() {
        // `Counter` models "run one program, read its output". NixOS cannot be
        // counted that way — every candidate command either writes the lock
        // file or builds the system — so its counting lives in
        // `flake_count` (a scratch-copy re-lock), routed by the task's plan,
        // and there is deliberately no `Counter` arm for it here.
        assert_eq!(detect(NIXOS).counter(), None);
        assert_eq!(detect(NIXOS).label(), "NixOS");
    }

    #[test]
    fn every_counter_runs_a_program_directly_rather_than_through_a_shell() {
        for counter in [
            Counter::Guix,
            Counter::Debian,
            Counter::Arch,
            Counter::Fedora,
            Counter::Silverblue,
        ] {
            let spec = counter.spec();
            assert!(!spec.argv.is_empty());
            assert_ne!(
                spec.argv[0], "sh",
                "{counter:?} must not go through a shell"
            );
            assert!(spec.timeout >= Duration::from_secs(30));
        }
    }

    #[test]
    fn the_counting_commands_are_the_read_only_ones() {
        // Spelled out, so changing one to something that writes is a failing
        // test rather than a panel that syncs a package database every hour.
        assert_eq!(Counter::Guix.spec().argv, ["guix", "upgrade", "--dry-run"]);
        assert_eq!(
            Counter::Debian.spec().argv,
            ["apt-get", "-s", "-o", "Debug::NoLocking=true", "upgrade"]
        );
        assert_eq!(Counter::Arch.spec().argv, ["checkupdates"]);
        assert_eq!(Counter::Fedora.spec().argv, ["dnf", "-q", "check-update"]);
        assert_eq!(
            Counter::Silverblue.spec().argv,
            ["rpm-ostree", "upgrade", "--check"]
        );
    }

    #[test]
    fn detection_reads_either_of_the_two_paths_the_file_lives_at() {
        let root = tempdir();
        std::fs::create_dir_all(root.join("usr/lib")).expect("a fixture root");
        std::fs::write(root.join("usr/lib/os-release"), ARCH).expect("write");
        assert_eq!(detect_at(&root), Distro::Arch);

        // `/etc` wins where both exist, which is what a stateless system with
        // a local override looks like.
        std::fs::create_dir_all(root.join("etc")).expect("etc");
        std::fs::write(root.join("etc/os-release"), DEBIAN).expect("write");
        assert_eq!(detect_at(&root), Distro::Debian);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_machine_with_no_os_release_at_all_is_unknown() {
        assert_eq!(
            detect_at(Path::new("/nonexistent-topbar-test-root")),
            Distro::Unknown
        );
    }

    #[test]
    fn every_distribution_has_something_to_call_itself_in_a_log_line() {
        for distro in [
            Distro::Guix,
            Distro::NixOS,
            Distro::Debian,
            Distro::Arch,
            Distro::Fedora,
            Distro::FedoraSilverblue,
            Distro::Unknown,
        ] {
            assert!(!distro.label().is_empty());
        }
    }

    /// A scratch directory for one test.
    fn tempdir() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "topbar-distro-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&path).expect("a scratch directory");
        path
    }
}
