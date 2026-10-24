//! Backing up the NAS with restic: the library and OwnNAS's own data (its database, file
//! history, and settings) go to a drive, an S3-compatible bucket, or another machine over SFTP,
//! as encrypted snapshots. OwnNAS runs `restic` for each step, on a schedule or when asked, keeps
//! the snapshots the admin chose (so many days, weeks, and months), checks a sample of the
//! backup once a month, and restores a folder from any snapshot into the library.
//!
//! The settings are in the database. The backup's password and the S3 secret key are in a file
//! next to it, which is never backed up itself: without the password a backup cannot be read.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const SETTING_CONFIG: &str = "nas_backup_config";
const SETTING_STATE: &str = "nas_backup_state";
const SECRETS_FILE: &str = "backup-secrets.json";
/// A copy of the database made for each backup, since the live one changes while it is read.
const STAGING_DIR: &str = "backup-staging";
/// restic's cache of the backup's index, which makes each run quicker.
const CACHE_DIR: &str = "backup-cache";
/// Every snapshot OwnNAS makes carries this tag and host, so its keep rules only touch those.
const TAG: &str = "ownnas";
const HOST: &str = "ownnas";
/// Where restored folders go, inside the library.
pub const RESTORE_FOLDER: &str = "Restored from backup";
/// `restore <snapshot>:<folder>` arrived in restic 0.17.
const MIN_VERSION: (u32, u32) = (0, 17);
/// A sample of the backup is read back this often.
const VERIFY_EVERY: i64 = 30 * 86400;
/// After a failed run, it is tried again this much later, a few times.
const RETRY_AFTER: i64 = 3600;
const RETRIES: u32 = 3;

// restic's exit codes.
const EXIT_PARTIAL: i32 = 3;
const EXIT_NO_REPOSITORY: i32 = 10;
const EXIT_LOCKED: i32 = 11;
const EXIT_WRONG_PASSWORD: i32 = 12;
/// Not restic's: OwnNAS stopped it at an error it was going to retry.
const EXIT_GAVE_UP: i32 = -2;

/// Where backups go.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Destination {
    /// "local", "s3", or "sftp".
    pub kind: String,
    /// local: a folder on this computer, such as a USB drive. sftp: the folder on the other machine.
    pub path: String,
    /// s3: "s3.amazonaws.com", "https://s3.eu-central-003.backblazeb2.com", "http://minio:9000"…
    pub endpoint: String,
    pub bucket: String,
    /// s3: a folder inside the bucket.
    pub prefix: String,
    pub region: String,
    pub access_key_id: String,
    /// sftp
    pub host: String,
    pub port: u16,
    pub user: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    pub enabled: bool,
    pub destination: Destination,
    /// "daily", "weekly", or "manual".
    pub frequency: String,
    /// When in the day, in minutes after midnight on this computer's clock, so a backup at
    /// 03:00 stays at 03:00 when the clocks change.
    pub minute: u32,
    /// For weekly backups, the day: 0 is Sunday.
    pub weekday: u8,
    pub keep_daily: u32,
    pub keep_weekly: u32,
    pub keep_monthly: u32,
    /// Leave the library's Trash out.
    pub skip_trash: bool,
    /// restic exclude patterns, such as `*.tmp` or a folder's full path.
    pub excludes: Vec<String>,
    /// When the schedule was last saved: slots before it are not caught up on.
    pub saved_at: i64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: false,
            destination: Destination::default(),
            frequency: "daily".into(),
            minute: 3 * 60,
            weekday: 0,
            keep_daily: 7,
            keep_weekly: 4,
            keep_monthly: 12,
            skip_trash: true,
            excludes: Vec::new(),
            saved_at: 0,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Secrets {
    pub password: String,
    pub s3_secret: String,
}

/// What a backup did.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Summary {
    pub snapshot: String,
    pub files_new: u64,
    pub files_changed: u64,
    pub files_unmodified: u64,
    pub data_added: u64,
    pub total_files: u64,
    pub total_bytes: u64,
    pub seconds: f64,
    /// Files restic could not read (the snapshot has everything else).
    pub unreadable: u64,
}

/// How the backups have gone, kept in the database.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct State {
    pub last_started: i64,
    pub last_finished: i64,
    /// "ok", "warning" (some files unreadable), "failed", or "stopped".
    pub last_result: String,
    pub last_error: Option<String>,
    pub last_summary: Option<Summary>,
    pub last_success: i64,
    pub failures_in_row: u32,
    pub last_verified: i64,
    pub verify_result: String,
    pub verify_error: Option<String>,
    /// What the backup takes at the destination, after deduplication and compression.
    pub stored_bytes: Option<u64>,
    pub last_restore: Option<Restored>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Restored {
    pub snapshot: String,
    /// The folder in the library it went to.
    pub to: String,
    pub finished: i64,
    pub error: Option<String>,
}

/// What is happening right now.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    /// "backup", "restore", or "verify".
    pub action: String,
    /// What that step is doing, in words.
    pub phase: String,
    pub percent: f64,
    pub files_done: u64,
    pub total_files: u64,
    pub bytes_done: u64,
    pub total_bytes: u64,
    pub seconds_remaining: Option<u64>,
    pub current: Option<String>,
    pub started: i64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub id: String,
    pub short_id: String,
    pub time: String,
    pub paths: Vec<String>,
    /// Where the library was when the snapshot was made.
    pub library: Option<String>,
    pub total_files: Option<u64>,
    pub total_bytes: Option<u64>,
    pub data_added: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub name: String,
    pub dir: bool,
    pub size: u64,
    pub mtime: String,
}

/// The backup worker: one restic command at a time.
#[derive(Default)]
pub struct NasBackup {
    progress: Mutex<Option<Progress>>,
    child: Mutex<Option<Arc<Mutex<Child>>>>,
    stop: AtomicBool,
    snapshots: Mutex<Option<Vec<Snapshot>>>,
    started: AtomicBool,
}

