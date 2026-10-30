//! One server's folder, as a FUSE filesystem:
//!
//! - Folders list from the server (and from the last listing seen when it can't be reached).
//!   Listings are fresh until the server's change feed says something in them changed (or for
//!   30 seconds, with servers from before the feed).
//! - Opening a file downloads it into the cache once; it is read from there until the server
//!   has a newer version. The least recently used files are dropped when the cache is over its
//!   limit, except changed ones and those to always keep.
//! - Writing goes to the cache; the file is sent when closed (after a moment, so a save that
//!   writes a temporary file and renames it over the real one sends only the real one), over
//!   the version it started from. When the server's changed meanwhile, the local version goes
//!   up as a conflict copy next to it and the file shows the server's version.
//! - Making folders, renaming, moving and deleting (to the server's Trash) happen on the server
//!   at once.

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsStr;
use std::fs::{self, File};
use std::os::unix::fs::FileExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fuser::{
    BackgroundSession, Config, Errno, FileAttr, FileHandle, FileType, Filesystem, FopenFlags, Generation, INodeNo, LockOwner,
    MountOption, OpenAccMode, OpenFlags, RenameFlags, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry,
    ReplyOpen, ReplyStatfs, ReplyWrite, ReplyXattr, Request, TimeOrNow, WriteFlags,
};
use ownnas_core::api::{Api, ApiError};

use crate::{ignored, join, log, split, under, Account, Cached, Index, Item, KEEP_XATTR, STATE_XATTR};

/// How long the kernel may reuse what it was told about a file or folder.
const TTL: Duration = Duration::from_secs(1);
/// Without the change feed (servers from before it), listings are looked at again after this.
const STALE_AFTER: Duration = Duration::from_secs(30);
/// A closed file is sent after this, unless it was renamed or written again meanwhile.
const SETTLE: Duration = Duration::from_secs(2);

const FOLDER: Item = Item { dir: true, size: 0, mtime: 0 };

