//! The list of folders this computer syncs, each one a local folder linked to a folder on a
//! server. Every folder keeps its own sign-in and state in `<folder>/.ownnas-sync`; this list
//! only says which folders there are, so `run` with no folder can sync all of them.
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use ownnas_core::model::{self, Config};

#[derive(Serialize, Deserialize, Default)]
struct List {
    folders: Vec<PathBuf>,
}

/// Where the list is kept: `%APPDATA%\OwnNAS\sync-folders.json` on Windows,
/// `~/.config/ownnas/sync-folders.json` elsewhere. `OWNNAS_CONFIG_DIR` puts it somewhere else.
fn list_path() -> PathBuf {
    if let Some(dir) = std::env::var_os("OWNNAS_CONFIG_DIR") {
        return PathBuf::from(dir).join("sync-folders.json");
    }
    let dir = if cfg!(windows) {
        std::env::var_os("APPDATA").map(|dir| PathBuf::from(dir).join("OwnNAS"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .map(|dir| dir.join("ownnas"))
    };
    dir.unwrap_or_else(|| PathBuf::from(".")).join("sync-folders.json")
}

fn load() -> List {
    fs::read(list_path()).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}

fn save(list: &List) -> Result<(), String> {
    let path = list_path();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|err| format!("Could not create {}: {err}", dir.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(list).map_err(|err| err.to_string())?;
    let temp = path.with_extension("tmp");
    fs::write(&temp, bytes).map_err(|err| format!("Could not write {}: {err}", temp.display()))?;
    fs::rename(&temp, &path).map_err(|err| format!("Could not write {}: {err}", path.display()))
}

/// A linked folder and its settings.
pub struct Linked {
    pub dir: PathBuf,
    pub config: Config,
}

/// Every folder on the list, with its settings. A folder whose settings can't be read (moved,
/// deleted, or on a drive that isn't connected) comes back as an error for that folder.
pub fn all() -> Vec<(PathBuf, Result<Config, String>)> {
    load().folders.into_iter().map(|dir| {
        let config = model::load_config(&dir);
        (dir, config)
    }).collect()
}

/// The folders on the list whose settings can be read.
pub fn linked() -> Vec<Linked> {
    all().into_iter().filter_map(|(dir, config)| Some(Linked { dir, config: config.ok()? })).collect()
}

pub fn add(dir: &Path) -> Result<(), String> {
    let mut list = load();
    if !list.folders.iter().any(|known| known == dir) {
        list.folders.push(dir.to_path_buf());
        save(&list)?;
    }
    Ok(())
}

/// Takes a folder off the list. True if it was on it.
pub fn forget(dir: &Path) -> Result<bool, String> {
    let mut list = load();
    let before = list.folders.len();
    list.folders.retain(|known| known != dir);
    if list.folders.len() == before {
        return Ok(false);
    }
    save(&list)?;
    Ok(true)
}

/// A server folder as `ownnas-sync` stores it: no slashes at either end, `/` between parts.
pub fn clean_remote(remote: &str) -> String {
    remote.replace('\\', "/").split('/').filter(|part| !part.is_empty() && *part != ".").collect::<Vec<_>>().join("/")
}

/// One server folder is the other or inside it ("" is the whole shared folder).
fn remote_overlaps(a: &str, b: &str) -> bool {
    let inside = |outer: &str, inner: &str| outer.is_empty() || inner == outer || inner.starts_with(&format!("{outer}/"));
    inside(a, b) || inside(b, a)
}

fn same_server(a: &Config, b: &Config) -> bool {
    a.server.trim_end_matches('/') == b.server.trim_end_matches('/') && a.username == b.username
}

/// Refuses a link that would sync the same files twice: a local folder inside another linked
/// one (or holding one), or a server folder inside another one linked from this computer.
/// Linking the same local folder again (to change its settings) is fine.
pub fn check_overlap(dir: &Path, config: &Config) -> Result<(), String> {
    for other in linked() {
        if other.dir == dir {
            continue;
        }
        if dir.starts_with(&other.dir) || other.dir.starts_with(dir) {
            return Err(format!(
                "{} is inside {} or holds it, and that folder already syncs. Pick a folder outside it.",
                dir.display(),
                other.dir.display()
            ));
        }
        if same_server(config, &other.config) && remote_overlaps(&config.remote, &other.config.remote) {
            return Err(format!(
                "{} on the server already syncs to {} (it is the same folder, or one holds the other). \
                 Run `ownnas-sync remove {}` first, or pick another server folder.",
                show_remote(&other.config.remote),
                other.dir.display(),
                other.dir.display()
            ));
        }
    }
    Ok(())
}

/// How to show a server folder to people.
pub fn show_remote(remote: &str) -> String {
    if remote.is_empty() { "the whole shared folder".to_string() } else { format!("/{remote}") }
}

/// The settings of a linked folder signed in to `server` (any server when `None`), to reuse its
/// sign-in for another folder.
pub fn sign_in_for(server: Option<&str>) -> Result<Config, String> {
    let mut found: Vec<Config> = Vec::new();
    for linked in linked() {
        if linked.config.token.is_empty() {
            continue;
        }
        if let Some(server) = server {
            if linked.config.server.trim_end_matches('/') != server.trim_end_matches('/') {
                continue;
            }
        }
        if !found.iter().any(|known| same_server(known, &linked.config)) {
            found.push(linked.config);
        }
    }
    match found.len() {
        0 => Err(match server {
            Some(server) => format!("No folder is signed in to {server} yet. Run `ownnas-sync setup <folder> --server {server} --username …` first."),
            None => "No folder is linked yet. Run `ownnas-sync setup <folder> --server … --username …` first.".to_string(),
        }),
        1 => Ok(found.remove(0)),
        _ => Err(format!(
            "Folders here are signed in to more than one server or account ({}). Say which with --server.",
            found.iter().map(|config| format!("{} as {}", config.server, config.username)).collect::<Vec<_>>().join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_remote_paths() {
        assert_eq!(clean_remote("/Photos/2024/"), "Photos/2024");
        assert_eq!(clean_remote("Work\\Clients"), "Work/Clients");
        assert_eq!(clean_remote("/"), "");
        assert_eq!(clean_remote("./a//b"), "a/b");
    }

    #[test]
    fn finds_overlapping_server_folders() {
        assert!(remote_overlaps("", "Photos"));
        assert!(remote_overlaps("Photos", "Photos"));
        assert!(remote_overlaps("Photos", "Photos/2024"));
        assert!(remote_overlaps("Photos/2024", "Photos"));
        assert!(!remote_overlaps("Photos", "Photos 2"));
        assert!(!remote_overlaps("Photos", "Documents"));
    }
}
