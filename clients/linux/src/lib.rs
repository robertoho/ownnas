//! OwnNAS on Linux: each server signed in to is a folder (`~/OwnNAS/<name>`), a FUSE filesystem
//! served by `ownnas-mount run` (a systemd user service). Folders list what the server has;
//! files take no space until opened, then stay in a cache (`~/.cache/ownnas`) kept under a size
//! limit by dropping what was used least recently, except what is set to "always keep". Edits
//! are saved in the cache and sent when the file is closed, only over the version they started
//! from: when the server's changed meanwhile, the local edit goes up as a conflict copy.
//!
//! The parts here are the same on every system (settings, the cache's bookkeeping, paths), so
//! they are tested anywhere; the FUSE filesystem is in `mount`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use ownnas_core::model::Stamp;
use serde::{Deserialize, Serialize};

#[cfg(target_os = "linux")]
pub mod mount;

/// Extended attributes on every file and folder in a mount: what is on this computer, and
/// whether to always keep it. File-manager menus and `ownnas-mount keep`/`free` use them.
pub const STATE_XATTR: &str = "user.ownnas.state";
pub const KEEP_XATTR: &str = "user.ownnas.keep";

/// A server signed in to, and where it is mounted.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Account {
    /// Tells this server's cache apart from others.
    pub id: String,
    pub server: String,
    pub username: String,
    /// The server's shared folder name.
    pub name: String,
    /// Where its files show.
    pub folder: PathBuf,
    /// A private CA to trust (PEM), and certificate fingerprints accepted.
    #[serde(default)]
    pub ca_pem: Option<String>,
    #[serde(default)]
    pub pinned: Vec<String>,
    /// The sign-in token (the settings file is readable by this user only).
    pub token: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Settings {
    pub accounts: Vec<Account>,
    /// How much the cache of opened files may take, in GB (files to always keep don't count).
    #[serde(default = "default_cache_gb")]
    pub cache_gb: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { accounts: Vec::new(), cache_gb: default_cache_gb() }
    }
}

fn default_cache_gb() -> u64 {
    10
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    match std::env::var_os(var) {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => home().join(fallback),
    }
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

/// `~/.config/ownnas`: the settings.
pub fn config_dir() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("ownnas")
}

/// `~/.cache/ownnas`: one folder per server, with the files opened and its index.
pub fn cache_dir() -> PathBuf {
    xdg("XDG_CACHE_HOME", ".cache").join("ownnas")
}

/// `~/.local/state/ownnas`: the log.
pub fn state_dir() -> PathBuf {
    xdg("XDG_STATE_HOME", ".local/state").join("ownnas")
}

pub fn load_settings() -> Settings {
    std::fs::read(config_dir().join("mount.json")).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}

pub fn save_settings(settings: &Settings) -> Result<(), String> {
    let text = serde_json::to_vec_pretty(settings).map_err(|err| err.to_string())?;
    write_private(&config_dir().join("mount.json"), &text)
}

/// Writes a file readable by this user only, all at once.
pub fn write_private(file: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let dir = file.parent().unwrap_or(std::path::Path::new("."));
    std::fs::create_dir_all(dir).map_err(|err| format!("Could not create {}: {err}", dir.display()))?;
    let temp = file.with_extension("new");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let written = options.open(&temp).and_then(|mut out| out.write_all(bytes).and_then(|_| out.sync_all()));
    written.map_err(|err| format!("Could not save {}: {err}", temp.display()))?;
    std::fs::rename(&temp, file).map_err(|err| format!("Could not save {}: {err}", file.display()))
}

/// A line in the log (`~/.local/state/ownnas/mount.log`) and on stderr (the service's journal),
/// with the time.
pub fn log(line: &str) {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let text = format!("{secs} {line}\n");
    eprint!("{text}");
    let dir = state_dir();
    let _ = std::fs::create_dir_all(&dir);
    let file = dir.join("mount.log");
    if std::fs::metadata(&file).is_ok_and(|meta| meta.len() > 5 * 1024 * 1024) {
        let _ = std::fs::rename(&file, file.with_extension("log.old"));
    }
    if let Ok(mut out) = std::fs::OpenOptions::new().create(true).append(true).open(file) {
        use std::io::Write;
        let _ = out.write_all(text.as_bytes());
    }
}

/// Names kept on this computer only: the shared ones, and Linux editors' and file managers'
/// temporary files (saving through one of these renames it over the real file, which is sent).
pub fn ignored(name: &str) -> bool {
    ownnas_core::local::ignored(name)
        || name.starts_with(".goutputstream-")
        || name.starts_with(".fuse_hidden")
        || name.ends_with(".swp")
        || name.ends_with(".swx")
        || name.ends_with(".kate-swp")
        || name == "4913"
}

/// A folder name for a server's mount: its shared folder name, safe as a file name.
pub fn folder_name(name: &str) -> String {
    let clean: String = name.chars().map(|c| if c == '/' || c.is_control() { '_' } else { c }).collect();
    let clean = clean.trim().trim_start_matches('.').to_string();
    if clean.is_empty() { "OwnNAS".into() } else { clean }
}

