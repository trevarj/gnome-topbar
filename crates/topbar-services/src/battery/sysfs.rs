//! The kernel's own view of the charge limit.
//!
//! `/sys/class/power_supply/BAT0/charge_control_{start,end}_threshold` is the
//! source of truth: it is what the firmware is actually enforcing, it updates
//! the instant a write lands, and UPower can lag behind it by seconds. So the
//! panel reads it whenever it can and writes it whenever it is allowed to,
//! falling back to UPower only when the files are root-owned.
//!
//! The root is a parameter rather than a constant so the tests can point it at
//! a temporary directory. Nothing in the crate writes to a real battery during
//! `cargo test`.

use std::fs;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

use super::model::{Thresholds, Write, ordered_writes};

/// Where the kernel publishes power supplies.
pub const POWER_SUPPLY: &str = "/sys/class/power_supply";

/// The two files a charge limit lives in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThresholdPaths {
    /// Where charging resumes.
    pub start: PathBuf,
    /// Where charging stops.
    pub end: PathBuf,
}

/// The first system battery under `root`.
///
/// Peripheral batteries are skipped: a wireless mouse reporting 40% has no
/// business becoming the laptop's battery indicator, and it advertises itself
/// with `scope=Device`. A battery with no `scope` file at all is a system one,
/// which is how it reads on most laptops.
pub fn battery_path(root: &Path) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            read_trimmed(&path.join("type"))
                .is_some_and(|kind| kind.eq_ignore_ascii_case("battery"))
        })
        .filter(|path| {
            !read_trimmed(&path.join("scope"))
                .is_some_and(|scope| scope.eq_ignore_ascii_case("device"))
        })
        .collect();
    // `read_dir` has no defined order, and a machine with two batteries would
    // otherwise pick a different one on every start.
    candidates.sort();
    candidates.into_iter().next()
}

/// The threshold files of `battery`, under either of the two kernel spellings.
///
/// `charge_control_*` is the modern one; `charge_{start,stop}_threshold` is
/// what older ThinkPad and ASUS drivers export. Both are checked because both
/// are still in the field.
pub fn threshold_paths(battery: &Path) -> Option<ThresholdPaths> {
    let modern = ThresholdPaths {
        start: battery.join("charge_control_start_threshold"),
        end: battery.join("charge_control_end_threshold"),
    };
    if modern.start.exists() && modern.end.exists() {
        return Some(modern);
    }

    let legacy = ThresholdPaths {
        start: battery.join("charge_start_threshold"),
        end: battery.join("charge_stop_threshold"),
    };
    if legacy.start.exists() && legacy.end.exists() {
        return Some(legacy);
    }

    None
}

/// The charge limit as the kernel has it, under `root`.
pub fn read_thresholds(root: &Path) -> Option<Thresholds> {
    let battery = battery_path(root)?;
    let paths = threshold_paths(&battery)?;
    Some(Thresholds {
        start: read_u8(&paths.start)?,
        end: read_u8(&paths.end)?,
        writable: writable(&paths),
    })
}

/// Whether this process may write both files.
///
/// Opening for writing is the only honest test: the mode bits say root owns
/// them, but a udev rule may have handed the group write access, and a
/// container may have taken it away again.
pub fn writable(paths: &ThresholdPaths) -> bool {
    can_write(&paths.start) && can_write(&paths.end)
}

fn can_write(path: &Path) -> bool {
    OpenOptions::new().write(true).open(path).is_ok()
}

/// Write a charge limit, returning what went wrong if anything did.
///
/// The current end is read first so the two writes can be ordered safely — see
/// [`ordered_writes`].
pub fn write_thresholds(root: &Path, start: u8, end: u8) -> Result<(), String> {
    let battery = battery_path(root).ok_or_else(|| "no system battery".to_string())?;
    let paths = threshold_paths(&battery)
        .ok_or_else(|| format!("no charge-threshold files under {}", battery.display()))?;
    if !writable(&paths) {
        return Err("the charge-threshold files are not writable".to_string());
    }

    let current_end = read_u8(&paths.end);
    for write in ordered_writes(&paths.start, &paths.end, start, end, current_end) {
        apply(&write)?;
    }
    Ok(())
}

/// Put one value in one file.
fn apply(write: &Write) -> Result<(), String> {
    fs::write(&write.path, format!("{}\n", write.value))
        .map_err(|error| format!("could not write {}: {error}", write.path.display()))
}

/// A sysfs file's contents, trimmed, or `None` when it is missing or empty.
fn read_trimmed(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|content| content.trim().to_string())
        .filter(|content| !content.is_empty())
}

