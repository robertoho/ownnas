//! OwnNAS in Windows Explorer: each server signed in to is a place in Explorer's navigation pane
//! (a Cloud Files sync root, as OneDrive's), where files show without being downloaded and
//! download when opened. What is added, changed, renamed or deleted there goes to the server,
//! and what changes on the server shows there, following the server's change feed.
//!
//! The parts here are the same on every system (settings, versions, paths), so they are tested
//! anywhere; the Windows parts are in `sync` and `secret`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[cfg(windows)]
pub mod secret;
#[cfg(windows)]
pub mod sync;

/// The name the places in Explorer are registered under.
pub const PROVIDER: &str = "OwnNAS";

/// A server signed in to, and where it is in Explorer.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Account {
    /// Tells this server's place apart from others (part of its Explorer registration).
    pub id: String,
    pub server: String,
    pub username: String,
    /// The server's shared folder name, shown in Explorer.
    pub name: String,
    /// The local folder Explorer shows it in.
    pub folder: PathBuf,
    /// A private CA to trust (PEM), and certificate fingerprints accepted.
    #[serde(default)]
    pub ca_pem: Option<String>,
    #[serde(default)]
    pub pinned: Vec<String>,
    /// The sign-in token, encrypted for this Windows user (DPAPI), in hex.
    pub token: String,
}

#[derive(Serialize, Deserialize, Default, Debug)]
pub struct Settings {
    pub accounts: Vec<Account>,
}

/// `%LOCALAPPDATA%\OwnNAS`, where the settings and the log are kept.
pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("OwnNAS")
}

pub fn load_settings() -> Settings {
    std::fs::read(data_dir().join("cloud.json")).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}

pub fn save_settings(settings: &Settings) -> Result<(), String> {
    let dir = data_dir();
    std::fs::create_dir_all(&dir).map_err(|err| format!("Could not create {}: {err}", dir.display()))?;
    let text = serde_json::to_vec_pretty(settings).map_err(|err| err.to_string())?;
    let file = dir.join("cloud.json");
    let temp = dir.join("cloud.json.new");
    std::fs::write(&temp, text).map_err(|err| format!("Could not save {}: {err}", temp.display()))?;
    std::fs::rename(&temp, &file).map_err(|err| format!("Could not save {}: {err}", file.display()))
}

/// A line in the log (`%LOCALAPPDATA%\OwnNAS\cloud.log`) and on the console, with the time.
pub fn log(line: &str) {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let text = format!("{secs} {line}\n");
    eprint!("{text}");
    let file = data_dir().join("cloud.log");
    if std::fs::metadata(&file).is_ok_and(|meta| meta.len() > 5 * 1024 * 1024) {
        let _ = std::fs::rename(&file, file.with_extension("log.old"));
    }
    if let Ok(mut out) = std::fs::OpenOptions::new().create(true).append(true).open(file) {
        use std::io::Write;
        let _ = out.write_all(text.as_bytes());
    }
}

/// A file's version on the server, as kept with its placeholder: `size:mtime-ms`. Folders keep
/// `dir`.
pub fn stamp_blob(size: u64, mtime_ms: i64) -> Vec<u8> {
    format!("{size}:{mtime_ms}").into_bytes()
}

/// The version a placeholder was made from (`None` for folders and anything else).
pub fn blob_stamp(blob: &[u8]) -> Option<ownnas_core::model::Stamp> {
    let text = std::str::from_utf8(blob).ok()?;
    let (size, mtime) = text.split_once(':')?;
    Some(ownnas_core::model::Stamp { size: size.parse().ok()?, mtime: mtime.parse().ok()? })
}

/// The server path (with `/`) of a local path under `root`; `""` is the top. `None` when it is
/// not under it.
pub fn server_path(root: &Path, local: &Path) -> Option<String> {
    let rest = local.strip_prefix(root).ok()?;
    let parts: Vec<String> = rest.components().map(|part| part.as_os_str().to_string_lossy().into_owned()).collect();
    Some(parts.join("/"))
}

/// The local path of a server path under `root`.
pub fn local_path(root: &Path, server: &str) -> PathBuf {
    server.split('/').filter(|part| !part.is_empty()).fold(root.to_path_buf(), |path, part| path.join(part))
}

/// Milliseconds since 1970 as Windows file time (100 ns since 1601).
pub fn file_time(ms: i64) -> u64 {
    ((ms.max(0) as u64) + 11_644_473_600_000) * 10_000
}

/// Names kept out of sync: Office lock files, Explorer's and other apps' own files.
pub fn ignored(name: &str) -> bool {
    ownnas_core::local::ignored(name) || name.ends_with(".tmp") || name.starts_with("~WRL")
}

/// A local folder name for a server's place: its shared folder name, safe for Windows.
pub fn folder_name(name: &str) -> String {
    let clean: String = name.chars().map(|c| if r#"<>:"/\|?*"#.contains(c) || c.is_control() { '_' } else { c }).collect();
    let clean = clean.trim().trim_end_matches('.').to_string();
    if clean.is_empty() { "OwnNAS".into() } else { format!("OwnNAS - {clean}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_go_with_placeholders_and_come_back() {
        let blob = stamp_blob(1234, 1_793_299_931_165);
        assert_eq!(blob, b"1234:1793299931165");
        let stamp = blob_stamp(&blob).unwrap();
        assert_eq!((stamp.size, stamp.mtime), (1234, 1_793_299_931_165));
        assert!(blob_stamp(b"dir").is_none());
        assert!(blob_stamp(b"").is_none());
    }

    #[test]
    fn paths_between_explorer_and_the_server() {
        let root = Path::new("C:/Users/ana/OwnNAS - root");
        assert_eq!(server_path(root, &root.join("Docs").join("a b.txt")).as_deref(), Some("Docs/a b.txt"));
        assert_eq!(server_path(root, root).as_deref(), Some(""));
        assert_eq!(server_path(root, Path::new("C:/elsewhere/x")), None);
        assert_eq!(local_path(root, "Docs/a b.txt"), root.join("Docs").join("a b.txt"));
        assert_eq!(local_path(root, ""), root);
    }

    #[test]
    fn times_in_windows_terms() {
        assert_eq!(file_time(0), 116_444_736_000_000_000, "1970 in 100 ns since 1601");
        assert_eq!(file_time(1_000), 116_444_736_010_000_000);
    }

    #[test]
    fn names() {
        assert!(ignored("~$Report.docx") && ignored("desktop.ini") && ignored("x.tmp") && !ignored("report.docx"));
        assert_eq!(folder_name("root"), "OwnNAS - root");
        assert_eq!(folder_name("a:b?"), "OwnNAS - a_b_");
        assert_eq!(folder_name(""), "OwnNAS");
    }
}
