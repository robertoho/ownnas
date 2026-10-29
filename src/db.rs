use rusqlite::{params, Connection};

use crate::auth::{self, SESSION_SECS};

#[derive(Clone)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub password_hash: String,
    pub is_admin: bool,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInfo {
    pub id: i64,
    pub username: String,
    pub is_admin: bool,
    pub created_at: i64,
}

pub fn open(path: &std::path::Path) -> Result<Connection, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|_| format!("Could not create {}", parent.display()))?;
    }
    let conn = Connection::open(path)
        .map_err(|_| format!("Could not open the database at {}", path.display()))?;
    crate::db_watch::watch(&conn);
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|_| "Could not configure SQLite".to_string())?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|_| "Could not enable SQLite WAL mode".to_string())?;
    attach_index(&conn, &index_path(path).to_string_lossy())
        .map_err(|_| format!("Could not open the index database at {}", index_path(path).display()))?;
    tune(&conn).map_err(|_| "Could not configure SQLite".to_string())?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(|_| "Could not enable SQLite foreign keys".to_string())?;
    migrate(&conn)?;
    Ok(conn)
}

/// The second database file, beside the first: what OwnNAS can build again by reading the
/// library (the name and photo indexes, the words in files, photo descriptions, hashes, folder
/// sizes). It takes nearly all the writing, so that can't slow down the rest; it can be deleted
/// to be rebuilt; and backups leave it out. Connections see its tables as their own (it is
/// attached as `idx`), so queries need not say where a table is.
pub const INDEX_FILE: &str = "ownnas-index.db";

/// The tables kept in `INDEX_FILE` (and the full-text table over `content_text`).
const REBUILDABLE: [&str; 6] = ["file_index", "content_text", "media_index", "image_vectors", "file_hashes", "folder_sizes"];

/// `INDEX_FILE` for OwnNAS's database; "<name>-index.db" for one named otherwise (in tests).
pub fn index_path(db: &std::path::Path) -> std::path::PathBuf {
    let stem = db.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    db.with_file_name(format!("{stem}-index.db"))
}

fn attach_index(conn: &Connection, file: &str) -> rusqlite::Result<()> {
    conn.execute("ATTACH DATABASE ?1 AS idx", [file])?;
    conn.query_row("PRAGMA idx.journal_mode = WAL", [], |_| Ok(()))
}

/// The size the write-ahead logs are cut back to once copied into their database. Without a
/// limit a log stays as large as it ever grew.
const WAL_LIMIT: i64 = 64 * 1024 * 1024;

/// What every connection to OwnNAS's databases is set up with: in WAL mode, `NORMAL` commits
/// write the log without waiting for the disk each time (a power cut can lose the last commits,
/// never corrupt the database), and the log is cut back to `WAL_LIMIT` when it is emptied. A
/// larger page cache than SQLite's 2 MB keeps the busy parts of the indexes in memory, and
/// sorting for big queries happens in memory rather than in temporary files.
fn tune(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(&format!(
        "PRAGMA main.synchronous = NORMAL; PRAGMA idx.synchronous = NORMAL;
         PRAGMA main.journal_size_limit = {WAL_LIMIT}; PRAGMA idx.journal_size_limit = {WAL_LIMIT};
         PRAGMA main.cache_size = -8000; PRAGMA idx.cache_size = -24000;
         PRAGMA temp_store = MEMORY;"
    ))
}

/// A further connection to the database (already made by `open`), for background work and
/// anything slow that should not hold up the shared one.
pub fn connect(path: &std::path::Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    crate::db_watch::watch(&conn);
    conn.busy_timeout(std::time::Duration::from_secs(10))?;
    attach_index(&conn, &index_path(path).to_string_lossy())?;
    tune(&conn)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    Ok(conn)
}

/// The files that make up the databases, with their sizes (0 when missing): the database and
/// its write-ahead log, then the index database and its log.
pub fn file_sizes(path: &std::path::Path) -> [(std::path::PathBuf, u64); 4] {
    let index = index_path(path);
    [path.to_path_buf(), wal_of(path), index.clone(), wal_of(&index)]
        .map(|file| {
            let size = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
            (file, size)
        })
}

/// A write-ahead log larger than this is worth a warning: something keeps it from being emptied.
pub const WAL_WARN: u64 = 1024 * 1024 * 1024;

/// One of the database files, for Settings → Health.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DbFile {
    /// "data" (`ownnas.db`) or "index" (`INDEX_FILE`).
    pub role: &'static str,
    pub name: String,
    pub size: u64,
    /// Its write-ahead log.
    pub wal: u64,
    /// Space inside the file no longer used, which compacting gives back to the disk.
    pub unused: u64,
}

pub fn file_report(conn: &Connection, path: &std::path::Path) -> Vec<DbFile> {
    let [(data, size), (_, wal), (index, index_size), (_, index_wal)] = file_sizes(path);
    let unused = |schema: &str| -> u64 {
        let pages: i64 = conn.query_row(&format!("PRAGMA {schema}.freelist_count"), [], |row| row.get(0)).unwrap_or(0);
        let page: i64 = conn.query_row(&format!("PRAGMA {schema}.page_size"), [], |row| row.get(0)).unwrap_or(0);
        (pages * page).max(0) as u64
    };
    let name = |file: &std::path::Path| file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    vec![
        DbFile { role: "data", name: name(&data), size, wal, unused: unused("main") },
        DbFile { role: "index", name: name(&index), size: index_size, wal: index_wal, unused: unused("idx") },
    ]
}

/// Rewrites both databases without their unused space, then empties the logs the rewrite
/// filled. Needs as much free disk as the larger file, and holds up writing meanwhile.
pub fn compact(conn: &Connection, path: &std::path::Path) -> Result<(), String> {
    conn.execute_batch("VACUUM main; VACUUM idx;").map_err(|err| format!("Could not compact the database: {err}"))?;
    checkpoint(path);
    Ok(())
}

fn wal_of(path: &std::path::Path) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("{}-wal", path.display()))
}

