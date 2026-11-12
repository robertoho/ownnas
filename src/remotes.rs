//! The servers and clouds set up in rclone (SFTP machines, OneDrive, Nextcloud, S3 buckets…),
//! browsed from OwnNAS. OwnNAS starts its own rclone (`rclone rcd`), listening only on this
//! computer with a password of its own, and asks it over rclone's remote-control API, so
//! connections and sign-ins stay open between clicks. Files are read through the same rclone,
//! which answers byte ranges, so videos can be skipped through.
//!
//! The administrator sees every remote and chooses who else may; the remotes' passwords and
//! tokens stay in rclone's config file and never reach the browser.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The administrator's choices: where rclone and its config file are (empty: the usual).
pub const SETTING: &str = "rclone";
/// An rclone nobody asked anything for this long is stopped (and started again when needed).
const IDLE: Duration = Duration::from_secs(15 * 60);

pub const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS remote_access (
        remote TEXT NOT NULL,
        user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        PRIMARY KEY (remote, user_id)
    );
";

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// The rclone program; empty: found on the PATH or in the usual install folders.
    pub program: String,
    /// rclone's config file; empty: rclone's own default for the user OwnNAS runs as.
    pub config: String,
}

pub fn load_settings(conn: &Connection) -> Settings {
    crate::db::get_setting(conn, SETTING).ok().flatten().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

/// The settings, read from the database at `db_path` (for backups, which run on their own).
pub fn settings_from(db_path: &Path) -> Settings {
    Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).map(|conn| load_settings(&conn)).unwrap_or_default()
}

pub fn save_settings(conn: &Connection, settings: &Settings) -> Result<(), String> {
    let text = serde_json::to_string(settings).map_err(|err| err.to_string())?;
    crate::db::set_setting(conn, SETTING, Some(&text))
}

/// Where rclone is: as chosen, else on the PATH or in the usual install folders a service's
/// bare PATH lacks.
pub fn program(settings: &Settings) -> Option<PathBuf> {
    if !settings.program.trim().is_empty() {
        let chosen = PathBuf::from(settings.program.trim());
        return chosen.is_file().then_some(chosen);
    }
    if let Some(found) = crate::helper_tools::find("rclone") {
        return Some(found);
    }
    let candidates: &[&str] = if cfg!(windows) { &[] } else { &["/opt/homebrew/bin/rclone", "/usr/local/bin/rclone", "/usr/bin/rclone", "/snap/bin/rclone"] };
    candidates.iter().map(PathBuf::from).find(|path| path.is_file())
}

/// The config file rclone would use with these settings.
pub fn config_file(program: &Path, settings: &Settings) -> Option<String> {
    if !settings.config.trim().is_empty() {
        return Some(settings.config.trim().to_string());
    }
    // "Configuration file is stored at:\n/home/ann/.config/rclone/rclone.conf"
    let output = Command::new(program).args(["config", "file"]).stdin(Stdio::null()).output().ok()?;
    String::from_utf8_lossy(&output.stdout).lines().map(str::trim).filter(|line| !line.is_empty() && !line.ends_with(':')).last().map(str::to_string)
}

pub fn version(program: &Path) -> Option<String> {
    let output = Command::new(program).arg("version").stdin(Stdio::null()).output().ok()?;
    String::from_utf8_lossy(&output.stdout).lines().next().map(|line| line.trim().to_string())
}

// ---- Who may see which remote ----

/// The users (besides administrators) each remote is shared with.
pub fn access(conn: &Connection) -> std::collections::HashMap<String, Vec<i64>> {
    let mut out: std::collections::HashMap<String, Vec<i64>> = std::collections::HashMap::new();
    if let Ok(mut stmt) = conn.prepare("SELECT remote, user_id FROM remote_access ORDER BY remote, user_id") {
        if let Ok(rows) = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))) {
            for (remote, user) in rows.flatten() {
                out.entry(remote).or_default().push(user);
            }
        }
    }
    out
}

pub fn set_access(conn: &Connection, remote: &str, users: &[i64]) -> Result<(), String> {
    let tx = conn.unchecked_transaction().map_err(|err| err.to_string())?;
    tx.execute("DELETE FROM remote_access WHERE remote = ?1", params![remote]).map_err(|err| err.to_string())?;
    for user in users {
        tx.execute("INSERT OR IGNORE INTO remote_access (remote, user_id) SELECT ?1, id FROM users WHERE id = ?2", params![remote, user]).map_err(|err| err.to_string())?;
    }
    tx.commit().map_err(|err| err.to_string())
}

