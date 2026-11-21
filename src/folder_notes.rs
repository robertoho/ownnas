//! Folder descriptions: a few words on what a folder holds ("Analiza wydawanych pozwoleń na
//! budowę myjni w okresie 2019-2025"), written for the folders that matter, so rarely used
//! documents can be found again by search and by Ask (`folder_ask.rs`).
//!
//! Descriptions are kept in the main database by path, and follow renames and moves (into the
//! Trash and back too) like folder appearance does (`db::rewrite_path_meta`). A folder without
//! a description belongs to the nearest described folder above it, so the working folders
//! inside a described one need nothing. A folder can be left out of Ask (with everything in it),
//! for noise or private folders.
//!
//! An SQLite full-text index (trigram, like the words in files) holds each description lower
//! case and without accents, so any word or part of one finds it, "pozwolen" as well as
//! "pozwoleń".

use std::collections::{HashMap, HashSet};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use unicode_normalization::UnicodeNormalization;

use crate::files::TRASH_DIR;

/// The longest description kept, in characters.
pub const MAX_DESCRIPTION: usize = 2000;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub path: String,
    pub description: String,
    pub leave_out: bool,
    pub updated_at: i64,
    pub updated_by: String,
}

/// Creates the tables (main database) and fills in the full-text index where rows lack it.
pub fn ensure(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS folder_notes (
            path TEXT PRIMARY KEY,
            description TEXT NOT NULL DEFAULT '',
            leave_out INTEGER NOT NULL DEFAULT 0,
            updated_at INTEGER NOT NULL,
            updated_by TEXT NOT NULL DEFAULT ''
        );",
    )
    .map_err(|_| "Could not create the folder descriptions".to_string())?;
    if fts(conn) {
        reindex_missing(conn)?;
    }
    Ok(())
}

/// Whether the full-text index is there (created on first use). False when this SQLite has no
/// FTS5: searches then read every description, which are few.
fn fts(conn: &Connection) -> bool {
    conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS main.folder_notes_fts USING fts5(folded, tokenize = 'trigram');
         CREATE TRIGGER IF NOT EXISTS main.folder_notes_gone AFTER DELETE ON folder_notes BEGIN
             DELETE FROM folder_notes_fts WHERE rowid = old.rowid;
         END;",
    )
    .is_ok()
}

/// Adds the descriptions the full-text index lacks (copied folders, and an index just made).
pub fn reindex_missing(conn: &Connection) -> Result<(), String> {
    let fail = |_| "Could not index the folder descriptions".to_string();
    let missing: Vec<(i64, String)> = {
        let mut stmt = conn
            .prepare("SELECT rowid, description FROM folder_notes WHERE description != '' AND rowid NOT IN (SELECT rowid FROM folder_notes_fts)")
            .map_err(fail)?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?))).map_err(fail)?;
        rows.filter_map(Result::ok).collect()
    };
    for (rowid, description) in missing {
        conn.execute("INSERT INTO folder_notes_fts(rowid, folded) VALUES (?1, ?2)", params![rowid, fold(&description)]).map_err(fail)?;
    }
    Ok(())
}

/// Lower case without accents, for comparing words written with or without them ("Łódź",
/// "lodz"). Letters that are not a letter plus an accent in Unicode are mapped by hand.
pub fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.nfkd() {
        if unicode_normalization::char::is_combining_mark(c) {
            continue;
        }
        match c {
            'ł' | 'Ł' => out.push('l'),
            'ø' | 'Ø' => out.push('o'),
            'đ' | 'Đ' => out.push('d'),
            'ß' => out.push_str("ss"),
            'æ' | 'Æ' => out.push_str("ae"),
            'œ' | 'Œ' => out.push_str("oe"),
            'ı' => out.push('i'),
            _ => out.extend(c.to_lowercase()),
        }
    }
    out
}

fn note_from(row: &rusqlite::Row) -> rusqlite::Result<Note> {
    Ok(Note { path: row.get(0)?, description: row.get(1)?, leave_out: row.get::<_, i64>(2)? != 0, updated_at: row.get(3)?, updated_by: row.get(4)? })
}

