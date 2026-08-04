//! Finding a backlight, and converting between its numbers and percentages.
//!
//! `/sys/class/backlight` holds one directory per controller, each with a
//! `brightness` and a `max_brightness`. Which one is *the* screen is a guess on
//! a machine with more than one, and the guess is v1's: prefer the integrated
//! GPU's controller (`intel_backlight`, `amdgpu_bl0`) over the firmware's
//! (`acpi_video0`), because the firmware one is usually a coarse duplicate of
//! it. Anything else comes last, in name order, so the choice is stable across
//! reboots.

use std::path::{Path, PathBuf};

/// Where the kernel publishes backlight controllers.
pub const BACKLIGHT_ROOT: &str = "/sys/class/backlight";

/// The subsystem name udev knows them by.
pub const SUBSYSTEM: &str = "backlight";

/// A backlight controller the panel can drive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backlight {
    /// Directory name, which is also what logind's `SetBrightness` takes.
    pub name: String,
    /// Where it lives.
    pub path: PathBuf,
    /// The raw value that means "full".
    pub max: u32,
}

impl Backlight {
    /// The raw value for a percentage, clamped to the device's range.
    pub fn raw(&self, percent: u32) -> u32 {
        let percent = percent.min(100);
        ((f64::from(percent) * f64::from(self.max)) / 100.0).round() as u32
    }

    /// The percentage a raw value represents.
    pub fn percent(&self, raw: u32) -> u32 {
        if self.max == 0 {
            return 0;
        }
        ((f64::from(raw.min(self.max)) * 100.0) / f64::from(self.max)).round() as u32
    }

    /// The current brightness, read straight from sysfs.
    pub fn read(&self) -> Option<u32> {
        read_u32(&self.path.join("brightness")).map(|raw| self.percent(raw))
    }

    /// Write a raw value to sysfs.
    ///
    /// The fallback for a machine with no logind, and the reason `topbar
    /// brightness` still works in a container. It needs write permission on
    /// the file, which a plain user usually does not have — hence the D-Bus
    /// path being tried first.
    pub fn write(&self, raw: u32) -> std::io::Result<()> {
        std::fs::write(self.path.join("brightness"), raw.to_string())
    }
}

/// Find the backlight to drive, if this machine has one.
pub fn discover() -> Option<Backlight> {
    discover_in(Path::new(BACKLIGHT_ROOT))
}

/// The same, rooted somewhere a test can write.
pub fn discover_in(root: &Path) -> Option<Backlight> {
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.join("brightness").exists() && path.join("max_brightness").exists())
        .collect();
    candidates.sort_by_key(|path| {
        let name = file_name(path);
        (rank(&name), name)
    });

    candidates.into_iter().find_map(|path| {
        let max = read_u32(&path.join("max_brightness"))?;
        // A controller reporting a maximum of zero cannot be scaled to a
        // percentage, and dividing by it later would be worse than skipping it.
        if max == 0 {
            return None;
        }
        Some(Backlight {
            name: file_name(&path),
            path,
            max,
        })
    })
}

/// Preference order: the GPU's controller, then the firmware's, then the rest.
fn rank(name: &str) -> u8 {
    let name = name.to_ascii_lowercase();
    if name.contains("intel") || name.contains("amdgpu") || name.contains("nvidia") {
        0
    } else if name.contains("amd") {
        1
    } else if name.contains("acpi") {
        2
    } else {
        3
    }
}

/// A path's last component, as a plain string.
fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_string()
}

/// Read a file holding one unsigned number.
fn read_u32(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a fake `/sys/class/backlight` and hand back its root.
    fn sysfs(label: &str, devices: &[(&str, u32, u32)]) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("topbar-backlight-{}-{label}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (name, current, max) in devices {
            let device = root.join(name);
            std::fs::create_dir_all(&device).expect("a writable temp dir");
            std::fs::write(device.join("brightness"), current.to_string()).expect("write");
            std::fs::write(device.join("max_brightness"), max.to_string()).expect("write");
        }
        root
    }

    #[test]
    fn percentages_and_raw_values_round_trip() {
        let backlight = Backlight {
            name: "intel_backlight".into(),
            path: PathBuf::from("/nonexistent"),
            max: 96_000,
        };
        assert_eq!(backlight.raw(0), 0);
        assert_eq!(backlight.raw(50), 48_000);
        assert_eq!(backlight.raw(100), 96_000);
        assert_eq!(
            backlight.raw(150),
            96_000,
            "a percentage cannot exceed full"
        );

        assert_eq!(backlight.percent(0), 0);
        assert_eq!(backlight.percent(48_000), 50);
        assert_eq!(backlight.percent(96_000), 100);
        assert_eq!(backlight.percent(u32::MAX), 100);
    }

    #[test]
    fn a_zero_maximum_cannot_divide_by_itself() {
        let backlight = Backlight {
            name: "broken".into(),
            path: PathBuf::from("/nonexistent"),
            max: 0,
        };
        assert_eq!(backlight.percent(10), 0);
        assert_eq!(backlight.raw(50), 0);
    }

    #[test]
    fn the_gpus_controller_wins_over_the_firmwares() {
        let root = sysfs(
            "preference",
            &[("acpi_video0", 5, 10), ("intel_backlight", 500, 1000)],
        );
        let found = discover_in(&root).expect("one of them");
        assert_eq!(found.name, "intel_backlight");
        assert_eq!(found.max, 1000);
        assert_eq!(found.read(), Some(50));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn firmware_is_still_better_than_nothing() {
        let root = sysfs(
            "firmware",
            &[("acpi_video0", 3, 10), ("something_else", 1, 2)],
        );
        assert_eq!(discover_in(&root).expect("one of them").name, "acpi_video0");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_choice_is_stable_when_nothing_is_preferred() {
        let root = sysfs("stable", &[("zzz", 1, 2), ("aaa", 1, 2)]);
        assert_eq!(discover_in(&root).expect("one of them").name, "aaa");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_machine_with_no_backlight_reports_none() {
        let root = sysfs("empty", &[]);
        std::fs::create_dir_all(&root).expect("a writable temp dir");
        assert_eq!(discover_in(&root), None);
        assert_eq!(discover_in(Path::new("/nonexistent-backlight-root")), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_controller_that_cannot_be_scaled_is_skipped() {
        let root = sysfs("zero-max", &[("aaa_broken", 0, 0), ("bbb_good", 5, 10)]);
        assert_eq!(discover_in(&root).expect("the usable one").name, "bbb_good");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_directory_missing_its_files_is_not_a_backlight() {
        let root = sysfs("partial", &[("real", 1, 2)]);
        std::fs::create_dir_all(root.join("aaa_incomplete")).expect("a writable temp dir");
        assert_eq!(discover_in(&root).expect("the real one").name, "real");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn writing_goes_to_the_file_the_kernel_reads() {
        let root = sysfs("write", &[("intel_backlight", 100, 1000)]);
        let backlight = discover_in(&root).expect("one");
        backlight
            .write(backlight.raw(75))
            .expect("a writable temp file");
        assert_eq!(backlight.read(), Some(75));
        let _ = std::fs::remove_dir_all(&root);
    }
}
