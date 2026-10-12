//! Shared data types and the persisted per-folder state.
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Folder (inside the synced folder) that holds the client's config, state, and safety copies.
pub const META_DIR: &str = ".ownnas-sync";

/// A file version: size plus modification time in milliseconds since the Unix epoch.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub struct Stamp {
    pub size: u64,
    pub mtime: i64,
}

/// One side's view of a path. Folders carry a zero stamp.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Node {
    pub dir: bool,
    pub stamp: Stamp,
}

impl Node {
    pub fn dir() -> Self {
        Node { dir: true, stamp: Stamp { size: 0, mtime: 0 } }
    }

    pub fn file(size: u64, mtime: i64) -> Self {
        Node { dir: false, stamp: Stamp { size, mtime } }
    }
}

/// What both sides looked like the last time a path was in sync.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub struct Base {
    pub dir: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local: Option<Stamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<Stamp>,
}

impl Base {
    pub fn dir() -> Self {
        Base { dir: true, local: None, remote: None }
    }

    pub fn file(local: Stamp, remote: Stamp) -> Self {
        Base { dir: false, local: Some(local), remote: Some(remote) }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Config {
    pub server: String,
    /// Folder on the server, relative to its shared root ("" for the whole share).
    #[serde(default)]
    pub remote: String,
    pub username: String,
    pub token: String,
    /// PEM file of a private CA to trust for HTTPS.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_cert: Option<PathBuf>,
}

#[derive(Serialize, Deserialize, Default, Debug)]
pub struct State {
    pub entries: BTreeMap<String, Base>,
}

pub fn meta_dir(root: &Path) -> PathBuf {
    root.join(META_DIR)
}

fn write_atomic(path: &Path, bytes: &[u8], private: bool) -> Result<(), String> {
    let temp = path.with_extension("tmp");
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = private;
    let mut file = options
        .open(&temp)
        .map_err(|err| format!("Could not write {}: {err}", temp.display()))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|err| format!("Could not write {}: {err}", temp.display()))?;
    drop(file);
    fs::rename(&temp, path).map_err(|err| format!("Could not write {}: {err}", path.display()))
}

pub fn load_config(root: &Path) -> Result<Config, String> {
    let path = meta_dir(root).join("config.json");
    let bytes = fs::read(&path).map_err(|_| {
        format!(
            "{} is not set up for sync. Run `ownnas-sync setup {}` first.",
            root.display(),
            root.display()
        )
    })?;
    serde_json::from_slice(&bytes).map_err(|err| format!("Could not read {}: {err}", path.display()))
}

pub fn save_config(root: &Path, config: &Config) -> Result<(), String> {
    let dir = meta_dir(root);
    fs::create_dir_all(&dir).map_err(|err| format!("Could not create {}: {err}", dir.display()))?;
    let bytes = serde_json::to_vec_pretty(config).map_err(|err| err.to_string())?;
    write_atomic(&dir.join("config.json"), &bytes, true)
}

pub fn load_state(root: &Path) -> Result<State, String> {
    let path = meta_dir(root).join("state.json");
    match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|err| format!("Could not read {}: {err}", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
        Err(err) => Err(format!("Could not read {}: {err}", path.display())),
    }
}

pub fn save_state(root: &Path, state: &State) -> Result<(), String> {
    let bytes = serde_json::to_vec(state).map_err(|err| err.to_string())?;
    // Apps sync without a saved config, so the folder may not exist yet.
    let dir = meta_dir(root);
    fs::create_dir_all(&dir).map_err(|err| format!("Could not create {}: {err}", dir.display()))?;
    write_atomic(&dir.join("state.json"), &bytes, false)
}