/// Copies the write-ahead logs into their databases and empties the log files, if no one is
/// reading from them. Returns the logs' size before, or `None` when they could not be emptied
/// now (they are then copied as far as they can be, and tried again next time).
pub fn checkpoint(path: &std::path::Path) -> Option<u64> {
    let [_, (_, wal), _, (_, index_wal)] = file_sizes(path);
    let conn = Connection::open(path).ok()?;
    // Waits for readers, holding up writers meanwhile: briefly, unless a log has grown large.
    let wait = if wal.max(index_wal) > WAL_LIMIT as u64 * 4 { 3000 } else { 500 };
    conn.busy_timeout(std::time::Duration::from_millis(wait)).ok()?;
    attach_index(&conn, &index_path(path).to_string_lossy()).ok()?;
    let mut emptied = true;
    for (schema, size) in [("main", wal), ("idx", index_wal)] {
        // (busy, pages in the log, pages copied): pages not copied are held by an open read.
        let (busy, pages, copied): (i64, i64, i64) = conn
            .query_row(&format!("PRAGMA {schema}.wal_checkpoint(TRUNCATE)"), [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap_or((1, -1, -1));
        if busy != 0 {
            emptied = false;
            if size > WAL_LIMIT as u64 * 4 {
                let file = if schema == "main" { path.to_path_buf() } else { index_path(path) };
                let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let mut text = format!(
                    "could not empty the write-ahead log of {name} ({} MB, {copied} of {pages} pages copied); open now:",
                    size / (1024 * 1024)
                );
                let open = crate::db_watch::held(std::time::Duration::from_secs(1));
                if open.is_empty() {
                    text.push_str(
                        " no read in OwnNAS. If the pages copied stay the same, another program has this file open \
                         (a SQLite viewer, or a backup, sync, or antivirus tool): close it, or keep it away from the data folder",
                    );
                }
                for line in open {
                    text.push_str("\n    ");
                    text.push_str(&line);
                }
                crate::db_watch::log(&text);
            }
        }
    }
    emptied.then_some(wal + index_wal)
}

/// Empties the write-ahead logs every few minutes, so they cannot grow while a long read holds
/// back SQLite's own checkpoints. And now and then brings the statistics SQLite plans queries
/// with up to date, as the indexes grow (each table sampled, so it stays quick).
pub fn start_checkpoints(path: std::path::PathBuf) {
    let _ = std::thread::Builder::new().name("db-checkpoint".into()).spawn(move || {
        let optimize = || {
            if let Ok(conn) = connect(&path) {
                let _ = conn.execute_batch("PRAGMA analysis_limit = 400; PRAGMA optimize = 0x10002;");
            }
        };
        optimize();
        // Each minute: a read held too long goes in the log, and a log grown large is emptied.
        // Every five minutes the logs are emptied anyway, and the statistics every hour.
        for round in (1..=60).cycle() {
            std::thread::sleep(std::time::Duration::from_secs(60));
            crate::db_watch::report_held();
            let [_, (_, wal), _, (_, index_wal)] = file_sizes(&path);
            if round % 5 == 0 || wal.max(index_wal) > WAL_LIMIT as u64 * 4 {
                checkpoint(&path);
            }
            if round == 60 {
                optimize();
            }
        }
    });
}

/// Moves the rebuildable tables a database from before `INDEX_FILE` still has into it, with
/// their rows (the words read from files and photo descriptions take long to make again), then
/// shrinks the database to what is left. Table by table, each copied before it is dropped: if
/// stopped part way, the next start begins that table again.
fn move_to_index(conn: &Connection) -> Result<(), String> {
    let sql_of = |kind: &str, name: &str| -> Option<String> {
        conn.query_row("SELECT sql FROM main.sqlite_schema WHERE type = ?1 AND name = ?2", [kind, name], |row| row.get(0)).ok()
    };
    let waiting: Vec<&str> = REBUILDABLE.iter().copied().filter(|table| sql_of("table", table).is_some()).collect();
    let fts = sql_of("table", "content_fts").is_some();
    if waiting.is_empty() && !fts {
        return Ok(());
    }
    let started = std::time::Instant::now();
    println!("Database: moving the search and photo indexes to {INDEX_FILE} (once; this can take a few minutes)…");
    let fail = |err: rusqlite::Error| format!("Could not move the indexes to {INDEX_FILE}: {err}");
    for table in waiting {
        let create = sql_of("table", table).unwrap_or_default();
        let Some(rest) = create.strip_prefix("CREATE TABLE ") else { continue };
        let columns: Vec<String> = {
            let mut stmt = conn.prepare("SELECT name FROM pragma_table_info(?1, 'main')").map_err(fail)?;
            let names = stmt.query_map([table], |row| row.get::<_, String>(0)).map_err(fail)?;
            names.filter_map(Result::ok).map(|name| format!("\"{name}\"")).collect()
        };
        let indexes: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT sql FROM main.sqlite_schema WHERE type = 'index' AND tbl_name = ?1 AND sql IS NOT NULL")
                .map_err(fail)?;
            let found = stmt.query_map([table], |row| row.get::<_, String>(0)).map_err(fail)?;
            found.filter_map(Result::ok).filter_map(|sql| sql.strip_prefix("CREATE INDEX ").map(|rest| format!("CREATE INDEX idx.{rest};"))).collect()
        };
        let columns = columns.join(", ");
        conn.execute_batch(&format!(
            "BEGIN;
             DROP TABLE IF EXISTS idx.{table};
             CREATE TABLE idx.{rest};
             INSERT INTO idx.{table}(rowid, {columns}) SELECT rowid, {columns} FROM main.{table};
             {}
             DROP TABLE main.{table};
             COMMIT;",
            indexes.join("\n")
        ))
        .map_err(|err| {
            let _ = conn.execute_batch("ROLLBACK");
            fail(err)
        })?;
    }
    if fts {
        // Built again from its own words, which keeps it in step with `content_text`'s rows.
        let moved = conn.execute_batch(
            "BEGIN;
             DROP TABLE IF EXISTS idx.content_fts;
             CREATE VIRTUAL TABLE idx.content_fts USING fts5(lower, compact, tokenize = 'trigram');
             INSERT INTO idx.content_fts(rowid, lower, compact) SELECT rowid, lower, compact FROM main.content_fts;
             DROP TABLE main.content_fts;
             COMMIT;",
        );
        if moved.is_err() {
            let _ = conn.execute_batch("ROLLBACK");
            // Not worth failing for: the words are read again into a new one.
            let _ = conn.execute_batch("DROP TABLE IF EXISTS main.content_fts; DELETE FROM idx.content_text;");
        }
    }
    // What the tables took is free space in the database file now: give it back to the disk.
    conn.execute_batch("VACUUM main").map_err(fail)?;
    println!("Database: indexes moved in {:.0?}", started.elapsed());
    Ok(())
}

/// The tables in `INDEX_FILE`.
fn index_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS idx.file_index (
            path TEXT PRIMARY KEY,
            parent TEXT,
            name_lower TEXT NOT NULL,
            dir INTEGER NOT NULL,
            size INTEGER NOT NULL,
            mtime INTEGER NOT NULL,
            kind TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx.file_index_parent ON file_index(parent);
        CREATE TABLE IF NOT EXISTS idx.content_text (
            path TEXT NOT NULL UNIQUE,
            size INTEGER NOT NULL,
            mtime INTEGER NOT NULL,
            body TEXT NOT NULL,
            indexed_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS idx.media_index (
            path TEXT PRIMARY KEY,
            kind TEXT NOT NULL,
            size INTEGER NOT NULL,
            mtime INTEGER NOT NULL,
            taken_at INTEGER NOT NULL,
            date_source TEXT NOT NULL,
            width INTEGER,
            height INTEGER,
            indexed_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx.media_index_taken ON media_index(taken_at DESC, path DESC);
        CREATE TABLE IF NOT EXISTS idx.image_vectors (
            path TEXT PRIMARY KEY,
            size INTEGER NOT NULL,
            mtime INTEGER NOT NULL,
            vector BLOB NOT NULL,
            indexed_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS idx.file_hashes (
            path TEXT PRIMARY KEY,
            size INTEGER NOT NULL,
            mtime INTEGER NOT NULL,
            sha256 TEXT NOT NULL,
            hashed_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS idx.folder_sizes (
            path TEXT PRIMARY KEY,
            bytes INTEGER NOT NULL,
            files INTEGER NOT NULL,
            truncated INTEGER NOT NULL,
            max_mtime INTEGER NOT NULL,
            fingerprint TEXT NOT NULL,
            scanned_at INTEGER NOT NULL
        );
        ",
    )
    .map_err(|_| "Could not create the index database's schema".to_string())
}

fn migrate(conn: &Connection) -> Result<(), String> {
    move_to_index(conn)?;
    index_schema(conn)?;
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS users (
            id INTEGER PRIMARY KEY,
            username TEXT NOT NULL UNIQUE COLLATE NOCASE,
            password_hash TEXT NOT NULL,
            created_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS sessions (
            token_hash TEXT PRIMARY KEY,
            user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            expires_at INTEGER NOT NULL,
            created_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS sessions_user ON sessions(user_id);
        CREATE TABLE IF NOT EXISTS bookmarks (
            user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            path TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            PRIMARY KEY (user_id, path)
        );
        CREATE TABLE IF NOT EXISTS recents (
            id INTEGER PRIMARY KEY,
            user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            path TEXT NOT NULL,
            name TEXT NOT NULL,
            opened_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS recents_user ON recents(user_id, opened_at);
        CREATE TABLE IF NOT EXISTS events (
            id INTEGER PRIMARY KEY,
            user_id INTEGER,
            username TEXT,
            action TEXT NOT NULL,
            detail TEXT NOT NULL,
            created_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS file_tags (
            path TEXT NOT NULL,
            tag TEXT NOT NULL COLLATE NOCASE,
            created_at INTEGER NOT NULL,
            PRIMARY KEY (path, tag)
        );
        CREATE INDEX IF NOT EXISTS file_tags_tag ON file_tags(tag COLLATE NOCASE);
        CREATE INDEX IF NOT EXISTS file_tags_path ON file_tags(path);
        CREATE TABLE IF NOT EXISTS file_comments (
            id INTEGER PRIMARY KEY,
            path TEXT NOT NULL,
            user_id INTEGER,
            username TEXT NOT NULL,
            body TEXT NOT NULL,
            created_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS file_comments_path ON file_comments(path, created_at);
        CREATE TABLE IF NOT EXISTS file_fingerprints (
            path TEXT PRIMARY KEY,
            size INTEGER NOT NULL,
            modified INTEGER NOT NULL,
            noted_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS folder_appearance (
            path TEXT PRIMARY KEY,
            color TEXT NOT NULL DEFAULT '',
            icon TEXT NOT NULL DEFAULT 'folder'
        );
        CREATE TABLE IF NOT EXISTS albums (
            id INTEGER PRIMARY KEY,
            parent_id INTEGER REFERENCES albums(id) ON DELETE CASCADE,
            name TEXT NOT NULL,
            cover_path TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS albums_parent ON albums(parent_id);
        CREATE TABLE IF NOT EXISTS album_items (
            album_id INTEGER NOT NULL REFERENCES albums(id) ON DELETE CASCADE,
            path TEXT NOT NULL,
            added_at INTEGER NOT NULL,
            added_by TEXT NOT NULL DEFAULT '',
            PRIMARY KEY (album_id, path)
        );
        CREATE INDEX IF NOT EXISTS album_items_path ON album_items(path);
        CREATE TABLE IF NOT EXISTS face_scans (
            path TEXT PRIMARY KEY,
            size INTEGER NOT NULL,
            mtime INTEGER NOT NULL,
            faces INTEGER NOT NULL,
            scanned_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS people (
            id INTEGER PRIMARY KEY,
            name TEXT,
            hidden INTEGER NOT NULL DEFAULT 0,
            centroid BLOB,
            faces INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS faces (
            id INTEGER PRIMARY KEY,
            path TEXT NOT NULL,
            x REAL NOT NULL,
            y REAL NOT NULL,
            w REAL NOT NULL,
            h REAL NOT NULL,
            score REAL NOT NULL,
            vector BLOB NOT NULL,
            person_id INTEGER REFERENCES people(id) ON DELETE SET NULL,
            manual INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS faces_path ON faces(path);
        CREATE INDEX IF NOT EXISTS faces_person ON faces(person_id);
        CREATE TABLE IF NOT EXISTS timeline_hidden (
            path TEXT PRIMARY KEY,
            hidden_at INTEGER NOT NULL,
            hidden_by TEXT NOT NULL DEFAULT ''
        );
        CREATE TABLE IF NOT EXISTS image_versions (
            sha256 TEXT NOT NULL,
            parent TEXT NOT NULL,
            operation TEXT NOT NULL,
            width INTEGER,
            height INTEGER,
            parent_width INTEGER,
            parent_height INTEGER,
            path TEXT NOT NULL,
            parent_path TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            created_by TEXT NOT NULL DEFAULT '',
            PRIMARY KEY (sha256, parent)
        );
        CREATE INDEX IF NOT EXISTS image_versions_parent ON image_versions(parent);
        CREATE TABLE IF NOT EXISTS backup_blocked (
            sha256 TEXT PRIMARY KEY,
            size INTEGER NOT NULL,
            name TEXT NOT NULL,
            path TEXT NOT NULL,
            taken_at INTEGER,
            preview BLOB,
            blocked_at INTEGER NOT NULL,
            blocked_by TEXT NOT NULL DEFAULT ''
        );
        CREATE TABLE IF NOT EXISTS file_versions (
            id INTEGER PRIMARY KEY,
            path TEXT NOT NULL,
            saved_at INTEGER NOT NULL,
            modified INTEGER NOT NULL,
            size INTEGER NOT NULL,
            by TEXT NOT NULL,
            reason TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS file_versions_path ON file_versions(path, saved_at);
        -- Ids are never reused: links and remembered choices keep naming the same workspace.
        CREATE TABLE IF NOT EXISTS workspaces (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS workspace_folders (
            workspace_id INTEGER NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
            folder TEXT NOT NULL,
            PRIMARY KEY (workspace_id, folder)
        );
        CREATE INDEX IF NOT EXISTS workspace_folders_folder ON workspace_folders(folder);
        ",
    )
    .map_err(|_| "Could not create the database schema".to_string())?;
    ensure_admin_column(conn)?;
    for column in ["lat", "lon"] {
        let has: bool = conn.prepare("SELECT 1 FROM pragma_table_info('media_index') WHERE name = ?1").and_then(|mut stmt| stmt.exists([column])).unwrap_or(false);
        if !has {
            conn.execute(&format!("ALTER TABLE media_index ADD COLUMN {column} REAL"), [])
                .map_err(|_| "Could not update the photo index".to_string())?;
        }
    }
    let has_workspace: bool = conn.prepare("SELECT 1 FROM pragma_table_info('albums') WHERE name = 'workspace_id'").and_then(|mut stmt| stmt.exists([])).unwrap_or(false);
    if !has_workspace {
        // Albums belong to a Gallery workspace, or to none (they show under All).
        conn.execute_batch(
            "ALTER TABLE albums ADD COLUMN workspace_id INTEGER REFERENCES workspaces(id) ON DELETE SET NULL;
             CREATE INDEX IF NOT EXISTS albums_workspace ON albums(workspace_id);",
        )
        .map_err(|_| "Could not update albums".to_string())?;
    }
    let version: i64 = conn
        .query_row("SELECT value FROM settings WHERE key = 'schema_version'", [], |row| row.get::<_, String>(0))
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    if version < 13 {
        // Photo sizes now follow the camera's rotation (portrait photos were stored as landscape):
        // have the next scan read every photo again.
        conn.execute("UPDATE media_index SET mtime = -1 WHERE kind = 'image'", [])
            .map_err(|_| "Could not update the photo index".to_string())?;
    }
    if version < 14 {
        // Videos now have the date they were filmed, and photos and videos where: read them all
        // again on the next scan.
        conn.execute("UPDATE media_index SET mtime = -1", [])
            .map_err(|_| "Could not update the photo index".to_string())?;
    }
    // An index database deleted to be rebuilt: the name index is not complete any more.
    conn.execute(
        "DELETE FROM settings WHERE key = ?1 AND NOT EXISTS (SELECT 1 FROM file_index)",
        [crate::file_index::SETTING_COMPLETE],
    )
    .map_err(|_| "Could not update the settings".to_string())?;
    conn.execute(
        "INSERT INTO settings(key, value) VALUES('schema_version', '14')
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [],
    )
    .map_err(|_| "Could not store the schema version".to_string())?;
    Ok(())
}

fn ensure_admin_column(conn: &Connection) -> Result<(), String> {
    let has_admin: bool = conn
        .prepare("PRAGMA table_info(users)")
        .and_then(|mut stmt| {
            let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
            for name in rows.flatten() {
                if name == "is_admin" {
                    return Ok(true);
                }
            }
            Ok(false)
        })
        .unwrap_or(false);
    if !has_admin {
        conn.execute(
            "ALTER TABLE users ADD COLUMN is_admin INTEGER NOT NULL DEFAULT 0",
            [],
        )
        .map_err(|_| "Could not add the admin column".to_string())?;
    }
    let admins: i64 = conn
        .query_row("SELECT COUNT(*) FROM users WHERE is_admin = 1", [], |row| row.get(0))
        .unwrap_or(0);
    if admins == 0 {
        // Existing installs: promote the oldest account so Settings stays reachable.
        conn.execute(
            "UPDATE users SET is_admin = 1
             WHERE id = (SELECT id FROM users ORDER BY id ASC LIMIT 1)",
            [],
        )
        .map_err(|_| "Could not promote the first administrator".to_string())?;
    }
    Ok(())
}

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn get_setting(conn: &Connection, key: &str) -> Result<Option<String>, String> {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", params![key], |row| row.get(0))
        .map(Some)
        .or_else(|err| match err {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            _ => Err("Could not read a setting".to_string()),
        })
}

pub fn set_setting(conn: &Connection, key: &str, value: Option<&str>) -> Result<(), String> {
    let result = match value {
        Some(value) => conn.execute(
            "INSERT INTO settings(key, value) VALUES(?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        ),
        None => conn.execute("DELETE FROM settings WHERE key = ?1", params![key]),
    };
    result.map(|_| ()).map_err(|_| "Could not save a setting".to_string())
}

pub fn count_users(conn: &Connection) -> Result<i64, String> {
    conn.query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))
        .map_err(|_| "Could not read users".to_string())
}

pub fn list_users(conn: &Connection) -> Result<Vec<String>, String> {
    Ok(list_user_infos(conn)?
        .into_iter()
        .map(|user| user.username)
        .collect())
}

pub fn list_user_infos(conn: &Connection) -> Result<Vec<UserInfo>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, username, is_admin, created_at
             FROM users
             ORDER BY username COLLATE NOCASE",
        )
        .map_err(|_| "Could not read users".to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(UserInfo {
                id: row.get(0)?,
                username: row.get(1)?,
                is_admin: row.get::<_, i64>(2)? != 0,
                created_at: row.get(3)?,
            })
        })
        .map_err(|_| "Could not read users".to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|_| "Could not read users".to_string())
}

pub fn count_admins(conn: &Connection) -> Result<i64, String> {
    conn.query_row("SELECT COUNT(*) FROM users WHERE is_admin = 1", [], |row| row.get(0))
        .map_err(|_| "Could not read administrators".to_string())
}

pub fn create_user(conn: &Connection, username: &str, password: &str) -> Result<(), String> {
    let make_admin = count_users(conn)? == 0;
    create_user_with_admin(conn, username, password, make_admin)
}

pub fn create_user_with_admin(
    conn: &Connection,
    username: &str,
    password: &str,
    is_admin: bool,
) -> Result<(), String> {
    auth::validate_username(username).map_err(|e| e.to_string())?;
    auth::validate_password(password).map_err(|e| e.to_string())?;
    let hash = auth::hash_password(password)?;
    let now = now_secs();
    let admin = if count_users(conn)? == 0 { true } else { is_admin };
    conn.execute(
        "INSERT INTO users(username, password_hash, created_at, is_admin) VALUES(?1, ?2, ?3, ?4)",
        params![username.trim(), hash, now, if admin { 1 } else { 0 }],
    )
    .map_err(|err| {
        if err.to_string().contains("UNIQUE") {
            "That username already exists".to_string()
        } else {
            "Could not save the user".to_string()
        }
    })?;
    Ok(())
}

pub fn find_user(conn: &Connection, username: &str) -> Result<Option<User>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, username, password_hash, is_admin FROM users WHERE username = ?1",
        )
        .map_err(|_| "Could not read users".to_string())?;
    let mut rows = stmt
        .query(params![username.trim()])
        .map_err(|_| "Could not read users".to_string())?;
    if let Some(row) = rows.next().map_err(|_| "Could not read users".to_string())? {
        Ok(Some(User {
            id: row.get(0).map_err(|_| "Could not read users".to_string())?,
            username: row.get(1).map_err(|_| "Could not read users".to_string())?,
            password_hash: row.get(2).map_err(|_| "Could not read users".to_string())?,
            is_admin: row.get::<_, i64>(3).map_err(|_| "Could not read users".to_string())? != 0,
        }))
    } else {
        Ok(None)
    }
}

pub fn set_password(conn: &Connection, username: &str, password: &str) -> Result<(), String> {
    auth::validate_password(password).map_err(|e| e.to_string())?;
    let user = find_user(conn, username)?.ok_or_else(|| "That user does not exist".to_string())?;
    let hash = auth::hash_password(password)?;
    conn.execute(
        "UPDATE users SET password_hash = ?1 WHERE id = ?2",
        params![hash, user.id],
    )
    .map_err(|_| "Could not update the password".to_string())?;
    conn.execute("DELETE FROM sessions WHERE user_id = ?1", params![user.id])
        .map_err(|_| "Could not sign existing sessions out".to_string())?;
    Ok(())
}

pub fn delete_user(conn: &Connection, username: &str) -> Result<(), String> {
    let user = find_user(conn, username)?.ok_or_else(|| "That user does not exist".to_string())?;
    if user.is_admin && count_admins(conn)? <= 1 {
        return Err("Cannot remove the last administrator".into());
    }
    let changed = conn
        .execute("DELETE FROM users WHERE username = ?1", params![username.trim()])
        .map_err(|_| "Could not delete the user".to_string())?;
    if changed == 0 {
        return Err("That user does not exist".into());
    }
    Ok(())
}

pub fn set_user_admin(conn: &Connection, username: &str, is_admin: bool) -> Result<(), String> {
    let user = find_user(conn, username)?.ok_or_else(|| "That user does not exist".to_string())?;
    if user.is_admin && !is_admin && count_admins(conn)? <= 1 {
        return Err("Cannot remove the last administrator".into());
    }
    conn.execute(
        "UPDATE users SET is_admin = ?1 WHERE id = ?2",
        params![if is_admin { 1 } else { 0 }, user.id],
    )
    .map_err(|_| "Could not update the administrator flag".to_string())?;
    Ok(())
}

pub fn create_session(conn: &Connection, user_id: i64) -> Result<String, String> {
    purge_expired(conn)?;
    let token = auth::new_token();
    let now = now_secs();
    conn.execute(
        "INSERT INTO sessions(token_hash, user_id, expires_at, created_at) VALUES(?1, ?2, ?3, ?4)",
        params![auth::token_hash(&token), user_id, now + SESSION_SECS, now],
    )
    .map_err(|_| "Could not create a session".to_string())?;
    Ok(token)
}

pub fn user_for_token(conn: &Connection, token: &str) -> Result<Option<User>, String> {
    let now = now_secs();
    let mut stmt = conn
        .prepare(
            "SELECT u.id, u.username, u.password_hash, u.is_admin
             FROM sessions s
             JOIN users u ON u.id = s.user_id
             WHERE s.token_hash = ?1 AND s.expires_at > ?2",
        )
        .map_err(|_| "Could not read the session".to_string())?;
    let mut rows = stmt
        .query(params![auth::token_hash(token), now])
        .map_err(|_| "Could not read the session".to_string())?;
    if let Some(row) = rows
        .next()
        .map_err(|_| "Could not read the session".to_string())?
    {
        // Sliding expiry: a session (such as a sync client's token) stays valid while in use.
        let _ = conn.execute(
            "UPDATE sessions SET expires_at = ?1 WHERE token_hash = ?2 AND expires_at < ?3",
            params![now + SESSION_SECS, auth::token_hash(token), now + SESSION_SECS / 2],
        );
        Ok(Some(User {
            id: row.get(0).map_err(|_| "Could not read the session".to_string())?,
            username: row.get(1).map_err(|_| "Could not read the session".to_string())?,
            password_hash: row.get(2).map_err(|_| "Could not read the session".to_string())?,
            is_admin: row.get::<_, i64>(3).map_err(|_| "Could not read the session".to_string())? != 0,
        }))
    } else {
        Ok(None)
    }
}

pub fn delete_session(conn: &Connection, token: &str) -> Result<(), String> {
    conn.execute(
        "DELETE FROM sessions WHERE token_hash = ?1",
        params![auth::token_hash(token)],
    )
    .map_err(|_| "Could not sign out".to_string())?;
    Ok(())
}

pub fn delete_other_sessions(conn: &Connection, user_id: i64, keep_token: &str) -> Result<(), String> {
    conn.execute(
        "DELETE FROM sessions WHERE user_id = ?1 AND token_hash != ?2",
        params![user_id, auth::token_hash(keep_token)],
    )
    .map_err(|_| "Could not update sessions".to_string())?;
    Ok(())
}

use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Recent {
    pub path: String,
    pub name: String,
    pub opened_at: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub username: String,
    pub action: String,
    pub detail: String,
    pub created_at: i64,
}

pub fn add_bookmark(conn: &Connection, user_id: i64, path: &str) -> Result<(), String> {
    conn.execute(
        "INSERT INTO bookmarks(user_id, path, created_at) VALUES(?1, ?2, ?3)
         ON CONFLICT(user_id, path) DO NOTHING",
        params![user_id, path, now_secs()],
    )
    .map_err(|_| "Could not save the bookmark".to_string())?;
    Ok(())
}

pub fn remove_bookmark(conn: &Connection, user_id: i64, path: &str) -> Result<(), String> {
    conn.execute(
        "DELETE FROM bookmarks WHERE user_id = ?1 AND path = ?2",
        params![user_id, path],
    )
    .map_err(|_| "Could not remove the bookmark".to_string())?;
    Ok(())
}

pub fn list_bookmarks(conn: &Connection, user_id: i64) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare("SELECT path FROM bookmarks WHERE user_id = ?1 ORDER BY created_at DESC")
        .map_err(|_| "Could not read bookmarks".to_string())?;
    let rows = stmt
        .query_map(params![user_id], |row| row.get(0))
        .map_err(|_| "Could not read bookmarks".to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|_| "Could not read bookmarks".to_string())
}

pub fn touch_recent(conn: &Connection, user_id: i64, path: &str, name: &str) -> Result<(), String> {
    conn.execute(
        "DELETE FROM recents WHERE user_id = ?1 AND path = ?2",
        params![user_id, path],
    )
    .map_err(|_| "Could not update recent files".to_string())?;
    conn.execute(
        "INSERT INTO recents(user_id, path, name, opened_at) VALUES(?1, ?2, ?3, ?4)",
        params![user_id, path, name, now_secs()],
    )
    .map_err(|_| "Could not update recent files".to_string())?;
    conn.execute(
        "DELETE FROM recents WHERE user_id = ?1 AND id NOT IN (
            SELECT id FROM recents WHERE user_id = ?1 ORDER BY opened_at DESC LIMIT 30
        )",
        params![user_id],
    )
    .map_err(|_| "Could not update recent files".to_string())?;
    Ok(())
}

pub fn list_recent(conn: &Connection, user_id: i64) -> Result<Vec<Recent>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT path, name, opened_at FROM recents
             WHERE user_id = ?1 ORDER BY opened_at DESC LIMIT 30",
        )
        .map_err(|_| "Could not read recent files".to_string())?;
    let rows = stmt
        .query_map(params![user_id], |row| {
            Ok(Recent {
                path: row.get(0)?,
                name: row.get(1)?,
                opened_at: row.get(2)?,
            })
        })
        .map_err(|_| "Could not read recent files".to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|_| "Could not read recent files".to_string())
}

pub fn log_event(
    conn: &Connection,
    user_id: Option<i64>,
    username: &str,
    action: &str,
    detail: &str,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO events(user_id, username, action, detail, created_at)
         VALUES(?1, ?2, ?3, ?4, ?5)",
        params![user_id, username, action, detail, now_secs()],
    )
    .map_err(|_| "Could not write the activity log".to_string())?;
    conn.execute(
        "DELETE FROM events WHERE id NOT IN (SELECT id FROM events ORDER BY id DESC LIMIT 300)",
        [],
    )
    .map_err(|_| "Could not trim the activity log".to_string())?;
    Ok(())
}

pub fn list_events(conn: &Connection) -> Result<Vec<Event>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT username, action, detail, created_at FROM events
             ORDER BY id DESC LIMIT 80",
        )
        .map_err(|_| "Could not read the activity log".to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(Event {
                username: row.get::<_, Option<String>>(0)?.unwrap_or_default(),
                action: row.get(1)?,
                detail: row.get(2)?,
                created_at: row.get(3)?,
            })
        })
        .map_err(|_| "Could not read the activity log".to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|_| "Could not read the activity log".to_string())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub id: i64,
    pub username: String,
    pub body: String,
    pub created_at: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Annotations {
    pub path: String,
    pub tags: Vec<String>,
    pub comments: Vec<Comment>,
    pub changed: bool,
    pub noted_at: Option<i64>,
    pub noted_size: Option<i64>,
    pub noted_modified: Option<i64>,
    pub current_size: i64,
    pub current_modified: i64,
}

pub fn normalize_tag(raw: &str) -> Result<String, String> {
    let tag = raw.trim();
    if tag.is_empty() {
        return Err("Enter a tag".into());
    }
    if tag.len() > 48 {
        return Err("Tags must be 48 characters or fewer".into());
    }
    if tag.contains('/') || tag.contains('\0') {
        return Err("That tag contains invalid characters".into());
    }
    Ok(tag.to_string())
}

pub fn normalize_comment(raw: &str) -> Result<String, String> {
    let body = raw.trim();
    if body.is_empty() {
        return Err("Enter a comment".into());
    }
    if body.len() > 4000 {
        return Err("Comments must be 4000 characters or fewer".into());
    }
    Ok(body.to_string())
}

pub fn list_annotations(
    conn: &Connection,
    path: &str,
    current_size: i64,
    current_modified: i64,
) -> Result<Annotations, String> {
    let fingerprint = get_fingerprint(conn, path)?;
    let changed = match &fingerprint {
        Some(fp) => fp.size != current_size || fp.modified != current_modified,
        None => false,
    };
    Ok(Annotations {
        path: path.to_string(),
        tags: list_tags(conn, path)?,
        comments: list_comments(conn, path)?,
        changed,
        noted_at: fingerprint.as_ref().map(|fp| fp.noted_at),
        noted_size: fingerprint.as_ref().map(|fp| fp.size),
        noted_modified: fingerprint.as_ref().map(|fp| fp.modified),
        current_size,
        current_modified,
    })
}

struct Fingerprint {
    size: i64,
    modified: i64,
    noted_at: i64,
}

fn get_fingerprint(conn: &Connection, path: &str) -> Result<Option<Fingerprint>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT size, modified, noted_at FROM file_fingerprints WHERE path = ?1",
        )
        .map_err(|_| "Could not read the file fingerprint".to_string())?;
    let mut rows = stmt
        .query(params![path])
        .map_err(|_| "Could not read the file fingerprint".to_string())?;
    match rows.next().map_err(|_| "Could not read the file fingerprint".to_string())? {
        Some(row) => Ok(Some(Fingerprint {
            size: row.get(0).map_err(|_| "Could not read the file fingerprint".to_string())?,
            modified: row.get(1).map_err(|_| "Could not read the file fingerprint".to_string())?,
            noted_at: row.get(2).map_err(|_| "Could not read the file fingerprint".to_string())?,
        })),
        None => Ok(None),
    }
}