const COLUMNS: &str = "path, description, leave_out, updated_at, updated_by";

pub fn get(conn: &Connection, path: &str) -> Result<Option<Note>, String> {
    conn.query_row(&format!("SELECT {COLUMNS} FROM folder_notes WHERE path = ?1"), [path], note_from)
        .optional()
        .map_err(|_| "Could not read the folder description".to_string())
}

/// Changes a folder's description and/or whether it is left out of Ask (`None` keeps what it
/// was). A folder with neither is forgotten.
pub fn set(conn: &Connection, path: &str, description: Option<&str>, leave_out: Option<bool>, by: &str) -> Result<Option<Note>, String> {
    if path.is_empty() {
        return Err("Choose a folder inside the library".into());
    }
    let old = get(conn, path)?;
    let description = match description {
        Some(text) => clean(text)?,
        None => old.as_ref().map(|note| note.description.clone()).unwrap_or_default(),
    };
    let leave_out = leave_out.unwrap_or_else(|| old.as_ref().is_some_and(|note| note.leave_out));
    let fail = |_| "Could not save the folder description".to_string();
    let tx = conn.unchecked_transaction().map_err(fail)?;
    let has_fts = fts(&tx);
    if let Some(rowid) = tx.query_row("SELECT rowid FROM folder_notes WHERE path = ?1", [path], |row| row.get::<_, i64>(0)).optional().map_err(fail)? {
        if has_fts {
            tx.execute("DELETE FROM folder_notes_fts WHERE rowid = ?1", [rowid]).map_err(fail)?;
        }
    }
    if description.is_empty() && !leave_out {
        tx.execute("DELETE FROM folder_notes WHERE path = ?1", [path]).map_err(fail)?;
    } else {
        tx.execute(
            "INSERT INTO folder_notes(path, description, leave_out, updated_at, updated_by) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(path) DO UPDATE SET description = excluded.description, leave_out = excluded.leave_out,
                 updated_at = excluded.updated_at, updated_by = excluded.updated_by",
            params![path, description, leave_out as i64, crate::db::now_secs(), by],
        )
        .map_err(fail)?;
        if has_fts && !description.is_empty() {
            let rowid: i64 = tx.query_row("SELECT rowid FROM folder_notes WHERE path = ?1", [path], |row| row.get(0)).map_err(fail)?;
            // A row left by a REPLACE elsewhere could hold this rowid: it is cleared first.
            tx.execute("DELETE FROM folder_notes_fts WHERE rowid = ?1", [rowid]).map_err(fail)?;
            tx.execute("INSERT INTO folder_notes_fts(rowid, folded) VALUES (?1, ?2)", params![rowid, fold(&description)]).map_err(fail)?;
        }
    }
    tx.commit().map_err(fail)?;
    get(conn, path)
}

/// One paragraph: spaces at the ends dropped, line breaks kept, at most `MAX_DESCRIPTION`.
fn clean(text: &str) -> Result<String, String> {
    let text = text.trim();
    if text.chars().count() > MAX_DESCRIPTION {
        return Err(format!("Keep the description under {MAX_DESCRIPTION} characters"));
    }
    if text.chars().any(|c| c.is_control() && c != '\n' && c != '\t') {
        return Err("The description has characters that cannot be shown".into());
    }
    Ok(text.replace("\r\n", "\n"))
}

/// Notes for these paths (their own only, not inherited).
pub fn for_paths(conn: &Connection, paths: &[String]) -> Result<HashMap<String, Note>, String> {
    let mut found = HashMap::new();
    for chunk in paths.chunks(200) {
        let placeholders = vec!["?"; chunk.len()].join(",");
        let mut stmt = conn
            .prepare(&format!("SELECT {COLUMNS} FROM folder_notes WHERE path IN ({placeholders})"))
            .map_err(|_| "Could not read folder descriptions".to_string())?;
        let rows = stmt.query_map(rusqlite::params_from_iter(chunk.iter()), note_from).map_err(|_| "Could not read folder descriptions".to_string())?;
        for note in rows.filter_map(Result::ok) {
            found.insert(note.path.clone(), note);
        }
    }
    Ok(found)
}