/// A sysfs file holding a percentage.
fn read_u8(path: &Path) -> Option<u8> {
    read_trimmed(path)?.parse().ok()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A temporary directory that removes itself.
    pub(crate) struct TempRoot(PathBuf);

    impl TempRoot {
        /// Make one, named after the test that asked for it.
        pub(crate) fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "topbar-battery-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("a temporary directory");
            Self(path)
        }

        /// The root to hand the service.
        pub(crate) fn path(&self) -> &Path {
            &self.0
        }

        /// Write a supply directory with the given files.
        pub(crate) fn supply(&self, name: &str, files: &[(&str, &str)]) -> PathBuf {
            let directory = self.0.join(name);
            fs::create_dir_all(&directory).expect("a supply directory");
            for (file, contents) in files {
                fs::write(directory.join(file), contents).expect("a supply file");
            }
            directory
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A battery with modern threshold files at `start`/`end`.
    pub(crate) fn battery_with_thresholds(root: &TempRoot, start: u8, end: u8) -> PathBuf {
        root.supply(
            "BAT0",
            &[
                ("type", "Battery\n"),
                ("status", "Discharging\n"),
                ("charge_control_start_threshold", &format!("{start}\n")),
                ("charge_control_end_threshold", &format!("{end}\n")),
            ],
        )
    }

    #[test]
    fn a_system_battery_is_found_and_a_mouse_is_not() {
        let root = TempRoot::new("scope");
        root.supply(
            "hidpp_battery_0",
            &[("type", "Battery\n"), ("scope", "Device\n")],
        );
        root.supply("BAT0", &[("type", "Battery\n")]);
        root.supply("AC", &[("type", "Mains\n")]);

        assert_eq!(
            battery_path(root.path()).and_then(|path| path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())),
            Some("BAT0".to_string())
        );
    }

    #[test]
    fn a_machine_with_no_battery_says_so() {
        let root = TempRoot::new("desktop");
        root.supply("AC", &[("type", "Mains\n")]);
        assert_eq!(battery_path(root.path()), None);
        assert_eq!(read_thresholds(root.path()), None);
    }

    #[test]
    fn the_modern_threshold_files_are_preferred_over_the_old_ones() {
        let root = TempRoot::new("both");
        let battery = root.supply(
            "BAT0",
            &[
                ("type", "Battery\n"),
                ("charge_control_start_threshold", "75\n"),
                ("charge_control_end_threshold", "80\n"),
                ("charge_start_threshold", "40\n"),
                ("charge_stop_threshold", "60\n"),
            ],
        );
        let paths = threshold_paths(&battery).expect("thresholds");
        assert!(paths.start.ends_with("charge_control_start_threshold"));
        assert_eq!(
            read_thresholds(root.path()).map(|limits| (limits.start, limits.end)),
            Some((75, 80))
        );
    }

    #[test]
    fn the_old_threshold_files_are_still_read() {
        let root = TempRoot::new("legacy");
        root.supply(
            "BAT0",
            &[
                ("type", "Battery\n"),
                ("charge_start_threshold", "40\n"),
                ("charge_stop_threshold", "60\n"),
            ],
        );
        assert_eq!(
            read_thresholds(root.path()).map(|limits| (limits.start, limits.end)),
            Some((40, 60))
        );
    }

    #[test]
    fn a_writable_pair_of_files_takes_a_new_limit() {
        let root = TempRoot::new("write");
        battery_with_thresholds(&root, 96, 100);

        let before = read_thresholds(root.path()).expect("thresholds");
        assert!(before.writable, "a temporary file is writable by its owner");
        assert!(!before.limited());

        write_thresholds(root.path(), 75, 80).expect("the files take the write");
        let after = read_thresholds(root.path()).expect("thresholds");
        assert_eq!((after.start, after.end), (75, 80));
        assert!(after.limited());
    }

    #[test]
    fn raising_the_limit_back_to_full_writes_the_end_first() {
        let root = TempRoot::new("raise");
        battery_with_thresholds(&root, 75, 80);
        // 96 is above the current end of 80, so a naive start-first write
        // would hand the kernel start >= end.
        write_thresholds(root.path(), 96, 100).expect("the files take the write");
        assert_eq!(
            read_thresholds(root.path()).map(|limits| (limits.start, limits.end)),
            Some((96, 100))
        );
    }

    #[test]
    fn a_battery_with_no_threshold_files_reports_none() {
        let root = TempRoot::new("plain");
        root.supply("BAT0", &[("type", "Battery\n"), ("status", "Full\n")]);
        assert_eq!(read_thresholds(root.path()), None);
        assert!(write_thresholds(root.path(), 75, 80).is_err());
    }

    #[test]
    fn unwritable_files_are_reported_rather_than_written() {
        let root = TempRoot::new("readonly");
        let battery = battery_with_thresholds(&root, 96, 100);
        for file in [
            "charge_control_start_threshold",
            "charge_control_end_threshold",
        ] {
            let path = battery.join(file);
            let mut permissions = fs::metadata(&path).expect("the file").permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(true);
            fs::set_permissions(&path, permissions).expect("read-only");
        }

        let limits = read_thresholds(root.path()).expect("thresholds are still readable");
        assert!(!limits.writable, "a root-owned file is not ours to write");
        assert!(write_thresholds(root.path(), 75, 80).is_err());
        assert_eq!(
            read_thresholds(root.path()).map(|limits| (limits.start, limits.end)),
            Some((96, 100)),
            "a refused write changes nothing"
        );
    }
}
