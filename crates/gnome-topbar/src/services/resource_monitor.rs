//! Shared resource usage sampling for CPU, memory, swap, and local disks.

use std::collections::HashSet;
use std::ffi::CString;
use std::fs;

const CPU_WARNING_THRESHOLD: u8 = 90;
const MEMORY_WARNING_THRESHOLD: u8 = 85;
const DISK_FREE_WARNING_THRESHOLD: u8 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuSample {
    idle: u64,
    total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceSnapshot {
    pub cpu_usage: Option<u8>,
    pub memory: MemorySnapshot,
    pub disks: Vec<DiskSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemorySnapshot {
    pub total_kib: u64,
    pub available_kib: u64,
    pub used_kib: u64,
    pub used_percent: u8,
    pub swap_total_kib: u64,
    pub swap_free_kib: u64,
    pub swap_used_kib: u64,
    pub swap_used_percent: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskSnapshot {
    pub mount_point: String,
    pub fs_type: String,
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub used_bytes: u64,
    pub used_percent: u8,
    pub free_percent: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceLevel {
    Normal,
    Warning,
}

impl ResourceSnapshot {
    pub fn root_disk(&self) -> Option<&DiskSnapshot> {
        self.disks.iter().find(|disk| disk.mount_point == "/")
    }
}

pub fn cpu_level(usage: Option<u8>) -> ResourceLevel {
    if usage.is_some_and(|usage| usage >= CPU_WARNING_THRESHOLD) {
        ResourceLevel::Warning
    } else {
        ResourceLevel::Normal
    }
}

pub fn memory_level(memory: &MemorySnapshot) -> ResourceLevel {
    if memory.used_percent >= MEMORY_WARNING_THRESHOLD {
        ResourceLevel::Warning
    } else {
        ResourceLevel::Normal
    }
}

pub fn disk_level(disk: &DiskSnapshot) -> ResourceLevel {
    if disk.free_percent <= DISK_FREE_WARNING_THRESHOLD {
        ResourceLevel::Warning
    } else {
        ResourceLevel::Normal
    }
}

pub fn read_resource_snapshot(
    previous_cpu: Option<CpuSample>,
) -> Result<(ResourceSnapshot, Option<CpuSample>), String> {
    let cpu_sample = Some(read_cpu_sample()?);
    let cpu_usage = previous_cpu
        .zip(cpu_sample)
        .and_then(|(previous, current)| cpu_usage_percent(previous, current));
    let memory = read_memory_snapshot()?;
    let disks = read_local_disks()?;

    Ok((
        ResourceSnapshot {
            cpu_usage,
            memory,
            disks,
        },
        cpu_sample,
    ))
}

fn read_cpu_sample() -> Result<CpuSample, String> {
    let stat = fs::read_to_string("/proc/stat").map_err(|e| format!("read /proc/stat: {e}"))?;
    parse_cpu_sample(&stat).ok_or_else(|| "parse /proc/stat cpu line".to_string())
}

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

pub fn cpu_usage_percent(previous: CpuSample, current: CpuSample) -> Option<u8> {
    let total_delta = current.total.checked_sub(previous.total)?;
    let idle_delta = current.idle.checked_sub(previous.idle)?;
    if total_delta == 0 || idle_delta > total_delta {
        return None;
    }

    let busy_delta = total_delta - idle_delta;
    Some(((busy_delta * 100 + total_delta / 2) / total_delta).min(100) as u8)
}

fn read_memory_snapshot() -> Result<MemorySnapshot, String> {
    let meminfo =
        fs::read_to_string("/proc/meminfo").map_err(|e| format!("read /proc/meminfo: {e}"))?;
    parse_memory_snapshot(&meminfo).ok_or_else(|| "parse /proc/meminfo".to_string())
}

pub fn parse_memory_snapshot(meminfo: &str) -> Option<MemorySnapshot> {
    let mut total = None;
    let mut available = None;
    let mut swap_total = Some(0);
    let mut swap_free = Some(0);

    for line in meminfo.lines() {
        if let Some(value) = parse_meminfo_kib(line, "MemTotal:") {
            total = Some(value);
        } else if let Some(value) = parse_meminfo_kib(line, "MemAvailable:") {
            available = Some(value);
        } else if let Some(value) = parse_meminfo_kib(line, "SwapTotal:") {
            swap_total = Some(value);
        } else if let Some(value) = parse_meminfo_kib(line, "SwapFree:") {
            swap_free = Some(value);
        }
    }

    let total = total?;
    let available = available?;
    if total == 0 || available > total {
        return None;
    }

    let used = total - available;
    let used_percent = percent_used(used, total);
    let swap_total = swap_total?;
    let swap_free = swap_free?.min(swap_total);
    let swap_used = swap_total - swap_free;
    let swap_used_percent = (swap_total > 0).then_some(percent_used(swap_used, swap_total));

    Some(MemorySnapshot {
        total_kib: total,
        available_kib: available,
        used_kib: used,
        used_percent,
        swap_total_kib: swap_total,
        swap_free_kib: swap_free,
        swap_used_kib: swap_used,
        swap_used_percent,
    })
}

fn parse_meminfo_kib(line: &str, key: &str) -> Option<u64> {
    line.strip_prefix(key)?
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()
}

fn read_local_disks() -> Result<Vec<DiskSnapshot>, String> {
    let mountinfo = fs::read_to_string("/proc/self/mountinfo")
        .map_err(|e| format!("read /proc/self/mountinfo: {e}"))?;
    let disks = disks_from_mountinfo(&mountinfo);
    let mut sampled_disks = Vec::new();

    for mut disk in disks {
        if populate_disk_usage(&mut disk).is_ok() && disk.total_bytes > 0 {
            sampled_disks.push(disk);
        }
    }

    sampled_disks.sort_by(|a, b| a.mount_point.cmp(&b.mount_point));
    Ok(sampled_disks)
}

pub fn disks_from_mountinfo(mountinfo: &str) -> Vec<DiskSnapshot> {
    let mut seen = HashSet::new();
    let mut disks = Vec::new();

    for line in mountinfo.lines() {
        let Some((pre, post)) = line.split_once(" - ") else {
            continue;
        };
        let pre_fields: Vec<&str> = pre.split_whitespace().collect();
        let post_fields: Vec<&str> = post.split_whitespace().collect();
        if pre_fields.len() < 5 || post_fields.is_empty() {
            continue;
        }

        let mount_point = unescape_mount_field(pre_fields[4]);
        let fs_type = post_fields[0].to_string();
        if !is_local_filesystem(&fs_type) || !seen.insert(mount_point.clone()) {
            continue;
        }

        disks.push(DiskSnapshot {
            mount_point,
            fs_type,
            total_bytes: 0,
            available_bytes: 0,
            used_bytes: 0,
            used_percent: 0,
            free_percent: 0,
        });
    }

    disks
}

fn is_local_filesystem(fs_type: &str) -> bool {
    let fs = fs_type.strip_prefix("fuse.").unwrap_or(fs_type);
    !matches!(
        fs,
        "autofs"
            | "binfmt_misc"
            | "bpf"
            | "cgroup"
            | "cgroup2"
            | "configfs"
            | "debugfs"
            | "devpts"
            | "devtmpfs"
            | "efivarfs"
            | "fusectl"
            | "portal"
            | "gvfsd-fuse"
            | "hugetlbfs"
            | "mqueue"
            | "nfs"
            | "nfs4"
            | "proc"
            | "pstore"
            | "ramfs"
            | "rpc_pipefs"
            | "securityfs"
            | "smb3"
            | "sysfs"
            | "tmpfs"
            | "tracefs"
            | "9p"
            | "cifs"
            | "sshfs"
    )
}

fn unescape_mount_field(value: &str) -> String {
    let mut out = String::new();
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            let digits: String = chars.by_ref().take(3).collect();
            if digits.len() == 3
                && digits.chars().all(|c| matches!(c, '0'..='7'))
                && let Ok(byte) = u8::from_str_radix(&digits, 8)
            {
                out.push(byte as char);
                continue;
            }
            out.push('\\');
            out.push_str(&digits);
        } else {
            out.push(ch);
        }
    }
    out
}

fn populate_disk_usage(disk: &mut DiskSnapshot) -> Result<(), String> {
    let path = CString::new(disk.mount_point.as_str())
        .map_err(|_| format!("mount point contains NUL: {}", disk.mount_point))?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    let rc = unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) };
    if rc != 0 {
        return Err(format!(
            "statvfs {}: {}",
            disk.mount_point,
            std::io::Error::last_os_error()
        ));
    }
    let stat = unsafe { stat.assume_init() };
    let block_size = stat.f_frsize.max(stat.f_bsize);
    let total = stat.f_blocks.saturating_mul(block_size);
    let available = stat.f_bavail.saturating_mul(block_size).min(total);
    let used = total.saturating_sub(available);

    disk.total_bytes = total;
    disk.available_bytes = available;
    disk.used_bytes = used;
    disk.used_percent = percent_used(used, total);
    disk.free_percent = 100u8.saturating_sub(disk.used_percent);
    Ok(())
}

