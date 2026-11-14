//! Keys that give an AI agent limited access over MCP (`mcp.rs`): which folders it may use and
//! how much it may do there, separate from the accounts people sign in with. A key opens only
//! `/mcp`, and a session token does not open `/mcp`, so neither can stand in for the other.
//!
//! Folders are stored as library paths and follow moves and renames like other path metadata
//! (`db::rewrite_path_meta`, `db::delete_path_meta`).

use crate::{auth, files};
#[cfg(test)]
use crate::db;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Starts every key, so one is easy to recognize (and to find if it leaks into a file).
const PREFIX: &str = "ownnas_mcp_";
/// Past this many entries, the oldest are dropped from the log.
const LOG_LIMIT: i64 = 50_000;
const LOG_TEXT_LIMIT: usize = 500;

/// A key that is allowed in right now.
#[derive(Clone, Debug)]
pub struct AgentKey {
    pub id: i64,
    pub name: String,
    /// "read"; "organise": also tag, make folders, rename, move, move to the Trash, and albums;
    /// or "write": also make and change files (each change keeps the version before it).
    pub level: String,
    /// Organising changes wait for a person to approve them.
    pub approve: bool,
    pub folders: Vec<String>,
    /// Folders and files it may not reach even inside `folders`: its own and those excluded for
    /// every key. An exclusion always beats an allowance.
    pub excluded: Vec<String>,
    /// Of `folders`, the ones it may only read (a key that can change things).
    pub read_only: Vec<String>,
    /// The tools it may use; `None` for all its level allows.
    pub tools: Option<Vec<String>>,
    pub calls_per_minute: u32,
    pub daily_changes: i64,
    /// The only file extensions it may see and use (lowercase, without the dot); `None` for all.
    pub extensions: Option<Vec<String>>,
}

/// Where a key may look: its folders (or files), and which file extensions count there.
#[derive(Clone, Debug, Default)]
pub struct Scope {
    pub folders: Vec<String>,
    /// Folders and files out of reach even inside `folders`.
    pub excluded: Vec<String>,
    /// Lowercase, without the dot; `None` for every file.
    pub extensions: Option<Vec<String>>,
}

/// Past this many entries, a folder is not looked through for files of other kinds, and is
/// treated as holding some (so it is not moved or put in the Trash by a limited key).
const FOLDER_LOOK_MOST: usize = 20_000;

impl Scope {
    /// Everything in the library (for checking where a key may be given).
    fn everything() -> Self {
        Scope { folders: vec![String::new()], excluded: Vec::new(), extensions: None }
    }

    /// Whether a library path is inside its folders and not excluded, by name only: a quick
    /// first pass for long lists; `check` has the last word.
    pub fn holds_name(&self, path: &str) -> bool {
        self.folders.iter().any(|folder| within(folder, path)) && !self.excluded.iter().any(|out| within(out, path))
    }

    /// Whether something excluded is at or inside `rel` (a library path as `check` gives it), so
    /// a key may not move, copy, or put it in the Trash and take what it cannot reach along.
    pub fn holds_excluded(&self, rel: &str) -> bool {
        self.excluded.iter().any(|out| within(rel, out))
    }

    /// Whether a file of this name fits the key's extensions.
    pub fn allows_name(&self, name: &str) -> bool {
        let Some(extensions) = &self.extensions else { return true };
        name.rsplit_once('.')
            .filter(|(stem, _)| !stem.is_empty())
            .is_some_and(|(_, ext)| extensions.iter().any(|allowed| allowed.eq_ignore_ascii_case(ext)))
    }

    /// Whether every file in a folder (and the folders in it) fits the extensions, so a
    /// limited key may move, copy, or put it in the Trash without touching files it cannot see.
    pub fn folder_fits(&self, full: &Path) -> bool {
        if self.extensions.is_none() {
            return true;
        }
        let mut pending = vec![full.to_path_buf()];
        let mut looked = 0;
        while let Some(dir) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else { return false };
            for entry in entries.flatten() {
                looked += 1;
                if looked > FOLDER_LOOK_MOST {
                    return false;
                }
                // Hidden files are out of every key's reach already, and folders holding them
                // could always be moved; the extensions do not change that.
                if entry.file_name().to_string_lossy().starts_with('.') {
                    continue;
                }
                let Ok(kind) = entry.file_type() else { return false };
                if kind.is_dir() {
                    pending.push(entry.path());
                } else if !self.allows_name(&entry.file_name().to_string_lossy()) {
                    return false;
                }
            }
        }
        true
    }
}

/// Whether library path `path` is `folder` or inside it ("" is the whole library).
pub fn within(folder: &str, path: &str) -> bool {
    folder.is_empty() || path == folder || path.strip_prefix(folder).is_some_and(|rest| rest.starts_with('/'))
}

/// Cleans extensions as given (".MD", " txt ") into what is stored ("md", "txt").
pub fn clean_extensions(list: &[String]) -> Result<Option<Vec<String>>, String> {
    let mut clean: Vec<String> = Vec::new();
    for item in list {
        let ext = item.trim().trim_start_matches('*').trim_start_matches('.').to_ascii_lowercase();
        if ext.is_empty() {
            continue;
        }
        if ext.len() > 20 || !ext.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_') {
            return Err(format!("“{}” is not a file extension", item.trim()));
        }
        if !clean.contains(&ext) {
            clean.push(ext);
        }
    }
    clean.sort();
    Ok((!clean.is_empty()).then_some(clean))
}

/// The limits a key has unless it is given its own.
pub const CALLS_PER_MINUTE: u32 = 300;
pub const DAILY_CHANGES: i64 = 500;

impl AgentKey {
    /// Where it may look: its folders and extensions, for `check`.
    pub fn scope(&self) -> Scope {
        Scope { folders: self.folders.clone(), excluded: self.excluded.clone(), extensions: self.extensions.clone() }
    }

    /// Whether a file of this name fits its extensions (all do without a limit).
    pub fn allows_name(&self, name: &str) -> bool {
        self.scope().allows_name(name)
    }

    /// Whether it may use a tool (its level is checked by the tool).
    pub fn may_use(&self, tool: &str) -> bool {
        self.tools.as_ref().is_none_or(|tools| tools.iter().any(|allowed| allowed == tool))
    }

    /// Whether it may change `rel` (a library path, as `check` gives it): the most specific of
    /// its folders holding it decides.
    pub fn may_change(&self, rel: &str) -> bool {
        let closest = self.folders.iter().filter(|folder| within(folder, rel)).max_by_key(|folder| folder.len());
        closest.is_some_and(|folder| !self.read_only.contains(folder))
    }

    pub fn can_organise(&self) -> bool {
        self.level == ORGANISE || self.level == WRITE
    }
    pub fn can_write(&self) -> bool {
        self.level == WRITE
    }
}

pub const READ: &str = "read";
pub const ORGANISE: &str = "organise";
pub const WRITE: &str = "write";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyInfo {
    pub id: i64,
    pub name: String,
    pub level: String,
    pub approve: bool,
    pub folders: Vec<String>,
    /// Its own exclusions (those for every key are listed apart).
    pub excluded: Vec<String>,
    pub read_only: Vec<String>,
    pub tools: Option<Vec<String>>,
    pub calls_per_minute: Option<u32>,
    pub daily_changes: Option<i64>,
    pub extensions: Option<Vec<String>>,
    pub created_by: String,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub revoked_at: Option<i64>,
    pub last_used_at: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub key_id: i64,
    pub key_name: String,
    pub at: i64,
    pub tool: String,
    pub path: String,
    pub detail: String,
    pub outcome: String,
}

/// A path a key may use: where it really is, once links are followed.
#[derive(Debug)]
pub struct Allowed {
    pub rel: String,
    pub full: PathBuf,
}

/// Whether a path is out of bounds for every key, whatever its folders: the Trash, and anything
/// hidden, such as `.env` files and `.ssh` folders. `rel` is relative to the canonical `root`.
fn concealed(root: &Path, rel: &Path) -> bool {
    let mut at = root.to_path_buf();
    rel.components().any(|part| {
        at.push(part);
        let name = part.as_os_str().to_string_lossy();
        files::entry_hidden(&at, &name) || files::is_trash_folder(&at)
    })
}

