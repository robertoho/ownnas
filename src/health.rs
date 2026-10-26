//! How the computer OwnNAS runs on is doing: its disks (how full, and whether the drives report
//! themselves healthy), temperatures, memory, and processor load. Drive health comes from
//! `smartctl` (smartmontools) when it is installed, else from what the system says on its own
//! (`diskutil` on macOS, `Get-PhysicalDisk` on Windows).

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

/// A disk counts as nearly full under this share free, or this much.
const LOW_SHARE: f64 = 0.10;
const LOW_BYTES: u64 = 10 * 1024 * 1024 * 1024;
/// And as full under these.
const FULL_SHARE: f64 = 0.03;
const FULL_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Drives are asked about their health at most this often.
const DRIVES_EVERY: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Volume {
    pub name: String,
    pub mount: String,
    pub file_system: String,
    pub total: u64,
    pub free: u64,
    pub removable: bool,
    /// "ssd", "hdd", or "".
    pub kind: String,
    /// "ok", "low", or "full".
    pub level: &'static str,
    /// What OwnNAS keeps on it: "library", "data".
    pub holds: Vec<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Drive {
    pub name: String,
    pub model: String,
    /// Whether the drive says it is healthy (`None` when it does not say).
    pub healthy: Option<bool>,
    pub status: String,
    pub temperature: Option<f64>,
    pub power_on_hours: Option<u64>,
    /// For SSDs: how much of its rated wear it has used, in percent.
    pub wear: Option<u64>,
    /// Counts that mean trouble when above zero (reallocated or pending sectors, media errors).
    pub warnings: Vec<String>,
    pub source: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sensor {
    pub label: String,
    pub celsius: f32,
    pub critical: Option<f32>,
}

#[derive(Default)]
pub struct Health {
    drives: Mutex<Option<(Instant, Vec<Drive>, Option<String>)>>,
}

/// How full a disk is, as a level.
pub fn level(total: u64, free: u64) -> &'static str {
    if total == 0 {
        return "ok";
    }
    let share = free as f64 / total as f64;
    if share < FULL_SHARE || free < FULL_BYTES {
        "full"
    } else if share < LOW_SHARE || free < LOW_BYTES {
        "low"
    } else {
        "ok"
    }
}

/// The disks mounted, those holding the library and OwnNAS's data first. Pseudo and system
/// volumes that only mirror another are left out.
pub fn volumes(library: &Path, data: &Path) -> Vec<Volume> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let library = plain(library);
    let data = plain(data);
    // The disk of a path is the one mounted deepest above it.
    let owner = |path: &Path| {
        disks.list().iter().filter(|disk| path.starts_with(disk.mount_point())).max_by_key(|disk| disk.mount_point().as_os_str().len()).map(|disk| disk.mount_point().to_path_buf())
    };
    let (library_disk, data_disk) = (owner(&library), owner(&data));
    let mut out: Vec<Volume> = Vec::new();
    for disk in disks.list() {
        let mount = disk.mount_point().to_path_buf();
        let mut holds = Vec::new();
        if library_disk.as_ref() == Some(&mount) {
            holds.push("library");
        }
        if data_disk.as_ref() == Some(&mount) {
            holds.push("data");
        }
        let text = mount.to_string_lossy();
        // macOS mounts parts of the system disk separately (VM, Preboot, …): they share its space.
        let system_part = text.starts_with("/System/Volumes/") || text.starts_with("/private/var/vm") || text.starts_with("/snap/") || text.starts_with("/boot/efi");
        // Mounted disk images (installers and the like) are small: not worth a line.
        let tiny = disk.total_space() < 1024 * 1024 * 1024;
        if (system_part || tiny) && holds.is_empty() || disk.total_space() == 0 {
            continue;
        }
        // Asked the same way as the status bar: on macOS, `available_space` also counts what the
        // system could purge (local snapshots, caches), which is not there to write to now.
        let (total, free) = crate::storage::disk(&mount).map_or((disk.total_space(), disk.available_space()), |own| (own.total, own.available));
        // The same space mounted twice (as APFS volumes in one container are): keep one.
        if holds.is_empty() && out.iter().any(|seen| seen.total == total && seen.free == free) {
            continue;
        }
        out.push(Volume {
            name: disk.name().to_string_lossy().into_owned(),
            mount: text.into_owned(),
            file_system: disk.file_system().to_string_lossy().into_owned(),
            total,
            free,
            removable: disk.is_removable(),
            kind: match disk.kind() {
                sysinfo::DiskKind::SSD => "ssd",
                sysinfo::DiskKind::HDD => "hdd",
                _ => "",
            }
            .into(),
            level: level(total, free),
            holds,
        });
    }
    out.sort_by_key(|volume| (volume.holds.is_empty(), volume.mount.clone()));
    out
}

