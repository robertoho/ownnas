//! Keeps one server's place in Explorer in sync (Windows Cloud Files API):
//!
//! - Folders are filled with placeholders when opened (`fetch_placeholders`): files that take no
//!   space until read, and download when they are (`fetch_data`). Each keeps the server's
//!   version (`size:mtime`) it was made from.
//! - Deleting or renaming in Explorer does the same on the server first (`delete`, `rename`).
//! - New and changed local files are noticed by a watcher and uploaded only over the version
//!   they started from; when the server's changed meanwhile, the local edit is kept as a
//!   conflict copy next to it and the file takes the server's version.
//! - Changes on the server come from its change feed: placeholders are added, updated (and
//!   emptied, so the new version downloads when next opened) or removed. Files with local edits
//!   not yet sent are never replaced or removed.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use cloud_filter::error::{CResult, CloudErrorKind};
use cloud_filter::filter::{info, ticket, Request, SyncFilter};
use cloud_filter::metadata::Metadata;
use cloud_filter::placeholder::{ConvertOptions, Placeholder, UpdateOptions};
use cloud_filter::placeholder_file::PlaceholderFile;
use cloud_filter::root::{HydrationType, PopulationType, ProtectionMode, SecurityId, Session, SyncRootId, SyncRootIdBuilder, SyncRootInfo};
use cloud_filter::utility::WriteAt;
use nt_time::FileTime;
use ownnas_core::api::{Api, ApiError};
use ownnas_core::browse::Entry;
use ownnas_core::model::Stamp;

use crate::{blob_stamp, file_time, ignored, local_path, log, server_path, stamp_blob, Account};

/// The Explorer registration of an account's place.
pub fn root_id(account: &Account) -> Result<SyncRootId, String> {
    let user = SecurityId::current_user().map_err(|err| format!("Could not tell who is signed in to Windows: {err}"))?;
    Ok(SyncRootIdBuilder::new(crate::PROVIDER).user_security_id(user).account_name(&account.id).build())
}

/// Puts an account's folder in Explorer's navigation pane (once).
pub fn register(account: &Account) -> Result<(), String> {
    fs::create_dir_all(&account.folder).map_err(|err| format!("Could not create {}: {err}", account.folder.display()))?;
    let id = root_id(account)?;
    if id.is_registered().map_err(|err| err.to_string())? {
        return Ok(());
    }
    let exe = std::env::current_exe().map_err(|err| err.to_string())?;
    let info = SyncRootInfo::default()
        .with_display_name(format!("OwnNAS - {}", account.name))
        .with_icon(format!("{},0", exe.display()))
        .with_version(env!("CARGO_PKG_VERSION"))
        .with_hydration_type(HydrationType::Full)
        .with_population_type(PopulationType::Full)
        .with_protection_mode(ProtectionMode::Unknown)
        .with_allow_pinning(true)
        .with_show_siblings_as_group(false)
        .with_path(&account.folder)
        .map_err(|err| format!("Could not use {}: {err}", account.folder.display()))?;
    id.register(info).map_err(|err| format!("Explorer did not take the place: {err}"))
}

/// Takes an account's place out of Explorer (its files on this computer stay as they are).
pub fn unregister(account: &Account) -> Result<(), String> {
    let id = root_id(account)?;
    if id.is_registered().map_err(|err| err.to_string())? {
        id.unregister().map_err(|err| format!("Could not remove the place from Explorer: {err}"))?;
    }
    Ok(())
}

/// A file or folder opened to be made or changed as a placeholder. Its handle is an ordinary
/// one: the Cloud Files calls that change placeholders take no other (those `CfOpenFileWithOplock`
/// gives are refused as invalid). `exclusive`: no one else may have it open (needed to empty it).
fn writable(path: &Path, exclusive: bool) -> windows::core::Result<Placeholder> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const SHARE_ALL: u32 = 0x1 | 0x2 | 0x4;
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(if exclusive { 0 } else { SHARE_ALL })
        // Folders open only with this.
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .map_err(|err| windows::core::Error::from_hresult(windows::core::HRESULT::from_win32(err.raw_os_error().unwrap_or(0) as u32)))?;
    Ok(Placeholder::from(file))
}

