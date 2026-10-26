//! The name index behind search: every file and folder in the library, with its size, date, and
//! kind, kept in OwnNAS's database so a search does not have to walk the disk. It is a cache:
//! folders whose modified time has not changed since the last scan are not read again (adding,
//! removing, or renaming something inside a folder changes its time), so a scan of an unchanged
//! library costs one look per folder. Changes made through OwnNAS are picked up within seconds,
//! ones made outside it at the next scan (every few minutes), and any folder can be read again
//! in full on request.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::files;
use crate::media_index::Speed;

/// Set once a scan of the whole library has finished: until then, search walks the disk.
pub(crate) const SETTING_COMPLETE: &str = "file_index_complete";
/// How often the library is looked through for changes made outside OwnNAS.
const RESCAN_EVERY: Duration = Duration::from_secs(5 * 60);
/// How long to wait after a change made through OwnNAS, so a burst of uploads is one scan.
const SETTLE: Duration = Duration::from_secs(3);

/// Photo libraries kept as one package (Apple Photos, Aperture) hold hundreds of thousands of
/// files of their own. Their own name is indexed, what is inside them is not.
pub fn is_package(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [".photoslibrary", ".photolibrary", ".aplibrary", ".migratedphotolibrary"].iter().any(|ext| lower.ends_with(ext))
}

/// Whether `rel` is inside such a package (which only a walk of the disk can search).
pub fn inside_package(rel: &str) -> bool {
    rel.split('/').any(is_package)
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub scanning: bool,
    /// What the current scan started from ("" for the whole library).
    pub scope: Option<String>,
    /// Whether the current scan reads every folder again, changed or not.
    pub full: bool,
    /// Folders looked at, and of those read again, in the current (or last) scan.
    pub folders: u64,
    pub folders_read: u64,
    /// Files and folders in the index.
    pub entries: u64,
    /// Whether the whole library has been indexed at least once (search uses the index).
    pub ready: bool,
    pub finished_at: Option<i64>,
    pub took_ms: Option<u64>,
    pub error: Option<String>,
}

/// A scan waiting to run: from `rel`, reading every folder again or only changed ones.
#[derive(Clone, Debug, PartialEq)]
struct Job {
    rel: String,
    full: bool,
}

pub struct FileIndex {
    status: Mutex<Status>,
    queue: Mutex<Vec<Job>>,
    signal: Condvar,
    settling: AtomicBool,
    stop: AtomicBool,
}

impl Default for FileIndex {
    fn default() -> Self {
        Self {
            status: Mutex::new(Status::default()),
            queue: Mutex::new(Vec::new()),
            signal: Condvar::new(),
            settling: AtomicBool::new(false),
            stop: AtomicBool::new(false),
        }
    }
}

impl FileIndex {
    pub fn status(&self) -> Status {
        self.status.lock().unwrap_or_else(|err| err.into_inner()).clone()
    }

    pub fn ready(&self) -> bool {
        self.status.lock().unwrap_or_else(|err| err.into_inner()).ready
    }

    /// Reads `rel` and everything under it again in full (`""` is the whole library). A scan
    /// under way stops and starts over when this asks for more than it covers.
    pub fn reindex(&self, rel: &str) {
        let job = Job { rel: rel.to_string(), full: true };
        {
            let status = self.status.lock().unwrap_or_else(|err| err.into_inner());
            if status.scanning && status.full && status.scope.as_deref().is_some_and(|scope| covers(scope, rel)) {
                return;
            }
        }
        self.push(job);
        // A scan of part of the library would only be redone: stop it and start this one.
        self.stop.store(true, Ordering::SeqCst);
    }

    /// Looks for changes a moment from now: after a change made through OwnNAS.
    pub fn changed(self: &Arc<Self>) {
        if self.settling.swap(true, Ordering::SeqCst) {
            return;
        }
        let index = self.clone();
        std::thread::spawn(move || {
            std::thread::sleep(SETTLE);
            index.settling.store(false, Ordering::SeqCst);
            index.push(Job { rel: String::new(), full: false });
        });
    }

