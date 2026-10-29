//! The words inside files (text, PDFs, documents, books, emails, and photos read with OCR), read
//! in the background into the database so a search finds them at once. It follows the search
//! index of names (`file_index`): after each of its scans, files that are new or changed since
//! they were read are read again, at the indexing speed chosen in Settings.
//!
//! The text lives in `content_text`, and an SQLite full-text index over it in `content_fts`
//! (trigram, so any part of a word matches, as names do).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rusqlite::{params, Connection};
use serde::Serialize;

use crate::media_index::Speed;

/// The most text kept of one file.
const MAX_TEXT: usize = 1024 * 1024;
/// Text files larger than this are read only this far.
const MAX_TEXT_FILE: u64 = 2 * 1024 * 1024;
/// Files picked at a time. Finding them reads the whole name index, so not too few.
const BATCH: usize = 2000;

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub reading: bool,
    /// Files read in this pass, and how many were waiting when it started.
    pub done: u64,
    pub waiting: u64,
    /// Files whose words are in the index.
    pub files: u64,
    pub current: Option<String>,
    /// Whether photos are read too (tesseract is installed).
    pub ocr: bool,
    pub finished_at: Option<i64>,
    pub error: Option<String>,
    /// False when this SQLite has no full-text search: searches then read files as they go.
    pub available: bool,
}

#[derive(Default)]
pub struct TextIndex {
    status: Mutex<Status>,
    /// Set to read everything again (or a folder, after Reindex) without waiting for a scan.
    wake: AtomicBool,
}

/// Creates the full-text table. Returns false when SQLite was built without FTS5.
pub fn ensure(conn: &Connection) -> bool {
    conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS idx.content_fts USING fts5(lower, compact, tokenize = 'trigram');
         CREATE TRIGGER IF NOT EXISTS idx.content_text_gone AFTER DELETE ON content_text BEGIN
             DELETE FROM content_fts WHERE rowid = old.rowid;
         END;",
    )
    .is_ok()
}

impl TextIndex {
    pub fn status(&self) -> Status {
        self.status.lock().unwrap_or_else(|err| err.into_inner()).clone()
    }

    /// Reads `rel` (and what is in it) again, as Reindex asks.
    pub fn reread(&self, db_path: &Path, rel: &str) {
        if let Ok(conn) = crate::db::connect(db_path) {
            let _ = conn.execute(
                "DELETE FROM content_text WHERE ?1 = '' OR path = ?1 OR (path >= ?1 || '/' AND path < ?1 || '0')",
                params![rel],
            );
        }
        self.wake.store(true, Ordering::SeqCst);
    }

    /// Reads in a background thread after each scan of the name index.
    pub fn start(
        self: &Arc<Self>,
        db_path: PathBuf,
        root: PathBuf,
        cache_dir: PathBuf,
        ocr: bool,
        names: Arc<crate::file_index::FileIndex>,
        speed: impl Fn() -> Speed + Send + 'static,
    ) {
        let available = crate::db::connect(&db_path).map(|conn| ensure(&conn)).unwrap_or(false);
        {
            let mut status = self.status.lock().unwrap_or_else(|err| err.into_inner());
            status.ocr = ocr;
            status.available = available;
        }
        if !available {
            return;
        }
        let index = self.clone();
        std::thread::Builder::new()
            .name("ownnas-text-index".into())
            .spawn(move || {
                let mut last_scan = None;
                loop {
                    let scan = names.status();
                    let woken = index.wake.swap(false, Ordering::SeqCst);
                    if scan.ready && !scan.scanning && (woken || scan.finished_at != last_scan) {
                        last_scan = scan.finished_at;
                        index.run(&db_path, &root, &cache_dir, ocr, &speed);
                    }
                    std::thread::sleep(Duration::from_secs(5));
                }
            })
            .expect("could not start the text index thread");
    }