pub fn touch_fingerprint(
    conn: &Connection,
    path: &str,
    size: i64,
    modified: i64,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO file_fingerprints(path, size, modified, noted_at) VALUES(?1, ?2, ?3, ?4)
         ON CONFLICT(path) DO UPDATE SET
            size = excluded.size,
            modified = excluded.modified,
            noted_at = excluded.noted_at",
        params![path, size, modified, now_secs()],
    )
    .map_err(|_| "Could not update the file fingerprint".to_string())?;
    Ok(())
}

pub fn list_tags(conn: &Connection, path: &str) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT tag FROM file_tags WHERE path = ?1 ORDER BY tag COLLATE NOCASE",
        )
        .map_err(|_| "Could not read tags".to_string())?;
    let rows = stmt
        .query_map(params![path], |row| row.get(0))
        .map_err(|_| "Could not read tags".to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|_| "Could not read tags".to_string())
}

pub fn tags_for_paths(
    conn: &Connection,
    paths: &[String],
) -> Result<std::collections::HashMap<String, Vec<String>>, String> {
    let mut map = std::collections::HashMap::new();
    if paths.is_empty() {
        return Ok(map);
    }
    // Keep batches small for SQLite variable limits.
    for chunk in paths.chunks(200) {
        let placeholders = std::iter::repeat("?")
            .take(chunk.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT path, tag FROM file_tags WHERE path IN ({placeholders}) ORDER BY tag COLLATE NOCASE"
        );
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|_| "Could not read tags".to_string())?;
        let mut rows = stmt
            .query(rusqlite::params_from_iter(chunk.iter()))
            .map_err(|_| "Could not read tags".to_string())?;
        while let Some(row) = rows
            .next()
            .map_err(|_| "Could not read tags".to_string())?
        {
            let path: String = row
                .get(0)
                .map_err(|_| "Could not read tags".to_string())?;
            let tag: String = row
                .get(1)
                .map_err(|_| "Could not read tags".to_string())?;
            map.entry(path).or_insert_with(Vec::new).push(tag);
        }
    }
    Ok(map)
}

