//! What the database connections are doing: the statements running now and the transactions
//! left open, with the thread doing each. SQLite cannot empty its write-ahead log past the
//! oldest read still open, so while one stays open the log grows with every write; this is
//! what lets the log say which read that is. Also writes down statements that took long.
//!
//! The log goes to the console and to `database.log` in the data folder.

use rusqlite::trace::{TraceEvent, TraceEventCodes};
use rusqlite::Connection;
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex, OnceLock};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

/// A statement that ran longer than this is written to the log when it finishes.
const SLOW: Duration = Duration::from_secs(2);
/// A read or transaction open longer than this is written to the log (again every `REPEAT`).
pub const HELD: Duration = Duration::from_secs(60);
const REPEAT: Duration = Duration::from_secs(10 * 60);
/// Past this size the log starts again, the old one kept as `database.log.old`.
const LOG_LIMIT: u64 = 5 * 1024 * 1024;

struct Open {
    thread: String,
    sql: String,
    since: Instant,
    reported: Option<Instant>,
}

#[derive(Default)]
struct Running {
    /// Statements started and not finished, by thread and SQL.
    statements: HashMap<(ThreadId, String), Open>,
    /// Transactions begun and not ended, by thread; `sql` is the first statement inside.
    transactions: HashMap<ThreadId, Open>,
}

static RUNNING: LazyLock<Mutex<Running>> = LazyLock::new(Mutex::default);
static LOG_FILE: OnceLock<PathBuf> = OnceLock::new();
/// Set while the write-ahead logs are being emptied with the indexers waiting.
static PAUSED: AtomicBool = AtomicBool::new(false);

/// The background indexers call this before each piece of database work: while the logs are
/// being emptied, they wait here, with no read open.
pub fn pause_point() {
    while PAUSED.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Runs `work` with the background indexers held at their `pause_point`, once the reads and
/// transactions they have open finish (waiting up to `wait` for them).
pub fn with_indexing_paused<T>(wait: Duration, work: impl FnOnce() -> T) -> T {
    PAUSED.store(true, Ordering::Relaxed);
    let until = Instant::now() + wait;
    while Instant::now() < until && indexer_busy() {
        std::thread::sleep(Duration::from_millis(100));
    }
    let done = work();
    PAUSED.store(false, Ordering::Relaxed);
    done
}

/// Whether an indexer thread (`ownnas-…-index`) has a statement running or a transaction open.
fn indexer_busy() -> bool {
    let running = running();
    let indexer = |open: &Open| open.thread.starts_with("ownnas-") && open.thread.ends_with("-index");
    running.statements.values().any(indexer) || running.transactions.values().any(indexer)
}

/// Writes the log to `database.log` in `data_dir` as well as the console.
pub fn log_to(data_dir: &std::path::Path) {
    let _ = LOG_FILE.set(data_dir.join("database.log"));
}

/// Follows what `conn` runs.
pub fn watch(conn: &Connection) {
    conn.trace_v2(TraceEventCodes::SQLITE_TRACE_STMT | TraceEventCodes::SQLITE_TRACE_PROFILE, Some(traced));
}

fn running() -> std::sync::MutexGuard<'static, Running> {
    RUNNING.lock().unwrap_or_else(|err| err.into_inner())
}

fn thread_name(thread: &std::thread::Thread) -> String {
    thread.name().map_or_else(|| format!("{:?}", thread.id()), str::to_string)
}

fn starts_with_word(sql: &str, word: &str) -> bool {
    let sql = sql.trim_start().as_bytes();
    sql.len() >= word.len() && sql[..word.len()].eq_ignore_ascii_case(word.as_bytes())
}