    fn run(&self, db_path: &Path, root: &Path, cache_dir: &Path, ocr: bool, speed: &dyn Fn() -> Speed) {
        let result = crate::db::connect(db_path).map_err(|err| err.to_string()).and_then(|conn| {
            let waiting = count_waiting(&conn, ocr)?;
            {
                let mut status = self.status.lock().unwrap_or_else(|err| err.into_inner());
                status.reading = waiting > 0;
                status.done = 0;
                status.waiting = waiting;
                status.error = None;
            }
            // Files that failed are kept (with no text), so a batch never comes back the same.
            loop {
                let batch = waiting_files(&conn, ocr, BATCH)?;
                if batch.is_empty() {
                    break;
                }
                for file in batch {
                    let started = Instant::now();
                    self.status.lock().unwrap_or_else(|err| err.into_inner()).current = Some(file.path.clone());
                    let text = read_text(&root.join(&file.path), &file.path, &file.kind, cache_dir, ocr).unwrap_or_default();
                    crate::db_watch::pause_point();
                    store(&conn, &file, &text)?;
                    {
                        let mut status = self.status.lock().unwrap_or_else(|err| err.into_inner());
                        status.done += 1;
                        status.waiting = status.waiting.max(status.done);
                    }
                    let rest = speed().rest(started.elapsed());
                    if !rest.is_zero() {
                        std::thread::sleep(rest);
                    }
                    if self.wake.load(Ordering::SeqCst) {
                        // Reindex asked for: start over with what it cleared.
                        return Ok(conn);
                    }
                }
            }
            forget_gone(&conn, root)?;
            Ok(conn)
        });
        let mut status = self.status.lock().unwrap_or_else(|err| err.into_inner());
        status.reading = false;
        status.current = None;
        match result {
            Ok(conn) => {
                status.files = conn.query_row("SELECT count(*) FROM content_text", [], |row| row.get::<_, i64>(0)).unwrap_or(0) as u64;
                status.finished_at = Some(crate::db::now_secs());
            }
            Err(err) => status.error = Some(err),
        }
    }
}

struct Waiting {
    path: String,
    kind: String,
    size: i64,
    mtime: i64,
}

/// The SQL that picks files the reader reads, from `file_index` as `f`.
fn readable_sql(ocr: bool) -> String {
    let names: Vec<String> = crate::documents::TEXT_FORMATS.iter().map(|ext| format!("f.name_lower GLOB '*.{ext}'")).collect();
    let image = if ocr { " OR f.kind = 'image'" } else { "" };
    format!("f.dir = 0 AND f.size > 0 AND (f.kind IN ('text', 'pdf'){image} OR {})", names.join(" OR "))
}

fn count_waiting(conn: &Connection, ocr: bool) -> Result<u64, String> {
    let sql = format!(
        "SELECT count(*) FROM file_index f LEFT JOIN content_text c ON c.path = f.path
         WHERE {} AND (c.path IS NULL OR c.size != f.size OR c.mtime != f.mtime)",
        readable_sql(ocr)
    );
    conn.query_row(&sql, [], |row| row.get::<_, i64>(0)).map(|n| n as u64).map_err(|err| err.to_string())
}

/// Files not read yet, or changed since: documents and text before photos (OCR is slow).
fn waiting_files(conn: &Connection, ocr: bool, limit: usize) -> Result<Vec<Waiting>, String> {
    let sql = format!(
        "SELECT f.path, f.kind, f.size, f.mtime FROM file_index f LEFT JOIN content_text c ON c.path = f.path
         WHERE {} AND (c.path IS NULL OR c.size != f.size OR c.mtime != f.mtime)
         ORDER BY f.kind = 'image', f.path LIMIT ?1",
        readable_sql(ocr)
    );
    let mut stmt = conn.prepare(&sql).map_err(|err| err.to_string())?;
    let rows = stmt
        .query_map(params![limit as i64], |row| Ok(Waiting { path: row.get(0)?, kind: row.get(1)?, size: row.get(2)?, mtime: row.get(3)? }))
        .map_err(|err| err.to_string())?;
    Ok(rows.filter_map(Result::ok).collect())
}

/// The words in one file, or `None` when it has none that can be read.
fn read_text(full: &Path, rel: &str, kind: &str, cache_dir: &Path, ocr: bool) -> Option<String> {
    let text = match kind {
        "text" => {
            use std::io::Read;
            let mut bytes = Vec::new();
            std::fs::File::open(full).ok()?.take(MAX_TEXT_FILE).read_to_end(&mut bytes).ok()?;
            if bytes.contains(&0) {
                return None;
            }
            String::from_utf8_lossy(&bytes).into_owned()
        }
        // PDFs and photos share the cache searches used before, so nothing read is read twice.
        "pdf" | "image" => crate::content_index::extract_for_search(full, kind, cache_dir, ocr)?.0,
        _ if crate::documents::has_text(rel) => crate::documents::plain_text(full)?,
        _ => return None,
    };
    Some(truncate(text))
}