/// Everything a restic command needs.
#[derive(Clone)]
pub struct Context {
    pub restic: PathBuf,
    pub root: PathBuf,
    pub data: PathBuf,
    pub db_path: PathBuf,
    pub config: Config,
    pub secrets: Secrets,
}

// ---- Settings and secrets ----

pub fn load_config(conn: &rusqlite::Connection) -> Config {
    crate::db::get_setting(conn, SETTING_CONFIG).ok().flatten().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

pub fn save_config(conn: &rusqlite::Connection, config: &Config) -> Result<(), String> {
    crate::db::set_setting(conn, SETTING_CONFIG, Some(&serde_json::to_string(config).unwrap_or_default()))
}

pub fn load_state(conn: &rusqlite::Connection) -> State {
    crate::db::get_setting(conn, SETTING_STATE).ok().flatten().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

fn save_state(conn: &rusqlite::Connection, state: &State) {
    let _ = crate::db::set_setting(conn, SETTING_STATE, Some(&serde_json::to_string(state).unwrap_or_default()));
}

pub fn load_secrets(data: &Path) -> Secrets {
    std::fs::read(data.join(SECRETS_FILE)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}

pub fn save_secrets(data: &Path, secrets: &Secrets) -> Result<(), String> {
    let path = data.join(SECRETS_FILE);
    let body = serde_json::to_vec_pretty(secrets).unwrap_or_default();
    let temp = data.join(format!("{SECRETS_FILE}.tmp"));
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp).map_err(|_| "Could not save the backup password".to_string())?;
        std::io::Write::write_all(&mut file, &body).map_err(|_| "Could not save the backup password".to_string())?;
    }
    std::fs::rename(&temp, &path).map_err(|_| "Could not save the backup password".to_string())
}

/// A password for a new backup: 32 letters and digits, easy to write down in groups.
pub fn new_password() -> String {
    const CHARS: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    let mut rng = rand::thread_rng();
    (0..32).map(|i| {
        let c = CHARS[rng.gen_range(0..CHARS.len())] as char;
        if i > 0 && i % 4 == 0 { format!("-{c}") } else { c.to_string() }
    }).collect()
}

/// Checks a destination before it is saved. Returns it tidied up.
pub fn validate(destination: &Destination, root: &Path, data: &Path) -> Result<Destination, String> {
    let mut d = destination.clone();
    for field in [&mut d.path, &mut d.endpoint, &mut d.bucket, &mut d.prefix, &mut d.region, &mut d.access_key_id, &mut d.host, &mut d.user] {
        *field = field.trim().to_string();
    }
    match d.kind.as_str() {
        "local" => {
            let path = PathBuf::from(&d.path);
            if !path.is_absolute() {
                return Err("Choose the backup folder by its full path, such as /Volumes/Backup/ownnas".into());
            }
            let real = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
            // The folder may not exist yet; what matters is where it would be.
            let target = match path.ancestors().find(|a| a.exists()) {
                Some(existing) => real(existing).join(path.strip_prefix(existing).unwrap_or(Path::new(""))),
                None => path.clone(),
            };
            for (inside, name) in [(real(root), "the library"), (real(data), "OwnNAS's data folder")] {
                if target.starts_with(&inside) || inside.starts_with(&target) {
                    return Err(format!("Keep the backup outside {name}: it would back itself up"));
                }
            }
        }
        "s3" => {
            if d.endpoint.is_empty() {
                d.endpoint = "s3.amazonaws.com".into();
            }
            d.endpoint = d.endpoint.trim_end_matches('/').to_string();
            d.prefix = d.prefix.trim_matches('/').to_string();
            if d.bucket.is_empty() || !d.bucket.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_') {
                return Err("Enter the bucket's name (letters, digits, dots, and dashes)".into());
            }
            if d.access_key_id.is_empty() {
                return Err("Enter the access key ID".into());
            }
        }
        "sftp" => {
            if d.host.is_empty() || d.host.contains(['/', ' ', '@']) {
                return Err("Enter the other machine's name or address".into());
            }
            if d.user.is_empty() || d.user.contains(['/', ' ', '@', ':']) {
                return Err("Enter the user name to sign in to the other machine with".into());
            }
            if d.path.is_empty() {
                return Err("Enter the folder on the other machine, such as /backups/ownnas".into());
            }
            if d.port == 0 {
                d.port = 22;
            }
        }
        _ => return Err("Choose where backups go".into()),
    }
    Ok(d)
}

/// The repository in restic's terms.
pub fn repository(d: &Destination) -> String {
    match d.kind.as_str() {
        "s3" => {
            let endpoint = &d.endpoint;
            let prefix = if d.prefix.is_empty() { String::new() } else { format!("/{}", d.prefix) };
            format!("s3:{endpoint}/{}{prefix}", d.bucket)
        }
        "sftp" => {
            // In the URL form an absolute path follows a second slash, "…:22//srv/backup".
            let port = if d.port == 0 { 22 } else { d.port };
            format!("sftp://{}@{}:{}/{}", d.user, d.host, port, d.path)
        }
        _ => d.path.clone(),
    }
}

/// A short description of the destination for the page.
pub fn describe(d: &Destination) -> String {
    match d.kind.as_str() {
        "s3" => format!("{}/{}{}", d.endpoint, d.bucket, if d.prefix.is_empty() { String::new() } else { format!("/{}", d.prefix) }),
        "sftp" => format!("{}@{}:{}", d.user, d.host, d.path),
        _ => d.path.clone(),
    }
}

// ---- Finding restic ----

/// Where restic is, also in the usual install folders a service's bare PATH lacks.
pub fn find_restic() -> Option<PathBuf> {
    if let Some(path) = crate::helper_tools::find("restic") {
        return Some(path);
    }
    let candidates: &[&str] = if cfg!(windows) { &[] } else { &["/opt/homebrew/bin/restic", "/usr/local/bin/restic", "/usr/bin/restic", "/snap/bin/restic"] };
    candidates.iter().map(PathBuf::from).find(|p| p.is_file())
}