/// Whether a user who is not an administrator may use the remote.
pub fn shared_with(conn: &Connection, remote: &str, user: i64) -> bool {
    conn.query_row("SELECT 1 FROM remote_access WHERE remote = ?1 AND user_id = ?2", params![remote, user], |_| Ok(())).is_ok()
}

pub fn shared_with_any(conn: &Connection, user: i64) -> bool {
    conn.query_row("SELECT 1 FROM remote_access WHERE user_id = ?1 LIMIT 1", params![user], |_| Ok(())).is_ok()
}

// ---- Paths ----

/// A remote's name as rclone allows them (letters, digits, and `_-.+@ `), so it cannot smuggle
/// anything into the "name:path" rclone is given.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 200 && !name.starts_with(['-', ' ']) && name.chars().all(|c| c.is_alphanumeric() || "_-.+@ ".contains(c))
}

/// A path inside a remote, tidied ("a/b/c", no leading slash); `None` when it tries to leave it.
pub fn clean_path(path: &str) -> Option<String> {
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => continue,
            ".." => return None,
            _ if part.chars().any(char::is_control) => return None,
            _ => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

fn percent_encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

// ---- The rclone OwnNAS runs ----

struct Daemon {
    child: Child,
    /// Closed when the daemon is dropped, which stops rclone (see `start`).
    stdin: Option<ChildStdin>,
    port: u16,
    password: String,
    started_with: (PathBuf, String),
    last_used: Instant,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        // Closing the pipe makes the wrapper stop rclone; give it a moment, then make sure.
        if let Some(mut stdin) = self.stdin.take() {
            let _ = stdin.write_all(b"\n");
        }
        for _ in 0..20 {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct Rclone {
    daemon: Mutex<Option<Daemon>>,
    watching: std::sync::atomic::AtomicBool,
}

/// The one rclone of this OwnNAS.
pub fn rclone() -> &'static Rclone {
    static ONE: OnceLock<Rclone> = OnceLock::new();
    ONE.get_or_init(|| Rclone { daemon: Mutex::new(None), watching: std::sync::atomic::AtomicBool::new(false) })
}

/// Where to reach the running rclone.
#[derive(Clone)]
pub struct Endpoint {
    port: u16,
    password: String,
}

impl Rclone {
    /// The running rclone, started (or restarted, when the settings changed or it stopped).
    pub fn endpoint(&'static self, settings: &Settings) -> Result<Endpoint, String> {
        let program = program(settings).ok_or("rclone is not installed on the server")?;
        let wanted = (program.clone(), settings.config.trim().to_string());
        let mut daemon = self.daemon.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(running) = daemon.as_mut() {
            if running.started_with == wanted && running.child.try_wait().ok().flatten().is_none() {
                running.last_used = Instant::now();
                return Ok(Endpoint { port: running.port, password: running.password.clone() });
            }
        }
        *daemon = None;
        let started = start(&program, &wanted.1)?;
        let endpoint = Endpoint { port: started.port, password: started.password.clone() };
        *daemon = Some(started);
        drop(daemon);
        self.watch_idle();
        Ok(endpoint)
    }

    /// Stops rclone (it starts again when needed): after the settings change, say.
    pub fn stop(&self) {
        let old = self.daemon.lock().unwrap_or_else(|e| e.into_inner()).take();
        drop(old);
    }

    fn watch_idle(&'static self) {
        if self.watching.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(60));
            let mut daemon = self.daemon.lock().unwrap_or_else(|e| e.into_inner());
            if daemon.as_ref().is_some_and(|running| running.last_used.elapsed() > IDLE) {
                let old = daemon.take();
                drop(daemon);
                drop(old);
            }
        });
    }
}