pub struct Inner {
    api: Api,
    name: String,
    /// The cache: `files/<blob>` and `index.json`.
    cache: PathBuf,
    readonly: bool,
    uid: u32,
    gid: u32,
    limit: u64,
    st: Mutex<St>,
    /// The server answered last time it was asked.
    online: AtomicBool,
    /// The server has a change feed.
    feed: AtomicBool,
    /// The index changed since it was saved.
    unsaved: AtomicBool,
    uploads: Mutex<mpsc::Sender<String>>,
    keeps: Mutex<mpsc::Sender<String>>,
    /// One download of a file at a time.
    fetching: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

struct St {
    index: Index,
    /// Listings fetched since starting, and when.
    fresh: HashMap<String, Instant>,
    paths: HashMap<u64, String>,
    inodes: HashMap<String, u64>,
    next_inode: u64,
    handles: HashMap<u64, Handle>,
    next_handle: u64,
    /// Open handles per file.
    open: HashMap<String, usize>,
}

struct Handle {
    path: String,
    file: Arc<File>,
    wrote: bool,
}

fn errno(err: &ApiError) -> Errno {
    match err {
        ApiError::NotFound => Errno::ENOENT,
        ApiError::Conflict => Errno::EEXIST,
        ApiError::Unauthorized | ApiError::Locked => Errno::EACCES,
        _ => Errno::EIO,
    }
}

fn millis(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

fn now_secs() -> i64 {
    millis(SystemTime::now()) / 1000
}

fn time_of(ms: i64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(ms.max(0) as u64)
}

fn name_of(name: &OsStr) -> Result<&str, Errno> {
    name.to_str().ok_or(Errno::EINVAL)
}

impl Inner {
    fn note(&self, line: &str) {
        log(&format!("[{}] {line}", self.name));
    }

    fn blob_path(&self, blob: u64) -> PathBuf {
        self.cache.join("files").join(blob.to_string())
    }

    fn changed(&self) {
        self.unsaved.store(true, Ordering::Relaxed);
    }

    pub fn save(&self) {
        if !self.unsaved.swap(false, Ordering::Relaxed) {
            return;
        }
        let text = {
            let st = self.st.lock().unwrap();
            serde_json::to_vec(&st.index)
        };
        let saved = text.map_err(|err| err.to_string()).and_then(|text| crate::write_private(&self.cache.join("index.json"), &text));
        if let Err(err) = saved {
            self.note(&format!("could not save the cache's index: {err}"));
            self.changed();
        }
    }

    fn drop_blobs(&self, blobs: Vec<u64>) {
        for blob in blobs {
            let _ = fs::remove_file(self.blob_path(blob));
        }
    }

    // --- Inodes --------------------------------------------------------------------------------

    fn path(&self, ino: INodeNo) -> Result<String, Errno> {
        if ino == INodeNo::ROOT {
            return Ok(String::new());
        }
        self.st.lock().unwrap().paths.get(&u64::from(ino)).cloned().ok_or(Errno::ENOENT)
    }

    fn inode(st: &mut St, path: &str) -> INodeNo {
        if path.is_empty() {
            return INodeNo::ROOT;
        }
        if let Some(&ino) = st.inodes.get(path) {
            return INodeNo(ino);
        }
        st.next_inode += 1;
        let ino = st.next_inode;
        st.inodes.insert(path.to_string(), ino);
        st.paths.insert(ino, path.to_string());
        INodeNo(ino)
    }

    fn attr(&self, ino: INodeNo, item: Item) -> FileAttr {
        let time = time_of(item.mtime);
        let perm = match (item.dir, self.readonly) {
            (true, false) => 0o755,
            (true, true) => 0o555,
            (false, false) => 0o644,
            (false, true) => 0o444,
        };
        FileAttr {
            ino,
            size: item.size,
            blocks: item.size.div_ceil(512),
            atime: time,
            mtime: time,
            ctime: time,
            crtime: time,
            kind: if item.dir { FileType::Directory } else { FileType::RegularFile },
            perm,
            nlink: if item.dir { 2 } else { 1 },
            uid: self.uid,
            gid: self.gid,
            rdev: 0,
            blksize: 4096,
            flags: 0,
        }
    }

    // --- What is where -------------------------------------------------------------------------

    /// Makes sure the folder's listing is known: fresh from the server, or the last one seen
    /// when the server can't be reached.
    fn ensure_listing(&self, dir: &str) -> Result<(), Errno> {
        {
            let st = self.st.lock().unwrap();
            if st.index.listings.contains_key(dir) {
                let fresh = st.fresh.get(dir).is_some_and(|at| self.feed.load(Ordering::Relaxed) || at.elapsed() < STALE_AFTER);
                if fresh || !self.online.load(Ordering::Relaxed) {
                    return Ok(());
                }
            }
        }
        match self.api.list(dir) {
            Ok(folder) => {
                self.online.store(true, Ordering::Relaxed);
                if folder.truncated {
                    self.note(&format!("{dir} has more than the server lists at once: only some of it shows"));
                }
                let entries: BTreeMap<String, Item> = folder
                    .entries
                    .iter()
                    .filter(|entry| !ignored(&entry.name))
                    .map(|entry| {
                        let mtime = if entry.mtime_ms > 0 { entry.mtime_ms } else { entry.modified * 1000 };
                        (entry.name.clone(), Item { dir: entry.dir, size: if entry.dir { 0 } else { entry.size }, mtime })
                    })
                    .collect();
                let mut st = self.st.lock().unwrap();
                st.index.listings.insert(dir.to_string(), entries);
                st.fresh.insert(dir.to_string(), Instant::now());
                self.changed();
                Ok(())
            }
            Err(ApiError::NotFound) => {
                let blobs = self.st.lock().unwrap().index.forget(dir);
                self.drop_blobs(blobs);
                Err(Errno::ENOENT)
            }
            Err(err) => {
                self.note(&format!("could not list {}: {err}", if dir.is_empty() { "the top folder" } else { dir }));
                if matches!(err, ApiError::Other(_)) {
                    self.online.store(false, Ordering::Relaxed);
                }
                if self.st.lock().unwrap().index.listings.contains_key(dir) { Ok(()) } else { Err(errno(&err)) }
            }
        }
    }

    /// A file changed here, as it is in the cache.
    fn local_item(st: &St, cache: &Inner, path: &str) -> Option<Item> {
        let cached = st.index.files.get(path).filter(|cached| cached.dirty)?;
        let meta = fs::metadata(cache.blob_path(cached.blob)).ok()?;
        Some(Item { dir: false, size: meta.len(), mtime: meta.modified().map_or(0, millis) })
    }

    /// What is at `path`: changed here, or as the server listed it.
    fn item(&self, path: &str) -> Result<Option<Item>, Errno> {
        if path.is_empty() {
            return Ok(Some(FOLDER));
        }
        if let Some(item) = Self::local_item(&self.st.lock().unwrap(), self, path) {
            return Ok(Some(item));
        }
        match self.ensure_listing(split(path).0) {
            Ok(()) => {}
            Err(err) if err == Errno::ENOENT => return Ok(None),
            Err(err) => return Err(err),
        }
        Ok(self.st.lock().unwrap().index.listed(path))
    }

    /// What is in a folder: as the server listed it, with what changed here.
    fn children(&self, dir: &str) -> Result<Vec<(String, Item)>, Errno> {
        self.ensure_listing(dir)?;
        let st = self.st.lock().unwrap();
        let mut all: BTreeMap<String, Item> = st.index.listings.get(dir).cloned().unwrap_or_default();
        for (path, cached) in &st.index.files {
            let (parent, name) = split(path);
            if cached.dirty && parent == dir {
                if let Some(item) = Self::local_item(&st, self, path) {
                    all.insert(name.to_string(), item);
                }
            }
        }
        Ok(all.into_iter().collect())
    }

    /// Made here and not on the server (yet).
    fn local_only(&self, path: &str) -> bool {
        self.st.lock().unwrap().index.files.get(path).is_some_and(|cached| cached.server.is_none())
    }

    fn state(&self, path: &str) -> Result<&'static str, Errno> {
        let item = self.item(path)?.ok_or(Errno::ENOENT)?;
        let st = self.st.lock().unwrap();
        let kept = st.index.kept(path);
        if item.dir {
            return Ok(if kept { "kept" } else { "online" });
        }
        Ok(match st.index.files.get(path) {
            Some(cached) if cached.dirty => "uploading",
            Some(cached) if cached.server == Some(item.stamp()) => {
                if kept { "kept" } else { "downloaded" }
            }
            _ => "online",
        })
    }

    // --- Content -------------------------------------------------------------------------------

    /// The cached content is the version the server has (or changed here).
    fn usable(&self, path: &str, item: Item) -> bool {
        let st = self.st.lock().unwrap();
        st.index.files.get(path).is_some_and(|cached| {
            (cached.dirty || cached.server == Some(item.stamp())) && self.blob_path(cached.blob).exists()
        })
    }

    /// Makes sure the file's content is in the cache. `empty`: it is about to be overwritten, so
    /// it starts empty instead of being downloaded.
    fn fill(&self, path: &str, empty: bool) -> Result<(), Errno> {
        let item = self.item(path)?.ok_or(Errno::ENOENT)?;
        if item.dir {
            return Err(Errno::EISDIR);
        }
        if empty {
            let mut st = self.st.lock().unwrap();
            let blob = match st.index.files.get(path) {
                Some(cached) => cached.blob,
                None => st.index.new_blob(),
            };
            File::create(self.blob_path(blob)).map_err(Errno::from)?;
            let server = st.index.files.get(path).map(|cached| cached.server).unwrap_or(Some(item.stamp()));
            st.index.files.insert(path.to_string(), Cached { blob, server, dirty: true, used: now_secs(), size: 0 });
            self.changed();
            return Ok(());
        }
        if self.usable(path, item) {
            if let Some(cached) = self.st.lock().unwrap().index.files.get_mut(path) {
                cached.used = now_secs();
            }
            return Ok(());
        }
        self.download(path)
    }

    fn download(&self, path: &str) -> Result<(), Errno> {
        let lock = self.fetching.lock().unwrap().entry(path.to_string()).or_default().clone();
        let _one = lock.lock().unwrap();
        // Done meanwhile by another open.
        if let Ok(Some(item)) = self.item(path) {
            if self.usable(path, item) {
                return Ok(());
            }
        }
        let (mut reader, stamp) = self.api.download(path).map_err(|err| {
            self.note(&format!("could not download {path}: {err}"));
            errno(&err)
        })?;
        let blob = self.st.lock().unwrap().index.new_blob();
        let part = self.blob_path(blob).with_extension("part");
        let copied = File::create(&part).and_then(|mut out| std::io::copy(&mut reader, &mut out).and_then(|_| out.sync_all()));
        if let Err(err) = copied {
            let _ = fs::remove_file(&part);
            self.note(&format!("could not download {path}: {err}"));
            return Err(Errno::EIO);
        }
        fs::rename(&part, self.blob_path(blob)).map_err(Errno::from)?;
        let mut st = self.st.lock().unwrap();
        if st.index.files.get(path).is_some_and(|cached| cached.dirty) {
            // Changed here meanwhile: that stays.
            drop(st);
            let _ = fs::remove_file(self.blob_path(blob));
            return Ok(());
        }
        let old = st.index.files.insert(path.to_string(), Cached { blob, server: Some(stamp), dirty: false, used: now_secs(), size: stamp.size });
        // The server may have a newer version than its listing said.
        let (parent, name) = split(path);
        if let Some(listing) = st.index.listings.get_mut(parent) {
            listing.insert(name.to_string(), Item { dir: false, size: stamp.size, mtime: stamp.mtime });
        }
        drop(st);
        if let Some(old) = old {
            let _ = fs::remove_file(self.blob_path(old.blob));
        }
        self.changed();
        Ok(())
    }

    fn open_handle(&self, path: &str, write: bool) -> Result<u64, Errno> {
        let mut st = self.st.lock().unwrap();
        let blob = st.index.files.get(path).ok_or(Errno::EIO)?.blob;
        let file = fs::OpenOptions::new().read(true).write(write).open(self.blob_path(blob)).map_err(Errno::from)?;
        st.next_handle += 1;
        let fh = st.next_handle;
        st.handles.insert(fh, Handle { path: path.to_string(), file: Arc::new(file), wrote: false });
        *st.open.entry(path.to_string()).or_default() += 1;
        Ok(fh)
    }

    fn mark_dirty(&self, path: &str) {
        if let Some(cached) = self.st.lock().unwrap().index.files.get_mut(path) {
            cached.dirty = true;
        }
        self.changed();
    }

    fn is_open(&self, path: &str) -> bool {
        self.st.lock().unwrap().open.get(path).is_some_and(|count| *count > 0)
    }

    fn queue_upload(&self, path: &str) {
        let _ = self.uploads.lock().unwrap().send(path.to_string());
    }

    // --- Sending -------------------------------------------------------------------------------

    /// Sends a file changed here, over the version it started from.
    fn upload(&self, path: &str) {
        if self.readonly || path.split('/').any(ignored) || self.is_open(path) {
            return;
        }
        let Some(cached) = self.st.lock().unwrap().index.files.get(path).cloned() else { return };
        if !cached.dirty {
            return;
        }
        let blob = self.blob_path(cached.blob);
        let Ok(meta) = fs::metadata(&blob) else { return };
        let (size, mtime) = (meta.len(), meta.modified().map_or(0, millis));
        let Ok(file) = File::open(&blob) else { return };
        match self.api.upload(path, cached.server, mtime, size, file) {
            Ok(stamp) => {
                self.online.store(true, Ordering::Relaxed);
                let mut st = self.st.lock().unwrap();
                let unchanged = fs::metadata(&blob).is_ok_and(|now| now.len() == size && now.modified().map_or(0, millis) == mtime);
                let open = st.open.get(path).is_some_and(|count| *count > 0);
                if let Some(now) = st.index.files.get_mut(path).filter(|now| now.blob == cached.blob) {
                    now.server = Some(stamp);
                    now.size = size;
                    // Written again while it was sent: that goes next.
                    now.dirty = !unchanged || open;
                }
                let (parent, name) = split(path);
                if let Some(listing) = st.index.listings.get_mut(parent) {
                    listing.insert(name.to_string(), Item { dir: false, size: stamp.size, mtime: stamp.mtime });
                }
                drop(st);
                self.changed();
                self.note(&format!("sent {path}"));
            }
            Err(ApiError::Conflict) => self.keep_both(path, &cached, mtime, size),
            Err(err) => self.note(&format!("could not send {path} (will try again): {err}")),
        }
    }

    /// Changed here and on the server: this computer's version goes up as a conflict copy, and
    /// the file shows the server's.
    fn keep_both(&self, path: &str, cached: &Cached, mtime: i64, size: u64) {
        let copy = ownnas_core::local::conflict_name(path, &ownnas_core::engine::host_name(), SystemTime::now());
        let sent = File::open(self.blob_path(cached.blob))
            .map_err(|err| ApiError::Other(err.to_string()))
            .and_then(|file| self.api.upload(&copy, None, mtime, size, file));
        if let Err(err) = sent {
            return self.note(&format!("{path} changed on the server too, and this computer's version could not be sent as a copy (will try again): {err}"));
        }
        self.note(&format!("{path} changed here and on the server: this computer's version is kept as {copy}"));
        let mut st = self.st.lock().unwrap();
        if st.open.get(path).is_some_and(|count| *count > 0) || st.index.files.get(path).is_none_or(|now| now.blob != cached.blob) {
            return;
        }
        st.index.files.remove(path);
        st.fresh.remove(split(path).0);
        drop(st);
        let _ = fs::remove_file(self.blob_path(cached.blob));
        self.changed();
    }

    /// Sends closed files once they settle; tries what could not be sent again now and then.
    fn send_loop(self: Arc<Self>, uploads: mpsc::Receiver<String>) {
        let mut due: HashMap<String, Instant> = HashMap::new();
        let mut retried = Instant::now();
        loop {
            let wait = due.values().min().map_or(Duration::from_secs(30), |at| at.saturating_duration_since(Instant::now()));
            match uploads.recv_timeout(wait) {
                Ok(path) => {
                    due.insert(path, Instant::now() + SETTLE);
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
            let ready: Vec<String> = due.iter().filter(|(_, at)| **at <= Instant::now()).map(|(path, _)| path.clone()).collect();
            for path in ready {
                due.remove(&path);
                self.upload(&path);
            }
            if retried.elapsed() >= Duration::from_secs(30) {
                retried = Instant::now();
                let dirty: Vec<String> =
                    self.st.lock().unwrap().index.files.iter().filter(|(_, cached)| cached.dirty).map(|(path, _)| path.clone()).collect();
                for path in dirty {
                    if !due.contains_key(&path) {
                        self.upload(&path);
                    }
                }
            }
        }
    }

    // --- Keeping and freeing -------------------------------------------------------------------

    fn keep(&self, path: &str) {
        self.st.lock().unwrap().index.keep.insert(path.to_string());
        self.changed();
        let _ = self.keeps.lock().unwrap().send(path.to_string());
    }

    /// Drops the downloaded files at or in `path` (not changed or open ones), unless a folder it
    /// is in is still to be kept.
    fn free(&self, path: &str) {
        let mut st = self.st.lock().unwrap();
        st.index.keep.retain(|kept| !under(kept, path));
        if st.index.kept(path) {
            return;
        }
        let gone: Vec<String> = st
            .index
            .files
            .iter()
            .filter(|(file, cached)| under(file, path) && !cached.dirty && !st.open.get(*file).is_some_and(|count| *count > 0))
            .map(|(file, _)| file.clone())
            .collect();
        let blobs: Vec<u64> = gone.iter().filter_map(|file| st.index.files.remove(file)).map(|cached| cached.blob).collect();
        drop(st);
        self.drop_blobs(blobs);
        self.changed();
    }

    /// Downloads what is at or in `path` that is not in the cache yet (or is older there).
    fn download_all(&self, path: &str) {
        let Ok(Some(item)) = self.item(path) else { return };
        if !item.dir {
            if !self.usable(path, item) {
                let _ = self.download(path);
            }
            return;
        }
        match self.api.tree(path) {
            Ok(tree) => {
                let mut fetched = 0;
                for (rel, node) in tree.nodes {
                    let file = join(path, &rel);
                    if node.dir || file.split('/').any(ignored) || !self.st.lock().unwrap().index.kept(&file) {
                        continue;
                    }
                    let item = Item { dir: false, size: node.stamp.size, mtime: node.stamp.mtime };
                    if !self.usable(&file, item) && self.download(&file).is_ok() {
                        fetched += 1;
                    }
                }
                if fetched > 0 {
                    self.note(&format!("downloaded {fetched} files to keep in {}", if path.is_empty() { "the top folder" } else { path }));
                }
            }
            Err(err) => self.note(&format!("could not look in {path} to keep it: {err}")),
        }
    }

    fn keep_loop(self: Arc<Self>, keeps: mpsc::Receiver<String>) {
        while let Ok(path) = keeps.recv() {
            if self.st.lock().unwrap().index.kept(&path) {
                self.download_all(&path);
            }
        }
    }

    /// Saves the index now and then, and keeps the cache under its limit.
    fn tidy_loop(self: Arc<Self>) {
        let mut evicted = Instant::now();
        loop {
            std::thread::sleep(Duration::from_secs(5));
            if evicted.elapsed() >= Duration::from_secs(60) {
                evicted = Instant::now();
                let mut st = self.st.lock().unwrap();
                let gone = st.index.evictable(self.limit, |path| st.open.get(path).is_some_and(|count| *count > 0));
                let blobs: Vec<u64> = gone.iter().filter_map(|path| st.index.files.remove(path)).map(|cached| cached.blob).collect();
                drop(st);
                if !blobs.is_empty() {
                    self.drop_blobs(blobs);
                    self.changed();
                }
            }
            self.save();
        }
    }

    // --- The server's changes ------------------------------------------------------------------

    /// Follows the server's change feed for as long as the program runs.
    fn follow(self: Arc<Self>) {
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
        let mut unreachable = false;
        loop {
            let asked = cursor.clone();
            match self.api.get_json::<Feed>("/api/sync/changes", &[("cursor", &asked), ("wait", "25")]) {
                Ok(feed) => {
                    self.online.store(true, Ordering::Relaxed);
                    self.feed.store(true, Ordering::Relaxed);
                    if unreachable {
                        unreachable = false;
                        self.note("connected");
                    }
                    if asked.is_empty() || feed.expired {
                        // Starting, or changes were missed: list everything again when opened.
                        let kept: Vec<String> = {
                            let mut st = self.st.lock().unwrap();
                            st.fresh.clear();
                            st.index.keep.iter().cloned().collect()
                        };
                        for path in kept {
                            let _ = self.keeps.lock().unwrap().send(path);
                        }
                    } else {
                        for change in feed.changes {
                            self.apply(&change.path, change.deleted);
                        }
                    }
                    cursor = feed.cursor;
                }
                Err(ApiError::NotFound) => {
                    // A server from before the change feed: listings go stale on their own.
                    self.online.store(true, Ordering::Relaxed);
                    self.feed.store(false, Ordering::Relaxed);
                    std::thread::sleep(Duration::from_secs(60));
                }
                Err(ApiError::Unauthorized) => {
                    self.note("the server signed this computer out: run `ownnas-mount setup` again");
                    return;
                }
                Err(err) => {
                    self.online.store(false, Ordering::Relaxed);
                    if !unreachable {
                        unreachable = true;
                        self.note(&format!("cannot reach the server (files on this computer still open): {err}"));
                    }
                    cursor.clear();
                    std::thread::sleep(Duration::from_secs(15));
                }
            }
        }
    }

    fn apply(&self, path: &str, deleted: bool) {
        if path.split('/').any(ignored) {
            return;
        }
        let mut st = self.st.lock().unwrap();
        st.fresh.remove(split(path).0);
        if deleted {
            st.fresh.retain(|dir, _| !under(dir, path));
            // Unless changed here: that is sent again (as a new file).
            let blobs: Vec<u64> = {
                let files: Vec<String> =
                    st.index.files.iter().filter(|(file, cached)| under(file, path) && !cached.dirty).map(|(file, _)| file.clone()).collect();
                files.iter().filter_map(|file| st.index.files.remove(file)).map(|cached| cached.blob).collect()
            };
            st.index.listings.retain(|dir, _| !under(dir, path));
            let (parent, name) = split(path);
            if let Some(listing) = st.index.listings.get_mut(parent) {
                listing.remove(name);
            }
            drop(st);
            self.drop_blobs(blobs);
            self.changed();
            return;
        }
        let kept = st.index.kept(path);
        drop(st);
        if kept {
            let _ = self.keeps.lock().unwrap().send(path.to_string());
        }
    }

    /// Forgets `path` here (deleted, or replaced).
    fn forget(&self, path: &str) {
        let mut st = self.st.lock().unwrap();
        let blobs = st.index.forget(path);
        let gone: Vec<String> = st.inodes.keys().filter(|key| under(key, path)).cloned().collect();
        for key in gone {
            if let Some(ino) = st.inodes.remove(&key) {
                st.paths.remove(&ino);
            }
        }
        drop(st);
        self.drop_blobs(blobs);
        self.changed();
    }

    /// Moves what is known about `from` to `to`.
    fn moved(&self, from: &str, to: &str) {
        let mut st = self.st.lock().unwrap();
        let replaced = st.index.rename(from, to);
        let gone: Vec<String> = st.inodes.keys().filter(|key| under(key, to)).cloned().collect();
        for key in gone {
            if let Some(ino) = st.inodes.remove(&key) {
                st.paths.remove(&ino);
            }
        }
        let renamed: Vec<(String, u64)> = st.inodes.iter().filter(|(key, _)| under(key, from)).map(|(key, ino)| (key.clone(), *ino)).collect();
        for (key, ino) in renamed {
            let new = crate::moved(&key, from, to);
            st.inodes.remove(&key);
            st.inodes.insert(new.clone(), ino);
            st.paths.insert(ino, new);
        }
        for handle in st.handles.values_mut() {
            if under(&handle.path, from) {
                handle.path = crate::moved(&handle.path, from, to);
            }
        }
        let open: Vec<(String, usize)> = st.open.iter().filter(|(key, _)| under(key, from)).map(|(key, count)| (key.clone(), *count)).collect();
        for (key, count) in open {
            st.open.remove(&key);
            st.open.insert(crate::moved(&key, from, to), count);
        }
        let fresh: Vec<String> = st.fresh.keys().filter(|key| under(key, from)).cloned().collect();
        for key in fresh {
            if let Some(at) = st.fresh.remove(&key) {
                st.fresh.insert(crate::moved(&key, from, to), at);
            }
        }
        drop(st);
        self.drop_blobs(replaced);
        self.changed();
    }

    /// Renamed or moved here: on the server first (unless it was never there).
    fn rename(&self, from: &str, to: &str, noreplace: bool) -> Result<(), Errno> {
        let source = self.item(from)?.ok_or(Errno::ENOENT)?;
        let target = self.item(to)?;
        if let Some(target) = target {
            if noreplace {
                return Err(Errno::EEXIST);
            }
            match (source.dir, target.dir) {
                (false, true) => return Err(Errno::EISDIR),
                (true, false) => return Err(Errno::ENOTDIR),
                (true, true) if !self.children(to)?.is_empty() => return Err(Errno::ENOTEMPTY),
                _ => {}
            }
        }
        if self.local_only(from) {
            // Only here (such as an editor's temporary file being saved over the real one):
            // the file at the target takes its content, sent over the target's version.
            let base = match target {
                Some(target) if !target.dir => {
                    let st = self.st.lock().unwrap();
                    match st.index.files.get(to) {
                        Some(cached) => cached.server,
                        None => Some(target.stamp()),
                    }
                }
                Some(_) => {
                    // An empty folder replaced by a file.
                    self.trash(to)?;
                    None
                }
                None => None,
            };
            self.moved(from, to);
            if let Some(cached) = self.st.lock().unwrap().index.files.get_mut(to) {
                cached.server = base;
                cached.dirty = true;
            }
            if !self.is_open(to) {
                self.queue_upload(to);
            }
            return Ok(());
        }
        if target.is_some() {
            if self.local_only(to) {
                self.forget(to);
            } else {
                self.trash(to)?;
            }
        }
        let (from_parent, _) = split(from);
        let (to_parent, to_name) = split(to);
        let mut at = from.to_string();
        if from_parent != to_parent {
            at = self.api.move_to(&at, to_parent).map_err(|err| {
                self.note(&format!("could not move {from} on the server: {err}"));
                errno(&err)
            })?;
        }
        if split(&at).1 != to_name {
            at = self.api.rename(&at, to_name).map_err(|err| {
                self.note(&format!("could not rename {from} on the server: {err}"));
                errno(&err)
            })?;
        }
        self.moved(from, to);
        if at != to {
            // The server chose another name: list again.
            self.st.lock().unwrap().fresh.remove(to_parent);
        }
        self.note(&format!("moved {from} to {to}"));
        Ok(())
    }

    /// Moves a file or folder to the server's Trash, and forgets it here.
    fn trash(&self, path: &str) -> Result<(), Errno> {
        match self.api.trash(path) {
            Ok(()) | Err(ApiError::NotFound) => {}
            Err(err) => {
                self.note(&format!("could not delete {path} on the server: {err}"));
                return Err(errno(&err));
            }
        }
        self.forget(path);
        self.note(&format!("moved {path} to the server's Trash"));
        Ok(())
    }
}

/// The filesystem the kernel asks.
pub struct Fs(pub Arc<Inner>);

fn reply_xattr(reply: ReplyXattr, size: u32, value: &[u8]) {
    if size == 0 {
        reply.size(value.len() as u32);
    } else if (size as usize) < value.len() {
        reply.error(Errno::ERANGE);
    } else {
        reply.data(value);
    }
}

impl Fs {
    fn entry(&self, path: &str, reply: ReplyEntry) {
        match self.0.item(path) {
            Ok(Some(item)) => {
                let ino = Inner::inode(&mut self.0.st.lock().unwrap(), path);
                reply.entry(&TTL, &self.0.attr(ino, item), Generation(0));
            }
            Ok(None) => reply.error(Errno::ENOENT),
            Err(err) => reply.error(err),
        }
    }

    fn child(&self, parent: INodeNo, name: &OsStr) -> Result<String, Errno> {
        Ok(join(&self.0.path(parent)?, name_of(name)?))
    }

    fn writable(&self) -> Result<(), Errno> {
        if self.0.readonly { Err(Errno::EROFS) } else { Ok(()) }
    }
}

impl Filesystem for Fs {
    fn lookup(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        match self.child(parent, name) {
            Ok(path) => self.entry(&path, reply),
            Err(err) => reply.error(err),
        }
    }

    fn getattr(&self, _req: &Request, ino: INodeNo, _fh: Option<FileHandle>, reply: ReplyAttr) {
        let result = self.0.path(ino).and_then(|path| self.0.item(&path)?.ok_or(Errno::ENOENT));
        match result {
            Ok(item) => reply.attr(&TTL, &self.0.attr(ino, item)),
            Err(err) => reply.error(err),
        }
    }

    fn setattr(
        &self,
        _req: &Request,
        ino: INodeNo,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<TimeOrNow>,
        mtime: Option<TimeOrNow>,
        _ctime: Option<SystemTime>,
        _fh: Option<FileHandle>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<fuser::BsdFileFlags>,
        reply: ReplyAttr,
    ) {
        let result = (|| {
            let path = self.0.path(ino)?;
            if let Some(size) = size {
                self.writable()?;
                self.0.fill(&path, size == 0)?;
                let blob = self.0.st.lock().unwrap().index.files.get(&path).map(|cached| cached.blob).ok_or(Errno::EIO)?;
                fs::OpenOptions::new().write(true).open(self.0.blob_path(blob)).and_then(|file| file.set_len(size)).map_err(Errno::from)?;
                self.0.mark_dirty(&path);
                if !self.0.is_open(&path) {
                    self.0.queue_upload(&path);
                }
            }
            if let Some(mtime) = mtime {
                // Kept for files changed here (as `cp -p` and `touch` set it); the server keeps
                // its own for the others.
                let blob = self.0.st.lock().unwrap().index.files.get(&path).filter(|cached| cached.dirty).map(|cached| cached.blob);
                if let Some(blob) = blob {
                    let time = match mtime {
                        TimeOrNow::SpecificTime(time) => time,
                        TimeOrNow::Now => SystemTime::now(),
                    };
                    let _ = File::options().write(true).open(self.0.blob_path(blob)).and_then(|file| file.set_modified(time));
                }
            }
            self.0.item(&path)?.ok_or(Errno::ENOENT)
        })();
        match result {
            Ok(item) => reply.attr(&TTL, &self.0.attr(ino, item)),
            Err(err) => reply.error(err),
        }
    }

    fn mkdir(&self, _req: &Request, parent: INodeNo, name: &OsStr, _mode: u32, _umask: u32, reply: ReplyEntry) {
        let result = (|| {
            self.writable()?;
            let path = self.child(parent, name)?;
            let name = name_of(name)?;
            if ignored(name) || name.starts_with(".Trash") {
                // File managers' own (such as their trash for this folder) stay off the server.
                return Err(Errno::EACCES);
            }
            match self.0.api.mkdir(&path) {
                Ok(()) | Err(ApiError::Conflict) => {}
                Err(err) => {
                    self.0.note(&format!("could not make {path}: {err}"));
                    return Err(errno(&err));
                }
            }
            let mut st = self.0.st.lock().unwrap();
            let (parent_path, _) = split(&path);
            if let Some(listing) = st.index.listings.get_mut(parent_path) {
                listing.insert(name.to_string(), FOLDER);
            }
            st.index.listings.insert(path.clone(), BTreeMap::new());
            st.fresh.insert(path.clone(), Instant::now());
            drop(st);
            self.0.changed();
            Ok(path)
        })();
        match result {
            Ok(path) => self.entry(&path, reply),
            Err(err) => reply.error(err),
        }
    }

    fn unlink(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        let result = (|| {
            self.writable()?;
            let path = self.child(parent, name)?;
            if self.0.local_only(&path) {
                self.0.forget(&path);
                return Ok(());
            }
            self.0.trash(&path)
        })();
        match result {
            Ok(()) => reply.ok(),
            Err(err) => reply.error(err),
        }
    }

    fn rmdir(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        let result = (|| {
            self.writable()?;
            let path = self.child(parent, name)?;
            if !self.0.children(&path)?.is_empty() {
                return Err(Errno::ENOTEMPTY);
            }
            self.0.trash(&path)
        })();
        match result {
            Ok(()) => reply.ok(),
            Err(err) => reply.error(err),
        }
    }

    fn rename(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        newparent: INodeNo,
        newname: &OsStr,
        flags: RenameFlags,
        reply: ReplyEmpty,
    ) {
        let result = (|| {
            self.writable()?;
            if flags.contains(RenameFlags::RENAME_EXCHANGE) || flags.contains(RenameFlags::RENAME_WHITEOUT) {
                return Err(Errno::EINVAL);
            }
            let from = self.child(parent, name)?;
            let to = self.child(newparent, newname)?;
            if from == to {
                return Ok(());
            }
            if under(&to, &from) {
                return Err(Errno::EINVAL);
            }
            self.0.rename(&from, &to, flags.contains(RenameFlags::RENAME_NOREPLACE))
        })();
        match result {
            Ok(()) => reply.ok(),
            Err(err) => reply.error(err),
        }
    }

    fn open(&self, _req: &Request, ino: INodeNo, flags: OpenFlags, reply: ReplyOpen) {
        let result = (|| {
            let path = self.0.path(ino)?;
            let write = flags.acc_mode() != OpenAccMode::O_RDONLY;
            if write {
                self.writable()?;
            }
            let truncate = write && flags.0 & libc::O_TRUNC != 0;
            self.0.fill(&path, truncate)?;
            self.0.open_handle(&path, write)
        })();
        match result {
            Ok(fh) => reply.opened(FileHandle(fh), FopenFlags::empty()),
            Err(err) => reply.error(err),
        }
    }

    fn create(&self, _req: &Request, parent: INodeNo, name: &OsStr, _mode: u32, _umask: u32, _flags: i32, reply: ReplyCreate) {
        let result = (|| {
            self.writable()?;
            let path = self.child(parent, name)?;
            let existing = self.0.item(&path)?;
            if existing.is_some_and(|item| item.dir) {
                return Err(Errno::EISDIR);
            }
            {
                let mut st = self.0.st.lock().unwrap();
                let blob = st.index.new_blob();
                File::create(self.0.blob_path(blob)).map_err(Errno::from)?;
                // Over a file the server has (one not listed yet): sent over its version.
                let server = existing.map(|item| item.stamp());
                let old = st.index.files.insert(path.clone(), Cached { blob, server, dirty: true, used: now_secs(), size: 0 });
                if let Some(old) = old {
                    let _ = fs::remove_file(self.0.blob_path(old.blob));
                }
            }
            self.0.changed();
            let fh = self.0.open_handle(&path, true)?;
            let item = self.0.item(&path)?.ok_or(Errno::EIO)?;
            let ino = Inner::inode(&mut self.0.st.lock().unwrap(), &path);
            Ok((self.0.attr(ino, item), fh))
        })();
        match result {
            Ok((attr, fh)) => reply.created(&TTL, &attr, Generation(0), FileHandle(fh), FopenFlags::empty()),
            Err(err) => reply.error(err),
        }
    }

    fn read(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        size: u32,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyData,
    ) {
        let Some(file) = self.0.st.lock().unwrap().handles.get(&u64::from(fh)).map(|handle| handle.file.clone()) else {
            return reply.error(Errno::EBADF);
        };
        let mut buffer = vec![0; size as usize];
        let mut filled = 0;
        while filled < buffer.len() {
            match file.read_at(&mut buffer[filled..], offset + filled as u64) {
                Ok(0) => break,
                Ok(count) => filled += count,
                Err(err) => return reply.error(err.into()),
            }
        }
        reply.data(&buffer[..filled]);
    }

    fn write(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        data: &[u8],
        _write_flags: WriteFlags,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyWrite,
    ) {
        let found = {
            let mut st = self.0.st.lock().unwrap();
            st.handles.get_mut(&u64::from(fh)).map(|handle| {
                handle.wrote = true;
                (handle.file.clone(), handle.path.clone())
            })
        };
        let Some((file, path)) = found else { return reply.error(Errno::EBADF) };
        match file.write_all_at(data, offset) {
            Ok(()) => {
                self.0.mark_dirty(&path);
                reply.written(data.len() as u32);
            }
            Err(err) => reply.error(err.into()),
        }
    }

    fn flush(&self, _req: &Request, _ino: INodeNo, _fh: FileHandle, _lock_owner: LockOwner, reply: ReplyEmpty) {
        reply.ok();
    }

    fn fsync(&self, _req: &Request, _ino: INodeNo, fh: FileHandle, _datasync: bool, reply: ReplyEmpty) {
        let file = self.0.st.lock().unwrap().handles.get(&u64::from(fh)).map(|handle| handle.file.clone());
        match file.map(|file| file.sync_all()) {
            Some(Err(err)) => reply.error(err.into()),
            _ => reply.ok(),
        }
    }

    fn release(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        let send = {
            let mut st = self.0.st.lock().unwrap();
            match st.handles.remove(&u64::from(fh)) {
                Some(handle) => {
                    let left = st.open.get_mut(&handle.path).map_or(0, |count| {
                        *count -= 1;
                        *count
                    });
                    if left == 0 {
                        st.open.remove(&handle.path);
                    }
                    let size = handle.file.metadata().map(|meta| meta.len());
                    let dirty = match st.index.files.get_mut(&handle.path) {
                        Some(cached) => {
                            if let Ok(size) = size {
                                cached.size = size;
                            }
                            cached.dirty
                        }
                        None => false,
                    };
                    // Sent once nothing has it open.
                    (dirty && left == 0).then_some(handle.path)
                }
                None => None,
            }
        };
        reply.ok();
        if let Some(path) = send {
            self.0.queue_upload(&path);
        }
    }

    fn readdir(&self, _req: &Request, ino: INodeNo, _fh: FileHandle, offset: u64, mut reply: ReplyDirectory) {
        let result = (|| {
            let path = self.0.path(ino)?;
            let children = self.0.children(&path)?;
            let mut st = self.0.st.lock().unwrap();
            let parent = Inner::inode(&mut st, split(&path).0);
            let mut all = vec![(ino, FileType::Directory, ".".to_string()), (parent, FileType::Directory, "..".to_string())];
            for (name, item) in children {
                let child = Inner::inode(&mut st, &join(&path, &name));
                all.push((child, if item.dir { FileType::Directory } else { FileType::RegularFile }, name));
            }
            Ok::<_, Errno>(all)
        })();
        match result {
            Ok(all) => {
                for (at, (ino, kind, name)) in all.iter().enumerate().skip(offset as usize) {
                    if reply.add(*ino, (at + 1) as u64, *kind, name) {
                        break;
                    }
                }
                reply.ok();
            }
            Err(err) => reply.error(err),
        }
    }

    fn statfs(&self, _req: &Request, _ino: INodeNo, reply: ReplyStatfs) {
        // The space for downloads: the cache's disk.
        use std::os::unix::ffi::OsStrExt;
        let Ok(path) = std::ffi::CString::new(self.0.cache.as_os_str().as_bytes()) else {
            return reply.statfs(0, 0, 0, 0, 0, 4096, 255, 4096);
        };
        let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(path.as_ptr(), &mut stats) } != 0 {
            return reply.statfs(0, 0, 0, 0, 0, 4096, 255, 4096);
        }
        reply.statfs(
            stats.f_blocks as u64,
            stats.f_bfree as u64,
            stats.f_bavail as u64,
            stats.f_files as u64,
            stats.f_ffree as u64,
            stats.f_bsize as u32,
            255,
            stats.f_frsize as u32,
        );
    }

    fn getxattr(&self, _req: &Request, ino: INodeNo, name: &OsStr, size: u32, reply: ReplyXattr) {
        let result = (|| {
            let path = self.0.path(ino)?;
            match name.to_str() {
                Some(STATE_XATTR) => Ok(self.0.state(&path)?.to_string()),
                Some(KEEP_XATTR) => Ok(if self.0.st.lock().unwrap().index.kept(&path) { "1" } else { "0" }.to_string()),
                _ => Err(Errno::NO_XATTR),
            }
        })();
        match result {
            Ok(value) => reply_xattr(reply, size, value.as_bytes()),
            Err(err) => reply.error(err),
        }
    }

    fn listxattr(&self, _req: &Request, _ino: INodeNo, size: u32, reply: ReplyXattr) {
        reply_xattr(reply, size, format!("{STATE_XATTR}\0{KEEP_XATTR}\0").as_bytes());
    }

    fn setxattr(&self, _req: &Request, ino: INodeNo, name: &OsStr, value: &[u8], _flags: i32, _position: u32, reply: ReplyEmpty) {
        let result = (|| {
            let path = self.0.path(ino)?;
            if name.to_str() != Some(KEEP_XATTR) {
                return Err(Errno::ENOTSUP);
            }
            self.0.item(&path)?.ok_or(Errno::ENOENT)?;
            match value {
                b"1" | b"true" | b"yes" => self.0.keep(&path),
                b"0" | b"false" | b"no" => self.0.free(&path),
                _ => return Err(Errno::EINVAL),
            }
            Ok(())
        })();
        match result {
            Ok(()) => reply.ok(),
            Err(err) => reply.error(err),
        }
    }

    fn removexattr(&self, _req: &Request, ino: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        match (self.0.path(ino), name.to_str()) {
            (Ok(path), Some(KEEP_XATTR)) => {
                self.0.free(&path);
                reply.ok();
            }
            (Err(err), _) => reply.error(err),
            _ => reply.error(Errno::NO_XATTR),
        }
    }
}

/// Mounts one server's folder; it stays mounted until the returned session is dropped.
pub fn start(account: &Account, api: Api, limit: u64) -> Result<(Arc<Inner>, BackgroundSession), String> {
    let cache = crate::cache_dir().join(&account.id);
    fs::create_dir_all(cache.join("files")).map_err(|err| format!("Could not create {}: {err}", cache.display()))?;
    let index: Index = fs::read(cache.join("index.json")).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
    // Left over from a download or a run that stopped: content nothing points to.
    let blobs: std::collections::HashSet<String> = index.files.values().map(|cached| cached.blob.to_string()).collect();
    if let Ok(files) = fs::read_dir(cache.join("files")) {
        for file in files.flatten() {
            if !blobs.contains(&file.file_name().to_string_lossy().into_owned()) {
                let _ = fs::remove_file(file.path());
            }
        }
    }
    let (readonly, online) = match api.account() {
        Ok(me) => (me.readonly, true),
        Err(_) => (false, false),
    };
    unmount_stale(&account.folder);
    fs::create_dir_all(&account.folder).map_err(|err| format!("Could not create {}: {err}", account.folder.display()))?;
    let (uploads, uploads_in) = mpsc::channel();
    let (keeps, keeps_in) = mpsc::channel();
    let inner = Arc::new(Inner {
        api,
        name: account.name.clone(),
        cache,
        readonly,
        uid: unsafe { libc::getuid() },
        gid: unsafe { libc::getgid() },
        limit,
        st: Mutex::new(St {
            index,
            fresh: HashMap::new(),
            paths: HashMap::new(),
            inodes: HashMap::new(),
            next_inode: 1,
            handles: HashMap::new(),
            next_handle: 0,
            open: HashMap::new(),
        }),
        online: AtomicBool::new(online),
        feed: AtomicBool::new(false),
        unsaved: AtomicBool::new(false),
        uploads: Mutex::new(uploads),
        keeps: Mutex::new(keeps),
        fetching: Mutex::new(HashMap::new()),
    });
    let mut config = Config::default();
    config.mount_options = vec![
        MountOption::FSName(format!("ownnas:{}", account.name)),
        MountOption::Subtype("ownnas".into()),
        MountOption::DefaultPermissions,
        MountOption::NoDev,
        MountOption::NoSuid,
        if readonly { MountOption::RO } else { MountOption::RW },
    ];
    config.n_threads = Some(8);
    let session = fuser::spawn_mount(Fs(inner.clone()), &account.folder, &config)
        .map_err(|err| format!("Could not mount {} at {}: {err}", account.name, account.folder.display()))?;
    for (name, work) in [
        ("follow", Box::new({ let inner = inner.clone(); move || inner.follow() }) as Box<dyn FnOnce() + Send>),
        ("send", Box::new({ let inner = inner.clone(); move || inner.send_loop(uploads_in) })),
        ("keep", Box::new({ let inner = inner.clone(); move || inner.keep_loop(keeps_in) })),
        ("tidy", Box::new({ let inner = inner.clone(); move || inner.tidy_loop() })),
    ] {
        std::thread::Builder::new().name(name.into()).spawn(work).map_err(|err| err.to_string())?;
    }
    // What was set to keep, and what was changed but not sent, before the last stop.
    let (kept, dirty): (Vec<String>, Vec<String>) = {
        let st = inner.st.lock().unwrap();
        (st.index.keep.iter().cloned().collect(), st.index.files.iter().filter(|(_, cached)| cached.dirty).map(|(path, _)| path.clone()).collect())
    };
    for path in kept {
        let _ = inner.keeps.lock().unwrap().send(path);
    }
    for path in dirty {
        inner.queue_upload(&path);
    }
    inner.note(&format!("mounted at {}", account.folder.display()));
    Ok((inner, session))
}

/// A mount left by a run that stopped without unmounting answers "Transport endpoint is not
/// connected" until it is unmounted.
fn unmount_stale(folder: &std::path::Path) {
    if let Err(err) = fs::metadata(folder) {
        if err.raw_os_error() == Some(libc::ENOTCONN) {
            for program in ["fusermount3", "fusermount"] {
                if std::process::Command::new(program).arg("-uz").arg(folder).status().is_ok_and(|status| status.success()) {
                    return;
                }
            }
        }
    }
}