fn traced(event: TraceEvent<'_>) {
    match event {
        // Triggers report themselves as comments ("-- TRIGGER name"): part of their statement.
        TraceEvent::Stmt(_, sql) if !sql.starts_with("--") => {
            let thread = std::thread::current();
            let open = || Open { thread: thread_name(&thread), sql: sql.to_string(), since: Instant::now(), reported: None };
            let mut running = running();
            if starts_with_word(sql, "BEGIN") {
                running.transactions.insert(thread.id(), Open { sql: String::new(), ..open() });
            } else if starts_with_word(sql, "COMMIT") || starts_with_word(sql, "END") || (starts_with_word(sql, "ROLLBACK") && !sql.to_ascii_uppercase().contains(" TO ")) {
                running.transactions.remove(&thread.id());
            } else {
                if let Some(transaction) = running.transactions.get_mut(&thread.id()).filter(|t| t.sql.is_empty()) {
                    transaction.sql = sql.to_string();
                }
                running.statements.entry((thread.id(), sql.to_string())).or_insert_with(open);
            }
        }
        TraceEvent::Profile(stmt, took) => {
            let thread = std::thread::current();
            let sql = stmt.sql();
            running().statements.remove(&(thread.id(), sql.to_string()));
            if took >= SLOW && !starts_with_word(&sql, "VACUUM") {
                log(&format!("slow statement, {} on thread {}: {}", seconds(took), thread_name(&thread), short(&sql)));
            }
        }
        _ => {}
    }
}

/// The SQL on one line, cut short.
fn short(sql: &str) -> String {
    let line = sql.split_whitespace().collect::<Vec<_>>().join(" ");
    match line.char_indices().nth(240) {
        Some((end, _)) => format!("{}…", &line[..end]),
        None => line,
    }
}

fn seconds(time: Duration) -> String {
    let secs = time.as_secs();
    if secs >= 3600 {
        format!("{} h {} min", secs / 3600, secs % 3600 / 60)
    } else if secs >= 60 {
        format!("{} min {} s", secs / 60, secs % 60)
    } else {
        format!("{:.1} s", time.as_secs_f64())
    }
}

/// The reads and transactions open longer than `min`, longest first, one line each.
pub fn held(min: Duration) -> Vec<String> {
    let running = running();
    let mut open: Vec<(Duration, String)> = running
        .statements
        .values()
        .map(|open| (open, "statement running"))
        .chain(running.transactions.values().map(|open| (open, "transaction open")))
        .filter(|(open, _)| open.since.elapsed() >= min)
        .map(|(open, what)| (open.since.elapsed(), format!("{what} for {} on thread {}: {}", seconds(open.since.elapsed()), open.thread, short(&open.sql))))
        .collect();
    open.sort_by_key(|(held, _)| std::cmp::Reverse(*held));
    open.into_iter().map(|(_, line)| line).collect()
}

/// Writes down the reads and transactions open longer than `HELD`, each once every `REPEAT`.
pub fn report_held() {
    let mut lines = Vec::new();
    {
        let mut running = running();
        let Running { statements, transactions } = &mut *running;
        for (open, what) in statements.values_mut().map(|open| (open, "statement running")).chain(transactions.values_mut().map(|open| (open, "transaction open"))) {
            if open.since.elapsed() >= HELD && open.reported.is_none_or(|at| at.elapsed() >= REPEAT) {
                open.reported = Some(Instant::now());
                lines.push(format!("{what} for {} on thread {}: {}", seconds(open.since.elapsed()), open.thread, short(&open.sql)));
            }
        }
    }
    for line in lines {
        log(&line);
    }
}

/// One line in the console and in `database.log`, with the time.
pub fn log(line: &str) {
    let line = format!("{} Database: {line}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
    println!("{line}");
    let Some(path) = LOG_FILE.get() else { return };
    if std::fs::metadata(path).is_ok_and(|meta| meta.len() > LOG_LIMIT) {
        let _ = std::fs::rename(path, path.with_extension("log.old"));
    }
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        // In one write, so lines from different threads do not run into each other.
        let _ = file.write_all(format!("{line}\n").as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_read_left_open() {
        let conn = Connection::open_in_memory().unwrap();
        watch(&conn);
        conn.execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES (1), (2);").unwrap();
        let mut stmt = conn.prepare("SELECT x FROM t /* held-open test */").unwrap();
        let mut rows = stmt.query([]).unwrap();
        rows.next().unwrap();
        let mine = |lines: Vec<String>| lines.into_iter().filter(|line| line.contains("held-open test")).count();
        assert_eq!(mine(held(Duration::ZERO)), 1, "a statement stepped part way is open");
        drop(rows);
        assert_eq!(mine(held(Duration::ZERO)), 0, "and closed once reset");

        conn.execute_batch("BEGIN; SELECT x FROM t /* held-open test */;").unwrap();
        assert!(held(Duration::ZERO).iter().any(|line| line.starts_with("transaction open") && line.contains("held-open test")));
        conn.execute_batch("COMMIT").unwrap();
        assert_eq!(mine(held(Duration::ZERO)), 0);
    }
}