/// Where `rel` really is, if the scope allows it: inside one of its folders (or one of its
/// files), and, for a file, with one of its extensions (folders always count, so they can be
/// browsed). Everything is compared after following links, so a link inside an allowed folder
/// cannot reach outside it, and a link named `notes.md` to `id_rsa` does not pass as Markdown.
/// `None` both for a path that is not allowed and one that does not exist, so a key cannot
/// learn what exists elsewhere.
pub fn check(root: &Path, scope: &Scope, rel: &str) -> Option<Allowed> {
    let folders = &scope.folders;
    let root = root.canonicalize().ok()?;
    let full = files::resolve(&root, rel).ok()?.full;
    let inside = full.strip_prefix(&root).ok()?;
    if concealed(&root, inside) {
        return None;
    }
    // `starts_with` compares whole path parts: "docs" does not allow "docs2".
    let allowed = folders.iter().any(|folder| files::resolve(&root, folder).is_ok_and(|folder| full.starts_with(&folder.full)));
    if !allowed {
        return None;
    }
    // An exclusion beats every allowance, however close. Compared after following links too, so
    // an excluded file cannot be reached through a link to it.
    if scope.excluded.iter().any(|out| files::resolve(&root, out).is_ok_and(|out| full.starts_with(&out.full))) {
        return None;
    }
    if scope.extensions.is_some() && !full.is_dir() {
        let name = full.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
        if !scope.allows_name(&name) {
            return None;
        }
    }
    Some(Allowed {
        rel: inside.components().map(|part| part.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/"),
        full: full.clone(),
    })
}

/// A folder (or a single file) as given for a key, as the library path to store.
fn folder_to_store(root: &Path, folder: &str) -> Result<String, String> {
    let rel = folder.trim().trim_matches('/');
    let refused = || format!("\"{}\" is not a folder or file an agent can be given", folder.trim());
    let found = check(root, &Scope::everything(), rel).ok_or_else(refused)?;
    if !found.full.is_dir() && !found.full.is_file() {
        return Err(refused());
    }
    Ok(found.rel)
}

/// Gives a key other folders (or files). Folders it keeps stay read-only if they were.
pub fn set_folders(conn: &mut Connection, root: &Path, id: i64, folders: &[String]) -> Result<bool, String> {
    if folders.is_empty() {
        return Err("Leave the key at least one folder".into());
    }
    let mut stored = folders.iter().map(|folder| folder_to_store(root, folder)).collect::<Result<Vec<_>, _>>()?;
    stored.sort();
    stored.dedup();
    let fail = |_| "Could not change the key".to_string();
    let exists: bool = conn
        .query_row("SELECT 1 FROM agent_keys WHERE id = ?1 AND revoked_at IS NULL", [id], |_| Ok(true))
        .optional()
        .map_err(fail)?
        .unwrap_or(false);
    if !exists {
        return Ok(false);
    }
    let read_only = read_only_of(conn, id)?;
    let tx = conn.transaction().map_err(fail)?;
    tx.execute("DELETE FROM agent_key_folders WHERE key_id = ?1", [id]).map_err(fail)?;
    for folder in &stored {
        tx.execute(
            "INSERT INTO agent_key_folders(key_id, path, read_only) VALUES (?1, ?2, ?3)",
            params![id, folder, read_only.contains(folder)],
        )
        .map_err(fail)?;
    }
    tx.commit().map_err(fail)?;
    Ok(true)
}

/// Makes a key and returns it with its secret, which is never available again.
pub fn create(conn: &mut Connection, root: &Path, user_id: i64, name: &str, level: &str, approve: bool, folders: &[String], expires_at: Option<i64>, now: i64) -> Result<(i64, String), String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return Err("Give the key a name of up to 80 characters".into());
    }
    if ![READ, ORGANISE, WRITE].contains(&level) {
        return Err("Choose read, organise or write access".into());
    }
    if expires_at.is_some_and(|at| at <= now) {
        return Err("The expiry date has already passed".into());
    }
    if folders.is_empty() {
        return Err("Choose at least one folder".into());
    }
    let mut stored = folders.iter().map(|folder| folder_to_store(root, folder)).collect::<Result<Vec<_>, _>>()?;
    stored.sort();
    stored.dedup();
    let fail = |_| "Could not store the key".to_string();
    let token = format!("{PREFIX}{}", auth::new_token());
    let tx = conn.transaction().map_err(fail)?;
    tx.execute(
        "INSERT INTO agent_keys(token_hash, name, level, user_id, created_at, expires_at, approve) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![auth::token_hash(&token), name, level, user_id, now, expires_at, approve && level != READ],
    )
    .map_err(fail)?;
    let id = tx.last_insert_rowid();
    for folder in &stored {
        tx.execute("INSERT INTO agent_key_folders(key_id, path) VALUES (?1, ?2)", params![id, folder]).map_err(fail)?;
    }
    tx.commit().map_err(fail)?;
    Ok((id, token))
}

fn folders_of(conn: &Connection, id: i64) -> Result<Vec<String>, String> {
    Ok(folders_with_modes(conn, id)?.into_iter().map(|(path, _)| path).collect())
}

fn read_only_of(conn: &Connection, id: i64) -> Result<Vec<String>, String> {
    Ok(folders_with_modes(conn, id)?.into_iter().filter(|(_, read_only)| *read_only).map(|(path, _)| path).collect())
}

fn folders_with_modes(conn: &Connection, id: i64) -> Result<Vec<(String, bool)>, String> {
    let fail = |_| "Could not read the key".to_string();
    let mut statement = conn.prepare("SELECT path, read_only FROM agent_key_folders WHERE key_id = ?1 ORDER BY path").map_err(fail)?;
    let rows = statement.query_map([id], |row| Ok((row.get(0)?, row.get(1)?))).map_err(fail)?;
    rows.collect::<Result<_, _>>().map_err(fail)
}

/// A key's exclusions, with or without those for every key (`EVERY_KEY` for only those).
fn excluded_of(conn: &Connection, id: i64, with_every_key: bool) -> Result<Vec<String>, String> {
    let fail = |_| "Could not read the key".to_string();
    let mut statement = conn
        .prepare("SELECT DISTINCT path FROM agent_key_excludes WHERE key_id = ?1 OR (?2 AND key_id = 0) ORDER BY path")
        .map_err(fail)?;
    let rows = statement.query_map(params![id, with_every_key], |row| row.get(0)).map_err(fail)?;
    rows.collect::<Result<_, _>>().map_err(fail)
}

/// The key id exclusions for every key are stored under (no key has it).
pub const EVERY_KEY: i64 = 0;

/// Folders and files excluded for every key.
pub fn excluded_for_every_key(conn: &Connection) -> Result<Vec<String>, String> {
    excluded_of(conn, EVERY_KEY, false)
}

/// What a person can do about a key's reach of some items from the file explorer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rule {
    /// Give it to the keys (as one of their folders or files), lifting an exclusion of it.
    Allow,
    /// Keep it out of their reach, even inside their folders.
    Exclude,
    /// Take away what was set on exactly these items, either way.
    Clear,
    /// Lift only an exclusion of exactly these items: for every key, the one for every key.
    Unexclude,
}

/// Which keys a rule is for: every key (an exclusion then also holds for keys made later), or
/// the ones chosen.
pub enum Keys {
    Every,
    Chosen(Vec<i64>),
}

