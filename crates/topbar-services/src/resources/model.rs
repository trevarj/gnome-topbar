//! Reading `/proc`, and everything that can be decided from what it says.
//!
//! Ported from v1's resource monitor, which got the arithmetic right and is
//! kept almost verbatim — with its tests, which are the part that proves it.
//! Two things changed, and both are noted where they happen: disks are
//! de-duplicated by the device behind them rather than by mount point, and the
//! CPU delta is discarded across a suspend rather than producing a reading from
//! two samples an hour apart.

use std::collections::HashSet;
use std::ffi::CString;

/// One reading of `/proc/stat`'s aggregate CPU line.
///
/// Meaningless on its own: CPU usage is a *difference* between two of these,
/// which is why the first sample of a session produces no number at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuSample {
    /// Jiffies spent idle, including waiting on I/O.
    idle: u64,
    /// Jiffies spent on anything at all.
    total: u64,
}

/// Read the aggregate CPU line out of `/proc/stat`.
///
/// The line is the one starting `cpu ` — with the trailing space, so `cpu0` and
/// its siblings are skipped. `idle` is the idle field plus iowait, because a
/// core waiting on a disk is not a core doing work; `total` is every field on
/// the line, however many the kernel prints, so a new column in some future
/// release is counted rather than silently dropped from the denominator.
pub fn parse_cpu_sample(stat: &str) -> Option<CpuSample> {
    let line = stat.lines().find(|line| line.starts_with("cpu "))?;
    let values: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|part| part.parse::<u64>().ok())
        .collect();
    if values.len() < 4 {
        return None;
    }
    let idle = values.get(3).copied().unwrap_or(0) + values.get(4).copied().unwrap_or(0);
    let total = values.iter().sum();
    Some(CpuSample { idle, total })
}

/// How busy the CPU was between two samples.
///
/// `None` where the answer would be a lie: no previous sample, a counter that
/// went backwards (which is what a CPU coming out of hotplug looks like), or
/// two identical readings. Rounded half-up and clamped, because 101% would be a
/// bar drawn past its own end.
pub fn cpu_usage_percent(previous: CpuSample, current: CpuSample) -> Option<u8> {
    let total_delta = current.total.checked_sub(previous.total)?;
    let idle_delta = current.idle.checked_sub(previous.idle)?;
    if total_delta == 0 || idle_delta > total_delta {
        return None;
    }
    let busy_delta = total_delta - idle_delta;
    Some(((busy_delta * 100 + total_delta / 2) / total_delta).min(100) as u8)
}

/// What `/proc/meminfo` says.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Memory {
    /// Total RAM, in KiB.
    pub total_kib: u64,
    /// What the kernel thinks is available for a new allocation, in KiB.
    pub available_kib: u64,
    /// Total minus available, in KiB.
    pub used_kib: u64,
    /// The same as a percentage.
    pub used_pct: u8,
    /// Total swap, in KiB. Zero on a machine with none.
    pub swap_total_kib: u64,
    /// Swap in use, in KiB.
    pub swap_used_kib: u64,
    /// The same as a percentage, or `None` where there is no swap.
    pub swap_used_pct: Option<u8>,
}

impl Memory {
    /// Whether this machine has swap at all.
    pub fn has_swap(&self) -> bool {
        self.swap_total_kib > 0
    }
}

/// Read `/proc/meminfo`.
///
/// `MemAvailable` rather than `MemFree`: the kernel computes it, it accounts
/// for reclaimable cache, and it is the number that answers "how much can I
/// still allocate". `MemFree` on a healthy machine is near zero, and a memory
/// bar drawn from it reads as a permanent emergency.
pub fn parse_memory(meminfo: &str) -> Option<Memory> {
    let mut total = None;
    let mut available = None;
    // Absent keys mean a machine with no swap, which parses fine.
    let mut swap_total = 0;
    let mut swap_free = 0;

    for line in meminfo.lines() {
        if let Some(value) = kib(line, "MemTotal:") {
            total = Some(value);
        } else if let Some(value) = kib(line, "MemAvailable:") {
            available = Some(value);
        } else if let Some(value) = kib(line, "SwapTotal:") {
            swap_total = value;
        } else if let Some(value) = kib(line, "SwapFree:") {
            swap_free = value;
        }
    }

    let total = total?;
    let available = available?;
    if total == 0 || available > total {
        return None;
    }

    let used = total - available;
    let swap_free = swap_free.min(swap_total);
    let swap_used = swap_total - swap_free;

    Some(Memory {
        total_kib: total,
        available_kib: available,
        used_kib: used,
        used_pct: percent(used, total),
        swap_total_kib: swap_total,
        swap_used_kib: swap_used,
        swap_used_pct: (swap_total > 0).then(|| percent(swap_used, swap_total)),
    })
}