/// Every note outside the Trash.
pub fn all(conn: &Connection) -> Result<Vec<Note>, String> {
    let mut stmt = conn
        .prepare(&format!("SELECT {COLUMNS} FROM folder_notes WHERE NOT (path = ?1 OR (path >= ?1 || '/' AND path < ?1 || '0')) ORDER BY path"))
        .map_err(|_| "Could not read folder descriptions".to_string())?;
    let rows = stmt.query_map([TRASH_DIR], note_from).map_err(|_| "Could not read folder descriptions".to_string())?;
    Ok(rows.filter_map(Result::ok).collect())
}

/// `path` and the folders above it, nearest first ("a/b/c", "a/b", "a").
pub fn self_and_above(path: &str) -> impl Iterator<Item = &str> {
    let mut next = Some(path).filter(|path| !path.is_empty());
    std::iter::from_fn(move || {
        let current = next?;
        next = current.rsplit_once('/').map(|(parent, _)| parent);
        Some(current)
    })
}

/// What a folder's own note, and the notes above it, say about it.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Standing {
    /// Its own note, if it has one.
    pub own: Option<Note>,
    /// The nearest folder above with a description, when it has none of its own: it belongs
    /// there.
    pub inherited: Option<Note>,
    /// The folder (itself or one above) that leaves it out of Ask.
    pub left_out_by: Option<String>,
}

pub fn standing(conn: &Connection, path: &str) -> Result<Standing, String> {
    let chain: Vec<String> = self_and_above(path).map(String::from).collect();
    let notes = for_paths(conn, &chain)?;
    let own = notes.get(path).cloned();
    let inherited = if own.as_ref().is_some_and(|note| !note.description.is_empty()) {
        None
    } else {
        chain.iter().skip(1).find_map(|above| notes.get(above).filter(|note| !note.description.is_empty()).cloned())
    };
    let left_out_by = chain.iter().find(|at| notes.get(at.as_str()).is_some_and(|note| note.leave_out)).cloned();
    Ok(Standing { own, inherited, left_out_by })
}

/// The described folders and the left-out ones, to tell where a file belongs.
#[derive(Default)]
pub struct Map {
    described: HashSet<String>,
    left_out: HashSet<String>,
}

impl Map {
    pub fn new(notes: &[Note]) -> Self {
        Self {
            described: notes.iter().filter(|note| !note.description.is_empty()).map(|note| note.path.clone()).collect(),
            left_out: notes.iter().filter(|note| note.leave_out).map(|note| note.path.clone()).collect(),
        }
    }

    /// Whether the folder itself or one above it is left out of Ask.
    pub fn left_out(&self, path: &str) -> bool {
        self_and_above(path).any(|at| self.left_out.contains(at))
    }

    /// The described folder a path belongs to: itself or the nearest one above with a
    /// description. `None` when there is none, or the path is left out of Ask.
    pub fn owner<'a>(&self, path: &'a str) -> Option<&'a str> {
        let mut owner = None;
        for at in self_and_above(path) {
            if self.left_out.contains(at) {
                return None;
            }
            if owner.is_none() && self.described.contains(at) {
                owner = Some(at);
            }
        }
        owner
    }
}

