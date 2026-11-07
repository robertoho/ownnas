//! The drives mounted on this computer that backups could go to: USB disks and other drives
//! besides the system's own (Linux: under /media, /run/media or /mnt, or another disk;
//! macOS: /Volumes; Windows: the drive letters other than the system's). Read when asked, so a
//! drive plugged in a moment ago shows.

use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Drive {
    pub path: String,
    pub name: String,
    pub total: u64,
    pub available: u64,
    pub filesystem: String,
    /// A USB disk, a card, or another drive that can be taken out.
    pub removable: bool,
    /// The library is on it: a backup there would be lost with the library.
    pub holds_library: bool,
}

fn space(path: &Path) -> Option<(u64, u64)> {
    let stats = fs4::statvfs(path).ok()?;
    Some((stats.total_space(), stats.available_space()))
}

#[cfg(unix)]
fn device(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|meta| meta.dev())
}

#[cfg(not(unix))]
fn device(_: &Path) -> Option<u64> {
    None
}

fn drive(path: PathBuf, filesystem: &str, removable: bool, library: &Path) -> Option<Drive> {
    let (total, available) = space(&path)?;
    if total == 0 {
        return None;
    }
    let holds_library = library.starts_with(&path) || (device(&path).is_some() && device(&path) == device(library));
    let name = path.file_name().map(|name| name.to_string_lossy().to_string()).filter(|name| !name.is_empty()).unwrap_or_else(|| path.display().to_string());
    Some(Drive { path: path.display().to_string(), name, total, available, filesystem: filesystem.to_string(), removable, holds_library })
}

/// "/media/ann/My\040Disk" as written in /proc/mounts, with its spaces back.
#[cfg(target_os = "linux")]
fn unescape(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            let code: String = chars.clone().take(3).collect();
            if code.len() == 3 && code.chars().all(|d| ('0'..='7').contains(&d)) {
                if let Ok(value) = u8::from_str_radix(&code, 8) {
                    out.push(value as char);
                    for _ in 0..3 {
                        chars.next();
                    }
                    continue;
                }
            }
        }
        out.push(c);
    }
    out
}

/// Whether a mount is a real drive worth offering, from /proc/mounts' fields.
#[cfg(target_os = "linux")]
pub fn offered(device: &str, mount: &str, filesystem: &str) -> bool {
    const PSEUDO: [&str; 24] = [
        "proc", "sysfs", "tmpfs", "devtmpfs", "devpts", "cgroup", "cgroup2", "overlay", "squashfs", "autofs", "debugfs", "tracefs", "securityfs", "pstore", "bpf", "fusectl",
        "configfs", "mqueue", "hugetlbfs", "ramfs", "efivarfs", "nsfs", "binfmt_misc", "rpc_pipefs",
    ];
    if PSEUDO.contains(&filesystem) || filesystem.starts_with("fuse.portal") || filesystem == "fuse.gvfsd-fuse" {
        return false;
    }
    if ["/media/", "/run/media/", "/mnt/"].iter().any(|place| mount.starts_with(place)) || mount == "/mnt" {
        return true;
    }
    // Another disk mounted elsewhere, but not the system's own places.
    let system = ["/", "/boot", "/boot/efi", "/home", "/var", "/usr", "/opt", "/srv", "/tmp", "/snap", "/var/lib", "/nix"];
    let disk = device.starts_with("/dev/sd") || device.starts_with("/dev/nvme") || device.starts_with("/dev/mmcblk") || device.starts_with("/dev/mapper/");
    disk && !system.contains(&mount) && !mount.starts_with("/boot") && !mount.starts_with("/snap/") && !mount.starts_with("/var/") && !mount.starts_with("/usr/")
}

#[cfg(target_os = "linux")]
pub fn list(library: &Path) -> Vec<Drive> {
    let mounts = std::fs::read_to_string("/proc/self/mounts").unwrap_or_default();
    let mut drives: Vec<Drive> = Vec::new();
    for line in mounts.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let [device_name, mount, filesystem, ..] = fields.as_slice() else { continue };
        let mount = unescape(mount);
        if !offered(device_name, &mount, filesystem) || drives.iter().any(|known| known.path == mount) {
            continue;
        }
        // The kernel says whether the disk can be taken out (sdb1 → sdb).
        let base: String = device_name.trim_start_matches("/dev/").chars().take_while(|c| c.is_ascii_alphabetic()).collect();
        let removable = std::fs::read_to_string(format!("/sys/block/{base}/removable")).is_ok_and(|value| value.trim() == "1") || mount.starts_with("/media/") || mount.starts_with("/run/media/");
        if let Some(found) = drive(PathBuf::from(&mount), filesystem, removable, library) {
            drives.push(found);
        }
    }
    drives
}

#[cfg(target_os = "macos")]
pub fn list(library: &Path) -> Vec<Drive> {
    let Ok(entries) = std::fs::read_dir("/Volumes") else { return Vec::new() };
    let mut drives = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        // The startup disk shows in /Volumes as a link to /.
        if std::fs::read_link(&path).is_ok_and(|target| target == Path::new("/")) || path.canonicalize().is_ok_and(|real| real == Path::new("/")) {
            continue;
        }
        // Hidden ones, and macOS's own volumes (mounted at times, but not for backups).
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || ["Recovery", "Preboot", "VM", "Update", "xarts", "iSCPreboot", "Hardware"].contains(&name.as_str()) {
            continue;
        }
        if let Some(found) = drive(path, "", true, library) {
            drives.push(found);
        }
    }
    drives.sort_by(|a, b| a.name.cmp(&b.name));
    drives
}

#[cfg(windows)]
pub fn list(library: &Path) -> Vec<Drive> {
    let system = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into()).to_ascii_uppercase();
    let mut drives = Vec::new();
    for letter in 'A'..='Z' {
        let root = format!("{letter}:\\");
        if format!("{letter}:") == system || !Path::new(&root).exists() {
            continue;
        }
        if let Some(mut found) = drive(PathBuf::from(&root), "", letter > 'C', library) {
            found.name = format!("{letter}:");
            drives.push(found);
        }
    }
    drives
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn list(_: &Path) -> Vec<Drive> {
    Vec::new()
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn real_drives_only() {
        assert!(offered("/dev/sdb1", "/media/ann/Backup", "exfat"));
        assert!(offered("/dev/sdc1", "/mnt/archive", "ext4"));
        assert!(offered("/dev/nvme1n1p1", "/data2", "ext4"));
        assert!(!offered("/dev/nvme0n1p2", "/", "ext4"));
        assert!(!offered("/dev/nvme0n1p1", "/boot/efi", "vfat"));
        assert!(!offered("tmpfs", "/run/user/1000", "tmpfs"));
        assert!(!offered("proc", "/proc", "proc"));
        assert_eq!(unescape("/media/ann/My\\040Disk"), "/media/ann/My Disk");
    }
}