    fn push(&self, job: Job) {
        let mut queue = self.queue.lock().unwrap_or_else(|err| err.into_inner());
        // A full scan of a folder covers a quick look at it, and of anything inside it.
        if queue.iter().any(|queued| covers(&queued.rel, &job.rel) && (queued.full || !job.full)) {
            return;
        }
        queue.retain(|queued| !(covers(&job.rel, &queued.rel) && (job.full || !queued.full)));
        queue.push(job);
        self.signal.notify_all();
    }

    /// Scans in a background thread: at start (only changed folders, after the first time),
    /// every few minutes, and when asked. `speed` says how hard it may work.
    pub fn start(self: &Arc<Self>, db_path: PathBuf, root: PathBuf, speed: impl Fn() -> Speed + Send + 'static) {
        if let Ok(conn) = open(&db_path) {
            let ready = crate::db::get_setting(&conn, SETTING_COMPLETE).ok().flatten().is_some();
            let mut status = self.status.lock().unwrap_or_else(|err| err.into_inner());
            status.ready = ready;
            status.entries = count(&conn);
        }
        self.push(Job { rel: String::new(), full: false });
        let index = self.clone();
        std::thread::Builder::new()
            .name("ownnas-file-index".into())
            .spawn(move || loop {
                let job = {
                    let queue = index.queue.lock().unwrap_or_else(|err| err.into_inner());
                    let (mut queue, _) = index
                        .signal
                        .wait_timeout_while(queue, RESCAN_EVERY, |queue| queue.is_empty())
                        .unwrap_or_else(|err| err.into_inner());
                    if queue.is_empty() { Job { rel: String::new(), full: false } } else { queue.remove(0) }
                };
                index.stop.store(false, Ordering::SeqCst);
                if !index.run(&db_path, &root, &job, &speed) {
                    // Stopped for a reindex: carry on after it.
                    index.push(job);
                }
            })
            .expect("could not start the file index thread");
    }

    /// Returns false when stopped part way.
    fn run(&self, db_path: &Path, root: &Path, job: &Job, speed: &dyn Fn() -> Speed) -> bool {
        let started = Instant::now();
        {
            let mut status = self.status.lock().unwrap_or_else(|err| err.into_inner());
            status.scanning = true;
            status.scope = Some(job.rel.clone());
            status.full = job.full;
            status.folders = 0;
            status.folders_read = 0;
            status.error = None;
        }
        let progress = |folders: u64, read: u64| {
            let mut status = self.status.lock().unwrap_or_else(|err| err.into_inner());
            status.folders = folders;
            status.folders_read = read;
        };
        let pace = |worked: Instant| {
            let rest = speed().rest(worked.elapsed());
            if !rest.is_zero() {
                std::thread::sleep(rest);
            }
        };
        let result = open(db_path).and_then(|conn| {
            let finished = scan(&conn, root, &job.rel, job.full, &progress, &pace, &|| self.stop.load(Ordering::SeqCst))?;
            if finished && job.rel.is_empty() {
                crate::db::set_setting(&conn, SETTING_COMPLETE, Some("1"))?;
            }
            Ok((finished, count(&conn)))
        });
        let mut status = self.status.lock().unwrap_or_else(|err| err.into_inner());
        status.scanning = false;
        match result {
            Ok((finished, entries)) => {
                status.entries = entries;
                if finished {
                    status.finished_at = Some(crate::db::now_secs());
                    status.took_ms = Some(started.elapsed().as_millis() as u64);
                    status.ready |= job.rel.is_empty();
                }
                finished
            }
            Err(err) => {
                status.error = Some(err);
                true
            }
        }
    }
}

/// Whether a scan from `scope` takes in `rel`.
fn covers(scope: &str, rel: &str) -> bool {
    scope.is_empty() || rel == scope || rel.starts_with(&format!("{scope}/"))
}