// --- Server paths ("" is the top, parts joined with "/") ----------------------------------------

pub fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() { name.to_string() } else { format!("{parent}/{name}") }
}

/// The folder a path is in, and its name.
pub fn split(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

/// `path` is `top` or inside it.
pub fn under(path: &str, top: &str) -> bool {
    top.is_empty() || path == top || (path.starts_with(top) && path.as_bytes().get(top.len()) == Some(&b'/'))
}

/// `path` (under `from`) moved to `to`.
pub fn moved(path: &str, from: &str, to: &str) -> String {
    if path == from {
        to.to_string()
    } else {
        join(to, &path[from.len() + 1..])
    }
}

// --- The cache's bookkeeping -------------------------------------------------------------------

/// What the server has at a path, as its folder's listing said.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub struct Item {
    pub dir: bool,
    pub size: u64,
    /// Milliseconds since 1970.
    pub mtime: i64,
}

impl Item {
    pub fn stamp(&self) -> Stamp {
        Stamp { size: self.size, mtime: self.mtime }
    }
}

/// A file with its content in the cache.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Cached {
    /// Its content is in the cache's `files/<blob>`.
    pub blob: u64,
    /// The server's version the content started from (`None`: made here, not on the server).
    pub server: Option<Stamp>,
    /// Changed here and not sent yet: never dropped or replaced.
    pub dirty: bool,
    /// Last opened, in seconds since 1970 (the least recently used go first).
    pub used: i64,
    pub size: u64,
}

/// The cache's bookkeeping for one server, saved as `index.json` next to its files.
#[derive(Serialize, Deserialize, Default, Debug)]
pub struct Index {
    pub files: BTreeMap<String, Cached>,
    /// Files and folders to always keep (with everything in them).
    pub keep: BTreeSet<String>,
    /// Folder listings last seen, so folders opened before still list when the server can't be
    /// reached.
    pub listings: BTreeMap<String, BTreeMap<String, Item>>,
    pub next_blob: u64,
}

impl Index {
    pub fn new_blob(&mut self) -> u64 {
        self.next_blob += 1;
        self.next_blob
    }

    /// Set to always keep, itself or a folder it is in.
    pub fn kept(&self, path: &str) -> bool {
        let mut at = path;
        loop {
            if self.keep.contains(at) {
                return true;
            }
            if at.is_empty() {
                return false;
            }
            at = split(at).0;
        }
    }

    /// What the server has at `path`, as last listed.
    pub fn listed(&self, path: &str) -> Option<Item> {
        let (parent, name) = split(path);
        self.listings.get(parent)?.get(name).copied()
    }

    /// Files to drop for the cache to fit `limit` bytes, least recently used first. Changed,
    /// kept and open files stay (and don't count).
    pub fn evictable(&self, limit: u64, open: impl Fn(&str) -> bool) -> Vec<String> {
        let mut candidates: Vec<(&String, &Cached)> =
            self.files.iter().filter(|(path, cached)| !cached.dirty && !self.kept(path) && !open(path)).collect();
        let mut total: u64 = candidates.iter().map(|(_, cached)| cached.size).sum();
        candidates.sort_by_key(|(_, cached)| cached.used);
        let mut drop = Vec::new();
        for (path, cached) in candidates {
            if total <= limit {
                break;
            }
            total -= cached.size;
            drop.push(path.clone());
        }
        drop
    }

    /// Moves everything known about `from` (and what is in it) to `to`. Returns the blobs of
    /// files that were at `to` before, to delete.
    pub fn rename(&mut self, from: &str, to: &str) -> Vec<u64> {
        let replaced = self.forget(to);
        let files: Vec<String> = self.files.keys().filter(|path| under(path, from)).cloned().collect();
        for path in files {
            if let Some(cached) = self.files.remove(&path) {
                self.files.insert(moved(&path, from, to), cached);
            }
        }
        let keep: Vec<String> = self.keep.iter().filter(|path| under(path, from)).cloned().collect();
        for path in keep {
            self.keep.remove(&path);
            self.keep.insert(moved(&path, from, to));
        }
        let listings: Vec<String> = self.listings.keys().filter(|path| under(path, from)).cloned().collect();
        for path in listings {
            if let Some(listing) = self.listings.remove(&path) {
                self.listings.insert(moved(&path, from, to), listing);
            }
        }
        let (from_parent, from_name) = split(from);
        let item = self.listings.get_mut(from_parent).and_then(|listing| listing.remove(from_name));
        if let Some(item) = item {
            let (to_parent, to_name) = split(to);
            if let Some(listing) = self.listings.get_mut(to_parent) {
                listing.insert(to_name.to_string(), item);
            }
        }
        replaced
    }