/// restic's version, such as (0, 19, 1).
pub fn version(restic: &Path) -> Option<(u32, u32, u32)> {
    let output = Command::new(restic).arg("version").stdin(Stdio::null()).output().ok()?;
    parse_version(&String::from_utf8_lossy(&output.stdout))
}

fn parse_version(text: &str) -> Option<(u32, u32, u32)> {
    let word = text.split_whitespace().nth(1)?;
    let mut parts = word.trim_start_matches('v').split('.').map(|p| p.chars().take_while(char::is_ascii_digit).collect::<String>().parse::<u32>().ok());
    Some((parts.next()??, parts.next()??, parts.next().flatten().unwrap_or(0)))
}

pub fn new_enough(version: (u32, u32, u32)) -> bool {
    (version.0, version.1) >= MIN_VERSION
}

// ---- The schedule ----

/// The next scheduled time after `now`, on this computer's clock.
pub fn next_slot(config: &Config, now: i64) -> Option<i64> {
    slots_in(config, now, &chrono::Local).find(|&slot| slot > now)
}

/// The scheduled times from a week before `now` to a week after it, in order.
fn slots_in<Tz: chrono::TimeZone>(config: &Config, now: i64, tz: &Tz) -> impl Iterator<Item = i64> {
    use chrono::{Datelike, Duration as Days, NaiveTime};
    let scheduled = matches!(config.frequency.as_str(), "daily" | "weekly");
    let weekly = config.frequency == "weekly";
    let minute = config.minute.min(24 * 60 - 1);
    let time = NaiveTime::from_hms_opt(minute / 60, minute % 60, 0).unwrap_or_default();
    let today = tz.timestamp_opt(now, 0).single().map(|at| at.date_naive());
    let weekday = u32::from(config.weekday % 7);
    let tz = tz.clone();
    (-8i64..=8)
        .filter_map(move |offset| {
            let day = today? + Days::days(offset);
            if !scheduled || (weekly && day.weekday().num_days_from_sunday() != weekday) {
                return None;
            }
            let at = day.and_time(time);
            // In the hour skipped when clocks go forward, an hour later.
            tz.from_local_datetime(&at)
                .earliest()
                .or_else(|| tz.from_local_datetime(&(at + Days::hours(1))).earliest())
                .map(|at| at.timestamp())
        })
}

/// Whether a scheduled backup should start now: a slot passed since the last one started (also
/// one missed while the computer was off), or the last one failed and a retry is due.
pub fn due(config: &Config, state: &State, now: i64) -> bool {
    due_in(config, state, now, &chrono::Local)
}

fn due_in<Tz: chrono::TimeZone>(config: &Config, state: &State, now: i64, tz: &Tz) -> bool {
    if !config.enabled {
        return false;
    }
    if state.last_result == "failed" && state.failures_in_row > 0 && state.failures_in_row <= RETRIES && now - state.last_finished >= RETRY_AFTER {
        return config.frequency != "manual";
    }
    match slots_in(config, now, tz).filter(|&slot| slot <= now).last() {
        Some(slot) => state.last_started < slot && slot >= config.saved_at,
        None => false,
    }
}

/// A notice for the page's Health line: the last backup failed, or none has worked for too long.
pub fn alert(config: &Config, state: &State, now: i64) -> Option<Value> {
    if !config.enabled {
        return None;
    }
    if state.last_result == "failed" {
        return Some(serde_json::json!({ "kind": "backup", "level": "low", "problem": "failed", "error": state.last_error, "lastSuccess": state.last_success }));
    }
    let interval = match config.frequency.as_str() {
        "daily" => 86400,
        "weekly" => 7 * 86400,
        _ => return None,
    };
    let since = if state.last_success > 0 { state.last_success } else { config.saved_at };
    (since > 0 && now - since > 2 * interval + 6 * 3600)
        .then(|| serde_json::json!({ "kind": "backup", "level": "low", "problem": "late", "lastSuccess": state.last_success }))
}

// ---- Running restic ----

impl Context {
    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(&self.restic);
        let d = &self.config.destination;
        if d.kind == "sftp" {
            // No password prompts: the other machine must accept this computer's SSH key.
            let port = if d.port == 0 { 22 } else { d.port };
            cmd.arg("-o").arg(format!("sftp.args=-p {port} -o BatchMode=yes -o StrictHostKeyChecking=accept-new -o ConnectTimeout=20"));
        }
        cmd.args(args)
            .env("RESTIC_REPOSITORY", repository(d))
            .env("RESTIC_PASSWORD", &self.secrets.password)
            .env("RESTIC_CACHE_DIR", self.data.join(CACHE_DIR))
            .env("RESTIC_PROGRESS_FPS", "1")
            .env_remove("RESTIC_PASSWORD_FILE")
            .env_remove("RESTIC_PASSWORD_COMMAND")
            .env_remove("RESTIC_REPOSITORY_FILE")
            .stdin(Stdio::null());
        if d.kind == "s3" {
            cmd.env("AWS_ACCESS_KEY_ID", &d.access_key_id).env("AWS_SECRET_ACCESS_KEY", &self.secrets.s3_secret);
            if !d.region.is_empty() {
                cmd.env("AWS_DEFAULT_REGION", &d.region);
            }
        }
        cmd
    }

    /// The library's Trash and OwnNAS's caches and live database stay out of the backup.
    fn excludes(&self) -> Vec<String> {
        let mut out: Vec<String> = ["thumbs", "text-cache", "terminal-uploads", "models", CACHE_DIR, SECRETS_FILE, &format!("{SECRETS_FILE}.tmp")]
            .iter()
            .map(|name| self.data.join(name).display().to_string())
            .collect();
        if let Some(name) = self.db_path.file_name().and_then(|n| n.to_str()) {
            for suffix in ["", "-wal", "-shm", "-journal"] {
                out.push(self.data.join(format!("{name}{suffix}")).display().to_string());
            }
        }
        if self.config.skip_trash {
            out.push(self.root.join(crate::files::TRASH_DIR).display().to_string());
        }
        out.extend(self.config.excludes.iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()));
        out
    }
}