fn open(db_path: &Path) -> Result<Connection, String> {
    crate::db::connect(db_path).map_err(|err| format!("Could not open the database: {err}"))
}

pub fn count(conn: &Connection) -> u64 {
    conn.query_row("SELECT count(*) FROM file_index WHERE path != ''", [], |row| row.get::<_, i64>(0)).unwrap_or(0) as u64
}

fn parent_of(rel: &str) -> &str {
    rel.rsplit_once('/').map_or("", |(parent, _)| parent)
}

fn name_of(rel: &str) -> &str {
    rel.rsplit_once('/').map_or(rel, |(_, name)| name)
}

/// A modified time to the nanosecond, so a change in the same second as a scan still shows.
fn stamp(time: std::io::Result<std::time::SystemTime>) -> i64 {
    time.ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos() as i64)
}

struct Entry {
    rel: String,
    dir: bool,
    size: i64,
    mtime: i64,
    kind: &'static str,
}

/// Brings the index in line with the library under `start`. Folders whose modified time is the
/// one indexed are not read again, unless `full`. Returns false when asked to stop part way.
fn scan(
    conn: &Connection,
    root: &Path,
    start: &str,
    full: bool,
    progress: &dyn Fn(u64, u64),
    pace: &dyn Fn(Instant),
    stop: &dyn Fn() -> bool,
) -> Result<bool, String> {
    let sql = |err: rusqlite::Error| err.to_string();
    let mut queue = VecDeque::from([start.to_string()]);
    let (mut folders, mut read) = (0u64, 0u64);
    // One transaction per folder. Looks are read-only; before writing, the transaction starts
    // again holding the write lock, so it waits its turn (a read turned write would fail at once
    // if another write came first). None is kept open across folders, nor while resting between
    // them: an open read stops SQLite from emptying its write-ahead log, which then grows
    // without end while the other indexes write.
    let mut tx = conn.unchecked_transaction().map_err(sql)?;
    while let Some(rel) = queue.pop_front() {
        macro_rules! start_writing {
            () => {
                tx.commit().map_err(sql)?;
                tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate).map_err(sql)?;
            };
        }
        if stop() {
            tx.commit().map_err(sql)?;
            return Ok(false);
        }
        let worked = Instant::now();
        folders += 1;
        let full_path = if rel.is_empty() { root.to_path_buf() } else { root.join(&rel) };
        let Ok(meta) = full_path.symlink_metadata() else {
            // Gone: forget it and everything that was in it.
            start_writing!();
            forget(&tx, &rel)?;
            continue;
        };
        if !meta.is_dir() || meta.file_type().is_symlink() {
            continue;
        }
        let mtime = stamp(meta.modified());
        let indexed: Option<i64> = tx
            .query_row("SELECT mtime FROM file_index WHERE path = ?1 AND dir = 1", [&rel], |row| row.get(0))
            .optional()
            .map_err(sql)?;
        let packaged = !rel.is_empty() && is_package(name_of(&rel));
        if !full && indexed == Some(mtime) {
            let mut stmt = tx.prepare("SELECT path FROM file_index WHERE parent = ?1 AND dir = 1").map_err(sql)?;
            let children: Vec<String> = stmt.query_map([&rel], |row| row.get(0)).map_err(sql)?.filter_map(Result::ok).collect();
            queue.extend(children);
        } else {
            read += 1;
            start_writing!();
            let mut found = Vec::new();
            if !packaged {
                if let Ok(entries) = std::fs::read_dir(&full_path) {
                    for entry in entries.flatten() {
                        let name = entry.file_name().to_string_lossy().into_owned();
                        let child = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
                        // Links are not followed: they could lead outside the library or loop.
                        let Ok(meta) = entry.path().symlink_metadata() else { continue };
                        if meta.file_type().is_symlink() {
                            continue;
                        }
                        let dir = meta.is_dir();
                        found.push(Entry {
                            kind: if dir { "folder" } else { files::kind_of(&name) },
                            rel: child,
                            dir,
                            size: if dir { 0 } else { meta.len() as i64 },
                            mtime: stamp(meta.modified()),
                        });
                    }
                }
            }
            let known: HashMap<String, bool> = {
                let mut stmt = tx.prepare("SELECT path, dir FROM file_index WHERE parent = ?1").map_err(sql)?;
                let rows = stmt.query_map([&rel], |row| Ok((row.get(0)?, row.get(1)?))).map_err(sql)?;
                rows.filter_map(Result::ok).collect()
            };
            let present: std::collections::HashSet<(&str, bool)> = found.iter().map(|entry| (entry.rel.as_str(), entry.dir)).collect();
            for (path, was_dir) in &known {
                if !present.contains(&(path.as_str(), *was_dir)) {
                    forget(&tx, path)?;
                }
            }
            {
                let mut stmt = tx
                    .prepare(
                        "INSERT INTO file_index(path, parent, name_lower, dir, size, mtime, kind) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)
                         ON CONFLICT(path) DO UPDATE SET size = excluded.size, kind = excluded.kind,
                           mtime = CASE WHEN file_index.dir = 1 THEN file_index.mtime ELSE excluded.mtime END",
                    )
                    .map_err(sql)?;
                for entry in &found {
                    let name = name_of(&entry.rel);
                    // A new folder gets no time, so it is read in turn.
                    let mtime = if entry.dir { -1 } else { entry.mtime };
                    stmt.execute(params![entry.rel, rel, name.to_lowercase(), entry.dir, entry.size, mtime, entry.kind]).map_err(sql)?;
                }
            }
            // The folder's own row keeps the time it had when read: changed later, it is read again.
            tx.execute(
                "INSERT INTO file_index(path, parent, name_lower, dir, size, mtime, kind) VALUES(?1, ?2, ?3, 1, 0, ?4, 'folder')
                 ON CONFLICT(path) DO UPDATE SET mtime = excluded.mtime",
                params![rel, if rel.is_empty() { None } else { Some(parent_of(&rel)) }, name_of(&rel).to_lowercase(), mtime],
            )
            .map_err(sql)?;
            queue.extend(found.into_iter().filter(|entry| entry.dir).map(|entry| entry.rel));
        }
        tx.commit().map_err(sql)?;
        if folders % 50 == 0 {
            progress(folders, read);
        }
        pace(worked);
        tx = conn.unchecked_transaction().map_err(sql)?;
    }
    tx.commit().map_err(sql)?;
    progress(folders, read);
    Ok(true)
}