/// Paths whose tags match `query` (substring, case-insensitive), optionally under a folder.
pub fn search_tags(
    conn: &Connection,
    query: &str,
    under: &str,
    exact: bool,
) -> Result<Vec<(String, String)>, String> {
    let needle = query.trim();
    if needle.is_empty() {
        return Ok(Vec::new());
    }
    let mut stmt = if exact {
        conn.prepare(
            "SELECT path, tag FROM file_tags
             WHERE tag = ?1 COLLATE NOCASE
             ORDER BY path COLLATE NOCASE
             LIMIT 200",
        )
    } else {
        conn.prepare(
            "SELECT path, tag FROM file_tags
             WHERE tag LIKE '%' || ?1 || '%' ESCAPE '\\' COLLATE NOCASE
             ORDER BY path COLLATE NOCASE
             LIMIT 200",
        )
    }
    .map_err(|_| "Could not search tags".to_string())?;
    let pattern = if exact {
        needle.to_string()
    } else {
        escape_like(needle)
    };
    let rows = stmt
        .query_map(params![pattern], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|_| "Could not search tags".to_string())?;
    let mut out = Vec::new();
    for row in rows {
        let (path, tag) = row.map_err(|_| "Could not search tags".to_string())?;
        if under.is_empty()
            || path == under
            || path.starts_with(&format!("{under}/"))
        {
            out.push((path, tag));
        }
        if out.len() >= 100 {
            break;
        }
    }
    Ok(out)
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

pub fn add_tag(conn: &Connection, path: &str, tag: &str) -> Result<String, String> {
    let tag = normalize_tag(tag)?;
    conn.execute(
        "INSERT INTO file_tags(path, tag, created_at) VALUES(?1, ?2, ?3)
         ON CONFLICT(path, tag) DO NOTHING",
        params![path, tag, now_secs()],
    )
    .map_err(|_| "Could not save the tag".to_string())?;
    Ok(tag)
}

pub fn remove_tag(conn: &Connection, path: &str, tag: &str) -> Result<(), String> {
    let tag = normalize_tag(tag)?;
    conn.execute(
        "DELETE FROM file_tags WHERE path = ?1 AND tag = ?2",
        params![path, tag],
    )
    .map_err(|_| "Could not remove the tag".to_string())?;
    Ok(())
}

pub fn list_comments(conn: &Connection, path: &str) -> Result<Vec<Comment>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, username, body, created_at FROM file_comments
             WHERE path = ?1 ORDER BY created_at ASC, id ASC",
        )
        .map_err(|_| "Could not read comments".to_string())?;
    let rows = stmt
        .query_map(params![path], |row| {
            Ok(Comment {
                id: row.get(0)?,
                username: row.get(1)?,
                body: row.get(2)?,
                created_at: row.get(3)?,
            })
        })
        .map_err(|_| "Could not read comments".to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|_| "Could not read comments".to_string())
}