/// The sync engine for one account.
pub struct Engine {
    api: Api,
    root: PathBuf,
    name: String,
    /// Local paths this engine is changing itself, whose events are not the person's doing.
    quiet: Mutex<HashMap<PathBuf, Instant>>,
    /// Files being sent: the change feed tells of them before they are marked as sent.
    sending: Mutex<HashSet<PathBuf>>,
}

impl Engine {
    pub fn new(account: &Account, api: Api) -> Arc<Self> {
        Arc::new(Engine { api, root: account.folder.clone(), name: account.name.clone(), quiet: Mutex::new(HashMap::new()), sending: Mutex::new(HashSet::new()) })
    }

    fn note(&self, line: &str) {
        log(&format!("[{}] {line}", self.name));
    }

    fn server(&self, local: &Path) -> Option<String> {
        server_path(&self.root, local)
    }

    /// Marks a local path as being changed here, for a few seconds.
    fn hush(&self, path: &Path) {
        self.quiet.lock().unwrap_or_else(|err| err.into_inner()).insert(path.to_path_buf(), Instant::now());
    }

    fn hushed(&self, path: &Path) -> bool {
        let mut quiet = self.quiet.lock().unwrap_or_else(|err| err.into_inner());
        quiet.retain(|_, at| at.elapsed() < Duration::from_secs(10));
        quiet.contains_key(path)
    }

    // --- Placeholders --------------------------------------------------------------------------

    fn metadata(entry: &Entry) -> Metadata {
        let time = FileTime::new(file_time(Self::millis(entry)));
        let base = if entry.dir { Metadata::directory() } else { Metadata::file().size(entry.size) };
        base.created(time).written(time).changed(time).accessed(time)
    }

    fn millis(entry: &Entry) -> i64 {
        if entry.mtime_ms > 0 { entry.mtime_ms } else { entry.modified.saturating_mul(1000) }
    }

    fn placeholder(entry: &Entry) -> PlaceholderFile {
        let blob = if entry.dir { b"dir".to_vec() } else { stamp_blob(entry.size, Self::millis(entry)) };
        PlaceholderFile::new(&entry.name).metadata(Self::metadata(entry)).blob(blob).mark_in_sync()
    }

    /// A placeholder for a server entry in the local folder `parent` (which exists).
    fn create(&self, parent: &Path, entry: &Entry) {
        self.hush(&parent.join(&entry.name));
        if let Err(err) = Self::placeholder(entry).create::<&Path>(parent) {
            self.note(&format!("could not add {}: {err}", entry.path));
        }
    }

    /// What a local placeholder is: (in sync, the version it was made from); `None` when it is
    /// not one. Read here rather than with cloud-filter, whose `blob()` starts at the end of the
    /// structure instead of at its `FileIdentity` field, 4 bytes earlier, so versions never read.
    fn state(path: &Path) -> Option<(bool, Option<Stamp>)> {
        use std::os::windows::fs::OpenOptionsExt;
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::Storage::CloudFilters::{
            CfGetPlaceholderInfo, CF_IN_SYNC_STATE_IN_SYNC, CF_PLACEHOLDER_INFO_STANDARD, CF_PLACEHOLDER_STANDARD_INFO,
        };
        const FILE_READ_ATTRIBUTES: u32 = 0x80;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        let file = fs::OpenOptions::new().access_mode(FILE_READ_ATTRIBUTES).share_mode(0x7).custom_flags(FILE_FLAG_BACKUP_SEMANTICS).open(path).ok()?;
        // In u64s, aligned for the structure.
        let mut buffer = vec![0u64; (std::mem::size_of::<CF_PLACEHOLDER_STANDARD_INFO>() + 4096) / 8];
        let mut length = 0u32;
        unsafe {
            CfGetPlaceholderInfo(HANDLE(file.as_raw_handle()), CF_PLACEHOLDER_INFO_STANDARD, buffer.as_mut_ptr().cast(), (buffer.len() * 8) as u32, Some(&mut length))
        }
        .ok()?;
        let info = unsafe { &*(buffer.as_ptr() as *const CF_PLACEHOLDER_STANDARD_INFO) };
        let bytes = unsafe { std::slice::from_raw_parts(buffer.as_ptr() as *const u8, (length as usize).min(buffer.len() * 8)) };
        let start = std::mem::offset_of!(CF_PLACEHOLDER_STANDARD_INFO, FileIdentity);
        let blob = bytes.get(start..start + info.FileIdentityLength as usize)?;
        Some((info.InSyncState == CF_IN_SYNC_STATE_IN_SYNC, blob_stamp(blob)))
    }