fn percent_used(used: u64, total: u64) -> u8 {
    if total == 0 {
        return 0;
    }
    ((used * 100 + total / 2) / total).min(100) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_proc_stat_cpu_sample() {
        let sample = parse_cpu_sample("cpu  10 20 30 40 5 0 0 0 0 0\ncpu0 1 2 3 4\n").unwrap();
        assert_eq!(sample.idle, 45);
        assert_eq!(sample.total, 105);
    }

    #[test]
    fn computes_cpu_usage_from_sample_delta() {
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
    fn parses_memory_and_swap_usage() {
        let meminfo = "\
MemTotal:       1000000 kB
MemAvailable:    250000 kB
SwapTotal:       200000 kB
SwapFree:         50000 kB
";
        let memory = parse_memory_snapshot(meminfo).unwrap();
        assert_eq!(memory.used_percent, 75);
        assert_eq!(memory.swap_used_percent, Some(75));
    }

    #[test]
    fn mountinfo_filters_virtual_and_network_filesystems() {
        let mountinfo = "\
25 1 8:1 / / rw,relatime - ext4 /dev/sda1 rw
26 25 0:20 / /proc rw,nosuid,nodev,noexec,relatime - proc proc rw
27 25 0:21 / /run rw,nosuid,nodev - tmpfs tmpfs rw
28 25 0:42 / /home rw,relatime - btrfs /dev/sda2 rw
29 25 0:43 / /mnt/share rw,relatime - nfs server:/share rw
30 25 0:44 / /mnt/my\\040disk rw,relatime - xfs /dev/sdb1 rw
31 25 0:45 / /run/user/1000/doc rw,relatime - fuse.portal portal rw
";
        let disks = disks_from_mountinfo(mountinfo);
        let mounts: Vec<&str> = disks.iter().map(|disk| disk.mount_point.as_str()).collect();
        assert_eq!(mounts, vec!["/", "/home", "/mnt/my disk"]);
    }

    #[test]
    fn classifies_thresholds() {
        let memory = MemorySnapshot {
            total_kib: 100,
            available_kib: 14,
            used_kib: 86,
            used_percent: 86,
            swap_total_kib: 0,
            swap_free_kib: 0,
            swap_used_kib: 0,
            swap_used_percent: None,
        };
        let disk = DiskSnapshot {
            mount_point: "/".to_string(),
            fs_type: "ext4".to_string(),
            total_bytes: 100,
            available_bytes: 10,
            used_bytes: 90,
            used_percent: 90,
            free_percent: 10,
        };

        assert_eq!(cpu_level(Some(90)), ResourceLevel::Warning);
        assert_eq!(memory_level(&memory), ResourceLevel::Warning);
        assert_eq!(disk_level(&disk), ResourceLevel::Warning);
    }
}