pub fn add_comment(
    conn: &Connection,
    path: &str,
    user_id: i64,
    username: &str,
    body: &str,
) -> Result<Comment, String> {
    let body = normalize_comment(body)?;
    let now = now_secs();
    conn.execute(
        "INSERT INTO file_comments(path, user_id, username, body, created_at)
         VALUES(?1, ?2, ?3, ?4, ?5)",
        params![path, user_id, username, body, now],
    )
    .map_err(|_| "Could not save the comment".to_string())?;
    let id = conn.last_insert_rowid();
    Ok(Comment {
        id,
        username: username.to_string(),
        body,
        created_at: now,
    })
}

pub fn remove_comment(conn: &Connection, id: i64) -> Result<(), String> {
    let changed = conn
        .execute("DELETE FROM file_comments WHERE id = ?1", params![id])
        .map_err(|_| "Could not remove the comment".to_string())?;
    if changed == 0 {
        return Err("Comment not found".into());
    }
    Ok(())
}

/// Move tags/comments (and bookmarks/recents) from one path tree to another.
pub fn rewrite_path_meta(conn: &Connection, from: &str, to: &str) -> Result<(), String> {
    if from.is_empty() || from == to {
        return Ok(());
    }
    delete_path_meta(conn, to)?;
    conn.execute(
        "UPDATE folder_appearance SET path = CASE WHEN path = ?1 THEN ?2 ELSE ?2 || substr(path, length(?1) + 1) END
         WHERE path = ?1 OR (path >= ?1 || '/' AND path < ?1 || '0')",
        params![from, to],
    ).map_err(|_| "Could not move folder appearance".to_string())?;
    let start = (from.len() + 1) as i64;
    conn.execute(
        "UPDATE file_tags SET path = CASE
            WHEN path = ?1 THEN ?2
            ELSE ?2 || substr(path, ?3)
         END
         WHERE path = ?1 OR (path >= ?1 || '/' AND path < ?1 || '0')",
        params![from, to, start],
    )
    .map_err(|_| "Could not update tags".to_string())?;
    conn.execute(
        "UPDATE file_comments SET path = CASE
            WHEN path = ?1 THEN ?2
            ELSE ?2 || substr(path, ?3)
         END
         WHERE path = ?1 OR (path >= ?1 || '/' AND path < ?1 || '0')",
        params![from, to, start],
    )
    .map_err(|_| "Could not update comments".to_string())?;
    conn.execute(
        "UPDATE file_fingerprints SET path = CASE
            WHEN path = ?1 THEN ?2
            ELSE ?2 || substr(path, ?3)
         END
         WHERE path = ?1 OR (path >= ?1 || '/' AND path < ?1 || '0')",
        params![from, to, start],
    )
    .map_err(|_| "Could not update fingerprints".to_string())?;
    conn.execute(
        "UPDATE bookmarks SET path = CASE
            WHEN path = ?1 THEN ?2
            ELSE ?2 || substr(path, ?3)
         END
         WHERE path = ?1 OR (path >= ?1 || '/' AND path < ?1 || '0')",
        params![from, to, start],
    )
    .map_err(|_| "Could not update bookmarks".to_string())?;
    conn.execute(
        "UPDATE recents SET path = CASE
            WHEN path = ?1 THEN ?2
            ELSE ?2 || substr(path, ?3)
         END
         WHERE path = ?1 OR (path >= ?1 || '/' AND path < ?1 || '0')",
        params![from, to, start],
    )
    .map_err(|_| "Could not update recent files".to_string())?;
    if let Some((_, name)) = to.rsplit_once('/') {
        let _ = conn.execute(
            "UPDATE recents SET name = ?1 WHERE path = ?2",
            params![name, to],
        );
    } else if !to.is_empty() {
        let _ = conn.execute(
            "UPDATE recents SET name = ?1 WHERE path = ?2",
            params![to, to],
        );
    }
    // Albums keep the photos they hold, wherever they move (into the Trash and back too).
    for (table, column) in [("album_items", "path"), ("albums", "cover_path"), ("media_index", "path"), ("timeline_hidden", "path"), ("image_vectors", "path"), ("face_scans", "path"), ("faces", "path"), ("file_hashes", "path"), ("content_text", "path"), ("file_versions", "path"), ("workspace_folders", "folder")] {
        conn.execute(
            &format!(
                "UPDATE OR REPLACE {table} SET {column} = CASE WHEN {column} = ?1 THEN ?2 ELSE ?2 || substr({column}, length(?1) + 1) END
                 WHERE {column} = ?1 OR ({column} >= ?1 || '/' AND {column} < ?1 || '0')"
            ),
            params![from, to],
        )
        .map_err(|_| "Could not move album entries".to_string())?;
    }
    invalidate_folder_sizes(conn, from)?;
    invalidate_folder_sizes(conn, to)?;
    Ok(())
}

