//! Read-only views of SQLite databases in the library: their tables and schema, rows a page at
//! a time, and SELECT queries. Databases are never written to or locked for writing, and
//! queries cannot reach other files on the server.

use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use rusqlite::limits::Limit;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};

/// Rows a page or a query returns at most.
pub const MAX_ROWS: usize = 1000;
/// Text longer than this (in characters) is cut short in results; the full value can be fetched.
const MAX_TEXT: usize = 1000;
/// Leading bytes of a blob shown as hex.
const BLOB_PREVIEW: usize = 24;

const HEADER: &[u8; 16] = b"SQLite format 3\0";

#[derive(Debug)]
pub enum ViewError {
    NotSqlite,
    NotFound(String),
    Invalid(String),
    TimedOut,
    Sqlite(String),
}

impl std::fmt::Display for ViewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotSqlite => write!(f, "This is not a SQLite database"),
            Self::NotFound(what) => write!(f, "{what}"),
            Self::Invalid(why) => write!(f, "{why}"),
            Self::TimedOut => write!(f, "The query took too long and was stopped"),
            Self::Sqlite(err) => write!(f, "{err}"),
        }
    }
}

impl From<rusqlite::Error> for ViewError {
    fn from(err: rusqlite::Error) -> Self {
        if let rusqlite::Error::SqliteFailure(code, _) = &err {
            if code.code == rusqlite::ErrorCode::OperationInterrupted {
                return Self::TimedOut;
            }
            if code.code == rusqlite::ErrorCode::NotADatabase {
                return Self::NotSqlite;
            }
        }
        Self::Sqlite(err.to_string())
    }
}

type Result<T> = std::result::Result<T, ViewError>;

pub fn is_sqlite(path: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 16];
    std::fs::File::open(path).and_then(|mut file| file.read_exact(&mut head)).is_ok() && &head == HEADER
}

/// Opens a database read-only. A database another program is changing is read as it is now.
pub fn open(path: &Path) -> Result<Connection> {
    if !is_sqlite(path) {
        return Err(ViewError::NotSqlite);
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_URI;
    let usable = |conn: Connection| -> Result<Connection> {
        // Nothing may be attached: ATTACH would read other databases on the server.
        conn.set_limit(Limit::SQLITE_LIMIT_ATTACHED, 0)?;
        conn.pragma_update(None, "query_only", true)?;
        conn.busy_timeout(Duration::from_secs(2))?;
        conn.query_row("SELECT count(*) FROM sqlite_schema", [], |_| Ok(()))?;
        Ok(conn)
    };
    match Connection::open_with_flags(file_uri(path, false), flags).map_err(ViewError::from).and_then(usable) {
        Ok(conn) => Ok(conn),
        // A write-ahead log or hot journal that a read-only connection may not touch (say, in a
        // folder the server cannot write to): read the file as it is on disk.
        Err(ViewError::Sqlite(_)) => Connection::open_with_flags(file_uri(path, true), flags).map_err(ViewError::from).and_then(usable),
        Err(err) => Err(err),
    }
}

fn file_uri(path: &Path, immutable: bool) -> String {
    // Only these characters mean something in a file: URI's path.
    let mut uri = String::from("file:");
    for c in path.to_string_lossy().chars() {
        match c {
            '?' | '#' | '%' => uri.push_str(&format!("%{:02X}", c as u8)),
            _ => uri.push(c),
        }
    }
    if immutable {
        format!("{uri}?mode=ro&immutable=1")
    } else {
        format!("{uri}?mode=ro")
    }
}

/// Runs `work`, interrupting whatever SQLite is doing once `limit` has passed.
fn with_deadline<T>(conn: &Connection, limit: Duration, work: impl FnOnce() -> Result<T>) -> Result<T> {
    let handle = conn.get_interrupt_handle();
    let (done, wait) = mpsc::channel::<()>();
    let watchdog = std::thread::spawn(move || {
        if wait.recv_timeout(limit) == Err(mpsc::RecvTimeoutError::Timeout) {
            handle.interrupt();
        }
    });
    let result = work();
    let _ = done.send(());
    let _ = watchdog.join();
    result
}

fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// One value as JSON: long text and blobs are cut short and say how long they are.
fn cell(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        // JavaScript numbers lose precision past 2^53.
        ValueRef::Integer(n) if n.unsigned_abs() > (1u64 << 53) => json!({ "int": n.to_string() }),
        ValueRef::Integer(n) => json!(n),
        ValueRef::Real(n) if n.is_finite() => json!(n),
        ValueRef::Real(n) => json!({ "real": n.to_string() }),
        ValueRef::Text(bytes) => {
            let text = String::from_utf8_lossy(bytes);
            let length = text.chars().count();
            if length > MAX_TEXT {
                json!({ "text": text.chars().take(MAX_TEXT).collect::<String>(), "length": length })
            } else {
                json!(text)
            }
        }
        ValueRef::Blob(bytes) => json!({
            "blob": bytes.len(),
            "hex": hex::encode(&bytes[..bytes.len().min(BLOB_PREVIEW)]),
        }),
    }
}

