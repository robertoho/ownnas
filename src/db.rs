use rusqlite::{params, Connection};

use crate::auth::{self, SESSION_SECS};

pub struct User {
    pub id: i64,
    pub username: String,
    pub password_hash: String,
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
        ",
    )
    .map_err(|_| "Could not create the database schema".to_string())?;
    conn.execute(
        "INSERT INTO settings(key, value) VALUES('schema_version', '1')
         ON CONFLICT(key) DO NOTHING",
        [],
    )
    .map_err(|_| "Could not store the schema version".to_string())?;
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
    let mut stmt = conn
        .prepare("SELECT username FROM users ORDER BY username COLLATE NOCASE")
        .map_err(|_| "Could not read users".to_string())?;
    let rows = stmt
        .query_map([], |row| row.get(0))
        .map_err(|_| "Could not read users".to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|_| "Could not read users".to_string())
}

pub fn create_user(conn: &Connection, username: &str, password: &str) -> Result<(), String> {
    auth::validate_username(username).map_err(|e| e.to_string())?;
    auth::validate_password(password).map_err(|e| e.to_string())?;
    let hash = auth::hash_password(password)?;
    let now = now_secs();
    conn.execute(
        "INSERT INTO users(username, password_hash, created_at) VALUES(?1, ?2, ?3)",
        params![username.trim(), hash, now],
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
        .prepare("SELECT id, username, password_hash FROM users WHERE username = ?1")
        .map_err(|_| "Could not read users".to_string())?;
    let mut rows = stmt
        .query(params![username.trim()])
        .map_err(|_| "Could not read users".to_string())?;
    if let Some(row) = rows.next().map_err(|_| "Could not read users".to_string())? {
        Ok(Some(User {
            id: row.get(0).map_err(|_| "Could not read users".to_string())?,
            username: row.get(1).map_err(|_| "Could not read users".to_string())?,
            password_hash: row.get(2).map_err(|_| "Could not read users".to_string())?,
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
    let changed = conn
        .execute("DELETE FROM users WHERE username = ?1", params![username.trim()])
        .map_err(|_| "Could not delete the user".to_string())?;
    if changed == 0 {
        return Err("That user does not exist".into());
    }
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
            "SELECT u.id, u.username, u.password_hash
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

fn purge_expired(conn: &Connection) -> Result<(), String> {
    conn.execute(
        "DELETE FROM sessions WHERE expires_at <= ?1",
        params![now_secs()],
    )
    .map_err(|_| "Could not clean up sessions".to_string())?;
    Ok(())
}