/// What a finished restic command printed, and how it ended.
struct Finished {
    code: i32,
    stdout: String,
    stderr: String,
}

/// The last useful line restic wrote to stderr, as a message for the page.
fn error_text(finished: &Finished) -> String {
    match finished.code {
        EXIT_WRONG_PASSWORD => return "The backup password does not open the backup at this destination. Enter the password it was made with.".into(),
        EXIT_LOCKED => return "Another program is using the backup right now (restic says it is locked). Try again later.".into(),
        EXIT_NO_REPOSITORY => return "There is no backup at this destination yet.".into(),
        _ => {}
    }
    // "Stat(<config/>) returned error, retrying after 928ms: Stat: The request signature…"
    if let Some(reason) = finished.stderr.lines().find_map(|l| l.split_once("retrying after ").and_then(|(_, rest)| rest.split_once(": ")).map(|(_, reason)| reason.trim())) {
        let reason = reason.trim_start_matches("Stat: ").trim_start_matches("Load: ");
        return explain(reason);
    }
    let line = finished
        .stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('{'))
        .filter(|l| !l.starts_with("Load(") && !l.starts_with("Save("))
        .find(|l| l.starts_with("Fatal:") || l.contains("rror"))
        .or_else(|| finished.stderr.lines().map(str::trim).rfind(|l| !l.is_empty()))
        .unwrap_or("restic failed")
        .trim_start_matches("Fatal: ")
        .to_string();
    explain(&line)
}

/// Adds what to do about the errors people run into most.
fn explain(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    if lower.contains("permission denied (publickey") || lower.contains("host key verification failed") {
        return format!("The other machine did not accept this computer's SSH key. Add the key shown on this page to its ~/.ssh/authorized_keys. ({line})");
    }
    if lower.contains("unable to start the sftp session") {
        return format!("Could not sign in to the other computer over SSH. Check its name, port, and user, and that it accepts this computer's SSH key. ({line})");
    }
    if lower.contains("connection refused") || lower.contains("no such host") || lower.contains("could not resolve") || lower.contains("i/o timeout") {
        return format!("Could not reach the destination. ({line})");
    }
    if lower.contains("access denied") || lower.contains("signature") || lower.contains("access key") || lower.contains("invalidaccesskeyid") {
        return format!("The bucket refused the access key. ({line})");
    }
    line.chars().take(400).collect()
}

impl NasBackup {
    pub fn progress(&self) -> Option<Progress> {
        self.progress.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn busy(&self) -> bool {
        self.progress().is_some()
    }

    fn set_progress(&self, update: impl FnOnce(&mut Progress)) {
        if let Some(progress) = self.progress.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            update(progress);
        }
    }

    /// Claims the worker for one action, or says what it is busy with.
    fn begin(&self, action: &str, phase: &str) -> Result<(), String> {
        let mut slot = self.progress.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(running) = slot.as_ref() {
            return Err(format!("A {} is already running", running.action));
        }
        self.stop.store(false, Ordering::SeqCst);
        *slot = Some(Progress { action: action.into(), phase: phase.into(), started: crate::db::now_secs(), ..Progress::default() });
        Ok(())
    }