fn start(program: &Path, config: &str) -> Result<Daemon, String> {
    // A free port on this computer only.
    let port = std::net::TcpListener::bind("127.0.0.1:0").and_then(|listener| listener.local_addr()).map_err(|_| "Could not find a free port for rclone")?.port();
    let password: String = {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        (0..32).map(|_| char::from(b"abcdefghijklmnopqrstuvwxyz0123456789"[rng.gen_range(0..36)])).collect()
    };
    let mut args: Vec<String> = vec!["rcd".into(), "--rc-serve".into(), format!("--rc-addr=127.0.0.1:{port}"), "--rc-user=ownnas".into()];
    if !config.is_empty() {
        args.push(format!("--config={config}"));
    }
    // On Unix rclone runs under a small shell that stops it when OwnNAS's end of the pipe
    // closes, so it never outlives OwnNAS (also when OwnNAS is killed).
    let mut command = if cfg!(unix) {
        let mut command = Command::new("/bin/sh");
        command.arg("-c").arg("\"$0\" \"$@\" </dev/null & pid=$!; read _; kill $pid 2>/dev/null; wait $pid").arg(program).args(&args);
        command
    } else {
        let mut command = Command::new(program);
        command.args(&args);
        command
    };
    // The password goes in the environment, where other users cannot read it.
    command.env("RCLONE_RC_PASS", &password).env_remove("RCLONE_RC_USER").stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = command.spawn().map_err(|_| "Could not start rclone")?;
    let stdin = child.stdin.take();
    let mut daemon = Daemon { child, stdin, port, password, started_with: (program.to_path_buf(), config.to_string()), last_used: Instant::now() };
    let endpoint = Endpoint { port, password: daemon.password.clone() };
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if daemon.child.try_wait().ok().flatten().is_some() {
            return Err("rclone stopped as it started; check its config file".into());
        }
        if endpoint.call("rc/noop", json!({})).is_ok() {
            daemon.last_used = Instant::now();
            return Ok(daemon);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err("rclone did not start in time".into())
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(5)).timeout_read(timeout).build()
}

fn basic(password: &str) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = format!("ownnas:{password}").into_bytes();
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16) | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8) | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    format!("Basic {out}")
}

impl Endpoint {
    /// Asks rclone (`rc/noop`, `operations/list`…) and returns its answer.
    pub fn call(&self, method: &str, body: Value) -> Result<Value, String> {
        let url = format!("http://127.0.0.1:{}/{method}", self.port);
        match agent(Duration::from_secs(120)).post(&url).set("Authorization", &basic(&self.password)).send_json(body) {
            Ok(response) => response.into_json().map_err(|_| "rclone answered something unexpected".to_string()),
            Err(ureq::Error::Status(_, response)) => {
                let answer: Value = response.into_json().unwrap_or(Value::Null);
                Err(friendly(answer.get("error").and_then(Value::as_str).unwrap_or("rclone could not do that")))
            }
            Err(_) => Err("Could not reach rclone".into()),
        }
    }

    /// A file of a remote as rclone serves it (with `range` passed on): status, headers, body.
    pub fn open(&self, remote: &str, path: &str, range: Option<&str>) -> Result<ureq::Response, String> {
        let url = format!("http://127.0.0.1:{}/{}/{}", self.port, percent_encode(&format!("[{remote}:]")), path.split('/').map(percent_encode).collect::<Vec<_>>().join("/"));
        let mut request = agent(Duration::from_secs(300)).get(&url).set("Authorization", &basic(&self.password));
        if let Some(range) = range {
            request = request.set("Range", range);
        }
        match request.call() {
            Ok(response) => Ok(response),
            Err(ureq::Error::Status(416, response)) => Ok(response),
            Err(ureq::Error::Status(404, _)) => Err("That file is not on the remote".into()),
            Err(ureq::Error::Status(_, response)) => Err(friendly(&response.into_string().unwrap_or_default())),
            Err(_) => Err("Could not reach rclone".into()),
        }
    }
}

/// rclone's error, in words someone can act on.
fn friendly(error: &str) -> String {
    let lower = error.to_ascii_lowercase();
    if lower.contains("didn't find section in config file") {
        "There is no such remote in rclone's config".into()
    } else if lower.contains("directory not found") || lower.contains("object not found") || lower.contains("not found") && lower.contains("dir") {
        "That folder is not on the remote".into()
    } else if lower.contains("trust token") || lower.contains("reauth") || lower.contains("invalid_grant") || lower.contains("token expired") || lower.contains("couldn't fetch token") {
        "The remote's sign-in has expired. On the server, run: rclone config reconnect <remote>:".into()
    } else if lower.contains("connection refused") || lower.contains("no route to host") || lower.contains("i/o timeout") || lower.contains("no such host") {
        "Could not reach the remote's server".into()
    } else if lower.contains("unable to authenticate") || lower.contains("permission denied") || lower.contains("401") || lower.contains("403") {
        "The remote turned the sign-in down; check its settings in rclone".into()
    } else {
        let first = error.lines().next().unwrap_or(error);
        format!("rclone: {}", first.chars().take(300).collect::<String>())
    }
}