    // --- Changes on the server -----------------------------------------------------------------

    /// Brings a local folder (and the folders under it this computer has) in line with the
    /// server, after changes may have been missed. Local edits not yet sent are kept.
    pub fn reconcile(&self, folder: &Path) {
        let Some(remote) = self.server(folder) else { return };
        let listing = match self.api.list(&remote) {
            Ok(listing) => listing,
            Err(err) => return self.note(&format!("could not list {remote}: {err}")),
        };
        let names: HashSet<String> = listing.entries.iter().map(|entry| entry.name.clone()).collect();
        for entry in &listing.entries {
            self.server_changed(entry);
        }
        let Ok(children) = fs::read_dir(folder) else { return };
        for child in children.flatten() {
            let name = child.file_name().to_string_lossy().into_owned();
            let path = child.path();
            if ignored(&name) {
                continue;
            }
            if !names.contains(&name) {
                self.server_removed(&path);
            } else if path.is_dir() {
                self.reconcile(&path);
            }
        }
    }

    /// A file or folder that is on the server (new or changed there).
    fn server_changed(&self, entry: &Entry) {
        let path = local_path(&self.root, &entry.path);
        let Some(parent) = path.parent() else { return };
        // Folders not opened here yet are filled when they are.
        if !parent.is_dir() {
            return;
        }
        if !path.exists() {
            return self.create(parent, entry);
        }
        // Being sent from here (the feed tells of it too), or not a file.
        if entry.dir || self.hushed(&path) || self.sending.lock().unwrap_or_else(|err| err.into_inner()).contains(&path) {
            return;
        }
        let Some((in_sync, base)) = Self::state(&path) else {
            return self.note(&format!("{} changed on the server, but it is not a placeholder here", entry.path));
        };
        let now = Stamp { size: entry.size, mtime: Self::millis(entry) };
        if base == Some(now) {
            if std::env::var_os("OWNNAS_TRACE").is_some() {
                self.note(&format!("{} is the server's version already", entry.path));
            }
            return;
        }
        // Local edits not yet sent win here; their upload finds the conflict.
        if !in_sync {
            return self.note(&format!("{} changed on the server and here: this computer's version goes up next", entry.path));
        }
        self.hush(&path);
        let blob = stamp_blob(now.size, now.mtime);
        let updated = writable(&path, true).and_then(|mut handle| {
            // Emptied: the new version downloads when the file is next opened.
            handle.update(UpdateOptions::default().metadata(Self::metadata(entry)).blob(&blob).dehydrate().mark_in_sync(), None).map(|_| ())
        });
        if updated.is_ok() {
            self.note(&format!("{} changed on the server: the new version downloads when opened", entry.path));
        }
        if let Err(err) = updated {
            // Not changed after all: what happens to it next is the person's doing.
            self.quiet.lock().unwrap_or_else(|err| err.into_inner()).remove(&path);
            self.note(&format!("could not update {} from {base:?} to {now:?} (open in an app?): {err}", entry.path));
        }
    }