/// Sets a rule on folders or files for keys. Returns notes on what could not be done as asked
/// (such as an item that stays excluded because a folder holding it is).
pub fn set_rule(conn: &mut Connection, root: &Path, paths: &[String], keys: &Keys, rule: Rule) -> Result<Vec<String>, String> {
    if paths.is_empty() {
        return Err("Choose at least one folder or file".into());
    }
    let mut stored = paths.iter().map(|path| folder_to_store(root, path)).collect::<Result<Vec<_>, _>>()?;
    stored.sort();
    stored.dedup();
    let fail = |_| "Could not change the keys".to_string();
    let active: Vec<(i64, String)> = {
        let mut statement = conn.prepare("SELECT id, name FROM agent_keys WHERE revoked_at IS NULL ORDER BY id").map_err(fail)?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?))).map_err(fail)?;
        rows.collect::<Result<_, _>>().map_err(fail)?
    };
    let chosen: Vec<(i64, String)> = match keys {
        Keys::Every => active.clone(),
        Keys::Chosen(ids) => {
            if ids.is_empty() {
                return Err("Choose at least one key".into());
            }
            if let Some(stray) = ids.iter().find(|id| !active.iter().any(|(active, _)| active == *id)) {
                return Err(format!("Key {stray} is no longer there"));
            }
            active.iter().filter(|(id, _)| ids.contains(id)).cloned().collect()
        }
    };
    let every = matches!(keys, Keys::Every);
    let mut notes = Vec::new();
    let tx = conn.transaction().map_err(fail)?;
    let count_folders = |tx: &rusqlite::Transaction, id: i64| -> Result<i64, String> {
        tx.query_row("SELECT count(*) FROM agent_key_folders WHERE key_id = ?1", [id], |row| row.get(0)).map_err(fail)
    };
    for path in &stored {
        let shown = if path.is_empty() { "/".to_string() } else { path.clone() };
        match rule {
            Rule::Allow => {
                // An exclusion of it for every key stays for the keys not chosen.
                let for_every: bool = tx
                    .query_row("SELECT 1 FROM agent_key_excludes WHERE key_id = 0 AND path = ?1", [path], |_| Ok(true))
                    .optional()
                    .map_err(fail)?
                    .unwrap_or(false);
                if for_every {
                    tx.execute("DELETE FROM agent_key_excludes WHERE key_id = 0 AND path = ?1", [path]).map_err(fail)?;
                    for (id, _) in active.iter().filter(|key| !chosen.contains(key)) {
                        tx.execute("INSERT OR IGNORE INTO agent_key_excludes(key_id, path) VALUES (?1, ?2)", params![id, path]).map_err(fail)?;
                    }
                }
                for (id, name) in &chosen {
                    tx.execute("DELETE FROM agent_key_excludes WHERE key_id = ?1 AND path = ?2", params![id, path]).map_err(fail)?;
                    // Inside one of its folders already, it needs no entry of its own.
                    let inside: bool = tx
                        .query_row(
                            "SELECT 1 FROM agent_key_folders WHERE key_id = ?1 AND path <> ?2 AND (path = '' OR substr(?2, 1, length(path) + 1) = path || '/')",
                            params![id, path],
                            |_| Ok(true),
                        )
                        .optional()
                        .map_err(fail)?
                        .unwrap_or(false);
                    if !inside {
                        tx.execute("INSERT OR IGNORE INTO agent_key_folders(key_id, path) VALUES (?1, ?2)", params![id, path]).map_err(fail)?;
                    }
                    let above: Option<String> = tx
                        .query_row(
                            "SELECT path FROM agent_key_excludes WHERE (key_id = ?1 OR key_id = 0) AND path <> ?2 AND (path = '' OR substr(?2, 1, length(path) + 1) = path || '/') ORDER BY length(path) LIMIT 1",
                            params![id, path],
                            |row| row.get(0),
                        )
                        .optional()
                        .map_err(fail)?;
                    if let Some(above) = above {
                        notes.push(format!("“{shown}” stays out of reach of “{name}”: “{}” is excluded, and an exclusion wins", if above.is_empty() { "/" } else { &above }));
                    }
                }
            }
            Rule::Exclude => {
                let owners: Vec<i64> = if every { vec![EVERY_KEY] } else { chosen.iter().map(|(id, _)| *id).collect() };
                for id in owners {
                    tx.execute("INSERT OR IGNORE INTO agent_key_excludes(key_id, path) VALUES (?1, ?2)", params![id, path]).map_err(fail)?;
                }
                if every {
                    // Its own exclusions are covered now.
                    tx.execute("DELETE FROM agent_key_excludes WHERE key_id <> 0 AND path = ?1", [path]).map_err(fail)?;
                }
                // Given exactly this, a key would keep a folder it cannot reach: take it away,
                // unless it is the key's last one.
                for (id, name) in &chosen {
                    let given: bool = tx
                        .query_row("SELECT 1 FROM agent_key_folders WHERE key_id = ?1 AND path = ?2", params![id, path], |_| Ok(true))
                        .optional()
                        .map_err(fail)?
                        .unwrap_or(false);
                    if given {
                        if count_folders(&tx, *id)? > 1 {
                            tx.execute("DELETE FROM agent_key_folders WHERE key_id = ?1 AND path = ?2", params![id, path]).map_err(fail)?;
                        } else {
                            notes.push(format!("“{shown}” is the only folder of “{name}”, which now reaches nothing"));
                        }
                    }
                }
            }
            Rule::Unexclude => {
                let owners: Vec<i64> = if every { vec![EVERY_KEY] } else { chosen.iter().map(|(id, _)| *id).collect() };
                for id in owners {
                    tx.execute("DELETE FROM agent_key_excludes WHERE key_id = ?1 AND path = ?2", params![id, path]).map_err(fail)?;
                }
            }
            Rule::Clear => {
                if every {
                    tx.execute("DELETE FROM agent_key_excludes WHERE path = ?1", [path]).map_err(fail)?;
                }
                for (id, name) in &chosen {
                    tx.execute("DELETE FROM agent_key_excludes WHERE key_id = ?1 AND path = ?2", params![id, path]).map_err(fail)?;
                    let given: bool = tx
                        .query_row("SELECT 1 FROM agent_key_folders WHERE key_id = ?1 AND path = ?2", params![id, path], |_| Ok(true))
                        .optional()
                        .map_err(fail)?
                        .unwrap_or(false);
                    if given {
                        if count_folders(&tx, *id)? > 1 {
                            tx.execute("DELETE FROM agent_key_folders WHERE key_id = ?1 AND path = ?2", params![id, path]).map_err(fail)?;
                        } else {
                            notes.push(format!("“{shown}” is the only folder of “{name}”, so it keeps it"));
                        }
                    }
                }
            }
        }
    }
    tx.commit().map_err(fail)?;
    Ok(notes)
}

/// How one key reaches an item, for showing it: "allowed" (given exactly), "inside" (in a folder
/// it was given), "excluded" (excluded exactly), "excludedInside" (in an excluded folder), or
/// "none".
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reach {
    pub key_id: i64,
    pub state: &'static str,
    /// The rule that decides it (the folder given, or the exclusion), if any.
    pub by: Option<String>,
    /// Whether that exclusion is for every key.
    pub every_key: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemAccess {
    pub path: String,
    pub keys: Vec<Reach>,
    /// Whether it (or a folder holding it) is excluded for every key.
    pub excluded_for_every_key: bool,
}

/// How each key that still works reaches each of these items (library paths). Items that no
/// key may ever be given (hidden ones, the Trash) are left out.
pub fn access(conn: &Connection, root: &Path, paths: &[String]) -> Result<Vec<ItemAccess>, String> {
    let fail = |_| "Could not read the keys".to_string();
    let ids: Vec<i64> = {
        let mut statement = conn.prepare("SELECT id FROM agent_keys WHERE revoked_at IS NULL ORDER BY id DESC").map_err(fail)?;
        let rows = statement.query_map([], |row| row.get(0)).map_err(fail)?;
        rows.collect::<Result<_, _>>().map_err(fail)?
    };
    let every = excluded_for_every_key(conn)?;
    let keys: Vec<(i64, Vec<String>, Vec<String>)> =
        ids.into_iter().map(|id| Ok((id, folders_of(conn, id)?, excluded_of(conn, id, false)?))).collect::<Result<_, String>>()?;
    let closest = |list: &[String], path: &str| list.iter().filter(|out| within(out, path)).max_by_key(|out| out.len()).cloned();
    let mut items = Vec::new();
    for path in paths {
        let Ok(rel) = folder_to_store(root, path) else { continue };
        let every_out = closest(&every, &rel);
        let reach = keys
            .iter()
            .map(|(id, folders, excluded)| {
                let own_out = closest(excluded, &rel);
                // The exclusion that decides is the outermost; either one wins over any allowance.
                let out = match (&own_out, &every_out) {
                    (Some(own), Some(all)) => Some(if all.len() <= own.len() { (all.clone(), true) } else { (own.clone(), false) }),
                    (Some(own), None) => Some((own.clone(), false)),
                    (None, Some(all)) => Some((all.clone(), true)),
                    (None, None) => None,
                };
                if let Some((by, every_key)) = out {
                    let state = if by == rel { "excluded" } else { "excludedInside" };
                    return Reach { key_id: *id, state, by: Some(by), every_key };
                }
                match closest(folders, &rel) {
                    Some(by) => Reach { key_id: *id, state: if by == rel { "allowed" } else { "inside" }, by: Some(by), every_key: false },
                    None => Reach { key_id: *id, state: "none", by: None, every_key: false },
                }
            })
            .collect();
        items.push(ItemAccess { path: rel, keys: reach, excluded_for_every_key: every_out.is_some() });
    }
    Ok(items)
}

/// What the file explorer marks: each folder or file given to a key that still works, with the
/// keys' names, and each one excluded, with the keys' names (or none for every key).
#[derive(Serialize, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Marks {
    pub shared: std::collections::BTreeMap<String, Vec<String>>,
    pub excluded: std::collections::BTreeMap<String, Vec<String>>,
    /// Paths excluded for every key.
    pub excluded_for_every_key: Vec<String>,
}