    /// Forgets `path` and what is in it: its listing entry, listings, cached files (their blobs
    /// are returned, to delete) and keep marks.
    pub fn forget(&mut self, path: &str) -> Vec<u64> {
        if path.is_empty() {
            return Vec::new();
        }
        let (parent, name) = split(path);
        if let Some(listing) = self.listings.get_mut(parent) {
            listing.remove(name);
        }
        self.listings.retain(|key, _| !under(key, path));
        self.keep.retain(|key| !under(key, path));
        let files: Vec<String> = self.files.keys().filter(|key| under(key, path)).cloned().collect();
        files.into_iter().filter_map(|key| self.files.remove(&key)).map(|cached| cached.blob).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cached(blob: u64, used: i64, size: u64, dirty: bool) -> Cached {
        Cached { blob, server: Some(Stamp { size, mtime: 1 }), dirty, used, size }
    }

    #[test]
    fn paths() {
        assert_eq!(join("", "a"), "a");
        assert_eq!(join("a", "b c"), "a/b c");
        assert_eq!(split("a/b/c"), ("a/b", "c"));
        assert_eq!(split("a"), ("", "a"));
        assert!(under("a/b", "a") && under("a", "a") && under("a", "") && !under("ab", "a") && !under("a", "a/b"));
        assert_eq!(moved("a/b/c", "a/b", "x"), "x/c");
        assert_eq!(moved("a/b", "a/b", "x/y"), "x/y");
    }

    #[test]
    fn kept_with_what_is_in_it() {
        let mut index = Index::default();
        index.keep.insert("Photos/2026".into());
        assert!(index.kept("Photos/2026") && index.kept("Photos/2026/a.jpg") && index.kept("Photos/2026/x/b.jpg"));
        assert!(!index.kept("Photos") && !index.kept("Photos/2025/a.jpg") && !index.kept("Photos/20266"));
        index.keep.insert(String::new());
        assert!(index.kept("anything/at/all"));
    }

    #[test]
    fn the_least_recently_used_go_first_and_changes_stay() {
        let mut index = Index::default();
        index.files.insert("old".into(), cached(1, 100, 400, false));
        index.files.insert("older but changed".into(), cached(2, 50, 400, true));
        index.files.insert("new".into(), cached(3, 300, 400, false));
        index.files.insert("middle".into(), cached(4, 200, 400, false));
        index.files.insert("open".into(), cached(5, 10, 400, false));
        index.files.insert("Kept/a".into(), cached(6, 20, 400, false));
        index.keep.insert("Kept".into());
        assert!(index.evictable(1200, |path| path == "open").is_empty(), "old, new and middle fit");
        assert_eq!(index.evictable(500, |path| path == "open"), vec!["old".to_string(), "middle".to_string()]);
        assert_eq!(index.evictable(0, |path| path == "open"), vec!["old".to_string(), "middle".to_string(), "new".to_string()]);
    }

    #[test]
    fn renames_take_everything_along() {
        let mut index = Index::default();
        index.files.insert("Docs/a.txt".into(), cached(1, 1, 1, false));
        index.files.insert("Docs/sub/b.txt".into(), cached(2, 1, 1, true));
        index.files.insert("Docsx/c.txt".into(), cached(3, 1, 1, false));
        index.files.insert("Work/old.txt".into(), cached(4, 1, 1, false));
        index.keep.insert("Docs/sub".into());
        index.listings.insert("".into(), BTreeMap::from([("Docs".to_string(), Item { dir: true, size: 0, mtime: 0 })]));
        index.listings.insert("Docs".into(), BTreeMap::new());
        index.listings.insert("Work".into(), BTreeMap::new());
        let replaced = index.rename("Docs", "Work");
        assert_eq!(replaced, vec![4], "what was at the target goes");
        let paths: Vec<&str> = index.files.keys().map(String::as_str).collect();
        assert_eq!(paths, ["Docsx/c.txt", "Work/a.txt", "Work/sub/b.txt"]);
        assert!(index.keep.contains("Work/sub") && !index.keep.contains("Docs/sub"));
        assert!(index.listings.contains_key("Work") && !index.listings.contains_key("Docs"));
        assert_eq!(index.listed("Work"), Some(Item { dir: true, size: 0, mtime: 0 }));
        assert_eq!(index.listed("Docs"), None);
    }

    #[test]
    fn forgetting_a_folder_forgets_what_is_in_it() {
        let mut index = Index::default();
        index.files.insert("Docs/a.txt".into(), cached(1, 1, 1, false));
        index.files.insert("Docs2/b.txt".into(), cached(2, 1, 1, false));
        index.listings.insert("Docs".into(), BTreeMap::new());
        index.listings.insert("".into(), BTreeMap::from([("Docs".to_string(), Item { dir: true, size: 0, mtime: 0 })]));
        index.keep.insert("Docs".into());
        assert_eq!(index.forget("Docs"), vec![1]);
        assert!(index.listed("Docs").is_none() && !index.listings.contains_key("Docs") && index.keep.is_empty());
        assert!(index.files.contains_key("Docs2/b.txt"));
        assert!(index.forget("").is_empty(), "the top is never forgotten");
    }

    #[test]
    fn names() {
        assert!(ignored(".goutputstream-ABC123") && ignored(".notes.txt.swp") && ignored("~$Report.docx") && !ignored("notes.txt"));
        assert_eq!(folder_name("library"), "library");
        assert_eq!(folder_name("a/b"), "a_b");
        assert_eq!(folder_name(".hidden"), "hidden");
        assert_eq!(folder_name(""), "OwnNAS");
    }
}