    /// Something gone from the server: removed here too, unless it holds local edits not sent.
    fn server_removed(&self, path: &Path) {
        if path.is_dir() {
            if let Ok(children) = fs::read_dir(path) {
                for child in children.flatten() {
                    self.server_removed(&child.path());
                }
            }
            self.hush(path);
            let _ = fs::remove_dir(path);
            return;
        }
        match Self::state(path) {
            Some((true, _)) => {
                self.hush(path);
                if let Err(err) = fs::remove_file(path) {
                    self.note(&format!("could not remove {}: {err}", path.display()));
                }
            }
            // Edited here, or never sent: it goes up as a new file instead.
            _ => {}
        }
    }

    /// Follows the server's change feed for as long as the program runs.
    pub fn follow(self: &Arc<Self>) {
        #[derive(serde::Deserialize)]
        struct Change {
            path: String,
            #[serde(default)]
            deleted: bool,
        }
        #[derive(serde::Deserialize)]
        struct Feed {
            cursor: String,
            expired: bool,
            changes: Vec<Change>,
        }
        let mut cursor = String::new();
        loop {
            let asked = cursor.clone();
            match self.api.get_json::<Feed>("/api/sync/changes", &[("cursor", &asked), ("wait", "25")]) {
                Ok(feed) => {
                    if asked.is_empty() || feed.expired {
                        // Starting, or changes were missed: look at everything this computer has.
                        self.reconcile(&self.root);
                    } else {
                        for change in &feed.changes {
                            if std::env::var_os("OWNNAS_TRACE").is_some() {
                                self.note(&format!("feed: {}{}", change.path, if change.deleted { " (deleted)" } else { "" }));
                            }
                            self.apply(&change.path, change.deleted);
                        }
                        // The feed tells of a change as it starts: a file being written may
                        // still list with its old version (NTFS sets the time later), so look
                        // again once it is done.
                        let engine = self.clone();
                        std::thread::spawn(move || {
                            std::thread::sleep(Duration::from_secs(3));
                            for change in feed.changes {
                                engine.apply(&change.path, change.deleted);
                            }
                        });
                    }
                    cursor = feed.cursor;
                }
                Err(ApiError::NotFound) => {
                    // A server from before the change feed: look again every minute.
                    self.reconcile(&self.root);
                    std::thread::sleep(Duration::from_secs(60));
                }
                Err(ApiError::Unauthorized) => {
                    self.note("the server signed this computer out: run `ownnas-cloud setup` again");
                    return;
                }
                Err(err) => {
                    self.note(&format!("cannot reach the server: {err}"));
                    std::thread::sleep(Duration::from_secs(15));
                }
            }
        }
    }

    fn apply(&self, remote: &str, deleted: bool) {
        if remote.split('/').any(ignored) {
            return;
        }
        let path = local_path(&self.root, remote);
        if deleted {
            if path.exists() {
                self.server_removed(&path);
            }
            return;
        }
        // What is there now, from its folder's listing.
        let parent = remote.rsplit_once('/').map_or("", |(parent, _)| parent);
        if !local_path(&self.root, parent).is_dir() {
            return;
        }
        match self.api.list(parent) {
            Ok(listing) => match listing.entries.iter().find(|entry| entry.path == remote) {
                Some(entry) => self.server_changed(entry),
                None => self.note(&format!("{remote} changed on the server, but its folder's listing does not have it")),
            },
            Err(err) => self.note(&format!("could not list {parent}: {err}")),
        }
    }

    // --- Changes here --------------------------------------------------------------------------

