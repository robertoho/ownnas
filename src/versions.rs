//! Earlier versions of files, kept when OwnNAS saves over them: an editor saving, an upload or a
//! sync replacing a file, or a version being restored. The copies live in OwnNAS's data folder
//! (`versions/`), never in the library, and are listed in `file_versions`, which follows the
//! file when it is moved or renamed through OwnNAS (into the Trash and back too).
//!
//! An editor saving every few seconds does not make a version each time: a file keeps at most
//! one version per `SESSION` of saves, the one from before they started.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::UNIX_EPOCH;

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

/// Saves this close together are one session of edits, with one version from before it.
const SESSION: i64 = 10 * 60;
/// Versions kept of one file, newest first.
pub const KEEP_PER_FILE: usize = 20;
/// Versions older than this are removed.
pub const KEEP_DAYS: i64 = 90;
/// Files larger than this are not versioned.
pub const MAX_FILE: u64 = 1024 * 1024 * 1024;
/// When all versions together pass this, the oldest go first.
pub const MAX_TOTAL: u64 = 20 * 1024 * 1024 * 1024;

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Version {
    pub id: i64,
    pub path: String,
    /// When it was replaced (seconds since 1970).
    pub saved_at: i64,
    /// When it had last been changed before that.
    pub modified: i64,
    pub size: u64,
    pub by: String,
    /// What replaced it: "edit", "upload", "sync", or "restore".
    pub reason: String,
}

pub fn store_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("versions")
}

fn blob(store: &Path, id: i64) -> PathBuf {
    store.join(format!("{:02}", id % 100)).join(id.to_string())
}

const COLUMNS: &str = "id, path, saved_at, modified, size, by, reason";

fn row(row: &rusqlite::Row) -> rusqlite::Result<Version> {
    Ok(Version {
        id: row.get(0)?,
        path: row.get(1)?,
        saved_at: row.get(2)?,
        modified: row.get(3)?,
        size: row.get::<_, i64>(4)? as u64,
        by: row.get(5)?,
        reason: row.get(6)?,
    })
}

/// Keeps a copy of the file at `full` (library path `rel`) before it is replaced. Errors are
/// not the save's problem: the save goes ahead without a version.
pub fn keep(conn: &Connection, store: &Path, full: &Path, rel: &str, by: &str, reason: &str) {
    let _ = try_keep(conn, store, full, rel, by, reason, crate::db::now_secs());
}

fn try_keep(conn: &Connection, store: &Path, full: &Path, rel: &str, by: &str, reason: &str, now: i64) -> Result<Option<i64>, String> {
    let Ok(meta) = std::fs::metadata(full) else { return Ok(None) };
    if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_FILE || rel.is_empty() {
        return Ok(None);
    }
    let modified = meta.modified().ok().and_then(|time| time.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as i64);
    // Saved again within a session of edits (by the same person): the version from before it
    // started stands for the whole session. A restore always keeps what it replaces.
    if reason != "restore" {
        let recent: Option<i64> = conn
            .query_row(
                "SELECT saved_at FROM file_versions WHERE path = ?1 AND by = ?2 AND reason = ?3 ORDER BY saved_at DESC LIMIT 1",
                params![rel, by, reason],
                |row| row.get(0),
            )
            .optional()
            .map_err(|err| err.to_string())?;
        if recent.is_some_and(|at| now - at < SESSION) {
            return Ok(None);
        }
    }
    conn.execute(
        "INSERT INTO file_versions(path, saved_at, modified, size, by, reason) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![rel, now, modified, meta.len() as i64, by, reason],
    )
    .map_err(|err| err.to_string())?;
    let id = conn.last_insert_rowid();
    let dest = blob(store, id);
    let copied = dest.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::copy(full, &dest));
    if copied.is_err() {
        let _ = std::fs::remove_file(&dest);
        let _ = conn.execute("DELETE FROM file_versions WHERE id = ?1", params![id]);
        return Err("Could not copy the file".into());
    }
    prune(conn, store, rel, now);
    Ok(Some(id))
}

/// The versions of `rel`, newest first.
pub fn list(conn: &Connection, rel: &str) -> Result<Vec<Version>, String> {
    let mut stmt = conn
        .prepare(&format!("SELECT {COLUMNS} FROM file_versions WHERE path = ?1 ORDER BY saved_at DESC, id DESC"))
        .map_err(|err| err.to_string())?;
    let rows = stmt.query_map(params![rel], row).map_err(|err| err.to_string())?;
    Ok(rows.filter_map(Result::ok).collect())
}