/// Copy tags/comments from one path tree onto another (for duplicate/copy).
pub fn copy_path_meta(conn: &Connection, from: &str, to: &str) -> Result<(), String> {
    if from.is_empty() || from == to {
        return Ok(());
    }
    conn.execute(
        "INSERT INTO folder_appearance(path, color, icon)
         SELECT CASE WHEN path = ?1 THEN ?2 ELSE ?2 || substr(path, length(?1) + 1) END, color, icon
         FROM folder_appearance WHERE path = ?1 OR (path >= ?1 || '/' AND path < ?1 || '0')
         ON CONFLICT(path) DO UPDATE SET color = excluded.color, icon = excluded.icon",
        params![from, to],
    ).map_err(|_| "Could not copy folder appearance".to_string())?;
    let start = (from.len() + 1) as i64;
    conn.execute(
        "INSERT INTO file_tags(path, tag, created_at)
         SELECT CASE
            WHEN path = ?1 THEN ?2
            ELSE ?2 || substr(path, ?3)
         END, tag, created_at
         FROM file_tags
         WHERE path = ?1 OR (path >= ?1 || '/' AND path < ?1 || '0')
         ON CONFLICT(path, tag) DO NOTHING",
        params![from, to, start],
    )
    .map_err(|_| "Could not copy tags".to_string())?;
    conn.execute(
        "INSERT INTO file_comments(path, user_id, username, body, created_at)
         SELECT CASE
            WHEN path = ?1 THEN ?2
            ELSE ?2 || substr(path, ?3)
         END, user_id, username, body, created_at
         FROM file_comments
         WHERE path = ?1 OR (path >= ?1 || '/' AND path < ?1 || '0')",
        params![from, to, start],
    )
    .map_err(|_| "Could not copy comments".to_string())?;
    conn.execute(
        "INSERT INTO file_fingerprints(path, size, modified, noted_at)
         SELECT CASE
            WHEN path = ?1 THEN ?2
            ELSE ?2 || substr(path, ?3)
         END, size, modified, noted_at
         FROM file_fingerprints
         WHERE path = ?1 OR (path >= ?1 || '/' AND path < ?1 || '0')
         ON CONFLICT(path) DO UPDATE SET
            size = excluded.size,
            modified = excluded.modified,
            noted_at = excluded.noted_at",
        params![from, to, start],
    )
    .map_err(|_| "Could not copy fingerprints".to_string())?;
    // The copy shows in the timeline right away; it is not added to the original's albums.
    conn.execute(
        "INSERT OR REPLACE INTO media_index(path, kind, size, mtime, taken_at, date_source, width, height, indexed_at, lat, lon)
         SELECT CASE WHEN path = ?1 THEN ?2 ELSE ?2 || substr(path, length(?1) + 1) END,
                kind, size, mtime, taken_at, date_source, width, height, indexed_at, lat, lon
         FROM media_index WHERE path = ?1 OR (path >= ?1 || '/' AND path < ?1 || '0')",
        params![from, to],
    )
    .map_err(|_| "Could not copy the photo index".to_string())?;
    Ok(())
}

/// A file's SHA-256 (hex) noted earlier, while it still has the same size and time.
pub fn cached_hash(conn: &Connection, path: &str, size: u64, mtime: i64) -> Option<String> {
    conn.query_row(
        "SELECT sha256 FROM file_hashes WHERE path = ?1 AND size = ?2 AND mtime = ?3",
        params![path, size as i64, mtime],
        |row| row.get(0),
    )
    .ok()
}

pub fn store_hash(conn: &Connection, path: &str, size: u64, mtime: i64, sha256: &str) -> Result<(), String> {
    conn.execute(
        "INSERT INTO file_hashes(path, size, mtime, sha256, hashed_at) VALUES(?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(path) DO UPDATE SET size = excluded.size, mtime = excluded.mtime, sha256 = excluded.sha256, hashed_at = excluded.hashed_at",
        params![path, size as i64, mtime, sha256, now_secs()],
    )
    .map_err(|_| "Could not store the file hash".to_string())?;
    Ok(())
}

/// One photo made from another (smaller, for now), both known by SHA-256. A photo's history is
/// a chain of these, so a phone with the original knows the server has it, resized.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageVersion {
    pub sha256: String,
    pub parent: String,
    /// What was done, as people read it: "Resized to 50%".
    pub operation: String,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub parent_width: Option<i64>,
    pub parent_height: Option<i64>,
    /// Where each was when it was made.
    pub path: String,
    pub parent_path: String,
    pub created_at: i64,
    pub created_by: String,
}

pub fn record_version(conn: &Connection, version: &ImageVersion) -> Result<(), String> {
    conn.execute(
        "INSERT OR REPLACE INTO image_versions(sha256, parent, operation, width, height, parent_width, parent_height, path, parent_path, created_at, created_by)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            version.sha256.to_ascii_lowercase(),
            version.parent.to_ascii_lowercase(),
            version.operation,
            version.width,
            version.height,
            version.parent_width,
            version.parent_height,
            version.path,
            version.parent_path,
            version.created_at,
            version.created_by
        ],
    )
    .map_err(|_| "Could not remember where the photo came from".to_string())?;
    Ok(())
}

fn version_rows(conn: &Connection, sql: &str, sha256: &str) -> Vec<ImageVersion> {
    let Ok(mut stmt) = conn.prepare(sql) else { return Vec::new() };
    stmt.query_map([sha256], |row| {
        Ok(ImageVersion {
            sha256: row.get(0)?,
            parent: row.get(1)?,
            operation: row.get(2)?,
            width: row.get(3)?,
            height: row.get(4)?,
            parent_width: row.get(5)?,
            parent_height: row.get(6)?,
            path: row.get(7)?,
            parent_path: row.get(8)?,
            created_at: row.get(9)?,
            created_by: row.get(10)?,
        })
    })
    .map(|rows| rows.filter_map(Result::ok).collect())
    .unwrap_or_default()
}

const VERSION_COLUMNS: &str = "sha256, parent, operation, width, height, parent_width, parent_height, path, parent_path, created_at, created_by";