pub fn marks(conn: &Connection, now: i64) -> Result<Marks, String> {
    let fail = |_| "Could not read the keys".to_string();
    let mut marks = Marks::default();
    let working = "k.revoked_at IS NULL AND (k.expires_at IS NULL OR k.expires_at > ?1)";
    {
        let mut statement = conn
            .prepare(&format!("SELECT f.path, k.name FROM agent_key_folders f JOIN agent_keys k ON k.id = f.key_id WHERE {working} ORDER BY k.name"))
            .map_err(fail)?;
        let rows = statement.query_map([now], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))).map_err(fail)?;
        for (path, name) in rows.filter_map(Result::ok) {
            marks.shared.entry(path).or_default().push(name);
        }
    }
    {
        let mut statement = conn
            .prepare(&format!("SELECT e.path, k.name FROM agent_key_excludes e JOIN agent_keys k ON k.id = e.key_id WHERE {working} ORDER BY k.name"))
            .map_err(fail)?;
        let rows = statement.query_map([now], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))).map_err(fail)?;
        for (path, name) in rows.filter_map(Result::ok) {
            marks.excluded.entry(path).or_default().push(name);
        }
    }
    marks.excluded_for_every_key = excluded_for_every_key(conn)?;
    Ok(marks)
}

fn tools_from(text: Option<String>) -> Option<Vec<String>> {
    text.and_then(|text| serde_json::from_str(&text).ok())
}

/// What a key may use, set by a person: the tools (`None` for all its level allows), its
/// limits (`None` for the usual ones), and which of its folders it may only read.
pub struct KeySettings {
    pub tools: Option<Vec<String>>,
    pub calls_per_minute: Option<u32>,
    pub daily_changes: Option<i64>,
    pub read_only: Vec<String>,
    /// The only file extensions it may use; `None` for all.
    pub extensions: Option<Vec<String>>,
}

pub fn set_settings(conn: &Connection, id: i64, settings: &KeySettings) -> Result<bool, String> {
    let fail = |_| "Could not change the key".to_string();
    if settings.calls_per_minute.is_some_and(|calls| !(1..=10_000).contains(&calls)) {
        return Err("Give between 1 and 10000 requests a minute".into());
    }
    if settings.daily_changes.is_some_and(|changes| !(0..=1_000_000).contains(&changes)) {
        return Err("Give between 0 and 1000000 changes a day".into());
    }
    if settings.tools.as_ref().is_some_and(Vec::is_empty) {
        return Err("Leave the key at least one tool".into());
    }
    let folders = folders_of(conn, id)?;
    if let Some(stray) = settings.read_only.iter().find(|folder| !folders.contains(folder)) {
        return Err(format!("“{stray}” is not one of the key's folders"));
    }
    let tools = settings.tools.as_ref().map(|tools| serde_json::to_string(tools).unwrap_or_default());
    let extensions = settings.extensions.as_ref().map(|list| serde_json::to_string(list).unwrap_or_default());
    let changed = conn
        .execute(
            "UPDATE agent_keys SET tools = ?2, calls_per_minute = ?3, daily_changes = ?4, extensions = ?5 WHERE id = ?1",
            params![id, tools, settings.calls_per_minute, settings.daily_changes, extensions],
        )
        .map_err(fail)?;
    for folder in &folders {
        conn.execute(
            "UPDATE agent_key_folders SET read_only = ?3 WHERE key_id = ?1 AND path = ?2",
            params![id, folder, settings.read_only.contains(folder)],
        )
        .map_err(fail)?;
    }
    Ok(changed > 0)
}

/// The key a secret belongs to, if it may be used now: not revoked, not expired, and made by
/// someone who is still an administrator.
pub fn authenticate(conn: &Connection, token: &str, now: i64) -> Result<Option<AgentKey>, String> {
    if !token.starts_with(PREFIX) {
        return Ok(None);
    }
    find(conn, "k.token_hash = ?1", &auth::token_hash(token), now)
}

/// A key by its id, if it may still be used now (to carry out a change someone approved).
pub fn usable(conn: &Connection, id: i64, now: i64) -> Result<Option<AgentKey>, String> {
    find(conn, "k.id = ?1", &id.to_string(), now)
}

fn find(conn: &Connection, which: &str, value: &str, now: i64) -> Result<Option<AgentKey>, String> {
    let fail = |_| "Could not read the key".to_string();
    let found = conn
        .query_row(
            &format!(
                "SELECT k.id, k.name, k.level, k.approve, k.tools, k.calls_per_minute, k.daily_changes, k.extensions FROM agent_keys k JOIN users u ON u.id = k.user_id
                 WHERE {which} AND k.revoked_at IS NULL AND (k.expires_at IS NULL OR k.expires_at > ?2) AND u.is_admin = 1"
            ),
            params![value, now],
            |row| {
                Ok((
                    (row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, bool>(3)?),
                    (row.get::<_, Option<String>>(4)?, row.get::<_, Option<u32>>(5)?, row.get::<_, Option<i64>>(6)?, row.get::<_, Option<String>>(7)?),
                ))
            },
        )
        .optional()
        .map_err(fail)?;
    let Some(((id, name, level, approve), (tools, calls, changes, extensions))) = found else { return Ok(None) };
    Ok(Some(AgentKey {
        id,
        name,
        level,
        approve,
        folders: folders_of(conn, id)?,
        excluded: excluded_of(conn, id, true)?,
        read_only: read_only_of(conn, id)?,
        tools: tools_from(tools),
        calls_per_minute: calls.unwrap_or(CALLS_PER_MINUTE),
        daily_changes: changes.unwrap_or(DAILY_CHANGES),
        extensions: tools_from(extensions),
    }))
}

/// Gives a key a new secret (the old one stops working at once) and returns it.
pub fn new_secret(conn: &Connection, id: i64) -> Result<String, String> {
    let token = format!("{PREFIX}{}", auth::new_token());
    let changed = conn
        .execute("UPDATE agent_keys SET token_hash = ?2 WHERE id = ?1 AND revoked_at IS NULL", params![id, auth::token_hash(&token)])
        .map_err(|_| "Could not change the key".to_string())?;
    if changed == 0 {
        return Err("That key is no longer there".into());
    }
    Ok(token)
}

/// Whether an organising key's changes wait for approval.
pub fn set_approve(conn: &Connection, id: i64, approve: bool) -> Result<bool, String> {
    let changed = conn
        .execute("UPDATE agent_keys SET approve = ?2 WHERE id = ?1 AND level <> ?3", params![id, approve, READ])
        .map_err(|_| "Could not change the key".to_string())?;
    Ok(changed > 0)
}