pub fn get(conn: &Connection, id: i64) -> Option<Version> {
    conn.query_row(&format!("SELECT {COLUMNS} FROM file_versions WHERE id = ?1"), params![id], row).optional().ok().flatten()
}

/// Where the copy of a version is.
pub fn file(store: &Path, id: i64) -> PathBuf {
    blob(store, id)
}

/// Puts a version back in place of the file (which becomes a version itself).
pub fn restore(conn: &Connection, store: &Path, root: &Path, id: i64, by: &str) -> Result<Version, String> {
    let version = get(conn, id).ok_or("That version is gone")?;
    let source = blob(store, id);
    if !source.is_file() {
        return Err("That version's copy is missing".into());
    }
    let target = crate::files::resolve(root, &version.path).map(|found| found.full).or_else(|err| match err {
        // Deleted since: put it back where it was.
        crate::files::FileError::NotFound => Ok(root.join(&version.path)),
        _ => Err("That file can no longer be written".to_string()),
    })?;
    if target.is_dir() {
        return Err("A folder now has that name".into());
    }
    let parent = target.parent().ok_or("That file can no longer be written")?;
    std::fs::create_dir_all(parent).map_err(|_| "Could not create its folder".to_string())?;
    let _ = try_keep(conn, store, &target, &version.path, by, "restore", crate::db::now_secs());
    let temp = parent.join(format!(".ownnas-restore-{}-{}.tmp", std::process::id(), rand::random::<u64>()));
    std::fs::copy(&source, &temp).map_err(|_| "Could not copy the version".to_string())?;
    if let Ok(file) = std::fs::OpenOptions::new().write(true).open(&temp) {
        let _ = file.set_modified(UNIX_EPOCH + std::time::Duration::from_secs(version.modified.max(0) as u64));
    }
    std::fs::rename(&temp, &target).map_err(|_| {
        let _ = std::fs::remove_file(&temp);
        "Could not put the version back".to_string()
    })?;
    Ok(version)
}

pub fn delete(conn: &Connection, store: &Path, id: i64) -> Result<(), String> {
    conn.execute("DELETE FROM file_versions WHERE id = ?1", params![id]).map_err(|err| err.to_string())?;
    let _ = std::fs::remove_file(blob(store, id));
    Ok(())
}

/// How many versions there are, and how much room they take.
pub fn totals(conn: &Connection) -> (u64, u64) {
    conn.query_row("SELECT count(*), coalesce(sum(size), 0) FROM file_versions", [], |row| {
        Ok((row.get::<_, i64>(0)? as u64, row.get::<_, i64>(1)? as u64))
    })
    .unwrap_or((0, 0))
}

static LAST_SWEEP: AtomicI64 = AtomicI64::new(0);

/// Removes versions past the limits (of `rel`, and, at most hourly, of every file), and copies
/// whose versions are gone (deleted with their file).
pub fn prune(conn: &Connection, store: &Path, rel: &str, now: i64) {
    let mut gone: Vec<i64> = Vec::new();
    let ids = |sql: &str, values: &[&dyn rusqlite::ToSql]| -> Vec<i64> {
        conn.prepare(sql)
            .and_then(|mut stmt| stmt.query_map(values, |row| row.get(0)).map(|rows| rows.filter_map(Result::ok).collect()))
            .unwrap_or_default()
    };
    gone.extend(ids(
        "SELECT id FROM file_versions WHERE path = ?1 ORDER BY saved_at DESC, id DESC LIMIT -1 OFFSET ?2",
        &[&rel, &(KEEP_PER_FILE as i64)],
    ));
    let sweep = now - LAST_SWEEP.load(Ordering::Relaxed) >= 3600;
    if sweep {
        LAST_SWEEP.store(now, Ordering::Relaxed);
        gone.extend(ids("SELECT id FROM file_versions WHERE saved_at < ?1", &[&(now - KEEP_DAYS * 86_400)]));
        // Over the room allowed: the oldest go, until it fits.
        let mut total = totals(conn).1;
        if total > MAX_TOTAL {
            let mut stmt = conn.prepare("SELECT id, size FROM file_versions ORDER BY saved_at, id").ok();
            let oldest: Vec<(i64, i64)> = stmt
                .as_mut()
                .and_then(|stmt| stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?))).ok().map(|rows| rows.filter_map(Result::ok).collect()))
                .unwrap_or_default();
            for (id, size) in oldest {
                if total <= MAX_TOTAL {
                    break;
                }
                gone.push(id);
                total = total.saturating_sub(size as u64);
            }
        }
    }
    for id in gone {
        let _ = delete(conn, store, id);
    }
    if sweep {
        sweep_orphans(conn, store);
    }
}