    fn end(&self) {
        *self.progress.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// Stops what is running.
    /// restic is interrupted as with Ctrl+C, so it removes its lock on the backup; if it has
    /// not ended a while later, it is killed.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        let Some(child) = self.child.lock().unwrap_or_else(|e| e.into_inner()).clone() else { return };
        let pid = child.lock().unwrap_or_else(|e| e.into_inner()).id();
        #[cfg(unix)]
        let interrupted = Command::new("kill").args(["-INT", &pid.to_string()]).status().is_ok_and(|s| s.success());
        #[cfg(not(unix))]
        let interrupted = { let _ = pid; false };
        if !interrupted {
            let _ = child.lock().unwrap_or_else(|e| e.into_inner()).kill();
            return;
        }
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(15));
            let mut child = child.lock().unwrap_or_else(|e| e.into_inner());
            if matches!(child.try_wait(), Ok(None)) {
                let _ = child.kill();
            }
        });
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    /// Runs a step of the action under way to the end (Stop ends it), handing each line of its
    /// JSON output to `line`.
    fn run(&self, cmd: Command, line: impl FnMut(&Value)) -> Result<Finished, String> {
        if self.stopped() {
            return Err("Stopped".into());
        }
        let finished = self.exec(cmd, true, false, line)?;
        if self.stopped() {
            return Err("Stopped".into());
        }
        Ok(finished)
    }

    /// Runs a command that only reads (the snapshots, a folder in one), also while a backup runs.
    /// It gives up at the first error restic would retry, so the page is not kept waiting.
    fn query(&self, cmd: Command, line: impl FnMut(&Value)) -> Result<Finished, String> {
        self.exec(cmd, false, true, line)
    }

    /// Tries the destination: like `run`, but a wrong key or address fails at once instead of
    /// being retried for minutes.
    fn probe(&self, cmd: Command) -> Result<Finished, String> {
        if self.stopped() {
            return Err("Stopped".into());
        }
        self.exec(cmd, true, true, |_| {})
    }

    fn exec(&self, mut cmd: Command, stoppable: bool, no_retries: bool, mut line: impl FnMut(&Value)) -> Result<Finished, String> {
        let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|_| "Could not start restic".to_string())?;
        let stdout = child.stdout.take();
        let stderr_pipe = child.stderr.take();
        let child = Arc::new(Mutex::new(child));
        if stoppable {
            *self.child.lock().unwrap_or_else(|e| e.into_inner()) = Some(child.clone());
        }
        let watched = child.clone();
        let stderr_thread = std::thread::spawn(move || {
            let mut text = String::new();
            let mut gave_up = false;
            if let Some(pipe) = stderr_pipe {
                for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                    if no_retries && !gave_up && line.contains("retrying after") {
                        gave_up = true;
                        let _ = watched.lock().unwrap_or_else(|e| e.into_inner()).kill();
                    }
                    if text.len() < 1 << 20 {
                        text.push_str(&line);
                        text.push('\n');
                    }
                }
            }
            (text, gave_up)
        });
        let mut out = String::new();
        if let Some(stdout) = stdout {
            for text in BufReader::new(stdout).lines().map_while(Result::ok) {
                match serde_json::from_str::<Value>(&text) {
                    Ok(value) => line(&value),
                    Err(_) => {
                        if out.len() < 1 << 20 {
                            out.push_str(&text);
                            out.push('\n');
                        }
                    }
                }
            }
        }
        if stoppable {
            self.child.lock().unwrap_or_else(|e| e.into_inner()).take();
        }
        let status = child.lock().unwrap_or_else(|e| e.into_inner()).wait();
        let (stderr, gave_up) = stderr_thread.join().unwrap_or_default();
        let code = match status {
            _ if gave_up => EXIT_GAVE_UP,
            Ok(status) => status.code().unwrap_or(-1),
            Err(_) => -1,
        };
        Ok(Finished { code, stdout: out, stderr })
    }

    /// Opens the backup at the destination, making a new one when there is none yet.
    /// Returns whether it was made now.
    pub fn connect(&self, ctx: &Context) -> Result<bool, String> {
        let finished = self.probe(ctx.command(&["cat", "config", "--no-lock"]))?;
        match finished.code {
            0 => Ok(false),
            EXIT_NO_REPOSITORY => {
                let made = self.probe(ctx.command(&["init"]))?;
                if made.code == 0 { Ok(true) } else { Err(error_text(&made)) }
            }
            _ => Err(error_text(&finished)),
        }
    }

    /// Tries the destination now, for the Save button. Not while something else runs.
    pub fn test(&self, ctx: &Context) -> Result<bool, String> {
        self.begin("connect", "Connecting to the destination")?;
        let result = self.connect(ctx);
        self.end();
        result
    }

    /// One backup: a copy of the database, the snapshot, the keep rules, and now and then a
    /// check that the backup reads back.
    pub fn backup(&self, ctx: &Context, conn: &rusqlite::Connection) -> Result<Summary, String> {
        self.begin("backup", "Getting ready")?;
        let mut state = load_state(conn);
        state.last_started = crate::db::now_secs();
        save_state(conn, &state);
        let result = self.backup_steps(ctx, conn);
        let mut state = load_state(conn);
        state.last_finished = crate::db::now_secs();
        match &result {
            Ok(summary) => {
                state.last_result = if summary.unreadable > 0 { "warning" } else { "ok" }.into();
                state.last_error = (summary.unreadable > 0).then(|| format!("{} files could not be read and are not in this backup", summary.unreadable));
                state.last_summary = Some(summary.clone());
                state.last_success = state.last_finished;
                state.failures_in_row = 0;
            }
            Err(err) if self.stopped() || err == "Stopped" => {
                state.last_result = "stopped".into();
                state.last_error = None;
            }
            Err(err) => {
                state.last_result = "failed".into();
                state.last_error = Some(err.clone());
                state.failures_in_row += 1;
            }
        }
        save_state(conn, &state);
        if result.is_ok() && !self.stopped() && state.last_finished - state.last_verified > VERIFY_EVERY {
            let _ = self.verify_steps(ctx, conn, "2%");
        }
        self.end();
        result
    }

    fn backup_steps(&self, ctx: &Context, conn: &rusqlite::Connection) -> Result<Summary, String> {
        // A consistent copy of the database, taken while OwnNAS keeps running.
        let staging = ctx.data.join(STAGING_DIR);
        std::fs::create_dir_all(&staging).map_err(|_| "Could not prepare the database copy".to_string())?;
        let copy = staging.join("ownnas.db");
        let _ = std::fs::remove_file(&copy);
        self.set_progress(|p| p.phase = "Copying OwnNAS's database".into());
        conn.execute("VACUUM INTO ?1", [copy.display().to_string()]).map_err(|err| format!("Could not copy the database: {err}"))?;

        self.set_progress(|p| p.phase = "Connecting to the destination".into());
        self.connect(ctx)?;
        // Locks left by a restic that did not end cleanly (only those whose program is gone).
        let _ = self.run(ctx.command(&["unlock"]), |_| {})?;

        self.set_progress(|p| p.phase = "Backing up".into());
        let root = ctx.root.display().to_string();
        let data = ctx.data.display().to_string();
        let mut args: Vec<String> = ["backup", "--json", "--tag", TAG, "--host", HOST].iter().map(|s| s.to_string()).collect();
        for pattern in ctx.excludes() {
            args.push("--exclude".into());
            args.push(pattern);
        }
        args.push(root);
        args.push(data);
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let mut summary = Summary::default();
        let mut unreadable = 0u64;
        let finished = self.run(ctx.command(&refs), |value| match value["message_type"].as_str() {
            Some("status") => self.set_progress(|p| apply_status(p, value)),
            Some("error") => unreadable += 1,
            Some("summary") => summary = parse_summary(value),
            _ => {}
        })?;
        if finished.code != 0 && finished.code != EXIT_PARTIAL {
            return Err(error_text(&finished));
        }
        summary.unreadable = unreadable;
        let _ = std::fs::remove_file(&copy);

        self.set_progress(|p| {
            p.phase = "Removing old snapshots".into();
            p.current = None;
        });
        let c = &ctx.config;
        let keep = [
            "forget".to_string(), "--tag".into(), TAG.into(), "--host".into(), HOST.into(), "--prune".into(),
            "--keep-last".into(), "1".into(),
            "--keep-daily".into(), c.keep_daily.to_string(),
            "--keep-weekly".into(), c.keep_weekly.to_string(),
            "--keep-monthly".into(), c.keep_monthly.to_string(),
        ];
        let refs: Vec<&str> = keep.iter().map(String::as_str).collect();
        let forgot = self.run(ctx.command(&refs), |_| {})?;
        if forgot.code != 0 {
            // The snapshot is safe; only the tidying failed.
            return Err(format!("The backup worked, but removing old snapshots failed: {}", error_text(&forgot)));
        }

        self.set_progress(|p| p.phase = "Measuring the backup".into());
        let mut stored = None;
        let stats = self.run(ctx.command(&["stats", "--json", "--mode", "raw-data", "--no-lock"]), |value| {
            if let Some(size) = value["total_size"].as_u64() {
                stored = Some(size);
            }
        })?;
        if stats.code == 0 && stored.is_some() {
            let mut state = load_state(conn);
            state.stored_bytes = stored;
            save_state(conn, &state);
        }
        *self.snapshots.lock().unwrap_or_else(|e| e.into_inner()) = None;
        Ok(summary)
    }

    /// Reads back a share of the backup (such as "5%") to check it is all there and intact.
    pub fn verify(&self, ctx: &Context, conn: &rusqlite::Connection, subset: &str) -> Result<(), String> {
        self.begin("verify", "Checking the backup")?;
        let result = self.verify_steps(ctx, conn, subset);
        self.end();
        result
    }

    fn verify_steps(&self, ctx: &Context, conn: &rusqlite::Connection, subset: &str) -> Result<(), String> {
        // After a backup this is still part of it, as far as the page is concerned.
        self.set_progress(|p| {
            p.phase = format!("Checking the backup (reading {subset} of it back)");
            p.percent = 0.0;
            p.current = None;
        });
        let finished = self.run(ctx.command(&["check", "--read-data-subset", subset]), |_| {});
        let mut state = load_state(conn);
        match &finished {
            Err(_) => return Err("Stopped".into()),
            Ok(done) if done.code == 0 => {
                state.last_verified = crate::db::now_secs();
                state.verify_result = "ok".into();
                state.verify_error = None;
            }
            Ok(done) => {
                state.last_verified = crate::db::now_secs();
                state.verify_result = "failed".into();
                let text = if done.code == 1 && (done.stdout.contains("error") || done.stderr.contains("error")) {
                    let all = format!("{}\n{}", done.stdout, done.stderr);
                    all.lines().map(str::trim).find(|l| l.to_ascii_lowercase().contains("error")).unwrap_or("The check found problems").to_string()
                } else {
                    error_text(done)
                };
                state.verify_error = Some(text);
            }
        }
        let ok = state.verify_result == "ok";
        let error = state.verify_error.clone();
        save_state(conn, &state);
        if ok { Ok(()) } else { Err(error.unwrap_or_default()) }
    }

    /// The snapshots at the destination, newest first.
    pub fn snapshots(&self, ctx: &Context, fresh: bool) -> Result<Vec<Snapshot>, String> {
        if !fresh {
            if let Some(list) = self.snapshots.lock().unwrap_or_else(|e| e.into_inner()).clone() {
                return Ok(list);
            }
        }
        let mut list: Vec<Snapshot> = Vec::new();
        let finished = self.query(ctx.command(&["snapshots", "--json", "--no-lock", "--tag", TAG]), |value| {
            if let Some(items) = value.as_array() {
                list = items.iter().map(|item| parse_snapshot(item, &ctx.data)).collect();
            }
        })?;
        if finished.code != 0 {
            return Err(error_text(&finished));
        }
        list.sort_by(|a, b| b.time.cmp(&a.time));
        *self.snapshots.lock().unwrap_or_else(|e| e.into_inner()) = Some(list.clone());
        Ok(list)
    }

    /// The folders and files directly inside a folder of a snapshot's library.
    pub fn browse(&self, ctx: &Context, snapshot: &str, library: &str, rel: &str) -> Result<Vec<Entry>, String> {
        check_id(snapshot)?;
        let dir = snapshot_path(library, rel)?;
        let mut entries = Vec::new();
        let finished = self.query(ctx.command(&["ls", "--json", "--no-lock", snapshot, &dir]), |value| {
            if value["struct_type"].as_str() == Some("node") && value["path"].as_str().map(|p| p.trim_end_matches('/')) != Some(dir.trim_end_matches('/')) {
                entries.push(Entry {
                    name: value["name"].as_str().unwrap_or_default().to_string(),
                    dir: value["type"].as_str() == Some("dir"),
                    size: value["size"].as_u64().unwrap_or(0),
                    mtime: value["mtime"].as_str().unwrap_or_default().to_string(),
                });
            }
        })?;
        if finished.code != 0 {
            return Err(error_text(&finished));
        }
        entries.retain(|e| !(rel.is_empty() && e.name == crate::files::TRASH_DIR));
        entries.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        Ok(entries)
    }

    /// Restores a folder (or the whole library) of a snapshot into `target`, a new folder.
    #[allow(clippy::too_many_arguments)]
    pub fn restore(&self, ctx: &Context, conn: &rusqlite::Connection, snapshot: &str, library: &str, rel: &str, target: &Path, shown_as: &str) -> Result<(), String> {
        check_id(snapshot)?;
        let from = snapshot_path(library, rel)?;
        self.begin("restore", "Restoring")?;
        let spec = format!("{snapshot}:{from}");
        let target_text = target.display().to_string();
        let result = self
            .run(ctx.command(&["restore", &spec, "--target", &target_text, "--json"]), |value| {
                if value["message_type"].as_str() == Some("status") {
                    self.set_progress(|p| apply_status(p, value));
                }
            })
            .and_then(|finished| if finished.code == 0 { Ok(()) } else { Err(error_text(&finished)) });
        let mut state = load_state(conn);
        state.last_restore = Some(Restored { snapshot: snapshot.chars().take(8).collect(), to: shown_as.into(), finished: crate::db::now_secs(), error: result.as_ref().err().cloned() });
        save_state(conn, &state);
        self.end();
        result
    }

    /// Starts the scheduler: once a minute it looks whether a backup is due.
    pub fn start_scheduler(self: &Arc<Self>, root: PathBuf, data: PathBuf, db_path: PathBuf) {
        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let worker = self.clone();
        std::thread::Builder::new()
            .name("ownnas-backup".into())
            .spawn(move || {
                let Ok(conn) = crate::db::open(&db_path) else { return };
                // A backup that was running when OwnNAS stopped did not finish.
                let mut state = load_state(&conn);
                if state.last_started > state.last_finished {
                    state.last_finished = state.last_started;
                    state.last_result = "failed".into();
                    state.last_error = Some("OwnNAS stopped during the backup".into());
                    state.failures_in_row += 1;
                    save_state(&conn, &state);
                }
                loop {
                    std::thread::sleep(Duration::from_secs(60));
                    let config = load_config(&conn);
                    if worker.busy() || !due(&config, &load_state(&conn), crate::db::now_secs()) {
                        continue;
                    }
                    let Some(restic) = find_restic() else {
                        let mut state = load_state(&conn);
                        state.last_started = crate::db::now_secs();
                        state.last_finished = state.last_started;
                        state.last_result = "failed".into();
                        state.last_error = Some("restic is not installed".into());
                        state.failures_in_row += 1;
                        save_state(&conn, &state);
                        continue;
                    };
                    let ctx = Context { restic, root: root.clone(), data: data.clone(), db_path: db_path.clone(), config, secrets: load_secrets(&data) };
                    let _ = worker.backup(&ctx, &conn);
                }
            })
            .ok();
    }
}