/// A path in full, as the disks list mounts them: on Windows, without the `\\?\` prefix that
/// resolving a path adds (`\\?\C:\Users` is on the disk mounted at `C:\`).
fn plain(path: &Path) -> std::path::PathBuf {
    let full = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let text = full.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => std::path::PathBuf::from(rest),
        _ => full,
    }
}

/// Temperatures the system reports, hottest first.
pub fn sensors() -> Vec<Sensor> {
    let components = sysinfo::Components::new_with_refreshed_list();
    let mut out: Vec<Sensor> = components
        .list()
        .iter()
        .filter_map(|component| {
            let celsius = component.temperature()?;
            (celsius.is_finite() && celsius > 0.0 && celsius < 150.0).then(|| Sensor {
                label: component.label().to_string(),
                celsius,
                critical: component.critical().filter(|c| c.is_finite() && *c > 0.0),
            })
        })
        .collect();
    out.sort_by(|a, b| b.celsius.total_cmp(&a.celsius));
    out
}

/// Memory, processor, and how long the computer has been up.
pub fn system() -> Value {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    sys.refresh_cpu_usage();
    std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL.max(Duration::from_millis(200)));
    sys.refresh_cpu_usage();
    let load = sysinfo::System::load_average();
    serde_json::json!({
        "host": sysinfo::System::host_name(),
        "os": sysinfo::System::long_os_version(),
        "uptime": sysinfo::System::uptime(),
        "cpu": sys.cpus().first().map(|cpu| cpu.brand().trim().to_string()),
        "cores": sys.cpus().len(),
        "cpuUsage": sys.global_cpu_usage(),
        "load": if load.one > 0.0 || load.five > 0.0 { Some([load.one, load.five, load.fifteen]) } else { None },
        "memory": { "total": sys.total_memory(), "used": sys.used_memory() },
        "swap": { "total": sys.total_swap(), "used": sys.used_swap() },
    })
}

impl Health {
    /// The drives' own health reports (asked again after a while, or when `fresh`), and a note
    /// when none could be asked.
    pub fn drives(&self, fresh: bool) -> (Vec<Drive>, Option<String>) {
        let mut cached = self.drives.lock().unwrap_or_else(|err| err.into_inner());
        if let Some((at, drives, note)) = cached.as_ref() {
            if !fresh && at.elapsed() < DRIVES_EVERY {
                return (drives.clone(), note.clone());
            }
        }
        let (drives, note) = ask_drives();
        *cached = Some((Instant::now(), drives.clone(), note.clone()));
        (drives, note)
    }
}

fn run(program: &str, args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new(program).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    Some(output.stdout)
}

