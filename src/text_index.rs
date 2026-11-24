//! The words inside files (text, PDFs, documents, books, emails, and photos read with OCR), read
//! in the background into the database so a search finds them at once. It follows the search
//! index of names (`file_index`): after a scan that found something added, changed, or gone, the
//! files there that are new or changed since they were read are read again, at the indexing speed
//! chosen in Settings. Only at start, and after Reindex of the whole library, does it look
//! through every file name.
//!
//! The text lives in `content_text`, and an SQLite full-text index over it in `content_fts`
//! (trigram, so any part of a word matches, as names do).

use std::collections::BTreeSet;
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
/// Files picked at a time. Finding them in the whole library reads every file name, so not too few.
const BATCH: usize = 2000;
/// Words read are written to the database this many files at a time, or after `STORE_AFTER`:
/// each write adds to the search index's own pages, and fewer, larger writes keep the
/// database's write-ahead log from growing as fast.
const STORE_EVERY: usize = 32;
const STORE_AFTER: Duration = Duration::from_secs(20);

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

pub struct TextIndex {
    status: Mutex<Status>,
    /// Set to read again (after Reindex) without waiting for a scan, or to stop a pass part way.
    wake: AtomicBool,
    /// What to look through for files to read: the whole library at first, then only what the
    /// name index's scans find added, changed, or gone (looking through every file name takes
    /// seconds on a large library, so it is not done after each scan).
    changed: Mutex<Changed>,
}

#[derive(Default)]
struct Changed {
    all: bool,
    paths: BTreeSet<String>,
}

impl Changed {
    fn any(&self) -> bool {
        self.all || !self.paths.is_empty()
    }

    fn add(&mut self, rel: &str) {
        if self.all {
            return;
        }
        if rel.is_empty() || self.paths.len() >= MAX_CHANGED {
            self.all = true;
            self.paths.clear();
        } else {
            self.paths.insert(rel.to_string());
        }
    }

    fn put_back(&mut self, taken: Changed) {
        if taken.all {
            self.all = true;
            self.paths.clear();
        }
        for path in taken.paths {
            self.add(&path);
        }
    }

    /// Where to look: `None` for the whole library, else the paths with none inside another.
    fn scopes(&self) -> Vec<Option<String>> {
        if self.all {
            return vec![None];
        }
        let mut kept: Vec<String> = Vec::new();
        for path in &self.paths {
            if kept.last().is_some_and(|last| path.strip_prefix(last.as_str()).is_some_and(|rest| rest.starts_with('/'))) {
                continue;
            }
            kept.push(path.clone());
        }
        kept.into_iter().map(Some).collect()
    }
}

/// More changes than this between passes: the whole library is looked through instead.
const MAX_CHANGED: usize = 5000;

