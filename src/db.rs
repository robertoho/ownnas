use rusqlite::{params, Connection};

use crate::auth::{self, SESSION_SECS};

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
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|_| "Could not configure SQLite".to_string())?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|_| "Could not enable SQLite WAL mode".to_string())?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(|_| "Could not enable SQLite foreign keys".to_string())?;
    migrate(&conn)?;
    Ok(conn)
}

fn migrate(conn: &Connection) -> Result<(), String> {
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
        ",
    )
    .map_err(|_| "Could not create the database schema".to_string())?;
    ensure_admin_column(conn)?;
    conn.execute(
        "INSERT INTO settings(key, value) VALUES('schema_version', '5')
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

fn path_like(prefix: &str) -> String {
    format!("{prefix}/%")
}

/// Move tags/comments (and bookmarks/recents) from one path tree to another.
pub fn rewrite_path_meta(conn: &Connection, from: &str, to: &str) -> Result<(), String> {
    if from.is_empty() || from == to {
        return Ok(());
    }
    delete_path_meta(conn, to)?;
    let like = path_like(from);
    let start = (from.len() + 1) as i64;
    conn.execute(
        "UPDATE file_tags SET path = CASE
            WHEN path = ?1 THEN ?2
            ELSE ?2 || substr(path, ?4)
         END
         WHERE path = ?1 OR path LIKE ?3",
        params![from, to, like, start],
    )
    .map_err(|_| "Could not update tags".to_string())?;
    conn.execute(
        "UPDATE file_comments SET path = CASE
            WHEN path = ?1 THEN ?2
            ELSE ?2 || substr(path, ?4)
         END
         WHERE path = ?1 OR path LIKE ?3",
        params![from, to, like, start],
    )
    .map_err(|_| "Could not update comments".to_string())?;
    conn.execute(
        "UPDATE file_fingerprints SET path = CASE
            WHEN path = ?1 THEN ?2
            ELSE ?2 || substr(path, ?4)
         END
         WHERE path = ?1 OR path LIKE ?3",
        params![from, to, like, start],
    )
    .map_err(|_| "Could not update fingerprints".to_string())?;
    conn.execute(
        "UPDATE bookmarks SET path = CASE
            WHEN path = ?1 THEN ?2
            ELSE ?2 || substr(path, ?4)
         END
         WHERE path = ?1 OR path LIKE ?3",
        params![from, to, like, start],
    )
    .map_err(|_| "Could not update bookmarks".to_string())?;
    conn.execute(
        "UPDATE recents SET path = CASE
            WHEN path = ?1 THEN ?2
            ELSE ?2 || substr(path, ?4)
         END
         WHERE path = ?1 OR path LIKE ?3",
        params![from, to, like, start],
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
    Ok(())
}

/// Copy tags/comments from one path tree onto another (for duplicate/copy).
pub fn copy_path_meta(conn: &Connection, from: &str, to: &str) -> Result<(), String> {
    if from.is_empty() || from == to {
        return Ok(());
    }
    let like = path_like(from);
    let start = (from.len() + 1) as i64;
    conn.execute(
        "INSERT INTO file_tags(path, tag, created_at)
         SELECT CASE
            WHEN path = ?1 THEN ?2
            ELSE ?2 || substr(path, ?4)
         END, tag, created_at
         FROM file_tags
         WHERE path = ?1 OR path LIKE ?3
         ON CONFLICT(path, tag) DO NOTHING",
        params![from, to, like, start],
    )
    .map_err(|_| "Could not copy tags".to_string())?;
    conn.execute(
        "INSERT INTO file_comments(path, user_id, username, body, created_at)
         SELECT CASE
            WHEN path = ?1 THEN ?2
            ELSE ?2 || substr(path, ?4)
         END, user_id, username, body, created_at
         FROM file_comments
         WHERE path = ?1 OR path LIKE ?3",
        params![from, to, like, start],
    )
    .map_err(|_| "Could not copy comments".to_string())?;
    conn.execute(
        "INSERT INTO file_fingerprints(path, size, modified, noted_at)
         SELECT CASE
            WHEN path = ?1 THEN ?2
            ELSE ?2 || substr(path, ?4)
         END, size, modified, noted_at
         FROM file_fingerprints
         WHERE path = ?1 OR path LIKE ?3
         ON CONFLICT(path) DO UPDATE SET
            size = excluded.size,
            modified = excluded.modified,
            noted_at = excluded.noted_at",
        params![from, to, like, start],
    )
    .map_err(|_| "Could not copy fingerprints".to_string())?;
    Ok(())
}

pub fn delete_path_meta(conn: &Connection, path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Ok(());
    }
    let like = path_like(path);
    conn.execute(
        "DELETE FROM file_tags WHERE path = ?1 OR path LIKE ?2",
        params![path, like],
    )
    .map_err(|_| "Could not remove tags".to_string())?;
    conn.execute(
        "DELETE FROM file_comments WHERE path = ?1 OR path LIKE ?2",
        params![path, like],
    )
    .map_err(|_| "Could not remove comments".to_string())?;
    conn.execute(
        "DELETE FROM file_fingerprints WHERE path = ?1 OR path LIKE ?2",
        params![path, like],
    )
    .map_err(|_| "Could not remove fingerprints".to_string())?;
    conn.execute(
        "DELETE FROM bookmarks WHERE path = ?1 OR path LIKE ?2",
        params![path, like],
    )
    .map_err(|_| "Could not remove bookmarks".to_string())?;
    conn.execute(
        "DELETE FROM recents WHERE path = ?1 OR path LIKE ?2",
        params![path, like],
    )
    .map_err(|_| "Could not remove recent files".to_string())?;
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
        let path = std::env::temp_dir().join(format!(
            "ownnas-db-{}-{}.sqlite",
            std::process::id(),
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