fn apply_status(p: &mut Progress, value: &Value) {
    p.percent = value["percent_done"].as_f64().unwrap_or(p.percent);
    p.total_files = value["total_files"].as_u64().unwrap_or(p.total_files);
    p.files_done = value["files_done"].as_u64().or_else(|| value["files_restored"].as_u64()).unwrap_or(p.files_done);
    p.total_bytes = value["total_bytes"].as_u64().unwrap_or(p.total_bytes);
    p.bytes_done = value["bytes_done"].as_u64().or_else(|| value["bytes_restored"].as_u64()).unwrap_or(p.bytes_done);
    p.seconds_remaining = value["seconds_remaining"].as_u64();
    p.current = value["current_files"].as_array().and_then(|files| files.first()).and_then(Value::as_str).map(str::to_string);
}

fn parse_summary(value: &Value) -> Summary {
    let n = |key: &str| value[key].as_u64().unwrap_or(0);
    Summary {
        snapshot: value["snapshot_id"].as_str().unwrap_or_default().to_string(),
        files_new: n("files_new"),
        files_changed: n("files_changed"),
        files_unmodified: n("files_unmodified"),
        // What was sent, after compression.
        data_added: value["data_added_packed"].as_u64().unwrap_or_else(|| n("data_added")),
        total_files: n("total_files_processed"),
        total_bytes: n("total_bytes_processed"),
        seconds: value["total_duration"].as_f64().unwrap_or(0.0),
        unreadable: 0,
    }
}