/// Where a photo came from: its parent, the parent's parent, and so on (newest step first).
pub fn earlier_versions(conn: &Connection, sha256: &str) -> Vec<ImageVersion> {
    let sql = format!("SELECT {VERSION_COLUMNS} FROM image_versions WHERE sha256 = ?1 ORDER BY created_at DESC LIMIT 1");
    let mut chain: Vec<ImageVersion> = Vec::new();
    let mut at = sha256.to_ascii_lowercase();
    while chain.len() < 32 {
        let Some(step) = version_rows(conn, &sql, &at).into_iter().next() else { break };
        if step.parent == sha256 || chain.iter().any(|seen| seen.parent == step.parent) {
            break;
        }
        at = step.parent.clone();
        chain.push(step);
    }
    chain
}

/// Every photo made from this one, and from those (closest first).
pub fn later_versions(conn: &Connection, sha256: &str) -> Vec<ImageVersion> {
    let sql = format!("SELECT {VERSION_COLUMNS} FROM image_versions WHERE parent = ?1 ORDER BY created_at");
    let mut found: Vec<ImageVersion> = Vec::new();
    let mut queue = vec![sha256.to_ascii_lowercase()];
    while let Some(at) = queue.pop() {
        for step in version_rows(conn, &sql, &at) {
            if found.len() >= 200 || step.sha256 == sha256 || found.iter().any(|seen| seen.sha256 == step.sha256) {
                continue;
            }
            queue.insert(0, step.sha256.clone());
            found.push(step);
        }
    }
    found
}

/// The library paths whose last known contents had this hash, with the size and time they had.
pub fn paths_with_hash(conn: &Connection, sha256: &str) -> Vec<(String, u64, i64)> {
    let Ok(mut stmt) = conn.prepare("SELECT path, size, mtime FROM file_hashes WHERE sha256 = ?1") else { return Vec::new() };
    stmt.query_map([sha256.to_ascii_lowercase()], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64, row.get::<_, i64>(2)?)))
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

/// A photo deleted from the server that phones must not back up again.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockedPhoto {
    pub sha256: String,
    pub size: i64,
    pub name: String,
    /// Where it was when it was deleted.
    pub path: String,
    pub taken_at: Option<i64>,
    pub blocked_at: i64,
    pub blocked_by: String,
}

pub fn block_backup(conn: &Connection, photo: &BlockedPhoto, preview: Option<&[u8]>) -> Result<(), String> {
    conn.execute(
        "INSERT INTO backup_blocked(sha256, size, name, path, taken_at, preview, blocked_at, blocked_by)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(sha256) DO UPDATE SET name = excluded.name, path = excluded.path, taken_at = excluded.taken_at,
           preview = COALESCE(excluded.preview, backup_blocked.preview), blocked_at = excluded.blocked_at, blocked_by = excluded.blocked_by",
        params![photo.sha256, photo.size, photo.name, photo.path, photo.taken_at, preview, photo.blocked_at, photo.blocked_by],
    )
    .map_err(|_| "Could not remember the deleted photo".to_string())?;
    Ok(())
}

/// Whether phones must not back this photo up: it was deleted on the server, or a photo made
/// from it (resized) was.
pub fn is_backup_blocked(conn: &Connection, sha256: &str) -> bool {
    let blocked = |hash: &str| conn.query_row("SELECT 1 FROM backup_blocked WHERE sha256 = ?1", [hash.to_ascii_lowercase()], |_| Ok(())).is_ok();
    blocked(sha256) || later_versions(conn, sha256).iter().any(|version| blocked(&version.sha256))
}

pub fn blocked_backups(conn: &Connection) -> Result<Vec<BlockedPhoto>, String> {
    let mut stmt = conn
        .prepare("SELECT sha256, size, name, path, taken_at, blocked_at, blocked_by FROM backup_blocked ORDER BY blocked_at DESC")
        .map_err(|_| "Could not read the deleted photos".to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(BlockedPhoto {
                sha256: row.get(0)?,
                size: row.get(1)?,
                name: row.get(2)?,
                path: row.get(3)?,
                taken_at: row.get(4)?,
                blocked_at: row.get(5)?,
                blocked_by: row.get(6)?,
            })
        })
        .map_err(|_| "Could not read the deleted photos".to_string())?;
    rows.collect::<Result<_, _>>().map_err(|_| "Could not read the deleted photos".to_string())
}

pub fn blocked_preview(conn: &Connection, sha256: &str) -> Option<Vec<u8>> {
    conn.query_row("SELECT preview FROM backup_blocked WHERE sha256 = ?1", [sha256], |row| row.get::<_, Option<Vec<u8>>>(0)).ok().flatten()
}

/// Lets phones back the photo up again. Returns whether it was blocked.
pub fn unblock_backup(conn: &Connection, sha256: &str) -> Result<bool, String> {
    conn.execute("DELETE FROM backup_blocked WHERE sha256 = ?1", [sha256])
        .map(|n| n > 0)
        .map_err(|_| "Could not change the deleted photos".to_string())
}

pub fn delete_path_meta(conn: &Connection, path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Ok(());
    }
    conn.execute(
        "DELETE FROM folder_appearance WHERE path = ?1 OR (path >= ?1 || '/' AND path < ?1 || '0')",
        params![path],
    ).map_err(|_| "Could not remove folder appearance".to_string())?;
    conn.execute(
        "DELETE FROM file_tags WHERE path = ?1 OR (path >= ?1 || '/' AND path < ?1 || '0')",
        params![path],
    )
    .map_err(|_| "Could not remove tags".to_string())?;
    conn.execute(
        "DELETE FROM file_comments WHERE path = ?1 OR (path >= ?1 || '/' AND path < ?1 || '0')",
        params![path],
    )
    .map_err(|_| "Could not remove comments".to_string())?;
    conn.execute(
        "DELETE FROM file_fingerprints WHERE path = ?1 OR (path >= ?1 || '/' AND path < ?1 || '0')",
        params![path],
    )
    .map_err(|_| "Could not remove fingerprints".to_string())?;
    conn.execute(
        "DELETE FROM bookmarks WHERE path = ?1 OR (path >= ?1 || '/' AND path < ?1 || '0')",
        params![path],
    )
    .map_err(|_| "Could not remove bookmarks".to_string())?;
    conn.execute(
        "DELETE FROM recents WHERE path = ?1 OR (path >= ?1 || '/' AND path < ?1 || '0')",
        params![path],
    )
    .map_err(|_| "Could not remove recent files".to_string())?;
    for (table, column) in [("album_items", "path"), ("media_index", "path"), ("timeline_hidden", "path"), ("image_vectors", "path"), ("face_scans", "path"), ("faces", "path"), ("file_hashes", "path"), ("content_text", "path"), ("file_versions", "path"), ("workspace_folders", "folder")] {
        conn.execute(
            &format!("DELETE FROM {table} WHERE {column} = ?1 OR ({column} >= ?1 || '/' AND {column} < ?1 || '0')"),
            params![path],
        )
        .map_err(|_| "Could not remove album entries".to_string())?;
    }
    conn.execute(
        "UPDATE albums SET cover_path = NULL WHERE cover_path = ?1 OR (cover_path >= ?1 || '/' AND cover_path < ?1 || '0')",
        params![path],
    )
    .map_err(|_| "Could not update album covers".to_string())?;
    // People who lost faces with it: keep their counts right, and drop unnamed ones left empty.
    conn.execute_batch(
        "UPDATE people SET faces = (SELECT count(*) FROM faces WHERE person_id = people.id);
         DELETE FROM people WHERE faces = 0 AND name IS NULL;",
    )
    .map_err(|_| "Could not update people".to_string())?;
    invalidate_folder_sizes(conn, path)?;
    Ok(())
}

#[derive(Clone, Debug)]
pub struct FolderSizeRow {
    pub bytes: u64,
    pub files: u64,
    pub truncated: bool,
    pub max_mtime: i64,
    pub fingerprint: String,
}

pub fn get_folder_size(conn: &Connection, path: &str) -> Result<Option<FolderSizeRow>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT path, bytes, files, truncated, max_mtime, fingerprint, scanned_at
             FROM folder_sizes WHERE path = ?1",
        )
        .map_err(|_| "Could not read folder sizes".to_string())?;
    let mut rows = stmt
        .query(params![path])
        .map_err(|_| "Could not read folder sizes".to_string())?;
    let Some(row) = rows.next().map_err(|_| "Could not read folder sizes".to_string())? else {
        return Ok(None);
    };
    Ok(Some(FolderSizeRow {
        bytes: row.get::<_, i64>(1).map_err(|_| "Could not read folder sizes".to_string())? as u64,
        files: row.get::<_, i64>(2).map_err(|_| "Could not read folder sizes".to_string())? as u64,
        truncated: row.get::<_, i64>(3).map_err(|_| "Could not read folder sizes".to_string())? != 0,
        max_mtime: row.get(4).map_err(|_| "Could not read folder sizes".to_string())?,
        fingerprint: row.get(5).map_err(|_| "Could not read folder sizes".to_string())?,
    }))
}