fn ask_drives() -> (Vec<Drive>, Option<String>) {
    if let Some(drives) = smartctl() {
        return (drives, None);
    }
    let note = Some(if cfg!(target_os = "macos") {
        "For temperatures, wear, and error counts, install smartmontools (brew install smartmontools)."
    } else if cfg!(windows) {
        "For temperatures, wear, and error counts, install smartmontools (winget install smartmontools.smartmontools)."
    } else {
        "Install smartmontools (sudo apt install smartmontools) to see the drives' health; reading it may need OwnNAS to run as root."
    }
    .to_string());
    let drives = if cfg!(target_os = "macos") {
        diskutil()
    } else if cfg!(windows) {
        windows_disks()
    } else {
        Vec::new()
    };
    (drives, note)
}

/// Every drive smartctl finds, with what it reports. `None` when smartctl is not installed.
fn smartctl() -> Option<Vec<Drive>> {
    let scan: Value = serde_json::from_slice(&run("smartctl", &["--scan-open", "--json"])?).ok()?;
    let mut drives = Vec::new();
    for device in scan["devices"].as_array().into_iter().flatten() {
        let Some(name) = device["name"].as_str() else { continue };
        let kind = device["type"].as_str().unwrap_or("auto");
        let Some(output) = run("smartctl", &["-a", "--json", "-d", kind, name]) else { continue };
        let Ok(report) = serde_json::from_slice::<Value>(&output) else { continue };
        drives.push(smart_drive(name, &report));
    }
    Some(drives)
}

fn smart_drive(name: &str, report: &Value) -> Drive {
    let healthy = report["smart_status"]["passed"].as_bool();
    let nvme = &report["nvme_smart_health_information_log"];
    let mut warnings = Vec::new();
    for attribute in report["ata_smart_attributes"]["table"].as_array().into_iter().flatten() {
        let raw = attribute["raw"]["value"].as_u64().unwrap_or(0);
        let label = match attribute["id"].as_u64() {
            Some(5) => "reallocated sectors",
            Some(187) => "uncorrectable errors",
            Some(197) => "sectors waiting to be reallocated",
            Some(198) => "sectors that could not be read",
            _ => continue,
        };
        if raw > 0 {
            warnings.push(format!("{raw} {label}"));
        }
    }
    if let Some(errors) = nvme["media_errors"].as_u64().filter(|n| *n > 0) {
        warnings.push(format!("{errors} media errors"));
    }
    if let (Some(spare), Some(threshold)) = (nvme["available_spare"].as_u64(), nvme["available_spare_threshold"].as_u64()) {
        if spare <= threshold {
            warnings.push(format!("spare space down to {spare}%"));
        }
    }
    Drive {
        name: name.to_string(),
        model: report["model_name"].as_str().or(report["model_family"].as_str()).unwrap_or("").to_string(),
        healthy,
        status: match healthy {
            Some(true) => "Passed".into(),
            Some(false) => "Failing".into(),
            None => report["smartctl"]["messages"][0]["string"].as_str().unwrap_or("Not reported").to_string(),
        },
        temperature: report["temperature"]["current"].as_f64(),
        power_on_hours: report["power_on_time"]["hours"].as_u64(),
        wear: nvme["percentage_used"].as_u64(),
        warnings,
        source: "smartctl",
    }
}

/// macOS: each physical disk's SMART status, as Disk Utility shows it.
fn diskutil() -> Vec<Drive> {
    let Some(list) = run("diskutil", &["list", "-plist", "physical"]) else { return Vec::new() };
    let Ok(list) = plist::Value::from_reader(std::io::Cursor::new(list)) else { return Vec::new() };
    let disks: Vec<String> = list
        .as_dictionary()
        .and_then(|dict| dict.get("WholeDisks"))
        .and_then(|value| value.as_array())
        .map(|disks| disks.iter().filter_map(|disk| disk.as_string().map(str::to_string)).collect())
        .unwrap_or_default();
    let mut drives = Vec::new();
    for disk in disks {
        let Some(info) = run("diskutil", &["info", "-plist", &disk]) else { continue };
        let Ok(info) = plist::Value::from_reader(std::io::Cursor::new(info)) else { continue };
        let Some(info) = info.as_dictionary() else { continue };
        let text = |key: &str| info.get(key).and_then(|value| value.as_string()).unwrap_or("").to_string();
        let status = text("SMARTStatus");
        // Disk images and the like have no drive behind them.
        if status.is_empty() || info.get("VirtualOrPhysical").and_then(|v| v.as_string()) == Some("Virtual") {
            continue;
        }
        drives.push(Drive {
            name: format!("/dev/{disk}"),
            model: text("MediaName"),
            healthy: match status.as_str() {
                "Verified" => Some(true),
                "Failing" => Some(false),
                _ => None,
            },
            status,
            temperature: None,
            power_on_hours: None,
            wear: None,
            warnings: Vec::new(),
            source: "diskutil",
        });
    }
    drives
}