/// Every key, newest first. Revoked ones stay listed, so the log can be read against them.
pub fn list(conn: &Connection) -> Result<Vec<KeyInfo>, String> {
    let fail = |_| "Could not read the keys".to_string();
    let mut statement = conn
        .prepare(
            "SELECT k.id, k.name, k.level, u.username, k.created_at, k.expires_at, k.revoked_at, k.last_used_at, k.approve,
                    k.tools, k.calls_per_minute, k.daily_changes, k.extensions
             FROM agent_keys k JOIN users u ON u.id = k.user_id ORDER BY k.id DESC",
        )
        .map_err(fail)?;
    let rows = statement
        .query_map([], |row| {
            Ok(KeyInfo {
                id: row.get(0)?,
                name: row.get(1)?,
                level: row.get(2)?,
                folders: Vec::new(),
                excluded: Vec::new(),
                created_by: row.get(3)?,
                created_at: row.get(4)?,
                expires_at: row.get(5)?,
                revoked_at: row.get(6)?,
                last_used_at: row.get(7)?,
                approve: row.get(8)?,
                tools: tools_from(row.get(9)?),
                calls_per_minute: row.get(10)?,
                daily_changes: row.get(11)?,
                extensions: tools_from(row.get(12)?),
                read_only: Vec::new(),
            })
        })
        .map_err(fail)?;
    let mut keys = rows.collect::<Result<Vec<_>, _>>().map_err(fail)?;
    for key in &mut keys {
        key.folders = folders_of(conn, key.id)?;
        key.excluded = excluded_of(conn, key.id, false)?;
        key.read_only = read_only_of(conn, key.id)?;
    }
    Ok(keys)
}

/// Stops a key working, at once. Returns whether it was still active.
pub fn revoke(conn: &Connection, id: i64, now: i64) -> Result<bool, String> {
    let changed = conn
        .execute("UPDATE agent_keys SET revoked_at = ?2 WHERE id = ?1 AND revoked_at IS NULL", params![id, now])
        .map_err(|_| "Could not revoke the key".to_string())?;
    Ok(changed > 0)
}

fn cut(text: &str) -> String {
    text.chars().take(LOG_TEXT_LIMIT).collect()
}

/// A change an organising key asked for.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub id: i64,
    pub key_id: i64,
    pub key_name: String,
    pub at: i64,
    pub tool: String,
    pub arguments: serde_json::Value,
    pub summary: String,
    /// "pending", "done", "declined", "failed" or "undone".
    pub status: String,
    pub result: String,
    pub decided_by: String,
    pub decided_at: Option<i64>,
    /// Whether it can be undone: made, and with a way back.
    pub can_undo: bool,
    #[serde(skip)]
    pub undo: Option<serde_json::Value>,
}

const CHANGE_COLUMNS: &str = "id, key_id, key_name, at, tool, arguments, summary, status, result, decided_by, decided_at, undo";

fn change_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Change> {
    let undo: Option<String> = row.get(11)?;
    let undo = undo.and_then(|text| serde_json::from_str(&text).ok());
    let status: String = row.get(7)?;
    Ok(Change {
        id: row.get(0)?,
        key_id: row.get(1)?,
        key_name: row.get(2)?,
        at: row.get(3)?,
        tool: row.get(4)?,
        arguments: serde_json::from_str(&row.get::<_, String>(5)?).unwrap_or(serde_json::Value::Null),
        summary: row.get(6)?,
        can_undo: status == "done" && undo.is_some(),
        status,
        result: row.get(8)?,
        decided_by: row.get(9)?,
        decided_at: row.get(10)?,
        undo,
    })
}

/// Writes down a change asked for; returns its id.
pub fn add_change(conn: &Connection, key: &AgentKey, tool: &str, arguments: &serde_json::Value, summary: &str, status: &str, undo: Option<&serde_json::Value>, result: &str, now: i64) -> Result<i64, String> {
    conn.execute(
        "INSERT INTO agent_changes(key_id, key_name, at, tool, arguments, summary, status, undo, result) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![key.id, key.name, now, tool, arguments.to_string(), cut(summary), status, undo.map(|undo| undo.to_string()), cut(result)],
    )
    .map_err(|_| "Could not write down the change".to_string())?;
    Ok(conn.last_insert_rowid())
}

/// Records what became of a change.
pub fn settle_change(conn: &Connection, id: i64, status: &str, undo: Option<&serde_json::Value>, result: &str, by: &str, now: i64) -> Result<(), String> {
    conn.execute(
        "UPDATE agent_changes SET status = ?2, undo = coalesce(?3, undo), result = ?4, decided_by = ?5, decided_at = ?6 WHERE id = ?1",
        params![id, status, undo.map(|undo| undo.to_string()), cut(result), by, now],
    )
    .map_err(|_| "Could not write down the change".to_string())?;
    Ok(())
}

pub fn change(conn: &Connection, id: i64) -> Result<Option<Change>, String> {
    conn.query_row(&format!("SELECT {CHANGE_COLUMNS} FROM agent_changes WHERE id = ?1"), [id], change_row)
        .optional()
        .map_err(|_| "Could not read the change".to_string())
}

/// The latest changes, newest first: for one key or all, and optionally only in one status.
pub fn changes(conn: &Connection, key_id: Option<i64>, status: Option<&str>, limit: i64) -> Result<Vec<Change>, String> {
    let fail = |_| "Could not read the changes".to_string();
    let mut statement = conn
        .prepare(&format!(
            "SELECT {CHANGE_COLUMNS} FROM agent_changes WHERE (?1 IS NULL OR key_id = ?1) AND (?2 IS NULL OR status = ?2) ORDER BY id DESC LIMIT ?3"
        ))
        .map_err(fail)?;
    let rows = statement.query_map(params![key_id, status, limit.clamp(1, 1000)], change_row).map_err(fail)?;
    rows.collect::<Result<_, _>>().map_err(fail)
}

/// What a key did since a time: calls by tool and outcome, calls by day, and its changes.
pub fn usage(conn: &Connection, key_id: i64, since: i64) -> Result<serde_json::Value, String> {
    let fail = |_| "Could not read the key's use".to_string();
    let mut tools: std::collections::BTreeMap<String, serde_json::Map<String, serde_json::Value>> = std::collections::BTreeMap::new();
    {
        let mut stmt = conn.prepare("SELECT tool, outcome, count(*) FROM agent_key_log WHERE key_id = ?1 AND at >= ?2 GROUP BY tool, outcome").map_err(fail)?;
        let rows = stmt.query_map(params![key_id, since], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?))).map_err(fail)?;
        for (tool, outcome, count) in rows.filter_map(Result::ok) {
            tools.entry(tool).or_default().insert(outcome, serde_json::json!(count));
        }
    }
    let mut days = Vec::new();
    {
        let mut stmt = conn.prepare("SELECT at / 86400 AS day, count(*), sum(outcome = 'refused') FROM agent_key_log WHERE key_id = ?1 AND at >= ?2 GROUP BY day ORDER BY day").map_err(fail)?;
        let rows = stmt.query_map(params![key_id, since], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?))).map_err(fail)?;
        for (day, calls, refused) in rows.filter_map(Result::ok) {
            days.push(serde_json::json!({ "day": day * 86400, "calls": calls, "refused": refused }));
        }
    }
    let mut changes = serde_json::Map::new();
    {
        let mut stmt = conn.prepare("SELECT status, count(*) FROM agent_changes WHERE key_id = ?1 AND at >= ?2 GROUP BY status").map_err(fail)?;
        let rows = stmt.query_map(params![key_id, since], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))).map_err(fail)?;
        for (status, count) in rows.filter_map(Result::ok) {
            changes.insert(status, serde_json::json!(count));
        }
    }
    let tools: Vec<serde_json::Value> = tools.into_iter().map(|(tool, outcomes)| serde_json::json!({ "tool": tool, "outcomes": outcomes })).collect();
    Ok(serde_json::json!({ "since": since, "tools": tools, "days": days, "changes": changes }))
}

/// Keys refused at least `least` times since a time (a sign of one looking for a way out of its
/// folders, or lost): id, name, and how many times.
pub fn refused_since(conn: &Connection, since: i64, least: i64) -> Result<Vec<(i64, String, i64)>, String> {
    let fail = |_| "Could not read the agent log".to_string();
    let mut stmt = conn
        .prepare(
            "SELECT l.key_id, l.key_name, count(*) AS refused FROM agent_key_log l JOIN agent_keys k ON k.id = l.key_id
             WHERE l.at >= ?1 AND l.outcome = 'refused' AND k.revoked_at IS NULL GROUP BY l.key_id HAVING refused >= ?2 ORDER BY refused DESC",
        )
        .map_err(fail)?;
    let rows = stmt.query_map(params![since, least], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).map_err(fail)?;
    rows.collect::<Result<_, _>>().map_err(fail)
}