struct Table {
    name: String,
    kind: String,
    sql: String,
}

fn tables(conn: &Connection) -> Result<Vec<Table>> {
    let mut stmt = conn.prepare(
        "SELECT name, type, coalesce(sql, '') FROM sqlite_schema WHERE type IN ('table', 'view') ORDER BY name COLLATE NOCASE",
    )?;
    let rows = stmt.query_map([], |row| Ok(Table { name: row.get(0)?, kind: row.get(1)?, sql: row.get(2)? }))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

fn find_table(conn: &Connection, name: &str) -> Result<Table> {
    tables(conn)?
        .into_iter()
        .find(|table| table.name == name)
        .ok_or_else(|| ViewError::NotFound(format!("There is no table called “{name}”")))
}

fn has_rowid(table: &Table) -> bool {
    table.kind == "table" && !table.sql.to_ascii_uppercase().replace(char::is_whitespace, "").contains("WITHOUTROWID")
}

struct Column {
    name: String,
    kind: String,
    pk: i64,
    not_null: bool,
}

fn columns(conn: &Connection, table: &str) -> Result<Vec<Column>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_xinfo({})", quote(table)))?;
    let rows = stmt.query_map([], |row| {
        Ok((
            Column { name: row.get(1)?, kind: row.get::<_, Option<String>>(2)?.unwrap_or_default(), pk: row.get(5)?, not_null: row.get::<_, i64>(3)? != 0 },
            row.get::<_, i64>(6)?,
        ))
    })?;
    // Hidden columns (of virtual tables) are not part of `SELECT *`.
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?.into_iter().filter(|(_, hidden)| *hidden != 1).map(|(column, _)| column).collect())
}

/// The tables and views, with their schema and how many rows they have (when counting is quick).
pub fn overview(conn: &Connection) -> Result<Value> {
    let mut list = Vec::new();
    for table in tables(conn)? {
        let rows = with_deadline(conn, Duration::from_millis(500), || {
            Ok(conn.query_row(&format!("SELECT count(*) FROM {}", quote(&table.name)), [], |row| row.get::<_, i64>(0))?)
        })
        .ok();
        let indexes: Vec<String> = {
            let mut stmt = conn.prepare("SELECT sql FROM sqlite_schema WHERE type = 'index' AND tbl_name = ?1 AND sql IS NOT NULL ORDER BY name")?;
            let found = stmt.query_map([&table.name], |row| row.get(0))?;
            found.collect::<std::result::Result<_, _>>()?
        };
        let columns: Vec<Value> = columns(conn, &table.name)
            .unwrap_or_default()
            .into_iter()
            .map(|column| json!({ "name": column.name, "type": column.kind, "pk": column.pk, "notNull": column.not_null }))
            .collect();
        list.push(json!({
            "name": table.name,
            "type": table.kind,
            "rows": rows,
            "sql": table.sql,
            "indexes": indexes,
            "columns": columns,
        }));
    }
    let version: String = conn.query_row("SELECT sqlite_version()", [], |row| row.get(0))?;
    Ok(json!({ "tables": list, "sqliteVersion": version }))
}

pub struct Page<'a> {
    pub table: &'a str,
    pub offset: usize,
    pub limit: usize,
    pub order: Option<&'a str>,
    pub descending: bool,
    pub filter: &'a str,
}