// ---- What OwnNAS asks ----

#[derive(Debug, Serialize, Clone)]
pub struct Remote {
    pub name: String,
    /// rclone's backend: "sftp", "onedrive", "s3", "webdav"…
    #[serde(rename = "type")]
    pub kind: String,
}

/// The remotes in rclone's config, by name (their settings stay here: only the kind is kept).
pub fn remotes(endpoint: &Endpoint) -> Result<Vec<Remote>, String> {
    let dump = endpoint.call("config/dump", json!({}))?;
    let mut out: Vec<Remote> = dump
        .as_object()
        .map(|all| all.iter().filter(|(name, _)| valid_name(name)).map(|(name, settings)| Remote { name: name.clone(), kind: settings.get("type").and_then(Value::as_str).unwrap_or("").to_string() }).collect())
        .unwrap_or_default();
    out.sort_by_key(|remote| remote.name.to_lowercase());
    Ok(out)
}

#[derive(Debug, Serialize)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub dir: bool,
    pub size: u64,
    /// Seconds since 1970 (0 when the remote does not say).
    pub modified: i64,
    pub mime: String,
}

/// What is in a folder of a remote: folders first, then files, by name.
pub fn list(endpoint: &Endpoint, remote: &str, path: &str) -> Result<Vec<Entry>, String> {
    let answer = endpoint.call("operations/list", json!({ "fs": format!("{remote}:"), "remote": path }))?;
    let mut out: Vec<Entry> = answer
        .get("list")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    let text = |key: &str| item.get(key).and_then(Value::as_str).unwrap_or("").to_string();
                    let modified = chrono::DateTime::parse_from_rfc3339(&text("ModTime")).map(|at| at.timestamp()).unwrap_or(0);
                    Entry {
                        name: text("Name"),
                        path: text("Path"),
                        dir: item.get("IsDir").and_then(Value::as_bool).unwrap_or(false),
                        size: item.get("Size").and_then(Value::as_i64).unwrap_or(0).max(0) as u64,
                        modified: if modified < 86_400 { 0 } else { modified },
                        mime: text("MimeType"),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_paths() {
        assert!(valid_name("vps-a76dac12.vps.ovh.net"));
        assert!(valid_name("my drive"));
        assert!(!valid_name("a:b"));
        assert!(!valid_name("../x"));
        assert!(!valid_name("-flag"));
        assert_eq!(clean_path("/a//b/./c/").as_deref(), Some("a/b/c"));
        assert_eq!(clean_path(""), Some(String::new()));
        assert_eq!(clean_path("a/../b"), None);
        assert_eq!(percent_encode("[odroid:]"), "%5Bodroid%3A%5D");
        assert_eq!(basic("x"), "Basic b3dubmFzOng=");
        assert_eq!(friendly("didn't find section in config file (\"x\")"), "There is no such remote in rclone's config");
    }

    #[test]
    fn talks_to_a_real_rclone() {
        // Only where rclone is installed: lists a local folder through it.
        let settings = Settings::default();
        if program(&settings).is_none() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("ownnas-rclone-{}", rand::random::<u64>()));
        std::fs::create_dir_all(dir.join("inside")).unwrap();
        std::fs::write(dir.join("hello.txt"), "hello there").unwrap();
        let endpoint = rclone().endpoint(&settings).unwrap();
        // A local path works where a remote's name would go ("/tmp/x:" is not needed for it).
        let answer = endpoint.call("operations/list", json!({ "fs": dir.display().to_string(), "remote": "" })).unwrap();
        let names: Vec<&str> = answer["list"].as_array().unwrap().iter().filter_map(|item| item["Name"].as_str()).collect();
        assert!(names.contains(&"hello.txt") && names.contains(&"inside"), "{names:?}");
        rclone().stop();
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