/// Writes down one thing a key did or was refused.
pub fn record(conn: &Connection, key: &AgentKey, tool: &str, path: &str, detail: &str, outcome: &str, now: i64) -> Result<(), String> {
    let fail = |_| "Could not write the agent log".to_string();
    conn.execute(
        "INSERT INTO agent_key_log(key_id, key_name, at, tool, path, detail, outcome) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![key.id, key.name, now, cut(tool), cut(path), cut(detail), outcome],
    )
    .map_err(fail)?;
    let entry = conn.last_insert_rowid();
    if entry % 256 == 0 {
        conn.execute("DELETE FROM agent_key_log WHERE id <= ?1", [entry - LOG_LIMIT]).map_err(fail)?;
    }
    conn.execute(
        "UPDATE agent_keys SET last_used_at = ?2 WHERE id = ?1 AND (last_used_at IS NULL OR last_used_at < ?2 - 60)",
        params![key.id, now],
    )
    .map_err(fail)?;
    Ok(())
}

/// The latest log entries, newest first, for one key or for all.
pub fn read_log(conn: &Connection, key_id: Option<i64>, limit: i64) -> Result<Vec<LogEntry>, String> {
    let fail = |_| "Could not read the agent log".to_string();
    let mut statement = conn
        .prepare(
            "SELECT key_id, key_name, at, tool, path, detail, outcome FROM agent_key_log
             WHERE ?1 IS NULL OR key_id = ?1 ORDER BY id DESC LIMIT ?2",
        )
        .map_err(fail)?;
    let rows = statement
        .query_map(params![key_id, limit.clamp(1, 1000)], |row| {
            Ok(LogEntry {
                key_id: row.get(0)?,
                key_name: row.get(1)?,
                at: row.get(2)?,
                tool: row.get(3)?,
                path: row.get(4)?,
                detail: row.get(5)?,
                outcome: row.get(6)?,
            })
        })
        .map_err(fail)?;
    rows.collect::<Result<_, _>>().map_err(fail)
}