fn parse_snapshot(item: &Value, data: &Path) -> Snapshot {
    let paths: Vec<String> = item["paths"].as_array().map(|a| a.iter().filter_map(|p| p.as_str().map(str::to_string)).collect()).unwrap_or_default();
    // The library is the path that is not OwnNAS's data folder.
    let data_name = data.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let data_text = data.display().to_string();
    let library = paths
        .iter()
        .find(|p| **p != data_text && !p.trim_end_matches(['/', '\\']).ends_with(&data_name))
        .or(paths.first())
        .cloned();
    let summary = &item["summary"];
    Snapshot {
        id: item["id"].as_str().unwrap_or_default().to_string(),
        short_id: item["short_id"].as_str().unwrap_or_default().to_string(),
        time: item["time"].as_str().unwrap_or_default().to_string(),
        paths,
        library,
        total_files: summary["total_files_processed"].as_u64(),
        total_bytes: summary["total_bytes_processed"].as_u64(),
        data_added: summary["data_added_packed"].as_u64().or_else(|| summary["data_added"].as_u64()),
    }
}

fn check_id(id: &str) -> Result<(), String> {
    if id.len() >= 8 && id.len() <= 64 && id.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err("Unknown snapshot".into())
    }
}

/// A folder of the snapshot's library, in restic's form ("/" separators; "C:\x" is "/C/x").
fn snapshot_path(library: &str, rel: &str) -> Result<String, String> {
    if rel.split(['/', '\\']).any(|part| part == ".." || part == ".") {
        return Err("Invalid folder".into());
    }
    let mut base = library.replace('\\', "/");
    if base.len() >= 2 && base.as_bytes()[1] == b':' {
        base = format!("/{}{}", &base[..1], &base[2..]);
    }
    let base = base.trim_end_matches('/').to_string();
    let rel = rel.trim_matches(['/', '\\']).replace('\\', "/");
    Ok(if rel.is_empty() { if base.is_empty() { "/".into() } else { base } } else { format!("{base}/{rel}") })
}

/// This computer's SSH public key, to add to the other machine for SFTP backups.
pub fn ssh_public_key() -> Option<String> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    let dir = PathBuf::from(home).join(".ssh");
    ["id_ed25519.pub", "id_ecdsa.pub", "id_rsa.pub"].iter().find_map(|name| std::fs::read_to_string(dir.join(name)).ok()).map(|key| key.trim().to_string())
}