/// A page of a table's rows, optionally sorted by a column and filtered by text in any column.
pub fn rows(conn: &Connection, page: &Page<'_>) -> Result<Value> {
    let table = find_table(conn, page.table)?;
    let columns = columns(conn, &table.name)?;
    if columns.is_empty() {
        return Err(ViewError::Invalid("This table has no columns that can be shown".into()));
    }
    let rowid = has_rowid(&table);
    let mut sql = String::from("SELECT ");
    if rowid {
        sql.push_str("rowid, ");
    }
    sql.push_str(&columns.iter().map(|column| quote(&column.name)).collect::<Vec<_>>().join(", "));
    sql.push_str(&format!(" FROM {}", quote(&table.name)));
    let filter = page.filter.trim();
    let mut where_clause = String::new();
    if !filter.is_empty() {
        let tests: Vec<String> = columns.iter().map(|column| format!("CAST({} AS TEXT) LIKE ?1 ESCAPE '\\'", quote(&column.name))).collect();
        where_clause = format!(" WHERE {}", tests.join(" OR "));
    }
    sql.push_str(&where_clause);
    if let Some(order) = page.order {
        if !columns.iter().any(|column| column.name == order) {
            return Err(ViewError::Invalid(format!("There is no column called “{order}”")));
        }
        sql.push_str(&format!(" ORDER BY {} {}", quote(order), if page.descending { "DESC" } else { "ASC" }));
    } else if rowid {
        sql.push_str(" ORDER BY rowid");
    }
    let limit = page.limit.clamp(1, MAX_ROWS);
    sql.push_str(&format!(" LIMIT {} OFFSET {}", limit, page.offset));
    let pattern = format!("%{}%", filter.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
    let started = Instant::now();
    let rows = with_deadline(conn, Duration::from_secs(10), || {
        let mut stmt = conn.prepare(&sql)?;
        let count = stmt.column_count();
        let mut found = if filter.is_empty() { stmt.query([])? } else { stmt.query([&pattern])? };
        let mut rows = Vec::new();
        while let Some(row) = found.next()? {
            let mut values = Vec::with_capacity(count);
            for index in 0..count {
                values.push(cell(row.get_ref(index)?));
            }
            rows.push(values);
        }
        Ok(rows)
    })?;
    let total = with_deadline(conn, Duration::from_secs(2), || {
        let count_sql = format!("SELECT count(*) FROM {}{}", quote(&table.name), where_clause);
        Ok(if filter.is_empty() {
            conn.query_row(&count_sql, [], |row| row.get::<_, i64>(0))?
        } else {
            conn.query_row(&count_sql, [&pattern], |row| row.get::<_, i64>(0))?
        })
    })
    .ok();
    let rows: Vec<Value> = rows
        .into_iter()
        .map(|mut values| {
            if rowid {
                let id = values.remove(0);
                json!({ "rowid": id, "values": values })
            } else {
                json!({ "values": values })
            }
        })
        .collect();
    Ok(json!({
        "columns": columns.iter().map(|column| json!({ "name": column.name, "type": column.kind, "pk": column.pk })).collect::<Vec<_>>(),
        "rows": rows,
        "total": total,
        "offset": page.offset,
        "rowid": rowid,
        "elapsedMs": started.elapsed().as_millis() as u64,
    }))
}

/// Skips leading blanks and SQL comments.
fn statement_start(sql: &str) -> &str {
    let mut rest = sql;
    loop {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix("--") {
            rest = after.split_once('\n').map_or("", |(_, tail)| tail);
        } else if let Some(after) = rest.strip_prefix("/*") {
            rest = after.split_once("*/").map_or("", |(_, tail)| tail);
        } else {
            return rest;
        }
    }
}

/// Runs one read-only statement: SELECT, WITH, VALUES, or EXPLAIN.
pub fn query(conn: &Connection, sql: &str) -> Result<Value> {
    let start = statement_start(sql);
    let keyword: String = start.chars().take_while(|c| c.is_ascii_alphabetic()).collect::<String>().to_ascii_uppercase();
    if !["SELECT", "WITH", "VALUES", "EXPLAIN"].contains(&keyword.as_str()) {
        return Err(ViewError::Invalid("Only SELECT, WITH, VALUES, and EXPLAIN queries can be run here".into()));
    }
    let started = Instant::now();
    with_deadline(conn, Duration::from_secs(10), || {
        let mut stmt = conn.prepare(sql).map_err(|err| match err {
            rusqlite::Error::MultipleStatement => ViewError::Invalid("Run one statement at a time".into()),
            other => other.into(),
        })?;
        if !stmt.readonly() {
            return Err(ViewError::Invalid("Only queries that read the database can be run here".into()));
        }
        let names: Vec<String> = stmt.column_names().into_iter().map(String::from).collect();
        let count = names.len();
        let mut found = stmt.query([])?;
        let mut rows = Vec::new();
        let mut truncated = false;
        while let Some(row) = found.next()? {
            if rows.len() == MAX_ROWS {
                truncated = true;
                break;
            }
            let mut values = Vec::with_capacity(count);
            for index in 0..count {
                values.push(cell(row.get_ref(index)?));
            }
            rows.push(json!({ "values": values }));
        }
        Ok(json!({
            "columns": names.iter().map(|name| json!({ "name": name })).collect::<Vec<_>>(),
            "rows": rows,
            "truncated": truncated,
            "elapsedMs": started.elapsed().as_millis() as u64,
        }))
    })
}

/// One whole value, for showing or downloading what the results cut short.
pub fn value(conn: &Connection, table: &str, rowid: i64, column: &str) -> Result<(Vec<u8>, bool)> {
    let table = find_table(conn, table)?;
    if !has_rowid(&table) {
        return Err(ViewError::Invalid("Values can only be fetched from tables with row ids".into()));
    }
    if !columns(conn, &table.name)?.iter().any(|known| known.name == column) {
        return Err(ViewError::Invalid(format!("There is no column called “{column}”")));
    }
    let sql = format!("SELECT {} FROM {} WHERE rowid = ?1", quote(column), quote(&table.name));
    with_deadline(conn, Duration::from_secs(10), || {
        let found = conn.query_row(&sql, [rowid], |row| {
            Ok(match row.get_ref(0)? {
                ValueRef::Null => (Vec::new(), true),
                ValueRef::Integer(n) => (n.to_string().into_bytes(), true),
                ValueRef::Real(n) => (n.to_string().into_bytes(), true),
                ValueRef::Text(bytes) => (bytes.to_vec(), true),
                ValueRef::Blob(bytes) => (bytes.to_vec(), false),
            })
        });
        match found {
            Ok(value) => Ok(value),
            Err(rusqlite::Error::QueryReturnedNoRows) => Err(ViewError::NotFound("That row no longer exists".into())),
            Err(err) => Err(err.into()),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder, removed when the test ends.
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("ownnas-sqlite-{}", rand::random::<u64>()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn sample(dir: &Path) -> std::path::PathBuf {
        let path = dir.join("shop.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT NOT NULL, price REAL, photo BLOB);
             CREATE INDEX items_name ON items(name);
             CREATE TABLE tags (tag TEXT PRIMARY KEY, n INTEGER) WITHOUT ROWID;
             CREATE VIEW cheap AS SELECT name FROM items WHERE price < 2;
             INSERT INTO items (name, price, photo) VALUES ('apple', 1.5, x'00ff10'), ('pear', 2.5, NULL), ('100% juice', 3, NULL);
             INSERT INTO tags VALUES ('fruit', 2);",
        )
        .unwrap();
        conn.execute("INSERT INTO items (name, price) VALUES (?1, 9)", [&"x".repeat(1500)]).unwrap();
        path
    }

    #[test]
    fn reads_tables_rows_and_queries_without_writing() {
        let dir = Scratch::new();
        let path = sample(dir.path());
        let before = std::fs::read(&path).unwrap();
        let conn = open(&path).unwrap();

        let overview = overview(&conn).unwrap();
        let names: Vec<&str> = overview["tables"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["cheap", "items", "tags"]);
        let items = &overview["tables"][1];
        assert_eq!(items["rows"], 4);
        assert_eq!(items["indexes"][0], "CREATE INDEX items_name ON items(name)");
        assert_eq!(items["columns"][0]["pk"], 1);

        let page = rows(&conn, &Page { table: "items", offset: 0, limit: 2, order: Some("price"), descending: true, filter: "" }).unwrap();
        assert_eq!(page["total"], 4);
        assert_eq!(page["rows"].as_array().unwrap().len(), 2);
        assert_eq!(page["rows"][0]["values"][2], 9.0);
        assert_eq!(page["rows"][0]["values"][1]["length"], 1500);
        // A filter matches text in any column; % and _ are matched literally.
        let page = rows(&conn, &Page { table: "items", offset: 0, limit: 50, order: None, descending: false, filter: "100%" }).unwrap();
        assert_eq!(page["total"], 1);
        assert_eq!(page["rows"][0]["values"][1], "100% juice");
        let apple = rows(&conn, &Page { table: "items", offset: 0, limit: 1, order: None, descending: false, filter: "" }).unwrap();
        assert_eq!(apple["rows"][0]["values"][3]["blob"], 3);
        assert_eq!(apple["rows"][0]["values"][3]["hex"], "00ff10");
        let tags = rows(&conn, &Page { table: "tags", offset: 0, limit: 10, order: None, descending: false, filter: "" }).unwrap();
        assert_eq!(tags["rowid"], false);
        assert!(rows(&conn, &Page { table: "nope", offset: 0, limit: 10, order: None, descending: false, filter: "" }).is_err());
        assert!(rows(&conn, &Page { table: "items", offset: 0, limit: 10, order: Some("x\"; DROP"), descending: false, filter: "" }).is_err());

        let result = query(&conn, "-- cheapest first\nSELECT name, price FROM items ORDER BY price LIMIT 1").unwrap();
        assert_eq!(result["columns"][0]["name"], "name");
        assert_eq!(result["rows"][0]["values"][0], "apple");
        let many = query(&conn, "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n LIMIT 1500) SELECT i FROM n").unwrap();
        assert_eq!(many["truncated"], true);
        assert_eq!(many["rows"].as_array().unwrap().len(), MAX_ROWS);

        for refused in [
            "DELETE FROM items",
            "UPDATE items SET price = 0",
            "ATTACH DATABASE 'other.db' AS other",
            "VACUUM INTO 'copy.db'",
            "PRAGMA writable_schema = 1",
            "SELECT 1; DELETE FROM items",
            "/* sneaky */ DROP TABLE items",
        ] {
            assert!(query(&conn, refused).is_err(), "{refused}");
        }
        assert!(!dir.path().join("copy.db").exists());

        let (bytes, text) = value(&conn, "items", 4, "name").unwrap();
        assert!(text);
        assert_eq!(bytes.len(), 1500);
        let (bytes, text) = value(&conn, "items", 1, "photo").unwrap();
        assert!(!text);
        assert_eq!(bytes, [0x00, 0xff, 0x10]);
        drop(conn);
        assert_eq!(std::fs::read(&path).unwrap(), before, "the database is unchanged");
    }

    #[test]
    fn long_queries_are_stopped_and_other_files_refused() {
        let dir = Scratch::new();
        let path = sample(dir.path());
        let conn = open(&path).unwrap();
        let started = Instant::now();
        let slow = with_deadline(&conn, Duration::from_millis(200), || {
            Ok(conn.query_row(
                "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n) SELECT count(*) FROM n",
                [],
                |row| row.get::<_, i64>(0),
            )?)
        });
        assert!(matches!(slow, Err(ViewError::TimedOut)));
        assert!(started.elapsed() < Duration::from_secs(5));

        let text = dir.path().join("notes.db");
        std::fs::write(&text, "not a database at all").unwrap();
        assert!(matches!(open(&text), Err(ViewError::NotSqlite)));
    }

    #[test]
    fn opens_paths_with_unusual_characters() {
        let dir = Scratch::new();
        // Windows does not allow `?` in names.
        let odd = dir.path().join(if cfg!(windows) { "100% café #1" } else { "100% café #1?" });
        std::fs::create_dir(&odd).unwrap();
        let path = sample(&odd);
        let conn = open(&path).unwrap();
        assert_eq!(overview(&conn).unwrap()["tables"].as_array().unwrap().len(), 3);
    }
}