pub fn upsert_folder_size(
    conn: &Connection,
    path: &str,
    bytes: u64,
    files: u64,
    truncated: bool,
    max_mtime: i64,
    fingerprint: &str,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO folder_sizes(path, bytes, files, truncated, max_mtime, fingerprint, scanned_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(path) DO UPDATE SET
            bytes = excluded.bytes,
            files = excluded.files,
            truncated = excluded.truncated,
            max_mtime = excluded.max_mtime,
            fingerprint = excluded.fingerprint,
            scanned_at = excluded.scanned_at",
        params![
            path,
            bytes as i64,
            files as i64,
            if truncated { 1 } else { 0 },
            max_mtime,
            fingerprint,
            now_secs()
        ],
    )
    .map_err(|_| "Could not save folder size".to_string())?;
    Ok(())
}

fn folder_parent(path: &str) -> Option<String> {
    if path.is_empty() {
        return None;
    }
    match path.rfind('/') {
        Some(idx) => Some(path[..idx].to_string()),
        None => Some(String::new()),
    }
}

/// Drop cached sizes for this path, its descendants, and its ancestors.
/// Call after OwnNAS changes files, or when a folder stamp no longer matches.
pub fn invalidate_folder_sizes(conn: &Connection, path: &str) -> Result<(), String> {
    if path.is_empty() {
        conn.execute("DELETE FROM folder_sizes", [])
            .map_err(|_| "Could not clear folder sizes".to_string())?;
        return Ok(());
    }
    conn.execute(
        "DELETE FROM folder_sizes WHERE path = ?1 OR (path >= ?1 || '/' AND path < ?1 || '0')",
        params![path],
    )
    .map_err(|_| "Could not clear folder sizes".to_string())?;
    let mut cursor = folder_parent(path);
    while let Some(parent) = cursor {
        conn.execute("DELETE FROM folder_sizes WHERE path = ?1", params![parent])
            .map_err(|_| "Could not clear folder sizes".to_string())?;
        cursor = if parent.is_empty() {
            None
        } else {
            folder_parent(&parent)
        };
    }
    Ok(())
}

/// Drop only ancestor caches (keep this path), e.g. before writing a fresh measurement.
pub fn invalidate_folder_size_ancestors(conn: &Connection, path: &str) -> Result<(), String> {
    let mut cursor = folder_parent(path);
    while let Some(parent) = cursor {
        conn.execute("DELETE FROM folder_sizes WHERE path = ?1", params![parent])
            .map_err(|_| "Could not clear folder sizes".to_string())?;
        cursor = if parent.is_empty() {
            None
        } else {
            folder_parent(&parent)
        };
    }
    Ok(())
}

fn purge_expired(conn: &Connection) -> Result<(), String> {
    conn.execute(
        "DELETE FROM sessions WHERE expires_at <= ?1",
        params![now_secs()],
    )
    .map_err(|_| "Could not clean up sessions".to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch_db() -> (PathBuf, Connection) {
        // The clock alone can repeat between tests running side by side (macOS has microseconds).
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "ownnas-db-{}-{}-{}.sqlite",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let conn = open(&path).unwrap();
        (path, conn)
    }

    #[test]
    fn tags_and_comments_follow_path_rewrites() {
        let (path, conn) = scratch_db();
        add_tag(&conn, "photos/a.jpg", "family").unwrap();
        touch_fingerprint(&conn, "photos/a.jpg", 12, 100).unwrap();
        add_tag(&conn, "photos/trip/b.jpg", "travel").unwrap();
        add_comment(&conn, "photos/a.jpg", 1, "admin", "keep this").unwrap();

        let notes = list_annotations(&conn, "photos/a.jpg", 12, 100).unwrap();
        assert!(!notes.changed);
        let changed = list_annotations(&conn, "photos/a.jpg", 99, 100).unwrap();
        assert!(changed.changed);

        rewrite_path_meta(&conn, "photos/a.jpg", "archive/a.jpg").unwrap();
        assert_eq!(list_tags(&conn, "archive/a.jpg").unwrap(), vec!["family".to_string()]);
        assert!(list_tags(&conn, "photos/a.jpg").unwrap().is_empty());
        assert_eq!(list_comments(&conn, "archive/a.jpg").unwrap()[0].body, "keep this");
        assert!(list_annotations(&conn, "archive/a.jpg", 12, 100).unwrap().noted_size.is_some());

        rewrite_path_meta(&conn, "photos", "media").unwrap();
        assert_eq!(list_tags(&conn, "media/trip/b.jpg").unwrap(), vec!["travel".to_string()]);

        copy_path_meta(&conn, "archive/a.jpg", "archive/a copy.jpg").unwrap();
        assert_eq!(list_tags(&conn, "archive/a copy.jpg").unwrap(), vec!["family".to_string()]);

        delete_path_meta(&conn, "archive").unwrap();
        assert!(list_tags(&conn, "archive/a.jpg").unwrap().is_empty());
        assert!(list_comments(&conn, "archive/a.jpg").unwrap().is_empty());
        assert!(list_annotations(&conn, "archive/a.jpg", 12, 100).unwrap().noted_size.is_none());

        let _ = std::fs::remove_file(path);
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct FolderAppearance {
    pub color: String,
    pub icon: String,
}

pub fn folder_appearance_for_paths(conn: &Connection, paths: &[String]) -> Result<std::collections::HashMap<String, FolderAppearance>, String> {
    let mut result = std::collections::HashMap::new();
    for chunk in paths.chunks(200) {
        let placeholders = vec!["?"; chunk.len()].join(",");
        let mut stmt = conn.prepare(&format!("SELECT path, color, icon FROM folder_appearance WHERE path IN ({placeholders})"))
            .map_err(|_| "Could not read folder appearance".to_string())?;
        let rows = stmt.query_map(rusqlite::params_from_iter(chunk.iter()), |row| {
            Ok((row.get::<_, String>(0)?, FolderAppearance { color: row.get(1)?, icon: row.get(2)? }))
        }).map_err(|_| "Could not read folder appearance".to_string())?;
        for row in rows { let (path, appearance) = row.map_err(|_| "Could not read folder appearance".to_string())?; result.insert(path, appearance); }
    }
    Ok(result)
}

pub fn set_folder_appearance(conn: &Connection, paths: &[String], color: &str, icon: &str) -> Result<(), String> {
    if !(color.is_empty() || (color.len() == 7 && color.starts_with('#') && color[1..].bytes().all(|c| c.is_ascii_hexdigit()))) {
        return Err("Choose a valid folder color".into());
    }
    if !["folder", "documents", "photos", "music", "video", "code", "work", "star", "heart", "archive", "cloud"].contains(&icon) {
        return Err("Choose a supported folder icon".into());
    }
    let tx = conn.unchecked_transaction().map_err(|_| "Could not save folder appearance".to_string())?;
    for path in paths {
        if color.is_empty() && icon == "folder" {
            tx.execute("DELETE FROM folder_appearance WHERE path = ?1", params![path])
        } else {
            tx.execute("INSERT INTO folder_appearance(path,color,icon) VALUES(?1,?2,?3) ON CONFLICT(path) DO UPDATE SET color=excluded.color, icon=excluded.icon", params![path, color.to_ascii_lowercase(), icon])
        }.map_err(|_| "Could not save folder appearance".to_string())?;
    }
    tx.commit().map_err(|_| "Could not save folder appearance".to_string())
}

#[cfg(test)]
mod appearance_tests {
    use super::*;
    #[test]
    fn appearance_follows_folder_trees_and_validates_values() {
        let conn = Connection::open_in_memory().unwrap(); attach_index(&conn, ":memory:").unwrap(); migrate(&conn).unwrap();
        let paths = vec!["żółć_100%".to_string(), "żółć_100%/child".to_string()];
        set_folder_appearance(&conn, &paths, "#AB12EF", "photos").unwrap();
        set_folder_appearance(&conn, &["żółćX100Y/child".into()], "#123456", "work").unwrap();
        assert!(set_folder_appearance(&conn, &paths, "red;", "folder").is_err());
        assert!(set_folder_appearance(&conn, &paths, "", "<script>").is_err());
        copy_path_meta(&conn, &paths[0], "copy").unwrap();
        rewrite_path_meta(&conn, &paths[0], ".ownnas-trash/item").unwrap();
        let moved = folder_appearance_for_paths(&conn, &[".ownnas-trash/item/child".into(), "copy/child".into(), "żółćX100Y/child".into()]).unwrap();
        assert_eq!(moved.len(), 3);
        assert_eq!(moved["copy/child"].color, "#ab12ef");
        rewrite_path_meta(&conn, ".ownnas-trash/item", "restored").unwrap();
        delete_path_meta(&conn, "copy").unwrap();
        assert!(folder_appearance_for_paths(&conn, &["copy/child".into()]).unwrap().is_empty());
        assert!(folder_appearance_for_paths(&conn, &["restored/child".into()]).unwrap().contains_key("restored/child"));
        set_folder_appearance(&conn, &["restored/child".into()], "", "folder").unwrap();
        assert!(folder_appearance_for_paths(&conn, &["restored/child".into()]).unwrap().is_empty());
    }
}