    /// Watches the folder for new and changed files, sending them once they settle.
    pub fn watch(self: &Arc<Self>) -> Result<notify::RecommendedWatcher, String> {
        use notify::Watcher;
        let (send, receive) = mpsc::channel::<PathBuf>();
        let mut watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
            if let Ok(event) = result {
                for path in event.paths {
                    let _ = send.send(path);
                }
            }
        })
        .map_err(|err| err.to_string())?;
        watcher.watch(&self.root, notify::RecursiveMode::Recursive).map_err(|err| err.to_string())?;
        let engine = self.clone();
        std::thread::spawn(move || {
            // Paths waiting to settle (an app may still be writing), with when they last changed.
            let mut waiting: HashMap<PathBuf, Instant> = HashMap::new();
            loop {
                match receive.recv_timeout(Duration::from_millis(500)) {
                    Ok(path) => {
                        waiting.insert(path, Instant::now());
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
                let ready: Vec<PathBuf> = waiting.iter().filter(|(_, at)| at.elapsed() > Duration::from_secs(2)).map(|(path, _)| path.clone()).collect();
                for path in ready {
                    waiting.remove(&path);
                    if !engine.local_changed(&path) {
                        // In use or the server unreachable: again in a while.
                        waiting.insert(path, Instant::now() + Duration::from_secs(10));
                    }
                }
            }
        });
        Ok(watcher)
    }

    /// Sends a new or changed local file or folder. False to try again later.
    fn local_changed(&self, path: &Path) -> bool {
        let Some(remote) = self.server(path) else { return true };
        if remote.is_empty() || remote.split('/').any(ignored) || self.hushed(path) {
            return true;
        }
        let Ok(meta) = fs::symlink_metadata(path) else { return true };
        if meta.file_type().is_symlink() {
            return true;
        }
        let state = Self::state(path);
        if meta.is_dir() {
            if state.is_some() {
                return true;
            }
            // A folder made here.
            match self.api.mkdir(&remote) {
                Ok(()) | Err(ApiError::Conflict) => {}
                Err(err) => {
                    self.note(&format!("could not make {remote}: {err}"));
                    return false;
                }
            }
            self.hush(path);
            let converted = writable(path, false).and_then(|mut handle| {
                handle.convert_to_placeholder(ConvertOptions::default().mark_in_sync().blob(b"dir".to_vec()), None).map(|_| ())
            });
            if let Err(err) = converted {
                self.note(&format!("could not mark {remote} as synced: {err}"));
            }
            // What is in it is sent on its own.
            if let Ok(children) = fs::read_dir(path) {
                for child in children.flatten() {
                    self.local_changed(&child.path());
                }
            }
            return true;
        }
        let base = match state {
            Some((true, _)) => return true,
            Some((false, base)) => base,
            None => None,
        };
        let size = meta.len();
        let mtime = meta.modified().ok().and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as i64);
        let file = match fs::File::open(path) {
            Ok(file) => file,
            // Still open in the app writing it.
            Err(_) => return false,
        };
        self.sending.lock().unwrap_or_else(|err| err.into_inner()).insert(path.to_path_buf());
        let done = match self.api.upload(&remote, base, mtime, size, file) {
            Ok(stamp) => {
                self.mark_sent(path, state.is_some(), stamp);
                self.note(&format!("sent {remote}"));
                true
            }
            Err(ApiError::Conflict) => {
                self.keep_both(path, &remote, mtime, size);
                true
            }
            Err(err) => {
                self.note(&format!("could not send {remote}: {err}"));
                false
            }
        };
        self.sending.lock().unwrap_or_else(|err| err.into_inner()).remove(path);
        done
    }

    fn mark_sent(&self, path: &Path, was_placeholder: bool, stamp: Stamp) {
        self.hush(path);
        let blob = stamp_blob(stamp.size, stamp.mtime);
        let marked = writable(path, false).and_then(|mut handle| {
            if was_placeholder {
                handle.update(UpdateOptions::default().blob(&blob).mark_in_sync(), None).map(|_| ())
            } else {
                handle.convert_to_placeholder(ConvertOptions::default().mark_in_sync().blob(blob.clone()), None).map(|_| ())
            }
        });
        if let Err(err) = marked {
            self.note(&format!("could not mark {} as synced: {err}", path.display()));
        }
    }

    /// Both changed: the local edit goes up as a conflict copy, and the file takes the
    /// server's version (an empty placeholder, which downloads when opened).
    fn keep_both(&self, path: &Path, remote: &str, mtime: i64, size: u64) {
        let copy = ownnas_core::local::conflict_name(remote, &ownnas_core::engine::host_name(), SystemTime::now());
        let sent = fs::File::open(path).map_err(|err| ApiError::Other(err.to_string())).and_then(|file| self.api.upload(&copy, None, mtime, size, file));
        if let Err(err) = sent {
            return self.note(&format!("{remote} changed on the server too, and the copy of this computer's version could not be sent: {err}"));
        }
        self.note(&format!("{remote} changed here and on the server: this computer's version is kept as {copy}"));
        let parent_remote = remote.rsplit_once('/').map_or("", |(parent, _)| parent);
        let (Some(parent), Ok(listing)) = (path.parent(), self.api.list(parent_remote)) else { return };
        self.hush(path);
        if fs::remove_file(path).is_ok() {
            if let Some(entry) = listing.entries.iter().find(|entry| entry.path == remote) {
                self.create(parent, entry);
            }
        }
    }
}