/// Makes an SSH key for this computer's account when it has none.
pub fn create_ssh_key() -> Result<String, String> {
    if let Some(key) = ssh_public_key() {
        return Ok(key);
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).ok_or("No home folder")?;
    let dir = PathBuf::from(home).join(".ssh");
    std::fs::create_dir_all(&dir).map_err(|_| "Could not create ~/.ssh".to_string())?;
    let status = Command::new("ssh-keygen")
        .args(["-t", "ed25519", "-N", "", "-C", "ownnas-backup", "-f"])
        .arg(dir.join("id_ed25519"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|_| "ssh-keygen is not installed".to_string())?;
    if !status.success() {
        return Err("ssh-keygen could not make a key".into());
    }
    ssh_public_key().ok_or_else(|| "ssh-keygen could not make a key".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(frequency: &str, minute: u32, weekday: u8) -> Config {
        Config { enabled: true, frequency: frequency.into(), minute, weekday, ..Config::default() }
    }

    fn last_in<Tz: chrono::TimeZone>(config: &Config, now: i64, tz: &Tz) -> Option<i64> {
        slots_in(config, now, tz).filter(|&slot| slot <= now).last()
    }

    fn next_in<Tz: chrono::TimeZone>(config: &Config, now: i64, tz: &Tz) -> Option<i64> {
        slots_in(config, now, tz).find(|&slot| slot > now)
    }

    #[test]
    fn repositories_in_restic_terms() {
        let s3 = Destination { kind: "s3".into(), endpoint: "https://s3.example.com".into(), bucket: "nas".into(), prefix: "home".into(), ..Default::default() };
        assert_eq!(repository(&s3), "s3:https://s3.example.com/nas/home");
        let sftp = Destination { kind: "sftp".into(), user: "me".into(), host: "box".into(), port: 2222, path: "/srv/backup".into(), ..Default::default() };
        assert_eq!(repository(&sftp), "sftp://me@box:2222//srv/backup");
        let relative = Destination { path: "backup".into(), ..sftp };
        assert_eq!(repository(&relative), "sftp://me@box:2222/backup");
    }

    #[test]
    fn daily_and_weekly_slots() {
        // 2026-10-24 (a Saturday) 10:00 UTC.
        let now = 1_792_836_000;
        let utc = chrono::Utc;
        let daily = config("daily", 2 * 60, 0);
        assert_eq!(last_in(&daily, now, &utc), Some(1_792_807_200)); // 02:00 the same day
        assert_eq!(next_in(&daily, now, &utc), Some(1_792_807_200 + 86400));
        let late = config("daily", 23 * 60, 0);
        assert_eq!(last_in(&late, now, &utc), Some(1_792_796_400)); // 23:00 the day before
        let sunday = config("weekly", 2 * 60, 0);
        assert_eq!(last_in(&sunday, now, &utc), Some(1_792_807_200 - 6 * 86400)); // Sunday the 18th
        assert_eq!(next_in(&sunday, now, &utc), Some(1_792_807_200 + 86400)); // Sunday the 25th
        assert_eq!(last_in(&config("manual", 0, 0), now, &utc), None);
        // On a clock two hours ahead of UTC, 02:00 is midnight UTC.
        let cest = chrono::FixedOffset::east_opt(2 * 3600).unwrap();
        assert_eq!(last_in(&daily, now, &cest), Some(1_792_807_200 - 2 * 3600));
    }

    #[test]
    fn when_a_backup_is_due() {
        let now = 1_792_836_000;
        let mut c = config("daily", 2 * 60, 0);
        c.saved_at = now - 30 * 86400;
        let mut s = State::default();
        assert!(due_in(&c, &s, now, &chrono::Utc), "the 02:00 backup was missed");
        s.last_started = now - 3600;
        assert!(!due_in(&c, &s, now, &chrono::Utc), "it ran after 02:00");
        // Saved after today's slot: wait for tomorrow's.
        c.saved_at = now - 60;
        assert!(!due_in(&c, &State::default(), now, &chrono::Utc));
        // A failure is retried an hour later, a few times.
        c.saved_at = now - 30 * 86400;
        s = State { last_started: now - 4000, last_finished: now - 3700, last_result: "failed".into(), failures_in_row: 1, ..State::default() };
        assert!(due_in(&c, &s, now, &chrono::Utc));
        s.failures_in_row = RETRIES + 1;
        assert!(!due_in(&c, &s, now, &chrono::Utc));
        c.enabled = false;
        assert!(!due_in(&c, &State::default(), now, &chrono::Utc));
    }

    #[test]
    fn alerts_when_failed_or_late() {
        let now = 1_792_836_000;
        let mut c = config("daily", 120, 0);
        c.saved_at = now - 10 * 86400;
        let ok = State { last_success: now - 86400, last_result: "ok".into(), ..State::default() };
        assert!(alert(&c, &ok, now).is_none());
        let late = State { last_success: now - 3 * 86400, last_result: "ok".into(), ..State::default() };
        assert_eq!(alert(&c, &late, now).unwrap()["problem"], "late");
        let failed = State { last_result: "failed".into(), last_error: Some("x".into()), ..State::default() };
        assert_eq!(alert(&c, &failed, now).unwrap()["problem"], "failed");
    }

    #[test]
    fn versions_and_paths() {
        assert_eq!(parse_version("restic 0.19.1 compiled with go1.27.1 on darwin/arm64"), Some((0, 19, 1)));
        assert!(new_enough((0, 17, 0)) && !new_enough((0, 16, 4)));
        assert_eq!(snapshot_path("/srv/library", "").unwrap(), "/srv/library");
        assert_eq!(snapshot_path("/srv/library/", "Docs/2024").unwrap(), "/srv/library/Docs/2024");
        assert_eq!(snapshot_path("C:\\Users\\me\\Library", "Docs").unwrap(), "/C/Users/me/Library/Docs");
        assert!(snapshot_path("/srv/library", "../etc").is_err());
    }

    #[test]
    fn local_destinations_stay_outside() {
        let dir = std::env::temp_dir().join(format!("ownnas-backup-test-{}", std::process::id()));
        let root = dir.join("library");
        let data = dir.join("data");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        let local = |path: PathBuf| Destination { kind: "local".into(), path: path.display().to_string(), ..Default::default() };
        assert!(validate(&local(root.join("backup")), &root, &data).is_err());
        assert!(validate(&local(data.join("x")), &root, &data).is_err());
        assert!(validate(&local(dir.clone()), &root, &data).is_err());
        assert!(validate(&local(dir.join("backup")), &root, &data).is_ok());
        assert!(validate(&local(PathBuf::from("relative")), &root, &data).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn passwords_are_long_and_grouped() {
        let password = new_password();
        assert_eq!(password.len(), 32 + 7);
        assert_eq!(password.split('-').count(), 8);
    }
}