/// Used by the tests here and in `mcp.rs`: the id of the signed-up user called `name`.
#[cfg(test)]
pub(crate) fn user_id(conn: &Connection, name: &str) -> i64 {
    db::find_user(conn, name).unwrap().unwrap().id
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const NOW: i64 = 1_000_000;

    struct Fixture {
        base: PathBuf,
        root: PathBuf,
        conn: Connection,
        user: i64,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.base);
        }
    }

    fn fixture() -> Fixture {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let base = std::env::temp_dir().join(format!("ownnas-keys-{}-{}", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        let root = base.join("root");
        for dir in ["docs/deep", "docs2", "secret", "docs/.hidden", ".ssh", ".ownnas-trash/old"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        for file in ["docs/a.txt", "docs/deep/b.txt", "docs2/c.txt", "secret/s.txt", "docs/.hidden/h.txt", "docs/.env", ".ssh/id", ".ownnas-trash/old/t.txt", "top.txt"] {
            fs::write(root.join(file), "x").unwrap();
        }
        fs::create_dir_all(base.join("outside")).unwrap();
        fs::write(base.join("outside/o.txt"), "x").unwrap();
        let root = root.canonicalize().unwrap();
        let conn = db::open(&base.join("data/ownnas.db")).unwrap();
        db::create_user(&conn, "admin", "a-long-enough-passphrase").unwrap();
        let user = user_id(&conn, "admin");
        Fixture { base, root, conn, user }
    }

    fn folders(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn in_scope(folders: &[String]) -> Scope {
        Scope { folders: folders.to_vec(), excluded: Vec::new(), extensions: None }
    }

    #[test]
    fn extensions_limit_files_but_not_folders() {
        let f = fixture();
        let scope = Scope { folders: folders(&["docs"]), excluded: Vec::new(), extensions: clean_extensions(&[".TXT".into(), " md ".into()]).unwrap() };
        assert_eq!(scope.extensions, Some(vec!["md".to_string(), "txt".to_string()]));
        assert!(check(&f.root, &scope, "docs/a.txt").is_some());
        assert!(check(&f.root, &scope, "docs/deep").is_some(), "folders can still be browsed");
        fs::write(f.root.join("docs/notes.pdf"), "x").unwrap();
        assert!(check(&f.root, &scope, "docs/notes.pdf").is_none());
        assert!(scope.allows_name("Report.MD") && !scope.allows_name("run.sh") && !scope.allows_name("md") && !scope.allows_name(".txt"));
        assert!(!scope.folder_fits(&f.root.join("docs")), "a folder holding a PDF does not fit .md and .txt");
        fs::remove_file(f.root.join("docs/notes.pdf")).unwrap();
        assert!(scope.folder_fits(&f.root.join("docs")));
        assert!(clean_extensions(&["sh;rm".into()]).is_err());
        assert_eq!(clean_extensions(&["".into(), " ".into()]).unwrap(), None);
    }

    #[test]
    fn a_key_can_be_given_other_folders_and_files() {
        let mut f = fixture();
        let token = make(&mut f, &["docs"]);
        let id = authenticate(&f.conn, &token, NOW).unwrap().unwrap().id;
        let root = f.root.clone();
        assert!(set_folders(&mut f.conn, &root, id, &folders(&["docs/deep", "top.txt"])).unwrap());
        let key = authenticate(&f.conn, &token, NOW).unwrap().unwrap();
        assert_eq!(key.folders, folders(&["docs/deep", "top.txt"]), "the secret keeps working with the new folders");
        assert!(check(&f.root, &key.scope(), "top.txt").is_some());
        assert!(check(&f.root, &key.scope(), "docs/a.txt").is_none());
        assert!(set_folders(&mut f.conn, &root, id, &[]).is_err());
        assert!(set_folders(&mut f.conn, &root, id, &folders(&["secret/../.ssh"])).is_err());
    }

    fn make(f: &mut Fixture, list: &[&str]) -> String {
        let root = f.root.clone();
        let user = f.user;
        create(&mut f.conn, &root, user, "test", "read", false, &folders(list), None, NOW).unwrap().1
    }

    #[test]
    fn the_closest_folder_decides_what_a_key_may_change() {
        let key = AgentKey {
            id: 1,
            name: "k".into(),
            level: WRITE.into(),
            approve: false,
            folders: vec!["Archive".into(), "Archive/Inbox".into(), "Work".into()],
            excluded: Vec::new(),
            read_only: vec!["Archive".into()],
            tools: Some(vec!["read_text".into()]),
            calls_per_minute: CALLS_PER_MINUTE,
            daily_changes: DAILY_CHANGES,
            extensions: None,
        };
        assert!(!key.may_change("Archive/old.txt"));
        assert!(!key.may_change("Archive"));
        assert!(key.may_change("Archive/Inbox/new.txt"), "a writable folder inside a read-only one");
        assert!(key.may_change("Work/a.txt"));
        assert!(!key.may_change("Archived/a.txt"), "not one of its folders");
        assert!(key.may_use("read_text"));
        assert!(!key.may_use("search"));
    }

    #[test]
    fn only_the_given_folders_are_allowed() {
        let f = fixture();
        let docs = folders(&["docs"]);
        assert_eq!(check(&f.root, &in_scope(&docs), "docs/a.txt").unwrap().rel, "docs/a.txt");
        assert_eq!(check(&f.root, &in_scope(&docs), "docs/deep/b.txt").unwrap().rel, "docs/deep/b.txt");
        assert!(check(&f.root, &in_scope(&docs), "docs").is_some());
        for refused in ["", "top.txt", "secret/s.txt", "docs2/c.txt", "docs2", "docs/../secret/s.txt", "../outside/o.txt", "docs/../../outside/o.txt", "/etc/passwd", "docs/missing.txt"] {
            assert!(check(&f.root, &in_scope(&docs), refused).is_none(), "{refused} should be refused");
        }
    }

    #[test]
    fn hidden_files_and_the_trash_are_refused_even_for_the_whole_library() {
        let f = fixture();
        let all = folders(&[""]);
        assert!(check(&f.root, &in_scope(&all), "top.txt").is_some());
        for refused in ["docs/.env", "docs/.hidden/h.txt", ".ssh/id", ".ssh", ".ownnas-trash", ".ownnas-trash/old/t.txt"] {
            assert!(check(&f.root, &in_scope(&all), refused).is_none(), "{refused} should be refused");
        }
    }

    #[cfg(unix)]
    #[test]
    fn links_are_judged_by_where_they_lead() {
        use std::os::unix::fs::symlink;
        let f = fixture();
        symlink(f.root.join("secret"), f.root.join("docs/to-secret")).unwrap();
        symlink(f.root.join("secret/s.txt"), f.root.join("docs/s-link.txt")).unwrap();
        symlink(f.root.join(".ssh/id"), f.root.join("docs/key-link")).unwrap();
        symlink(f.base.join("outside"), f.root.join("docs/to-outside")).unwrap();
        symlink(f.root.join("docs/a.txt"), f.root.join("docs/a-link.txt")).unwrap();
        let docs = folders(&["docs"]);
        for refused in ["docs/to-secret", "docs/to-secret/s.txt", "docs/s-link.txt", "docs/key-link", "docs/to-outside/o.txt"] {
            assert!(check(&f.root, &in_scope(&docs), refused).is_none(), "{refused} should be refused");
        }
        assert_eq!(check(&f.root, &in_scope(&docs), "docs/a-link.txt").unwrap().rel, "docs/a.txt");
        // Nor can a folder be given through a link to somewhere refused.
        assert!(folder_to_store(&f.root, "docs/key-link").is_err());
        assert_eq!(folder_to_store(&f.root, "docs/to-secret").unwrap(), "secret");
    }

    #[test]
    fn a_key_opens_only_while_it_is_valid() {
        let mut f = fixture();
        let token = make(&mut f, &["docs", "/docs/", "docs2"]);
        assert!(token.starts_with(PREFIX));
        let key = authenticate(&f.conn, &token, NOW).unwrap().unwrap();
        assert_eq!(key.folders, ["docs", "docs2"]);
        assert!(authenticate(&f.conn, "ownnas_mcp_wrong", NOW).unwrap().is_none());
        assert!(authenticate(&f.conn, &token[PREFIX.len()..], NOW).unwrap().is_none());

        let stored: String = f.conn.query_row("SELECT token_hash FROM agent_keys", [], |row| row.get(0)).unwrap();
        assert_ne!(stored, token, "only a hash is stored");

        assert!(revoke(&f.conn, key.id, NOW).unwrap());
        assert!(!revoke(&f.conn, key.id, NOW).unwrap());
        assert!(authenticate(&f.conn, &token, NOW).unwrap().is_none());
    }

    #[test]
    fn a_key_expires() {
        let mut f = fixture();
        let (root, user) = (f.root.clone(), f.user);
        let (_, token) = create(&mut f.conn, &root, user, "soon", "read", false, &folders(&["docs"]), Some(NOW + 10), NOW).unwrap();
        assert!(authenticate(&f.conn, &token, NOW + 9).unwrap().is_some());
        assert!(authenticate(&f.conn, &token, NOW + 10).unwrap().is_none());
        assert!(create(&mut f.conn, &root, user, "past", "read", false, &folders(&["docs"]), Some(NOW), NOW).is_err());
    }

    #[test]
    fn a_key_goes_with_the_administrator_who_made_it() {
        let mut f = fixture();
        db::create_user_with_admin(&f.conn, "second", "a-long-enough-passphrase", true).unwrap();
        let (root, second) = (f.root.clone(), user_id(&f.conn, "second"));
        let (_, token) = create(&mut f.conn, &root, second, "theirs", "read", false, &folders(&["docs"]), None, NOW).unwrap();
        assert!(authenticate(&f.conn, &token, NOW).unwrap().is_some());
        db::set_user_admin(&f.conn, "second", false).unwrap();
        assert!(authenticate(&f.conn, &token, NOW).unwrap().is_none());
        db::set_user_admin(&f.conn, "second", true).unwrap();
        assert!(authenticate(&f.conn, &token, NOW).unwrap().is_some());
        f.conn.execute("DELETE FROM users WHERE id = ?1", [second]).unwrap();
        assert!(authenticate(&f.conn, &token, NOW).unwrap().is_none());
    }

    #[test]
    fn making_a_key_checks_what_it_is_given() {
        let mut f = fixture();
        let (root, user) = (f.root.clone(), f.user);
        let mut try_make = |name: &str, level: &str, list: &[&str]| create(&mut f.conn, &root, user, name, level, false, &folders(list), None, NOW).map(|_| ());
        assert!(try_make("", "read", &["docs"]).is_err());
        assert!(try_make("bad", "edit", &["docs"]).is_err(), "only the levels there are");
        assert!(try_make("writer", "write", &["docs"]).is_ok());
        assert!(try_make("tidy", "organise", &["docs"]).is_ok());
        assert!(try_make("none", "read", &[]).is_err());
        assert!(try_make("file", "read", &["top.txt"]).is_ok(), "a key can be given a single file");
        assert!(try_make("missing", "read", &["nope"]).is_err());
        assert!(try_make("up", "read", &["../outside"]).is_err());
        assert!(try_make("hidden", "read", &[".ssh"]).is_err());
        assert!(try_make("trash", "read", &[".ownnas-trash/old"]).is_err());
        assert!(try_make("whole", "read", &[""]).is_ok());
    }

    #[test]
    fn folders_follow_moves_and_deletion_removes_them() {
        let mut f = fixture();
        let token = make(&mut f, &["docs/deep"]);
        let folders_now = |f: &Fixture| authenticate(&f.conn, &token, NOW).unwrap().unwrap().folders;

        db::rewrite_path_meta(&f.conn, "docs", "papers").unwrap();
        assert_eq!(folders_now(&f), ["papers/deep"]);
        db::rewrite_path_meta(&f.conn, "papers/deep", ".ownnas-trash/deep").unwrap();
        assert_eq!(folders_now(&f), [".ownnas-trash/deep"]);
        fs::create_dir_all(f.root.join(".ownnas-trash/deep")).unwrap();
        assert!(check(&f.root, &in_scope(&folders_now(&f)), ".ownnas-trash/deep").is_none(), "in the Trash it is out of reach");
        db::rewrite_path_meta(&f.conn, ".ownnas-trash/deep", "docs/deep").unwrap();
        assert_eq!(folders_now(&f), ["docs/deep"]);
        // A name that merely starts the same is left alone.
        db::rewrite_path_meta(&f.conn, "docs/dee", "docs/other").unwrap();
        assert_eq!(folders_now(&f), ["docs/deep"]);
        db::delete_path_meta(&f.conn, "docs").unwrap();
        assert!(folders_now(&f).is_empty(), "with no folders left the key reaches nothing");
    }

    #[test]
    fn the_log_outlives_a_revoked_key() {
        let mut f = fixture();
        let token = make(&mut f, &["docs"]);
        let key = authenticate(&f.conn, &token, NOW).unwrap().unwrap();
        record(&f.conn, &key, "read_text", "docs/a.txt", "", "ok", NOW).unwrap();
        record(&f.conn, &key, "read_text", &"x".repeat(5000), "", "denied", NOW + 1).unwrap();
        revoke(&f.conn, key.id, NOW + 2).unwrap();
        let log = read_log(&f.conn, Some(key.id), 10).unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].outcome, "denied");
        assert_eq!(log[0].path.len(), LOG_TEXT_LIMIT);
        assert_eq!(log[1].path, "docs/a.txt");
        assert_eq!(read_log(&f.conn, Some(key.id + 1), 10).unwrap().len(), 0);
        let listed = list(&f.conn).unwrap();
        assert_eq!(listed[0].created_by, "admin");
        assert_eq!(listed[0].last_used_at, Some(NOW));
        assert_eq!(listed[0].revoked_at, Some(NOW + 2));
    }

    #[test]
    fn an_exclusion_beats_an_allowance_inside_an_allowed_folder() {
        let f = fixture();
        fs::create_dir_all(f.root.join("docs/private/inner")).unwrap();
        fs::write(f.root.join("docs/private/inner/p.txt"), "x").unwrap();
        let scope = Scope { folders: folders(&["docs", "docs/private/inner"]), excluded: folders(&["docs/private", "docs/a.txt"]), extensions: None };
        assert!(check(&f.root, &scope, "docs/deep/b.txt").is_some(), "the rest of the folder is still allowed");
        assert!(check(&f.root, &scope, "docs").is_some());
        for refused in ["docs/a.txt", "docs/private", "docs/private/inner", "docs/private/inner/p.txt"] {
            assert!(check(&f.root, &scope, refused).is_none(), "{refused} should be refused: an exclusion wins, even over a folder given inside it");
        }
        assert!(scope.holds_excluded("docs") && scope.holds_excluded("docs/private") && !scope.holds_excluded("docs/deep"));
        assert!(!scope.holds_name("docs/private/x") && scope.holds_name("docs/privately") && !scope.holds_name("docs/a.txt"));
    }

    #[cfg(unix)]
    #[test]
    fn an_excluded_file_cannot_be_reached_through_a_link() {
        use std::os::unix::fs::symlink;
        let f = fixture();
        symlink(f.root.join("docs/a.txt"), f.root.join("docs/deep/a-link.txt")).unwrap();
        let scope = Scope { folders: folders(&["docs"]), excluded: folders(&["docs/a.txt"]), extensions: None };
        assert!(check(&f.root, &scope, "docs/deep/a-link.txt").is_none());
    }

    fn reach(f: &Fixture, path: &str, id: i64) -> (&'static str, Option<String>, bool) {
        let items = access(&f.conn, &f.root, &[path.to_string()]).unwrap();
        let found = items[0].keys.iter().find(|reach| reach.key_id == id).unwrap();
        (found.state, found.by.clone(), found.every_key)
    }

    #[test]
    fn rules_set_from_the_file_explorer_decide_what_keys_reach() {
        let mut f = fixture();
        let (root, user) = (f.root.clone(), f.user);
        let (one, t1) = create(&mut f.conn, &root, user, "one", "read", false, &folders(&["docs"]), None, NOW).unwrap();
        let (two, t2) = create(&mut f.conn, &root, user, "two", "read", false, &folders(&["docs2"]), None, NOW).unwrap();
        let scope_of = |f: &Fixture, token: &str| authenticate(&f.conn, token, NOW).unwrap().unwrap().scope();

        // Excluding for one key.
        set_rule(&mut f.conn, &root, &folders(&["docs/deep"]), &Keys::Chosen(vec![one]), Rule::Exclude).unwrap();
        assert!(check(&f.root, &scope_of(&f, &t1), "docs/deep/b.txt").is_none());
        assert!(check(&f.root, &scope_of(&f, &t1), "docs/a.txt").is_some());
        assert_eq!(reach(&f, "docs/deep/b.txt", one), ("excludedInside", Some("docs/deep".into()), false));
        assert_eq!(reach(&f, "docs/a.txt", one), ("inside", Some("docs".into()), false));
        assert_eq!(reach(&f, "docs/a.txt", two).0, "none");

        // Allowing for every key gives the item to each, and lifts its exact exclusion.
        set_rule(&mut f.conn, &root, &folders(&["top.txt", "docs/deep"]), &Keys::Every, Rule::Allow).unwrap();
        assert!(check(&f.root, &scope_of(&f, &t2), "top.txt").is_some());
        assert!(check(&f.root, &scope_of(&f, &t1), "docs/deep/b.txt").is_some());
        assert_eq!(reach(&f, "top.txt", two), ("allowed", Some("top.txt".into()), false));
        assert_eq!(authenticate(&f.conn, &t1, NOW).unwrap().unwrap().folders, folders(&["docs", "top.txt"]), "docs/deep was in docs already");

        // Excluding for every key holds for keys made later too, and beats their folders.
        set_rule(&mut f.conn, &root, &folders(&["docs/a.txt"]), &Keys::Every, Rule::Exclude).unwrap();
        let (_, t3) = create(&mut f.conn, &root, user, "later", "read", false, &folders(&["docs"]), None, NOW).unwrap();
        assert!(check(&f.root, &scope_of(&f, &t3), "docs/a.txt").is_none());
        assert!(check(&f.root, &scope_of(&f, &t1), "docs/a.txt").is_none());
        assert_eq!(reach(&f, "docs/a.txt", one), ("excluded", Some("docs/a.txt".into()), true));
        assert_eq!(excluded_for_every_key(&f.conn).unwrap(), folders(&["docs/a.txt"]));

        // Allowing it again for one key keeps it excluded for the others.
        set_rule(&mut f.conn, &root, &folders(&["docs/a.txt"]), &Keys::Chosen(vec![one]), Rule::Allow).unwrap();
        assert!(check(&f.root, &scope_of(&f, &t1), "docs/a.txt").is_some());
        assert!(check(&f.root, &scope_of(&f, &t3), "docs/a.txt").is_none());
        assert!(excluded_for_every_key(&f.conn).unwrap().is_empty());

        // An exclusion of a folder holding it still wins, and the person is told so.
        set_rule(&mut f.conn, &root, &folders(&["docs"]), &Keys::Chosen(vec![two]), Rule::Exclude).unwrap();
        let notes = set_rule(&mut f.conn, &root, &folders(&["docs/a.txt"]), &Keys::Chosen(vec![two]), Rule::Allow).unwrap();
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(check(&f.root, &scope_of(&f, &t2), "docs/a.txt").is_none());

        // Clearing takes away what was set on exactly those items, but not a key's last folder.
        set_rule(&mut f.conn, &root, &folders(&["docs", "docs/a.txt"]), &Keys::Chosen(vec![two]), Rule::Clear).unwrap();
        assert_eq!(reach(&f, "docs/a.txt", two).0, "none");
        assert!(check(&f.root, &scope_of(&f, &t2), "top.txt").is_some(), "other rules stay");
        let notes = set_rule(&mut f.conn, &root, &folders(&["docs"]), &Keys::Chosen(vec![one, 3]), Rule::Clear).unwrap();
        assert!(notes.iter().any(|note| note.contains("later")), "{notes:?}");
        set_rule(&mut f.conn, &root, &folders(&["docs/deep"]), &Keys::Every, Rule::Exclude).unwrap();
        set_rule(&mut f.conn, &root, &folders(&["docs/deep"]), &Keys::Chosen(vec![one]), Rule::Exclude).unwrap();
        set_rule(&mut f.conn, &root, &folders(&["docs/deep"]), &Keys::Every, Rule::Unexclude).unwrap();
        assert!(excluded_for_every_key(&f.conn).unwrap().is_empty());
        assert_eq!(reach(&f, "docs/deep", one).0, "excluded", "a key's own exclusion stays");
        let shown = marks(&f.conn, NOW).unwrap();
        assert_eq!(shown.shared.get("top.txt"), Some(&folders(&["one", "two"])));
        assert_eq!(shown.excluded.get("docs/deep"), Some(&folders(&["one"])));
        assert!(set_rule(&mut f.conn, &root, &folders(&[".ssh"]), &Keys::Every, Rule::Exclude).is_err(), "hidden things are out of reach already");
        assert!(set_rule(&mut f.conn, &root, &folders(&["docs"]), &Keys::Chosen(vec![99]), Rule::Allow).is_err());
    }

    #[test]
    fn exclusions_follow_moves_and_go_with_what_is_deleted() {
        let mut f = fixture();
        let token = make(&mut f, &["docs"]);
        let root = f.root.clone();
        set_rule(&mut f.conn, &root, &folders(&["docs/deep"]), &Keys::Every, Rule::Exclude).unwrap();
        db::rewrite_path_meta(&f.conn, "docs", "papers").unwrap();
        let key = authenticate(&f.conn, &token, NOW).unwrap().unwrap();
        assert_eq!(key.excluded, ["papers/deep"]);
        db::delete_path_meta(&f.conn, "papers/deep").unwrap();
        assert!(authenticate(&f.conn, &token, NOW).unwrap().unwrap().excluded.is_empty());
    }
}