/// Described folders under `rel` (outside the Trash) with every word of `query` (or part of a
/// word) in their description, with or without accents. The description goes with each.
pub fn search(conn: &Connection, rel: &str, query: &str, limit: usize) -> Result<Vec<(String, String)>, String> {
    let words: Vec<String> = fold(query).split_whitespace().map(String::from).collect();
    if words.is_empty() || query.trim().chars().count() < 2 {
        return Ok(Vec::new());
    }
    let fail = |_| "Could not search the folder descriptions".to_string();
    let under = "(?1 = '' OR n.path >= ?1 || '/' AND n.path < ?1 || '0') AND NOT (n.path = ?2 OR (n.path >= ?2 || '/' AND n.path < ?2 || '0'))";
    // The index needs three letters; shorter words are checked on what it finds.
    let indexed: Vec<&String> = words.iter().filter(|word| word.chars().count() >= 3).collect();
    let mut found: Vec<(String, String)> = if !indexed.is_empty() && fts(conn) {
        let clauses: Vec<String> = (0..indexed.len()).map(|at| format!("x.folded GLOB ?{}", at + 4)).collect();
        let sql = format!(
            "SELECT n.path, n.description FROM folder_notes_fts x JOIN folder_notes n ON n.rowid = x.rowid
             WHERE {} AND {under} ORDER BY n.path LIMIT ?3",
            clauses.join(" AND ")
        );
        let mut args: Vec<rusqlite::types::Value> = vec![rel.to_string().into(), TRASH_DIR.to_string().into(), (limit as i64 * 4).into()];
        args.extend(indexed.iter().map(|word| glob(word).into()));
        let mut stmt = conn.prepare(&sql).map_err(fail)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), |row| Ok((row.get(0)?, row.get(1)?))).map_err(fail)?;
        rows.filter_map(Result::ok).collect()
    } else {
        let mut stmt = conn
            .prepare(&format!("SELECT n.path, n.description FROM folder_notes n WHERE n.description != '' AND {under} ORDER BY n.path"))
            .map_err(fail)?;
        let rows = stmt.query_map(params![rel, TRASH_DIR], |row| Ok((row.get(0)?, row.get(1)?))).map_err(fail)?;
        rows.filter_map(Result::ok).collect()
    };
    found.retain(|(_, description)| {
        let folded = fold(description);
        words.iter().all(|word| folded.contains(word.as_str()))
    });
    found.truncate(limit);
    Ok(found)
}

/// `*text*` for GLOB, its own characters taken literally.
fn glob(text: &str) -> String {
    let mut out = String::from("*");
    for c in text.chars() {
        if matches!(c, '*' | '?' | '[') {
            out.push('[');
            out.push(c);
            out.push(']');
        } else {
            out.push(c);
        }
    }
    out.push('*');
    out
}

/// A first draft of a description, made without a model from what the indexes know: the
/// subfolders, the kinds of files, the words file names share, and the words found most in the
/// files' text here but not everywhere. Labels aside, it is in the library's own language.
pub fn suggest(conn: &Connection, path: &str) -> Result<String, String> {
    let fail = |err: rusqlite::Error| err.to_string();
    let (from, to) = (format!("{path}/"), format!("{path}0"));
    let mut parts = Vec::new();

    let mut stmt = conn
        .prepare("SELECT path FROM file_index WHERE parent = ?1 AND dir = 1 ORDER BY name_lower LIMIT 13")
        .map_err(fail)?;
    let subfolders: Vec<String> = stmt
        .query_map([path], |row| row.get::<_, String>(0))
        .map_err(fail)?
        .filter_map(Result::ok)
        .map(|sub| sub.rsplit('/').next().unwrap_or(&sub).to_string())
        .filter(|name| !name.starts_with('.'))
        .collect();
    if !subfolders.is_empty() {
        let more = if subfolders.len() > 12 { ", …" } else { "" };
        parts.push(format!("Subfolders: {}{more}.", subfolders.iter().take(12).cloned().collect::<Vec<_>>().join(", ")));
    }

    let mut stmt = conn
        .prepare("SELECT name_lower FROM file_index WHERE path >= ?1 AND path < ?2 AND dir = 0 LIMIT 20000")
        .map_err(fail)?;
    let names: Vec<String> = stmt.query_map([&from, &to], |row| row.get::<_, String>(0)).map_err(fail)?.filter_map(Result::ok).collect();
    if !names.is_empty() {
        let mut kinds: HashMap<String, usize> = HashMap::new();
        let mut name_words: HashMap<String, usize> = HashMap::new();
        for name in &names {
            let (stem, ext) = match name.rsplit_once('.') {
                Some((stem, ext)) if !stem.is_empty() && ext.len() <= 5 => (stem, ext.to_uppercase()),
                _ => (name.as_str(), String::new()),
            };
            if !ext.is_empty() {
                *kinds.entry(ext).or_default() += 1;
            }
            let distinct: HashSet<&str> = words_of(stem).collect();
            for word in distinct {
                *name_words.entry(word.to_string()).or_default() += 1;
            }
        }
        let mut kinds: Vec<(String, usize)> = kinds.into_iter().collect();
        kinds.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let total = if names.len() >= 20000 { "20,000+ files".to_string() } else if names.len() == 1 { "1 file".to_string() } else { format!("{} files", names.len()) };
        let listed: Vec<String> = kinds.iter().take(5).map(|(ext, count)| format!("{count} {ext}")).collect();
        parts.push(if listed.is_empty() { format!("{total}.") } else { format!("{total}: {}.", listed.join(", ")) });
        let floor = if names.len() < 3 { 1 } else { 2 };
        let common = top_words(name_words.into_iter().filter(|(_, count)| *count >= floor), 8);
        if !common.is_empty() {
            parts.push(format!("Often in file names: {}.", common.join(", ")));
        }
    }

    let common = common_text_words(conn, &from, &to)?;
    if !common.is_empty() {
        parts.push(format!("Common words: {}.", common.join(", ")));
    }
    Ok(parts.join(" "))
}