impl Default for TextIndex {
    fn default() -> Self {
        Self {
            status: Mutex::default(),
            wake: AtomicBool::new(false),
            changed: Mutex::new(Changed { all: true, paths: BTreeSet::new() }),
        }
    }
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
        self.changed(rel);
        self.wake.store(true, Ordering::SeqCst);
    }

    /// `rel` was added, changed, or is gone: look there in the next pass.
    fn changed(&self, rel: &str) {
        self.changed.lock().unwrap_or_else(|err| err.into_inner()).add(rel);
    }

    /// Reads in a background thread after each scan of the name index that found something.
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
        let told = Arc::downgrade(self);
        names.on_found(move |rel| {
            if let Some(index) = told.upgrade() {
                index.changed(rel);
            }
        });
        let index = self.clone();
        std::thread::Builder::new()
            .name("ownnas-text-index".into())
            .spawn(move || loop {
                let scan = names.status();
                index.wake.store(false, Ordering::SeqCst);
                let pending = index.changed.lock().unwrap_or_else(|err| err.into_inner()).any();
                if scan.ready && !scan.scanning && pending {
                    let taken = std::mem::take(&mut *index.changed.lock().unwrap_or_else(|err| err.into_inner()));
                    if let Err(taken) = index.run(&db_path, &root, &cache_dir, ocr, &speed, taken) {
                        // Tried again after a while, with whatever has changed since.
                        index.changed.lock().unwrap_or_else(|err| err.into_inner()).put_back(taken);
                        std::thread::sleep(Duration::from_secs(60));
                    }
                }
                std::thread::sleep(Duration::from_secs(5));
            })
            .expect("could not start the text index thread");
    }

    /// One pass through `taken`; gives it back when the pass failed or was stopped part way.
    fn run(&self, db_path: &Path, root: &Path, cache_dir: &Path, ocr: bool, speed: &dyn Fn() -> Speed, taken: Changed) -> Result<(), Changed> {
        {
            let mut status = self.status.lock().unwrap_or_else(|err| err.into_inner());
            status.done = 0;
            status.waiting = 0;
            status.error = None;
        }
        let mut stopped = false;
        let result = crate::db::connect(db_path).map_err(|err| err.to_string()).and_then(|conn| {
            let mut read: Vec<(Waiting, String)> = Vec::new();
            let mut first_read = Instant::now();
            let flush = |read: &mut Vec<(Waiting, String)>| -> Result<(), String> {
                if read.is_empty() {
                    return Ok(());
                }
                crate::db_watch::heavy_pause_point();
                store(&conn, read)?;
                let mut status = self.status.lock().unwrap_or_else(|err| err.into_inner());
                status.done += read.len() as u64;
                status.waiting = status.waiting.max(status.done);
                read.clear();
                Ok(())
            };
            'scopes: for scope in taken.scopes() {
                let scope = scope.as_deref();
                // Files that failed are kept (with no text), so a batch never comes back the same.
                let mut first = true;
                loop {
                    let batch = waiting_files(&conn, ocr, scope, BATCH)?;
                    let last = batch.len() < BATCH;
                    {
                        let mut status = self.status.lock().unwrap_or_else(|err| err.into_inner());
                        if first && !last && scope.is_none() {
                            // Many waiting: how many, for the progress shown in Settings.
                            drop(status);
                            let waiting = count_waiting(&conn, ocr)?;
                            status = self.status.lock().unwrap_or_else(|err| err.into_inner());
                            status.waiting = status.waiting.max(status.done + waiting);
                        } else if first || scope.is_some() {
                            status.waiting = status.waiting.max(status.done + read.len() as u64 + batch.len() as u64);
                        }
                        status.reading = status.waiting > status.done;
                    }
                    first = false;
                    for file in batch {
                        let started = Instant::now();
                        self.status.lock().unwrap_or_else(|err| err.into_inner()).current = Some(file.path.clone());
                        let text = read_text(&root.join(&file.path), &file.path, &file.kind, cache_dir, ocr).unwrap_or_default();
                        if read.is_empty() {
                            first_read = Instant::now();
                        }
                        read.push((file, text));
                        if read.len() >= STORE_EVERY || first_read.elapsed() >= STORE_AFTER {
                            flush(&mut read)?;
                        }
                        let rest = speed().rest(started.elapsed());
                        if !rest.is_zero() {
                            std::thread::sleep(rest);
                        }
                        if self.wake.load(Ordering::SeqCst) {
                            // Reindex asked for: start over with what it cleared.
                            flush(&mut read)?;
                            stopped = true;
                            break 'scopes;
                        }
                    }
                    flush(&mut read)?;
                    // Files changed while these were read are told by the next scan.
                    if last {
                        break;
                    }
                }
                forget_gone(&conn, root, scope)?;
            }
            Ok(conn)
        });
        let mut status = self.status.lock().unwrap_or_else(|err| err.into_inner());
        status.reading = false;
        status.current = None;
        match result {
            Ok(conn) => {
                status.files = conn.query_row("SELECT count(*) FROM content_text", [], |row| row.get::<_, i64>(0)).unwrap_or(0) as u64;
                status.finished_at = Some(crate::db::now_secs());
                drop(status);
                if stopped {
                    self.changed.lock().unwrap_or_else(|err| err.into_inner()).put_back(taken);
                }
                Ok(())
            }
            Err(err) => {
                status.error = Some(err);
                Err(taken)
            }
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

/// Looks at every file name in the library: seconds on a large one.
fn count_waiting(conn: &Connection, ocr: bool) -> Result<u64, String> {
    let sql = format!(
        "SELECT count(*) FROM file_index f LEFT JOIN content_text c ON c.path = f.path
         WHERE {} AND (c.path IS NULL OR c.size != f.size OR c.mtime != f.mtime)",
        readable_sql(ocr)
    );
    conn.query_row(&sql, [], |row| row.get::<_, i64>(0)).map(|n| n as u64).map_err(|err| err.to_string())
}

/// Files not read yet, or changed since, at or under `scope` (`None`: the whole library, which
/// looks at every file name): documents and text before photos (OCR is slow).
fn waiting_files(conn: &Connection, ocr: bool, scope: Option<&str>, limit: usize) -> Result<Vec<Waiting>, String> {
    let within = if scope.is_some() { "(f.path = ?2 OR (f.path >= ?2 || '/' AND f.path < ?2 || '0')) AND" } else { "" };
    let sql = format!(
        "SELECT f.path, f.kind, f.size, f.mtime FROM file_index f LEFT JOIN content_text c ON c.path = f.path
         WHERE {within} {} AND (c.path IS NULL OR c.size != f.size OR c.mtime != f.mtime)
         ORDER BY f.kind = 'image', f.path LIMIT ?1",
        readable_sql(ocr)
    );
    let mut stmt = conn.prepare(&sql).map_err(|err| err.to_string())?;
    let row = |row: &rusqlite::Row| Ok(Waiting { path: row.get(0)?, kind: row.get(1)?, size: row.get(2)?, mtime: row.get(3)? });
    let found: Vec<Waiting> = match scope {
        Some(scope) => stmt.query_map(params![limit as i64, scope], row).map_err(|err| err.to_string())?.filter_map(Result::ok).collect(),
        None => stmt.query_map(params![limit as i64], row).map_err(|err| err.to_string())?.filter_map(Result::ok).collect(),
    };
    Ok(found)
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

/// Writes the words of the files read, in one transaction.
fn store(conn: &Connection, read: &[(Waiting, String)]) -> Result<(), String> {
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate).map_err(|err| err.to_string())?;
    for (file, text) in read {
        let lower = text.to_lowercase();
        // PDFs often space their letters out ("H e l l o"): keep the words run together too.
        let compact: String = if file.kind == "pdf" { lower.chars().filter(|c| !c.is_whitespace()).collect() } else { String::new() };
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
    }
    tx.commit().map_err(|err| err.to_string())
}

/// Drops the words of files that are gone, at or under `scope` (`None`: anywhere). One moved
/// through OwnNAS keeps its words (they move with it), even before the name index has seen it in
/// its new place.
fn forget_gone(conn: &Connection, root: &Path, scope: Option<&str>) -> Result<(), String> {
    let within = if scope.is_some() { "(c.path = ?1 OR (c.path >= ?1 || '/' AND c.path < ?1 || '0')) AND" } else { "" };
    let mut stmt = conn
        .prepare(&format!("SELECT c.path FROM content_text c WHERE {within} NOT EXISTS (SELECT 1 FROM file_index f WHERE f.path = c.path)"))
        .map_err(|err| err.to_string())?;
    let path = |row: &rusqlite::Row| row.get::<_, String>(0);
    let known: Vec<String> = match scope {
        Some(scope) => stmt.query_map(params![scope], path).map_err(|err| err.to_string())?.filter_map(Result::ok).collect(),
        None => stmt.query_map([], path).map_err(|err| err.to_string())?.filter_map(Result::ok).collect(),
    };
    let gone: Vec<String> = known.into_iter().filter(|path| !root.join(path).is_file()).collect();
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
        store(conn, &[(file, text.to_string())]).unwrap();
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
    fn looks_only_where_a_scan_found_changes() {
        let mut changed = Changed::default();
        assert!(!changed.any());
        for path in ["Work/b.txt", "Work", "Workshop/c.txt", "Home/a.txt"] {
            changed.add(path);
        }
        assert_eq!(changed.scopes(), [Some("Home/a.txt".to_string()), Some("Work".into()), Some("Workshop/c.txt".into())], "one inside another is looked at once");
        changed.add("");
        assert_eq!(changed.scopes(), [None], "the whole library");
        let mut many = Changed::default();
        for n in 0..=MAX_CHANGED {
            many.add(&format!("f{n}"));
        }
        assert!(many.all && many.paths.is_empty(), "too many: the whole library");
        assert!(TextIndex::default().changed.lock().unwrap().all, "everything at first");

        let conn = conn();
        let file = |path: &str, mtime: i64| {
            conn.execute(
                "INSERT OR REPLACE INTO file_index(path, parent, name_lower, dir, size, mtime, kind) VALUES (?1, '', lower(?1), 0, 10, ?2, 'text')",
                params![path, mtime],
            )
            .unwrap();
        };
        file("Work/a.txt", 1);
        file("Work/Old/b.txt", 1);
        file("Workshop/c.txt", 1);
        file("Home/report.docx", 1);
        let waiting = |scope: Option<&str>| waiting_files(&conn, false, scope, 100).unwrap().into_iter().map(|file| file.path).collect::<Vec<_>>();
        assert_eq!(waiting(None), ["Home/report.docx", "Work/Old/b.txt", "Work/a.txt", "Workshop/c.txt"]);
        assert_eq!(waiting(Some("Work")), ["Work/Old/b.txt", "Work/a.txt"], "not the folder next to it");
        assert_eq!(waiting(Some("Work/a.txt")), ["Work/a.txt"]);
        store(&conn, &[(Waiting { path: "Work/a.txt".into(), kind: "text".into(), size: 10, mtime: 1 }, "x".into())]).unwrap();
        assert_eq!(waiting(Some("Work")), ["Work/Old/b.txt"], "read already");
        file("Work/a.txt", 2);
        assert_eq!(waiting(Some("Work")), ["Work/Old/b.txt", "Work/a.txt"], "changed since");

        // Words of a file gone go when its place is looked at, or the whole library is.
        add(&conn, "Gone/x.txt", "text", "gone");
        add(&conn, "Elsewhere/y.txt", "text", "gone too");
        let root = std::env::temp_dir();
        forget_gone(&conn, &root, Some("Gone")).unwrap();
        assert_eq!(search(&conn, "", "gone", 10).unwrap().len(), 1);
        forget_gone(&conn, &root, None).unwrap();
        assert!(search(&conn, "", "gone", 10).unwrap().is_empty());
        assert_eq!(waiting(Some("Work/a.txt")), ["Work/a.txt"], "files still there keep theirs");
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