/// One `Key:  1234 kB` line.
fn kib(line: &str, key: &str) -> Option<u64> {
    line.strip_prefix(key)?
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()
}

/// A share of a whole, rounded half-up and clamped.
pub fn percent(part: u64, whole: u64) -> u8 {
    if whole == 0 {
        return 0;
    }
    ((part * 100 + whole / 2) / whole).min(100) as u8
}

/// One mounted filesystem worth showing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Disk {
    /// Where it is mounted.
    pub mount: String,
    /// Its filesystem type.
    pub fs_type: String,
    /// The `major:minor` of the device behind it, which is what dedup uses.
    pub device: String,
    /// Capacity in bytes.
    pub total: u64,
    /// In use, in bytes.
    pub used: u64,
    /// The same as a percentage.
    pub used_pct: u8,
}

/// Filesystem types that are not a disk.
///
/// A deny-list rather than an allow-list, so a filesystem nobody here has heard
/// of — bcachefs, some future thing — still shows up. Network filesystems are
/// excluded as well as pseudo ones: `statvfs` on a hung NFS mount blocks, and a
/// panel that stalled for thirty seconds every five because a server went away
/// would be worse than one that never mentioned the share.
const NOT_A_DISK: &[&str] = &[
    "autofs",
    "binfmt_misc",
    "bpf",
    "cgroup",
    "cgroup2",
    "cifs",
    "configfs",
    "debugfs",
    "devpts",
    "devtmpfs",
    "efivarfs",
    "fuse.gvfsd-fuse",
    "fuse.portal",
    "fusectl",
    "hugetlbfs",
    "mqueue",
    "nfs",
    "nfs4",
    "nsfs",
    "overlay",
    "proc",
    "pstore",
    "ramfs",
    "rpc_pipefs",
    "securityfs",
    "smb3",
    "squashfs",
    "sshfs",
    "sysfs",
    "tmpfs",
    "tracefs",
    "9p",
];

/// Whether a filesystem type is one with a disk behind it.
fn is_real(fs_type: &str) -> bool {
    !NOT_A_DISK.contains(&fs_type)
        && !NOT_A_DISK.contains(&fs_type.strip_prefix("fuse.").unwrap_or(fs_type))
}

/// Every real filesystem in `/proc/self/mountinfo`, de-duplicated.
///
/// **De-duplicated by device, not by mount point** — the one change from v1.
/// A `nix store` bind mount, `/var/lib/docker/…` overlays and a `boot` mounted
/// twice are all the same disk with the same free space, and v1's mount-point
/// key listed each of them as a row. Keeping the *shortest* mount point per
/// device is what makes the row say `/` rather than `/nix/store`.
///
/// A btrfs machine with subvolumes at `/` and `/home` genuinely does have one
/// device and two mount points, so the two of them collapse to one row here
/// where v1 showed two. They report identical numbers, because they are the
/// same filesystem: one row is the honest count.
pub fn parse_mountinfo(mountinfo: &str) -> Vec<Disk> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut disks: Vec<Disk> = Vec::new();

    for line in mountinfo.lines() {
        // The optional fields between index 5 and the separator are why this is
        // split on `" - "` rather than by position.
        let Some((before, after)) = line.split_once(" - ") else {
            continue;
        };
        let head: Vec<&str> = before.split_whitespace().collect();
        let tail: Vec<&str> = after.split_whitespace().collect();
        // 0 mount id, 1 parent id, 2 major:minor, 3 root, 4 mount point.
        if head.len() < 5 || tail.is_empty() {
            continue;
        }

        let device = head[2].to_string();
        let mount = unescape(head[4]);
        let fs_type = tail[0].to_string();
        if !is_real(&fs_type) {
            continue;
        }
        if !seen.insert(device.clone()) {
            // Already have this disk. Keep whichever mount point is shorter:
            // `/` says more than `/nix/store` about the same filesystem.
            if let Some(kept) = disks.iter_mut().find(|disk| disk.device == device)
                && mount.len() < kept.mount.len()
            {
                kept.mount = mount;
            }
            continue;
        }

        disks.push(Disk {
            mount,
            fs_type,
            device,
            total: 0,
            used: 0,
            used_pct: 0,
        });
    }

    disks.sort_by(|a, b| a.mount.cmp(&b.mount));
    disks
}