fn truncate(mut text: String) -> String {
    if text.len() > MAX_TEXT {
        let mut end = MAX_TEXT;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

fn store(conn: &Connection, file: &Waiting, text: &str) -> Result<(), String> {
    let lower = text.to_lowercase();
    // PDFs often space their letters out ("H e l l o"): keep the words run together too.
    let compact: String = if file.kind == "pdf" { lower.chars().filter(|c| !c.is_whitespace()).collect() } else { String::new() };
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate).map_err(|err| err.to_string())?;
    tx.execute("DELETE FROM content_text WHERE path = ?1", params![file.path]).map_err(|err| err.to_string())?;
    tx.execute(
        "INSERT INTO content_text(path, size, mtime, body, indexed_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![file.path, file.size, file.mtime, text, crate::db::now_secs()],
    )
    .map_err(|err| err.to_string())?;
    if !text.trim().is_empty() {
        tx.execute("INSERT INTO content_fts(rowid, lower, compact) VALUES (last_insert_rowid(), ?1, ?2)", params![lower, compact])
            .map_err(|err| err.to_string())?;
    }
    tx.commit().map_err(|err| err.to_string())
}

/// Drops the words of files that are gone. One moved through OwnNAS keeps its words (they move
/// with it), even before the name index has seen it in its new place.
fn forget_gone(conn: &Connection, root: &Path) -> Result<(), String> {
    let mut stmt = conn
        .prepare("SELECT path FROM content_text WHERE path NOT IN (SELECT path FROM file_index)")
        .map_err(|err| err.to_string())?;
    let gone: Vec<String> = stmt
        .query_map([], |row| row.get(0))
        .map_err(|err| err.to_string())?
        .filter_map(Result::ok)
        .filter(|path: &String| !root.join(path).is_file())
        .collect();
    for path in gone {
        conn.execute("DELETE FROM content_text WHERE path = ?1", params![path]).map_err(|err| err.to_string())?;
    }
    Ok(())
}

/// A file whose words have the search in them, with the words around it.
pub struct ContentHit {
    pub path: String,
    pub snippet: Option<String>,
}

/// Files under `rel` whose words have `query` in them.
pub fn search(conn: &Connection, rel: &str, query: &str, limit: usize) -> Result<Vec<ContentHit>, String> {
    let lower = query.trim().to_lowercase();
    if lower.chars().count() < 2 {
        return Ok(Vec::new());
    }
    let compact: String = lower.chars().filter(|c| !c.is_whitespace()).collect();
    let mut stmt = conn
        .prepare(
            "SELECT c.path, c.body FROM content_fts x JOIN content_text c ON c.rowid = x.rowid
             WHERE (x.lower GLOB ?1 OR x.compact GLOB ?2)
               AND (?3 = '' OR (c.path >= ?3 || '/' AND c.path < ?3 || '0'))
             LIMIT ?4",
        )
        .map_err(|err| err.to_string())?;
    let rows = stmt
        .query_map(params![glob(&lower), glob(&compact), rel, limit as i64], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
        .map_err(|err| err.to_string())?;
    Ok(rows
        .filter_map(Result::ok)
        .map(|(path, body)| ContentHit { snippet: crate::files::content_snippet(&body, query), path })
        .collect())
}

/// `*text*` for GLOB, with GLOB's own characters taken literally.
fn glob(text: &str) -> String {
    let mut out = String::from("*");
    for c in text.chars() {
        match c {
            '*' | '?' | '[' => {
                out.push('[');
                out.push(c);
                out.push(']');
            }
            _ => out.push(c),
        }
    }
    out.push('*');
    out
}

/// Files under `rel` of these kinds whose words are not (or no longer) in the index, which a
/// search still has to read itself.
pub fn unread(conn: &Connection, rel: &str, kinds: &[&str], limit: usize) -> Vec<String> {
    let marks = kinds.iter().map(|kind| format!("'{kind}'")).collect::<Vec<_>>().join(", ");
    let sql = format!(
        "SELECT f.path FROM file_index f LEFT JOIN content_text c ON c.path = f.path AND c.size = f.size AND c.mtime = f.mtime
         WHERE f.dir = 0 AND f.kind IN ({marks}) AND c.path IS NULL
           AND (?1 = '' OR (f.path >= ?1 || '/' AND f.path < ?1 || '0'))
         ORDER BY length(f.path) - length(replace(f.path, '/', '')), f.path
         LIMIT ?2"
    );
    let Ok(mut stmt) = conn.prepare(&sql) else { return Vec::new() };
    stmt.query_map(params![rel, limit as i64], |row| row.get(0)).map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default()
}

/// Whether the index has any words at all.
#[cfg(test)]
fn has_any(conn: &Connection) -> bool {
    conn.query_row("SELECT count(*) FROM content_fts", [], |row| row.get::<_, i64>(0)).unwrap_or(0) > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let dir = std::env::temp_dir().join(format!("ownnas-text-{}-{}", std::process::id(), rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        let conn = crate::db::open(&dir.join("ownnas.db")).unwrap();
        assert!(ensure(&conn), "SQLite has FTS5");
        conn
    }

    fn add(conn: &Connection, path: &str, kind: &str, text: &str) {
        let file = Waiting { path: path.into(), kind: kind.into(), size: 1, mtime: 1 };
        store(conn, &file, text).unwrap();
    }

    #[test]
    fn finds_any_part_of_the_words_in_a_folder() {
        let conn = conn();
        add(&conn, "Work/report.docx", "file", "Quarterly Résumé of the Ñandú project");
        add(&conn, "Home/scan.pdf", "pdf", "I n v o i c e  number 42");
        add(&conn, "Home/notes.txt", "text", "buy 50% more [eggs]*");
        let paths = |rel: &str, q: &str| search(&conn, rel, q, 10).unwrap().into_iter().map(|hit| hit.path).collect::<Vec<_>>();
        assert_eq!(paths("", "résumé"), ["Work/report.docx"]);
        assert_eq!(paths("", "ÑANDÚ"), ["Work/report.docx"], "any case, accents included");
        assert_eq!(paths("", "terly"), ["Work/report.docx"], "inside a word");
        assert_eq!(paths("", "invoice"), ["Home/scan.pdf"], "spaced-out PDF letters");
        assert_eq!(paths("", "[eggs]*"), ["Home/notes.txt"], "GLOB characters taken literally");
        assert_eq!(paths("", "50%"), ["Home/notes.txt"]);
        assert!(paths("Work", "invoice").is_empty(), "only under the folder");
        assert_eq!(search(&conn, "", "ñandú", 10).unwrap()[0].snippet.as_deref().map(|s| s.contains("Ñandú")), Some(true));
    }

    #[test]
    fn words_follow_a_move_and_go_with_a_delete() {
        let conn = conn();
        add(&conn, "a/doc.txt", "text", "hello world");
        crate::db::rewrite_path_meta(&conn, "a", "b").unwrap();
        assert_eq!(search(&conn, "", "hello", 10).unwrap()[0].path, "b/doc.txt");
        crate::db::delete_path_meta(&conn, "b").unwrap();
        assert!(search(&conn, "", "hello", 10).unwrap().is_empty());
        assert!(!has_any(&conn), "the full-text rows go with it");
    }

    #[test]
    fn reads_word_and_opendocument_text() {
        let dir = std::env::temp_dir().join(format!("ownnas-docs-{}-{}", std::process::id(), rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        let write_zip = |name: &str, part: &str, xml: &str| {
            let file = std::fs::File::create(dir.join(name)).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            zip.start_file(part, zip::write::SimpleFileOptions::default()).unwrap();
            std::io::Write::write_all(&mut zip, xml.as_bytes()).unwrap();
            zip.finish().unwrap();
        };
        write_zip(
            "a.docx",
            "word/document.xml",
            r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>
              <w:p><w:r><w:t>Hello</w:t></w:r><w:r><w:t xml:space="preserve"> world</w:t></w:r></w:p>
              <w:p><w:r><w:t>Second</w:t></w:r></w:p></w:body></w:document>"#,
        );
        write_zip(
            "b.odt",
            "content.xml",
            r#"<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"><office:body><office:text><text:h>Title</text:h><text:p>One<text:s/>two</text:p></office:text></office:body></office:document-content>"#,
        );
        assert_eq!(crate::documents::plain_text(&dir.join("a.docx")).unwrap(), "Hello world\nSecond\n");
        assert_eq!(crate::documents::plain_text(&dir.join("b.odt")).unwrap(), "Title\nOne two\n");
        let _ = std::fs::remove_dir_all(dir);
    }
}