/// Explorer's requests, answered from the server.
pub struct Filter(pub Arc<Engine>);

fn fail(err: ApiError) -> CloudErrorKind {
    match err {
        ApiError::NotFound => CloudErrorKind::NotInSync,
        ApiError::Unauthorized => CloudErrorKind::AuthenticationFailed,
        _ => CloudErrorKind::NetworkUnavailable,
    }
}

impl SyncFilter for Filter {
    /// A folder is opened: its contents as placeholders.
    fn fetch_placeholders(&self, request: Request, ticket: ticket::FetchPlaceholders, _info: info::FetchPlaceholders) -> CResult<()> {
        let folder = request.path();
        let remote = self.0.server(&folder).ok_or(CloudErrorKind::NotUnderSyncRoot)?;
        let listing = self.0.api.list(&remote).map_err(fail)?;
        let mut items: Vec<PlaceholderFile> = listing
            .entries
            .iter()
            .filter(|entry| !ignored(&entry.name) && !folder.join(&entry.name).exists())
            .map(Engine::placeholder)
            .collect();
        ticket.pass_with_placeholder(&mut items).map_err(|_| CloudErrorKind::InvalidRequest)
    }

    /// A file is read: its contents from the server, in 4 KiB-aligned pieces.
    fn fetch_data(&self, request: Request, ticket: ticket::FetchData, _info: info::FetchData) -> CResult<()> {
        let path = request.path();
        let remote = self.0.server(&path).ok_or(CloudErrorKind::NotUnderSyncRoot)?;
        let (mut body, stamp) = self.0.api.download(&remote).map_err(fail)?;
        // The server has another version than this placeholder: update it first.
        if blob_stamp(request.file_blob()).is_some_and(|known| known != stamp) {
            self.0.note(&format!("{remote} changed on the server: updating it"));
            self.0.apply(&remote, false);
            return Err(CloudErrorKind::NotInSync);
        }
        let total = request.file_size();
        let mut buffer = vec![0u8; 4 * 1024 * 1024];
        let mut offset = 0u64;
        loop {
            let mut filled = 0;
            while filled < buffer.len() {
                match body.read(&mut buffer[filled..]) {
                    Ok(0) => break,
                    Ok(read) => filled += read,
                    Err(_) => return Err(CloudErrorKind::NetworkUnavailable),
                }
            }
            if filled == 0 {
                break;
            }
            ticket.write_at(&buffer[..filled], offset).map_err(|_| CloudErrorKind::InvalidRequest)?;
            offset += filled as u64;
            let _ = ticket.report_progress(total, offset);
            if filled < buffer.len() {
                break;
            }
        }
        Ok(())
    }