/// Undo mountinfo's octal escaping: `\040` is a space.
fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(character) = chars.next() {
        if character != '\\' {
            out.push(character);
            continue;
        }
        let digits: String = chars.clone().take(3).collect();
        if digits.len() == 3
            && digits.chars().all(|c| c.is_digit(8))
            && let Ok(byte) = u8::from_str_radix(&digits, 8)
        {
            for _ in 0..3 {
                chars.next();
            }
            out.push(byte as char);
        } else {
            out.push('\\');
        }
    }
    out
}

/// Fill in one disk's capacity from the kernel.
///
/// `f_bavail` rather than `f_bfree`: the difference is the blocks reserved for
/// root, and counting those as free would make a full disk look like it had
/// five percent left. `df` makes the same choice, and the number has to match
/// what the user sees in a terminal.
pub fn measure(disk: &mut Disk) -> bool {
    let Ok(path) = CString::new(disk.mount.as_str()) else {
        return false;
    };
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `path` is a valid NUL-terminated C string that outlives the call,
    // and `stat` is a correctly sized, aligned allocation for the out
    // parameter, initialised by the call when it returns zero.
    let code = unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) };
    if code != 0 {
        return false;
    }
    // SAFETY: `statvfs` returned zero, so it wrote a complete struct.
    let stat = unsafe { stat.assume_init() };

    // `f_frsize` is the fragment size and `f_bsize` the preferred I/O block;
    // they agree on every filesystem in use, and the larger is the safe one
    // to multiply by where they do not.
    let block = stat.f_frsize.max(stat.f_bsize);
    let total = stat.f_blocks.saturating_mul(block);
    let available = stat.f_bavail.saturating_mul(block).min(total);
    if total == 0 {
        return false;
    }

    disk.used = total - available;
    disk.total = total;
    disk.used_pct = percent(disk.used, total);
    true
}

/// Everything the panel knows about this machine's resources.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResourceState {
    /// How busy the CPU is, or `None` before the second sample.
    pub cpu_pct: Option<u8>,
    /// What memory is doing.
    pub memory: Memory,
    /// Every real filesystem, by mount point.
    pub disks: Vec<Disk>,
}

/// Write a byte count the way a person would.
///
/// Three bands, from v1: ten gigabytes and up loses its decimal because
/// "465 GiB" is as much precision as a disk row needs, and anything under a
/// gigabyte is in mebibytes because "0.4 GiB" reads as nothing at all.
pub fn bytes(count: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    let value = count as f64;
    if count >= 10 * 1024 * 1024 * 1024 {
        format!("{:.0} GiB", value / GIB)
    } else if count >= 1024 * 1024 * 1024 {
        format!("{:.1} GiB", value / GIB)
    } else {
        format!("{:.0} MiB", value / MIB)
    }
}

/// The same, from a count in KiB.
pub fn kib_bytes(count: u64) -> String {
    bytes(count.saturating_mul(1024))
}