/// Windows: what Storage Spaces says of each physical disk.
fn windows_disks() -> Vec<Drive> {
    let Some(output) = run(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", "Get-PhysicalDisk | Select-Object FriendlyName,DeviceId,HealthStatus,OperationalStatus | ConvertTo-Json"],
    ) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_slice::<Value>(&output) else { return Vec::new() };
    let disks = if value.is_array() { value.as_array().cloned().unwrap_or_default() } else { vec![value] };
    disks
        .iter()
        .map(|disk| {
            let health = disk["HealthStatus"].as_str().map(str::to_string).or_else(|| disk["HealthStatus"].as_u64().map(|n| n.to_string())).unwrap_or_default();
            Drive {
                name: format!("Disk {}", disk["DeviceId"].as_str().unwrap_or("?")),
                model: disk["FriendlyName"].as_str().unwrap_or("").to_string(),
                healthy: match health.as_str() {
                    "Healthy" | "0" => Some(true),
                    "Warning" | "Unhealthy" | "1" | "2" => Some(false),
                    _ => None,
                },
                status: health,
                temperature: None,
                power_on_hours: None,
                wear: None,
                warnings: Vec::new(),
                source: "Get-PhysicalDisk",
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_and_smart_reports() {
        const GB: u64 = 1024 * 1024 * 1024;
        assert_eq!(level(1000 * GB, 500 * GB), "ok");
        assert_eq!(level(1000 * GB, 50 * GB), "low", "under a tenth");
        assert_eq!(level(64 * GB, 8 * GB), "low", "under 10 GB");
        assert_eq!(level(1000 * GB, 20 * GB), "full", "under 3%");
        assert_eq!(level(0, 0), "ok");

        let report = serde_json::json!({
            "model_name": "Example SSD",
            "smart_status": { "passed": true },
            "temperature": { "current": 41 },
            "power_on_time": { "hours": 12000 },
            "ata_smart_attributes": { "table": [ { "id": 5, "raw": { "value": 8 } }, { "id": 9, "raw": { "value": 12000 } } ] },
        });
        let drive = smart_drive("/dev/sda", &report);
        assert_eq!((drive.healthy, drive.temperature, drive.power_on_hours), (Some(true), Some(41.0), Some(12000)));
        assert_eq!(drive.warnings, ["8 reallocated sectors"]);
        let nvme = serde_json::json!({ "smart_status": { "passed": false }, "nvme_smart_health_information_log": { "percentage_used": 97, "available_spare": 5, "available_spare_threshold": 10, "media_errors": 2 } });
        let drive = smart_drive("/dev/nvme0", &nvme);
        assert_eq!((drive.status.as_str(), drive.wear), ("Failing", Some(97)));
        assert_eq!(drive.warnings, ["2 media errors", "spare space down to 5%"]);
    }

    #[test]
    fn finds_the_disk_of_the_library() {
        let here = std::env::temp_dir();
        let found = volumes(&here, &here);
        assert!(found.first().is_some_and(|volume| volume.holds == ["library", "data"]), "{found:?}");
        let _ = system();
        let _ = sensors();
    }
}