/// Removes `rel` and everything under it from the index.
fn forget(conn: &Connection, rel: &str) -> Result<(), String> {
    if rel.is_empty() {
        conn.execute("DELETE FROM file_index", []).map_err(|err| err.to_string())?;
        return Ok(());
    }
    conn.execute(
        "DELETE FROM file_index WHERE path = ?1 OR (path >= ?1 || '/' AND path < ?1 || '0')",
        [rel],
    )
    .map_err(|err| err.to_string())?;
    Ok(())
}

/// A file or folder whose name has the search in it.
pub struct NameHit {
    pub path: String,
    pub dir: bool,
}

/// Files and folders under `rel` whose names have `query` in them (lower case), folders and the
/// shallowest first.
pub fn search_names(conn: &Connection, rel: &str, query: &str, limit: usize) -> Result<Vec<NameHit>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT path, dir FROM file_index
             WHERE path != '' AND instr(name_lower, ?1) > 0
               AND (?2 = '' OR (path >= ?2 || '/' AND path < ?2 || '0'))
             ORDER BY dir DESC, length(path) - length(replace(path, '/', '')), path
             LIMIT ?3",
        )
        .map_err(|err| err.to_string())?;
    let rows = stmt
        .query_map(params![query, rel, limit as i64], |row| Ok(NameHit { path: row.get(0)?, dir: row.get(1)? }))
        .map_err(|err| err.to_string())?;
    Ok(rows.filter_map(Result::ok).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Library {
        root: PathBuf,
        conn: Connection,
    }

    impl Drop for Library {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn library() -> Library {
        let root = std::env::temp_dir().join(format!("ownnas-file-index-{}-{}", std::process::id(), rand::random::<u64>()));
        fs::create_dir_all(root.join("lib")).unwrap();
        let conn = crate::db::open(&root.join("ownnas.db")).unwrap();
        Library { root, conn }
    }

    fn scan_all(library: &Library, start: &str, full: bool) -> (u64, u64) {
        let counts = std::cell::Cell::new((0, 0));
        assert!(scan(&library.conn, &library.root.join("lib"), start, full, &|f, r| counts.set((f, r)), &|_| {}, &|| false).unwrap());
        counts.get()
    }

    fn names(library: &Library, query: &str, rel: &str) -> Vec<String> {
        search_names(&library.conn, rel, query, 100).unwrap().into_iter().map(|hit| hit.path).collect()
    }

    /// Let the clock move on, so a change gives a folder a new time.
    fn next_second() {
        std::thread::sleep(Duration::from_millis(20));
    }

    #[test]
    fn indexes_finds_and_follows_changes() {
        let library = library();
        let lib = library.root.join("lib");
        fs::create_dir_all(lib.join("iPod/iPod Photo Cache")).unwrap();
        fs::create_dir_all(lib.join("Photos Library.photoslibrary/resources/Photo Cache")).unwrap();
        fs::create_dir_all(lib.join("docs/old")).unwrap();
        fs::write(lib.join("docs/old/photo cache notes.txt"), b"hello").unwrap();
        scan_all(&library, "", false);
        assert_eq!(names(&library, "photo cache", ""), ["iPod/iPod Photo Cache", "docs/old/photo cache notes.txt"]);
        assert_eq!(names(&library, "photoslibrary", ""), ["Photos Library.photoslibrary"]);
        assert_eq!(names(&library, "cache", "docs"), ["docs/old/photo cache notes.txt"]);
        assert_eq!(crate::text_index::unread(&library.conn, "", &["text"], 10), ["docs/old/photo cache notes.txt"]);

        // Nothing changed: only the folders' times are looked at.
        let (folders, read) = scan_all(&library, "", false);
        assert_eq!((folders, read), (6, 0));

        next_second();
        fs::rename(lib.join("docs/old"), lib.join("docs/archive")).unwrap();
        fs::write(lib.join("iPod/new photo cache.jpg"), b"x").unwrap();
        let (_, read) = scan_all(&library, "", false);
        assert_eq!(read, 3, "docs, iPod, and the renamed folder");
        assert_eq!(names(&library, "photo cache", ""), ["iPod/iPod Photo Cache", "iPod/new photo cache.jpg", "docs/archive/photo cache notes.txt"]);

        // A full scan of one folder reads it whether or not its time changed.
        let (folders, read) = scan_all(&library, "docs", true);
        assert_eq!((folders, read), (2, 2));

        fs::remove_dir_all(lib.join("docs")).unwrap();
        scan_all(&library, "", false);
        assert_eq!(names(&library, "notes", ""), Vec::<String>::new());
    }

    #[test]
    fn queued_scans_merge() {
        let index = FileIndex::default();
        index.push(Job { rel: "a/b".into(), full: false });
        index.push(Job { rel: "a".into(), full: true });
        index.push(Job { rel: "a/c".into(), full: true });
        index.push(Job { rel: "".into(), full: false });
        let queue = index.queue.lock().unwrap().clone();
        assert_eq!(queue, [Job { rel: "a".into(), full: true }, Job { rel: "".into(), full: false }]);
        assert!(inside_package("x/Photos Library.photoslibrary/y"));
        assert!(!inside_package("x/photos"));
    }
}