/// "7.2 / 16 GiB", the way the memory row reads.
///
/// The unit is written once where both halves share it. "7.2 GiB / 16 GiB" is
/// the same information said twice, and the row is 360 pixels wide.
pub fn used_of(used: u64, total: u64) -> String {
    let used = bytes(used);
    let total = bytes(total);
    match (used.rsplit_once(' '), total.rsplit_once(' ')) {
        (Some((number, unit)), Some((_, whole_unit))) if unit == whole_unit => {
            format!("{number} / {total}")
        }
        _ => format!("{used} / {total}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_aggregate_cpu_line_is_the_one_without_a_number_on_it() {
        // v1's fixture, kept: idle 40 + iowait 5, and every field summed.
        let sample =
            parse_cpu_sample("cpu  10 20 30 40 5 0 0 0 0 0\ncpu0 1 2 3 4\n").expect("a sample");
        assert_eq!(sample.idle, 45);
        assert_eq!(sample.total, 105);
    }

    #[test]
    fn a_line_with_too_few_fields_is_not_a_sample() {
        assert!(parse_cpu_sample("cpu  1 2 3\n").is_none());
        assert!(parse_cpu_sample("cpu0 1 2 3 4\n").is_none());
        assert!(parse_cpu_sample("").is_none());
    }

    #[test]
    fn usage_is_the_difference_between_two_samples() {
        let previous = CpuSample {
            idle: 100,
            total: 200,
        };
        let current = CpuSample {
            idle: 150,
            total: 300,
        };
        assert_eq!(cpu_usage_percent(previous, current), Some(50));
    }

    #[test]
    fn the_first_sample_of_a_session_produces_no_reading() {
        // There is nothing to subtract from, which is why `cpu_pct` is an
        // Option and the card says "…" rather than 0% for one interval.
        let sample = parse_cpu_sample("cpu  10 20 30 40 5\n").expect("a sample");
        assert_eq!(cpu_usage_percent(sample, sample), None, "no time passed");
    }

    #[test]
    fn a_counter_that_went_backwards_produces_nothing_rather_than_nonsense() {
        // What a suspend, or a CPU coming out of hotplug, looks like.
        let after = CpuSample {
            idle: 10,
            total: 20,
        };
        let before = CpuSample {
            idle: 100,
            total: 200,
        };
        assert_eq!(cpu_usage_percent(before, after), None);

        // And an idle delta larger than the total delta is arithmetic nobody
        // should draw a bar from.
        let odd = CpuSample {
            idle: 500,
            total: 210,
        };
        assert_eq!(cpu_usage_percent(before, odd), None);
    }

    #[test]
    fn memory_is_read_from_what_the_kernel_says_is_available() {
        // v1's fixture, byte for byte.
        let meminfo = "\
MemTotal:       1000000 kB
MemAvailable:    250000 kB
SwapTotal:       200000 kB
SwapFree:         50000 kB
";
        let memory = parse_memory(meminfo).expect("a reading");
        assert_eq!(memory.used_pct, 75);
        assert_eq!(memory.used_kib, 750_000);
        assert_eq!(memory.swap_used_pct, Some(75));
        assert_eq!(memory.swap_used_kib, 150_000);
        assert!(memory.has_swap());
    }

    #[test]
    fn a_machine_with_no_swap_says_so_rather_than_reporting_zero_percent() {
        let memory = parse_memory("MemTotal: 100 kB\nMemAvailable: 40 kB\n").expect("a reading");
        assert_eq!(memory.used_pct, 60);
        assert_eq!(memory.swap_used_pct, None, "so the row is not drawn at all");
        assert!(!memory.has_swap());
    }

    #[test]
    fn nonsense_meminfo_is_no_reading_rather_than_a_wrong_one() {
        assert!(parse_memory("").is_none());
        assert!(parse_memory("MemTotal: 100 kB\n").is_none(), "no available");
        assert!(parse_memory("MemTotal: 0 kB\nMemAvailable: 0 kB\n").is_none());
        assert!(
            parse_memory("MemTotal: 10 kB\nMemAvailable: 20 kB\n").is_none(),
            "more available than exists"
        );
    }

    #[test]
    fn mountinfo_keeps_the_real_filesystems_and_drops_the_rest() {
        // v1's fixture, extended with the two cases v1 got wrong.
        let mountinfo = "\
25 1 8:1 / / rw,relatime - ext4 /dev/sda1 rw
26 25 0:20 / /proc rw,nosuid,nodev,noexec,relatime - proc proc rw
27 25 0:21 / /run rw,nosuid,nodev - tmpfs tmpfs rw
28 25 0:42 / /home rw,relatime - btrfs /dev/sda2 rw
29 25 0:43 / /mnt/share rw,relatime - nfs server:/share rw
30 25 0:44 / /mnt/my\\040disk rw,relatime - xfs /dev/sdb1 rw
31 25 0:45 / /run/user/1000/doc rw,relatime - fuse.portal portal rw
";
        let disks = parse_mountinfo(mountinfo);
        let mounts: Vec<&str> = disks.iter().map(|disk| disk.mount.as_str()).collect();
        assert_eq!(
            mounts,
            ["/", "/home", "/mnt/my disk"],
            "pseudo, network and tmpfs mounts are not disks"
        );
    }

    #[test]
    fn one_disk_mounted_twice_is_one_row_under_its_shortest_path() {
        // The change from v1: a bind mount of the store, and a boot partition
        // mounted at two paths, both showed up twice in v1's list with
        // identical numbers.
        let mountinfo = "\
25 1 8:1 / / rw - ext4 /dev/sda1 rw
32 25 8:1 /nix/store /nix/store ro - ext4 /dev/sda1 ro
33 25 8:2 / /boot rw - vfat /dev/sda2 rw
34 25 8:2 / /boot/efi rw - vfat /dev/sda2 rw
";
        let disks = parse_mountinfo(mountinfo);
        let mounts: Vec<&str> = disks.iter().map(|disk| disk.mount.as_str()).collect();
        assert_eq!(mounts, ["/", "/boot"], "one row per device, shortest path");
    }

    #[test]
    fn a_filesystem_nobody_here_has_heard_of_is_still_a_disk() {
        // A deny-list, so bcachefs and whatever comes next appear rather than
        // needing a release of the panel first.
        let disks = parse_mountinfo("25 1 8:1 / /data rw - bcachefs /dev/sda1 rw\n");
        assert_eq!(disks.len(), 1);
        assert_eq!(disks[0].fs_type, "bcachefs");
    }

    #[test]
    fn a_malformed_mountinfo_line_is_skipped_rather_than_fatal() {
        let disks = parse_mountinfo(
            "nonsense\n25 1 8:1 / \n25 1 8:1 / / rw - \n26 1 8:2 / /data rw - ext4 /dev/sdb rw\n",
        );
        assert_eq!(disks.len(), 1);
        assert_eq!(disks[0].mount, "/data");
    }

    #[test]
    fn octal_escapes_in_a_mount_point_are_undone() {
        assert_eq!(unescape("/mnt/my\\040disk"), "/mnt/my disk");
        assert_eq!(unescape("/plain"), "/plain");
        // A backslash that is not an escape is kept as one.
        assert_eq!(unescape("/odd\\path"), "/odd\\path");
        assert_eq!(unescape("/trailing\\"), "/trailing\\");
    }

    #[test]
    fn a_share_is_rounded_the_way_a_reading_should_be() {
        assert_eq!(percent(0, 100), 0);
        assert_eq!(percent(1, 3), 33);
        assert_eq!(percent(2, 3), 67);
        assert_eq!(percent(100, 100), 100);
        assert_eq!(percent(200, 100), 100, "clamped, not drawn past the end");
        assert_eq!(percent(1, 0), 0, "and no division by zero");
    }

    #[test]
    fn byte_counts_read_the_way_a_person_writes_them() {
        assert_eq!(bytes(512 * 1024 * 1024), "512 MiB");
        assert_eq!(bytes(2 * 1024 * 1024 * 1024 + 512 * 1024 * 1024), "2.5 GiB");
        assert_eq!(bytes(16 * 1024 * 1024 * 1024), "16 GiB");
        // A 500GB SSD, as the disk row shows it.
        assert_eq!(bytes(465 * 1024 * 1024 * 1024), "465 GiB");
        assert_eq!(kib_bytes(16 * 1024 * 1024), "16 GiB");
    }

    #[test]
    fn the_memory_row_reads_as_a_fraction_with_the_unit_written_once() {
        let used = 7 * 1024 * 1024 * 1024 + 205 * 1024 * 1024;
        let total = 16_u64 * 1024 * 1024 * 1024;
        assert_eq!(used_of(used, total), "7.2 / 16 GiB");

        // Where the two halves land in different units, both are named: "800 /
        // 16 GiB" would be a lie by a factor of a thousand.
        let little = 800 * 1024 * 1024;
        assert_eq!(used_of(little, total), "800 MiB / 16 GiB");
    }

    #[test]
    fn the_root_filesystem_can_actually_be_measured() {
        // The one test here that touches the machine, and it only reads: `/`
        // exists on every box this will ever run on, and a `statvfs` that
        // answered nonsense would make every disk row wrong.
        let mut disk = Disk {
            mount: "/".to_string(),
            fs_type: "ext4".to_string(),
            device: "0:0".to_string(),
            total: 0,
            used: 0,
            used_pct: 0,
        };
        assert!(measure(&mut disk), "statvfs on / should work");
        assert!(disk.total > 0);
        assert!(disk.used <= disk.total);
        assert!(disk.used_pct <= 100);
    }

    #[test]
    fn a_mount_point_that_is_not_there_is_not_measured() {
        let mut disk = Disk {
            mount: "/nonexistent-topbar-test-mount".to_string(),
            fs_type: "ext4".to_string(),
            device: "0:0".to_string(),
            total: 0,
            used: 0,
            used_pct: 0,
        };
        assert!(!measure(&mut disk));
    }
}