    /// Deleted in Explorer: to the server's Trash first.
    fn delete(&self, request: Request, ticket: ticket::Delete, info: info::Delete) -> CResult<()> {
        let path = request.path();
        if info.is_undelete() || self.0.hushed(&path) {
            return ticket.pass().map_err(|_| CloudErrorKind::InvalidRequest);
        }
        let remote = self.0.server(&path).ok_or(CloudErrorKind::NotUnderSyncRoot)?;
        match self.0.api.trash(&remote) {
            Ok(()) | Err(ApiError::NotFound) => {}
            Err(err) => {
                self.0.note(&format!("could not delete {remote} on the server: {err}"));
                return Err(fail(err));
            }
        }
        self.0.note(&format!("moved {remote} to the server's Trash"));
        ticket.pass().map_err(|_| CloudErrorKind::InvalidRequest)
    }

    /// Renamed or moved in Explorer: on the server first.
    fn rename(&self, request: Request, ticket: ticket::Rename, info: info::Rename) -> CResult<()> {
        let source = request.path();
        let target = info.target_path();
        if self.0.hushed(&source) || !info.source_in_scope() {
            return ticket.pass().map_err(|_| CloudErrorKind::InvalidRequest);
        }
        let from = self.0.server(&source).ok_or(CloudErrorKind::NotUnderSyncRoot)?;
        let result = if !info.target_in_scope() {
            // Moved out of the place: gone from the server's view.
            self.0.api.trash(&from)
        } else {
            let to = self.0.server(&target).ok_or(CloudErrorKind::NotUnderSyncRoot)?;
            let (from_parent, _) = from.rsplit_once('/').unwrap_or(("", &from));
            let (to_parent, to_name) = to.rsplit_once('/').unwrap_or(("", &to));
            let mut at = from.clone();
            let mut done = Ok(());
            if from_parent != to_parent {
                match self.0.api.move_to(&at, to_parent) {
                    Ok(path) => at = path,
                    Err(err) => done = Err(err),
                }
            }
            if done.is_ok() && at.rsplit('/').next() != Some(to_name) {
                done = self.0.api.rename(&at, to_name).map(|_| ());
            }
            done
        };
        match result {
            Ok(()) => {
                self.0.note(&format!("moved {from} on the server"));
                ticket.pass().map_err(|_| CloudErrorKind::InvalidRequest)
            }
            Err(err) => {
                self.0.note(&format!("could not move {from} on the server: {err}"));
                Err(fail(err))
            }
        }
    }

    /// Explorer's "Free up space": the file goes back to a placeholder.
    fn dehydrate(&self, _request: Request, ticket: ticket::Dehydrate, _info: info::Dehydrate) -> CResult<()> {
        ticket.pass().map_err(|_| CloudErrorKind::InvalidRequest)
    }
}

/// Connects an account's place and keeps it in sync until the program ends.
pub fn run(account: &Account, api: Api) -> Result<(), String> {
    register(account)?;
    let engine = Engine::new(account, api);
    let _connection = Session::new()
        .connect(&account.folder, Filter(engine.clone()))
        .map_err(|err| format!("Could not connect {}: {err}", account.folder.display()))?;
    let _watcher = engine.watch()?;
    engine.note("connected");
    engine.follow();
    Ok(())
}

/// Keeps every signed-in account in sync, each on its own thread, until the program ends.
pub fn run_all() -> Result<(), String> {
    let settings = crate::load_settings();
    if settings.accounts.is_empty() {
        return Err("No servers yet: run `ownnas-cloud setup --server … --username …`.".into());
    }
    let threads: Vec<_> = settings
        .accounts
        .into_iter()
        .map(|account| {
            std::thread::spawn(move || {
                let started = crate::secret::unprotect(&account.token).and_then(|token| {
                    let trust = ownnas_core::trust::Trust { ca_pem: account.ca_pem.as_ref().map(|pem| pem.as_bytes().to_vec()), pinned: account.pinned.clone() };
                    let api = Api::with_trust(&account.server, Some(token), &trust)?;
                    run(&account, api)
                });
                if let Err(err) = started {
                    log(&format!("[{}] stopped: {err}", account.name));
                }
            })
        })
        .collect();
    for thread in threads {
        let _ = thread.join();
    }
    Ok(())
}