/// Copies no version lists any more (their file was deleted for good).
fn sweep_orphans(conn: &Connection, store: &Path) {
    let Ok(shards) = std::fs::read_dir(store) else { return };
    for shard in shards.flatten() {
        let Ok(files) = std::fs::read_dir(shard.path()) else { continue };
        for file in files.flatten() {
            let Some(id) = file.file_name().to_str().and_then(|name| name.parse::<i64>().ok()) else { continue };
            let listed = conn.query_row("SELECT 1 FROM file_versions WHERE id = ?1", params![id], |_| Ok(())).optional().ok().flatten().is_some();
            if !listed {
                let _ = std::fs::remove_file(file.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Setup {
        dir: PathBuf,
        conn: Connection,
        store: PathBuf,
        root: PathBuf,
    }

    impl Drop for Setup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn setup() -> Setup {
        let dir = std::env::temp_dir().join(format!("ownnas-versions-{}-{}", std::process::id(), rand::random::<u32>()));
        let root = dir.join("lib");
        std::fs::create_dir_all(&root).unwrap();
        let conn = crate::db::open(&dir.join("ownnas.db")).unwrap();
        Setup { store: store_dir(&dir), root: root.canonicalize().unwrap(), dir, conn }
    }

    #[test]
    fn keeps_one_version_per_session_and_restores_it() {
        let s = setup();
        let file = s.root.join("notes.txt");
        std::fs::write(&file, "first").unwrap();
        let now = 1_800_000_000;
        assert!(try_keep(&s.conn, &s.store, &file, "notes.txt", "ana", "edit", now).unwrap().is_some());
        std::fs::write(&file, "second").unwrap();
        // Saved again a minute later: still the same session.
        assert!(try_keep(&s.conn, &s.store, &file, "notes.txt", "ana", "edit", now + 60).unwrap().is_none());
        // Someone else, or later on: a new version.
        assert!(try_keep(&s.conn, &s.store, &file, "notes.txt", "ben", "edit", now + 120).unwrap().is_some());
        std::fs::write(&file, "third").unwrap();
        let versions = list(&s.conn, "notes.txt").unwrap();
        assert_eq!(versions.len(), 2);
        assert_eq!(std::fs::read_to_string(file_of(&s, versions[1].id)).unwrap(), "first");

        restore(&s.conn, &s.store, &s.root, versions[1].id, "ana").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "first");
        let after = list(&s.conn, "notes.txt").unwrap();
        assert_eq!(after.len(), 3, "what was replaced is kept too");
        let kept = after.iter().find(|version| version.reason == "restore").unwrap();
        assert_eq!(std::fs::read_to_string(file_of(&s, kept.id)).unwrap(), "third");
    }

    fn file_of(s: &Setup, id: i64) -> PathBuf {
        file(&s.store, id)
    }

    #[test]
    fn follows_moves_and_goes_with_a_delete() {
        let s = setup();
        std::fs::create_dir_all(s.root.join("a")).unwrap();
        let file = s.root.join("a/doc.txt");
        std::fs::write(&file, "x").unwrap();
        let id = try_keep(&s.conn, &s.store, &file, "a/doc.txt", "ana", "edit", 1_800_000_000).unwrap().unwrap();
        crate::db::rewrite_path_meta(&s.conn, "a", "b").unwrap();
        assert_eq!(list(&s.conn, "b/doc.txt").unwrap().len(), 1);
        crate::db::delete_path_meta(&s.conn, "b").unwrap();
        assert!(list(&s.conn, "b/doc.txt").unwrap().is_empty());
        LAST_SWEEP.store(0, Ordering::Relaxed);
        prune(&s.conn, &s.store, "", 1_800_000_000);
        assert!(!file_of(&s, id).exists(), "its copy is swept");
    }

    #[test]
    fn keeps_at_most_so_many_per_file() {
        let s = setup();
        let file = s.root.join("n.txt");
        std::fs::write(&file, "x").unwrap();
        for i in 0..(KEEP_PER_FILE as i64 + 3) {
            try_keep(&s.conn, &s.store, &file, "n.txt", "ana", "restore", 1_800_000_000 + i).unwrap();
        }
        assert_eq!(list(&s.conn, "n.txt").unwrap().len(), KEEP_PER_FILE);
    }
}