/// Words of three letters or more, lower case, numbers left out (dates and counts say little).
fn words_of(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !c.is_alphanumeric()).filter(|word| word.chars().count() >= 3 && word.chars().any(char::is_alphabetic) && !word.chars().all(|c| c.is_ascii_hexdigit() && word.len() > 8))
}

fn top_words(counts: impl Iterator<Item = (String, usize)>, take: usize) -> Vec<String> {
    let mut counts: Vec<(String, usize)> = counts.collect();
    counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    counts.into_iter().take(take).map(|(word, _)| word).collect()
}

/// The words in more of the folder's files than of the library's (a sample of it): those say
/// what the folder is about, while words every file has ("the", "jest", "para") do not.
fn common_text_words(conn: &Connection, from: &str, to: &str) -> Result<Vec<String>, String> {
    let fail = |err: rusqlite::Error| err.to_string();
    let read = |sql: &str, args: &[&dyn rusqlite::ToSql]| -> Result<Vec<String>, String> {
        let mut stmt = conn.prepare(sql).map_err(fail)?;
        let rows = stmt.query_map(args, |row| row.get::<_, String>(0)).map_err(fail)?;
        Ok(rows.filter_map(Result::ok).collect())
    };
    let here = read("SELECT substr(body, 1, 20000) FROM content_text WHERE path >= ?1 AND path < ?2 AND body != '' LIMIT 300", &[&from, &to])?;
    if here.is_empty() {
        return Ok(Vec::new());
    }
    let highest: i64 = conn.query_row("SELECT COALESCE(max(rowid), 0) FROM content_text", [], |row| row.get(0)).map_err(fail)?;
    let picks: Vec<String> = (0..400).map(|_| (rand::random::<u64>() % highest.max(1) as u64 + 1).to_string()).collect();
    let library = read(&format!("SELECT substr(body, 1, 20000) FROM content_text WHERE rowid IN ({}) AND body != ''", picks.join(",")), &[])?;
    let frequency = |texts: &[String]| -> HashMap<String, usize> {
        let mut counts: HashMap<String, usize> = HashMap::new();
        for text in texts {
            let lower = text.to_lowercase();
            let distinct: HashSet<&str> = words_of(&lower).filter(|word| word.chars().count() >= 4).collect();
            for word in distinct {
                *counts.entry(word.to_string()).or_default() += 1;
            }
        }
        counts
    };
    let (in_here, in_library) = (frequency(&here), frequency(&library));
    let (n_here, n_library) = (here.len() as f32, library.len().max(1) as f32);
    let floor = if here.len() < 4 { 1 } else { 2 };
    let mut scored: Vec<(String, f32)> = in_here
        .into_iter()
        .filter(|(_, count)| *count >= floor)
        .map(|(word, count)| {
            let elsewhere = in_library.get(&word).copied().unwrap_or(0) as f32 / n_library;
            let share = count as f32 / n_here;
            (word, share - elsewhere)
        })
        .filter(|(_, score)| *score > 0.15)
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    Ok(scored.into_iter().take(10).map(|(word, _)| word).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn conn() -> Connection {
        let dir = std::env::temp_dir().join(format!("ownnas-notes-{}-{}", std::process::id(), rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        crate::db::open(&dir.join("ownnas.db")).unwrap()
    }

    fn found(conn: &Connection, rel: &str, query: &str) -> Vec<String> {
        search(conn, rel, query, 20).unwrap().into_iter().map(|(path, _)| path).collect()
    }

    #[test]
    fn descriptions_are_found_by_any_part_of_any_word() {
        let conn = conn();
        set(&conn, "Firma/Urząd/Myjnie", Some("Analiza wydawanych pozwoleń na budowę myjni w okresie 2019-2025"), None, "ana").unwrap();
        set(&conn, "Firma/Kadry", Some("Umowy o pracę i aneksy"), None, "ana").unwrap();
        set(&conn, "Home/Car", Some("Car wash receipts"), None, "ana").unwrap();
        assert_eq!(found(&conn, "", "pozwolen"), ["Firma/Urząd/Myjnie"], "without the accent");
        assert_eq!(found(&conn, "", "POZWOLEŃ"), ["Firma/Urząd/Myjnie"], "any case, with it");
        assert_eq!(found(&conn, "", "myjni 2019"), ["Firma/Urząd/Myjnie"], "every word, anywhere");
        assert_eq!(found(&conn, "", "budow"), ["Firma/Urząd/Myjnie"], "part of a word");
        assert!(found(&conn, "", "myjni 2030").is_empty());
        assert_eq!(found(&conn, "", "wash"), ["Home/Car"]);
        assert_eq!(found(&conn, "Firma", "an"), ["Firma/Kadry", "Firma/Urząd/Myjnie"], "short words, under a folder");
        assert!(found(&conn, "Home", "umowy").is_empty(), "only under the folder");
        // Changing the description changes what finds it; emptying it forgets the folder.
        set(&conn, "Home/Car", Some("Insurance"), None, "ana").unwrap();
        assert!(found(&conn, "", "wash").is_empty());
        assert_eq!(found(&conn, "", "insur"), ["Home/Car"]);
        set(&conn, "Home/Car", Some(""), None, "ana").unwrap();
        assert!(get(&conn, "Home/Car").unwrap().is_none());
        assert!(found(&conn, "", "insur").is_empty());
        assert!(set(&conn, "Home", Some(&"x".repeat(MAX_DESCRIPTION + 1)), None, "ana").is_err());
    }

    #[test]
    fn descriptions_follow_moves_copies_and_the_trash() {
        let conn = conn();
        set(&conn, "Work/Permits", Some("Car wash permits"), None, "ana").unwrap();
        set(&conn, "Work/Permits/Drafts", None, Some(true), "ana").unwrap();
        crate::db::rewrite_path_meta(&conn, "Work", "Archive 2025").unwrap();
        assert!(get(&conn, "Work/Permits").unwrap().is_none());
        assert_eq!(get(&conn, "Archive 2025/Permits").unwrap().unwrap().description, "Car wash permits");
        assert!(get(&conn, "Archive 2025/Permits/Drafts").unwrap().unwrap().leave_out);
        assert_eq!(found(&conn, "", "wash"), ["Archive 2025/Permits"], "search follows the move");

        crate::db::copy_path_meta(&conn, "Archive 2025/Permits", "Copy").unwrap();
        assert_eq!(found(&conn, "", "wash"), ["Archive 2025/Permits", "Copy"], "a copy is described and found too");

        let trashed = format!("{TRASH_DIR}/1/Permits");
        crate::db::rewrite_path_meta(&conn, "Archive 2025/Permits", &trashed).unwrap();
        assert_eq!(found(&conn, "", "wash"), ["Copy"], "nothing in the Trash is found");
        assert!(all(&conn).unwrap().iter().all(|note| !note.path.starts_with(TRASH_DIR)));
        crate::db::rewrite_path_meta(&conn, &trashed, "Back").unwrap();
        assert_eq!(found(&conn, "", "wash"), ["Back", "Copy"], "restored with its description");

        crate::db::delete_path_meta(&conn, "Back").unwrap();
        assert!(get(&conn, "Back").unwrap().is_none());
        assert!(get(&conn, "Back/Drafts").unwrap().is_none());
        assert_eq!(found(&conn, "", "wash"), ["Copy"]);
    }

    #[test]
    fn folders_belong_to_the_nearest_described_one_above() {
        let conn = conn();
        set(&conn, "Firma", Some("Company documents"), None, "ana").unwrap();
        set(&conn, "Firma/Myjnie", Some("Car wash permits 2019-2025"), None, "ana").unwrap();
        set(&conn, "Firma/Myjnie/Private", None, Some(true), "ana").unwrap();
        let standing = standing(&conn, "Firma/Myjnie/2021/Wnioski").unwrap();
        assert!(standing.own.is_none());
        assert_eq!(standing.inherited.unwrap().path, "Firma/Myjnie", "the nearest one, not the top");
        assert!(standing.left_out_by.is_none());
        let own = super::standing(&conn, "Firma/Myjnie").unwrap();
        assert_eq!(own.own.unwrap().description, "Car wash permits 2019-2025");
        assert!(own.inherited.is_none());
        assert_eq!(super::standing(&conn, "Firma/Myjnie/Private/x").unwrap().left_out_by.as_deref(), Some("Firma/Myjnie/Private"));

        let map = Map::new(&all(&conn).unwrap());
        assert_eq!(map.owner("Firma/Myjnie/2021/Wnioski/a.pdf"), Some("Firma/Myjnie"));
        assert_eq!(map.owner("Firma/Kadry/umowa.pdf"), Some("Firma"));
        assert_eq!(map.owner("Firma/Myjnie/Private/secret.pdf"), None, "left out, with what is inside");
        assert_eq!(map.owner("Home/a.pdf"), None);
        assert!(map.left_out("Firma/Myjnie/Private"));
        assert!(!map.left_out("Firma/Myjnie"));
    }

    #[test]
    fn a_draft_comes_from_names_kinds_and_words() {
        let conn = conn();
        let add = |path: &str, dir: bool| {
            let parent = path.rsplit_once('/').map(|(parent, _)| parent).unwrap_or("");
            let name = path.rsplit('/').next().unwrap().to_lowercase();
            conn.execute(
                "INSERT INTO file_index(path, parent, name_lower, dir, size, mtime, kind) VALUES (?1, ?2, ?3, ?4, 1, 1, 'file')",
                params![path, parent, name, dir as i64],
            )
            .unwrap();
        };
        add("Myjnie", true);
        add("Myjnie/2019", true);
        add("Myjnie/2020", true);
        for (at, name) in ["Pozwolenie myjnia Kraków.pdf", "Pozwolenie myjnia Gdańsk.pdf", "Decyzja myjnia.pdf", "zestawienie.xlsx"].iter().enumerate() {
            let path = format!("Myjnie/{}/{name}", 2019 + at % 2);
            add(&path, false);
            conn.execute(
                "INSERT INTO content_text(path, size, mtime, body, indexed_at) VALUES (?1, 1, 1, ?2, 1)",
                params![path, "Decyzja o pozwoleniu na budowę myjni samochodowej, jest to dokument"],
            )
            .unwrap();
        }
        for at in 0..20 {
            conn.execute(
                "INSERT INTO content_text(path, size, mtime, body, indexed_at) VALUES (?1, 1, 1, 'jest to dokument o czymś innym', 1)",
                params![format!("Other/{at}.txt")],
            )
            .unwrap();
        }
        let draft = suggest(&conn, "Myjnie").unwrap();
        assert!(draft.contains("Subfolders: 2019, 2020."), "{draft}");
        assert!(draft.contains("4 files: 3 PDF, 1 XLSX."), "{draft}");
        assert!(draft.contains("Often in file names: myjnia, pozwolenie."), "{draft}");
        assert!(draft.contains("Common words:") && draft.contains("myjni") && draft.contains("samochodowej"), "{draft}");
        assert!(!draft.contains("dokument"), "words every file has are left out: {draft}");
    }
}
