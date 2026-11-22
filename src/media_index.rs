//! The photo and video index behind the Gallery timeline: where each picture is, when it was
//! taken, and how big it is. It is a cache in OwnNAS's database, rebuilt by scanning the
//! library; photos are only ever opened for reading.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use rusqlite::{params, Connection};
use serde::Serialize;

use crate::files;

/// How hard background indexing may work the machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Speed {
    /// One thread, resting three times as long as it works (about a quarter of one core),
    /// with outside tools at low priority.
    Slow,
    /// One thread, resting as long as it works (about half of one core), at low priority.
    Medium,
    /// As many threads as half the processor's cores (up to four), without resting.
    Fast,
}

impl Speed {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "slow" => Some(Self::Slow),
            "medium" => Some(Self::Medium),
            "fast" => Some(Self::Fast),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Slow => "slow",
            Self::Medium => "medium",
            Self::Fast => "fast",
        }
    }

    fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Slow,
            2 => Self::Fast,
            _ => Self::Medium,
        }
    }

    fn workers(self) -> usize {
        let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
        match self {
            Self::Slow => 1,
            Self::Medium => 1,
            Self::Fast => (cores / 2).clamp(1, 4),
        }
    }

    /// How long to rest after working for `worked`.
    pub(crate) fn rest(self, worked: Duration) -> Duration {
        let rest = match self {
            Self::Slow => worked * 3,
            Self::Medium => worked,
            Self::Fast => Duration::ZERO,
        };
        rest.min(Duration::from_secs(5))
    }
}

/// The setting that keeps the chosen speed.
pub const SETTING_SPEED: &str = "index_speed";

/// How often the library is scanned again on its own.
const RESCAN_EVERY: Duration = Duration::from_secs(30 * 60);
/// Index rows written per transaction, so the database is never held for long.
const BATCH: usize = 200;

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub scanning: bool,
    /// Photos and videos found so far in the current (or last) scan.
    pub seen: u64,
    /// Of those, how many were new or changed and had to be read.
    pub read: u64,
    /// Photos and videos in the index.
    pub total: u64,
    /// Unix time the last scan finished.
    pub finished_at: Option<i64>,
    pub error: Option<String>,
    /// Making thumbnails for what the scan found, after it.
    pub thumbnailing: bool,
    pub thumbs_done: u64,
    pub thumbs_total: u64,
    pub speed: Option<Speed>,
    /// Describing photos for search ("search") or looking for faces ("faces"), after thumbnails.
    pub analyzing: Option<String>,
    pub analysis_done: u64,
    pub analysis_total: u64,
    /// Waiting for a backup of the NAS to end before going on.
    pub paused_for_backup: bool,
}

/// Runs scans in a background thread: at start, every half hour, and when asked to.
pub struct Indexer {
    status: Mutex<Status>,
    wake: Mutex<bool>,
    signal: Condvar,
    speed: std::sync::atomic::AtomicU8,
    soon: std::sync::atomic::AtomicBool,
    /// The photos' search descriptions, as searched in memory.
    pub descriptions: crate::smart::DescriptionCache,
    #[cfg(ownnas_ml)]
    pub models: std::sync::OnceLock<Arc<crate::ml::Models>>,
}

impl Default for Indexer {
    fn default() -> Self {
        Self {
            status: Mutex::new(Status::default()),
            wake: Mutex::new(false),
            signal: Condvar::new(),
            speed: std::sync::atomic::AtomicU8::new(1),
            soon: std::sync::atomic::AtomicBool::new(false),
            descriptions: crate::smart::DescriptionCache::default(),
            #[cfg(ownnas_ml)]
            models: std::sync::OnceLock::new(),
        }
    }
}

impl Indexer {
    pub fn speed(&self) -> Speed {
        Speed::from_u8(self.speed.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// Takes effect at once, even in the middle of a scan.
    pub fn set_speed(&self, speed: Speed) {
        self.speed.store(speed as u8, std::sync::atomic::Ordering::Relaxed);
        crate::thumbs::set_low_priority(speed != Speed::Fast);
    }

    /// Rests after a piece of work, as the speed asks.
    fn pace(&self, started: std::time::Instant) {
        let rest = self.speed().rest(started.elapsed());
        if !rest.is_zero() {
            std::thread::sleep(rest);
        }
        crate::db_watch::heavy_pause_point();
    }

    pub fn status(&self) -> Status {
        let mut status = self.status.lock().unwrap_or_else(|err| err.into_inner()).clone();
        status.speed = Some(self.speed());
        status.paused_for_backup = crate::db_watch::held_for_backup();
        // Asked for and about to begin: whoever asked should wait for it to end.
        status.scanning = status.scanning || *self.wake.lock().unwrap_or_else(|err| err.into_inner());
        status
    }

    /// Asks for a scan a minute from now, once: for uploads that arrive one after another.
    pub fn rescan_soon(self: &Arc<Self>) {
        if self.soon.swap(true, std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let indexer = self.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(60));
            indexer.soon.store(false, std::sync::atomic::Ordering::Relaxed);
            indexer.rescan();
        });
    }

    /// Asks for a scan as soon as the current one (if any) finishes.
    pub fn rescan(&self) {
        *self.wake.lock().unwrap_or_else(|err| err.into_inner()) = true;
        self.signal.notify_all();
    }

    pub fn start(self: &Arc<Self>, db_path: PathBuf, root: PathBuf, thumbs: PathBuf, ffmpeg: bool) {
        let indexer = self.clone();
        std::thread::Builder::new()
            .name("ownnas-media-index".into())
            .spawn(move || loop {
                indexer.run(&db_path, &root);
                indexer.make_thumbnails(&db_path, &root, &thumbs, ffmpeg);
                #[cfg(ownnas_ml)]
                indexer.analyze(&db_path, &root, &thumbs, ffmpeg);
                let wake = indexer.wake.lock().unwrap_or_else(|err| err.into_inner());
                let (mut wake, _) = indexer
                    .signal
                    .wait_timeout_while(wake, RESCAN_EVERY, |asked| !*asked)
                    .unwrap_or_else(|err| err.into_inner());
                *wake = false;
            })
            .expect("could not start the media index thread");
    }

    fn run(&self, db_path: &Path, root: &Path) {
        {
            let mut status = self.status.lock().unwrap_or_else(|err| err.into_inner());
            *status = Status { scanning: true, total: status.total, finished_at: status.finished_at, ..Status::default() };
        }
        let result = open(db_path).and_then(|conn| scan_paced(&conn, root, &|seen, read| {
            let mut status = self.status.lock().unwrap_or_else(|err| err.into_inner());
            status.seen = seen;
            status.read = read;
        }, &|started| self.pace(started)).map(|_| count(&conn)));
        // Photos that are gone may have left the descriptions searched in memory.
        self.descriptions.invalidate();
        let mut status = self.status.lock().unwrap_or_else(|err| err.into_inner());
        status.scanning = false;
        status.finished_at = Some(crate::db::now_secs());
        match result {
            Ok(total) => status.total = total,
            Err(err) => status.error = Some(err),
        }
    }
}

impl Indexer {
    /// Makes the thumbnails the Gallery will ask for, newest photos first, so scrolling
    /// through it does not wait for them. Ones already made are skipped quickly.
    fn make_thumbnails(&self, db_path: &Path, root: &Path, thumbs: &Path, ffmpeg: bool) {
        let paths: Vec<(String, String)> = match open(db_path).and_then(|conn| {
            let mut stmt = conn
                .prepare("SELECT path, kind FROM media_index ORDER BY taken_at DESC, path DESC")
                .map_err(|err| err.to_string())?;
            let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?))).map_err(|err| err.to_string())?;
            Ok(rows.filter_map(Result::ok).collect())
        }) {
            Ok(paths) => paths,
            Err(_) => return,
        };
        {
            let mut status = self.status.lock().unwrap_or_else(|err| err.into_inner());
            status.thumbnailing = true;
            status.thumbs_done = 0;
            status.thumbs_total = paths.len() as u64;
        }
        let next = std::sync::atomic::AtomicUsize::new(0);
        // As many workers as the fastest speed allows; the speed (which can change meanwhile)
        // decides how many of them work and how long they rest.
        std::thread::scope(|scope| {
            for worker in 0..Speed::Fast.workers() {
                let next = &next;
                let paths = &paths;
                scope.spawn(move || loop {
                    if worker >= self.speed().workers() {
                        if next.load(std::sync::atomic::Ordering::Relaxed) >= paths.len() {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(250));
                        continue;
                    }
                    let index = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some((path, kind)) = paths.get(index) else { break };
                    let started = std::time::Instant::now();
                    let name = path.rsplit('/').next().unwrap_or(path);
                    // A new scan was asked for: stop here, it will start this again.
                    if *self.wake.lock().unwrap_or_else(|err| err.into_inner()) {
                        break;
                    }
                    if files::can_thumb(name, ffmpeg) {
                        let _ = crate::thumbs::ensure(&root.join(path), thumbs, kind == "video");
                        self.pace(started);
                    }
                    let mut status = self.status.lock().unwrap_or_else(|err| err.into_inner());
                    status.thumbs_done += 1;
                });
            }
        });
        let mut status = self.status.lock().unwrap_or_else(|err| err.into_inner());
        status.thumbnailing = false;
    }
}

#[cfg(ownnas_ml)]
impl Indexer {
    fn asked_to_stop(&self) -> bool {
        *self.wake.lock().unwrap_or_else(|err| err.into_inner())
    }

    fn set_analysis(&self, stage: Option<&str>, done: u64, total: u64) {
        let mut status = self.status.lock().unwrap_or_else(|err| err.into_inner());
        status.analyzing = stage.map(String::from);
        status.analysis_done = done;
        status.analysis_total = total;
    }

    /// Describes photos for search and looks for faces, when those features are on and their
    /// models are here. Photos are only read.
    fn analyze(&self, db_path: &Path, root: &Path, thumbs: &Path, ffmpeg: bool) {
        let Some(models) = self.models.get() else { return };
        let Ok(conn) = open(db_path) else { return };
        let enabled = |feature: crate::ml::Feature| {
            conn.query_row("SELECT value FROM settings WHERE key = ?1", [feature.setting()], |row| row.get::<_, String>(0)).ok().as_deref() == Some("1")
                && models.ready(feature)
        };
        let threads = || if self.speed() == Speed::Fast { 4 } else { 1 };
        let (total, mut described, mut scanned) = crate::smart::progress(&conn).unwrap_or_default();
        if enabled(crate::ml::Feature::Search) {
            loop {
                let Ok(batch) = crate::smart::pending_descriptions(&conn, 1000) else { break };
                if batch.is_empty() {
                    break;
                }
                for item in batch {
                    if self.asked_to_stop() {
                        self.set_analysis(None, 0, 0);
                        return;
                    }
                    self.set_analysis(Some("search"), described as u64, total as u64);
                    let started = std::time::Instant::now();
                    // The thumbnail is plenty for a 224-pixel model, and covers videos and HEIC.
                    let vector = crate::thumbs::ensure(&root.join(&item.path), thumbs, item.kind == "video")
                        .and_then(|thumb| image::open(&thumb).map_err(|err| err.to_string()))
                        .and_then(|image| models.describe_image(&image, threads()))
                        .unwrap_or_default();
                    let _ = crate::smart::store_description(&conn, &item, &vector);
                    described += 1;
                    self.descriptions.invalidate();
                    self.pace(started);
                }
            }
        }
        if enabled(crate::ml::Feature::Faces) {
            let Ok(mut people) = crate::smart::PeopleIndex::load(&conn) else { return };
            loop {
                let Ok(batch) = crate::smart::pending_faces(&conn, 1000) else { break };
                if batch.is_empty() {
                    break;
                }
                for item in batch {
                    if self.asked_to_stop() {
                        self.set_analysis(None, 0, 0);
                        return;
                    }
                    self.set_analysis(Some("faces"), scanned as u64, total as u64);
                    let started = std::time::Instant::now();
                    let found = crate::thumbs::decode_for_analysis(&root.join(&item.path), 1600)
                        .map(|image| image.to_rgb8())
                        .and_then(|image| {
                            let (w, h) = (image.width() as f32, image.height() as f32);
                            Ok(models
                                .find_faces(&image, threads())?
                                .into_iter()
                                .map(|(face, vector)| crate::smart::FoundFace {
                                    x: (face.x / w).clamp(0.0, 1.0),
                                    y: (face.y / h).clamp(0.0, 1.0),
                                    w: (face.width / w).clamp(0.0, 1.0),
                                    h: (face.height / h).clamp(0.0, 1.0),
                                    score: face.score,
                                    vector,
                                })
                                .collect::<Vec<_>>())
                        })
                        .unwrap_or_default();
                    let _ = crate::smart::store_faces(&conn, &item, &found, &mut people);
                    scanned += 1;
                    self.pace(started);
                }
            }
        }
        let _ = (ffmpeg, described, scanned);
        self.set_analysis(None, 0, 0);
    }
}

fn open(db_path: &Path) -> Result<Connection, String> {
    crate::db::connect(db_path).map_err(|err| format!("Could not open the database: {err}"))
}

pub fn count(conn: &Connection) -> u64 {
    conn.query_row("SELECT count(*) FROM media_index", [], |row| row.get::<_, i64>(0)).unwrap_or(0) as u64
}

/// Folders the timeline leaves out: the Trash, hidden folders, and Apple Photos' own library
/// (its originals sit next to thumbnails and caches that would show up as duplicates).
fn skip_folder(rel: &str, name: &str) -> bool {
    name.starts_with('.') || files::is_under_trash(rel) || name.to_ascii_lowercase().ends_with(".photoslibrary")
}

struct Found {
    rel: String,
    full: PathBuf,
    kind: &'static str,
    size: i64,
    mtime: i64,
}

/// Brings the index in line with the library. `progress(seen, read)` is called as it goes.
#[cfg(test)]
pub fn scan(conn: &Connection, root: &Path, progress: &dyn Fn(u64, u64)) -> Result<(), String> {
    scan_paced(conn, root, progress, &|_| {})
}

/// `scan`, calling `pace` after reading each photo so it can rest.
fn scan_paced(conn: &Connection, root: &Path, progress: &dyn Fn(u64, u64), pace: &dyn Fn(std::time::Instant)) -> Result<(), String> {
    let known: HashMap<String, (i64, i64)> = {
        let mut stmt = conn.prepare("SELECT path, size, mtime FROM media_index").map_err(|err| err.to_string())?;
        let rows = stmt
            .query_map([], |row| Ok((row.get::<_, String>(0)?, (row.get(1)?, row.get(2)?))))
            .map_err(|err| err.to_string())?;
        rows.filter_map(Result::ok).collect()
    };
    let mut seen = HashSet::new();
    let mut live = Vec::new();
    let mut pending = Vec::new();
    let mut read = 0u64;
    let mut stack = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, rel_dir)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let rel = if rel_dir.is_empty() { name.clone() } else { format!("{rel_dir}/{name}") };
            // Links are not followed: they could lead outside the library or loop.
            let Ok(meta) = entry.path().symlink_metadata() else { continue };
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                if !skip_folder(&rel, &name) {
                    stack.push((entry.path(), rel));
                }
                continue;
            }
            let kind = files::kind_of(&name);
            if !meta.is_file() || (kind != "image" && kind != "video") || name.starts_with('.') {
                continue;
            }
            let extension = name.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase()).unwrap_or_default();
            // A Live Photo's video belongs to its photo, and plays from there.
            if kind == "video" && extension == "mov" {
                let stem = name.rsplit_once('.').map_or(name.as_str(), |(stem, _)| stem);
                let photo = ["HEIC", "heic", "JPG", "jpg", "JPEG", "jpeg"].iter().map(|ext| format!("{stem}.{ext}")).find(|photo| dir.join(photo).is_file());
                if let Some(photo) = photo {
                    live.push(if rel_dir.is_empty() { photo } else { format!("{rel_dir}/{photo}") });
                    continue;
                }
            }
            // Design files, icons, and textures are pictures, but not photos for the timeline.
            if crate::photo_formats::is_layered(&extension) || crate::photo_formats::is_other_raster(&extension) {
                continue;
            }
            let size = meta.len() as i64;
            let mtime = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as i64);
            seen.insert(rel.clone());
            if known.get(&rel) != Some(&(size, mtime)) {
                pending.push(Found { rel, full: entry.path(), kind, size, mtime });
                if pending.len() >= BATCH {
                    read += pending.len() as u64;
                    store(conn, std::mem::take(&mut pending), pace)?;
                }
            }
            if seen.len() % 500 == 0 {
                progress(seen.len() as u64, read + pending.len() as u64);
            }
        }
    }
    read += pending.len() as u64;
    store(conn, pending, pace)?;
    // Forget what is gone (moves made by OwnNAS have already updated their rows).
    let gone: Vec<&String> = known.keys().filter(|path| !seen.contains(*path) && !files::is_under_trash(path)).collect();
    for chunk in gone.chunks(BATCH) {
        let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate).map_err(|err| err.to_string())?;
        for path in chunk {
            tx.execute("DELETE FROM media_index WHERE path = ?1", [path]).map_err(|err| err.to_string())?;
        }
        tx.commit().map_err(|err| err.to_string())?;
    }
    crate::smart::forget_unindexed(conn)?;
    note_moving(conn, root, &live, pace)?;
    progress(seen.len() as u64, read);
    Ok(())
}

/// Marks the photos that move: those not looked at yet (from before this was kept), and those
/// whose Live Photo video arrived after them.
fn note_moving(conn: &Connection, root: &Path, live: &[String], pace: &dyn Fn(std::time::Instant)) -> Result<(), String> {
    let sql = |err: rusqlite::Error| err.to_string();
    for part in live.chunks(BATCH) {
        let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate).map_err(sql)?;
        for path in part {
            tx.execute("UPDATE media_index SET motion = 1 WHERE path = ?1 AND motion IS NOT 1", [path]).map_err(sql)?;
        }
        tx.commit().map_err(sql)?;
        crate::db_watch::heavy_pause_point();
    }
    loop {
        let waiting: Vec<String> = {
            let mut stmt = conn.prepare("SELECT path FROM media_index WHERE kind = 'image' AND motion IS NULL LIMIT ?1").map_err(sql)?;
            let rows = stmt.query_map([BATCH as i64], |row| row.get(0)).map_err(sql)?;
            rows.filter_map(Result::ok).collect()
        };
        if waiting.is_empty() {
            return Ok(());
        }
        // Read before the transaction, so the database is locked only to write.
        let looked: Vec<(String, bool)> = waiting
            .into_iter()
            .map(|path| {
                let started = std::time::Instant::now();
                let moving = crate::photo_formats::moves(&root.join(&path));
                pace(started);
                (path, moving)
            })
            .collect();
        let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate).map_err(sql)?;
        for (path, moving) in looked {
            tx.execute("UPDATE media_index SET motion = ?2 WHERE path = ?1", rusqlite::params![path, moving]).map_err(sql)?;
        }
        tx.commit().map_err(sql)?;
    }
}

fn store(conn: &Connection, found: Vec<Found>, pace: &dyn Fn(std::time::Instant)) -> Result<(), String> {
    if found.is_empty() {
        return Ok(());
    }
    // Reading dates happens before the transaction, so the database is locked only to write.
    let rows: Vec<(Found, Taken, bool)> = found.into_iter().map(|item| {
        let started = std::time::Instant::now();
        let taken = if item.kind == "image" { read_photo(&item.full) } else { read_video(&item.full) };
        let moving = item.kind == "image" && crate::photo_formats::moves(&item.full);
        pace(started);
        (item, taken, moving)
    }).collect();
    let now = crate::db::now_secs();
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate).map_err(|err| err.to_string())?;
    for (item, taken, moving) in rows {
        // A date set by hand wins over the file's own.
        let (taken_at, source) = match crate::photo_dates::manual(&tx, &item.rel) {
            Some(at) => (at, crate::photo_dates::SOURCE),
            None => dated(&taken, &item.rel, item.mtime),
        };
        let (lat, lon) = taken.place.map_or((None, None), |(lat, lon)| (Some(lat), Some(lon)));
        tx.execute(
            "INSERT OR REPLACE INTO media_index(path, kind, size, mtime, taken_at, date_source, width, height, indexed_at, lat, lon, motion)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![item.rel, item.kind, item.size, item.mtime, taken_at, source, taken.width, taken.height, now, lat, lon, moving],
        )
        .map_err(|err| err.to_string())?;
    }
    tx.commit().map_err(|err| err.to_string())
}

/// The date a photo or video goes under, with where it came from: the camera's ("exif", or
/// "utc" for a video's real instant), the file name's ("name"), or the file's modified time
/// ("file"). The first two and dates set by hand are wall clocks with no time zone, kept as UTC.
fn dated(taken: &Taken, rel: &str, mtime: i64) -> (i64, &'static str) {
    if let Some(at) = taken.at {
        return (at, taken.source.unwrap_or("exif"));
    }
    let name = rel.rsplit('/').next().unwrap_or(rel);
    let local = |secs: i64| chrono::DateTime::from_timestamp(secs, 0).map(|utc| utc.with_timezone(&chrono::Local).naive_local());
    let today = local(crate::db::now_secs()).map_or_else(|| chrono::Utc::now().date_naive(), |now| now.date());
    match date_in_name(name, today) {
        // A name with only a day: the time the file was saved, when that is the same day.
        Some(found) if !found.has_time => {
            let saved = local(mtime).filter(|saved| saved.date() == found.day()).map(|saved| saved.and_utc().timestamp());
            (saved.unwrap_or(found.at), "name")
        }
        Some(found) => (found.at, "name"),
        None => (mtime, "file"),
    }
}

/// The date a photo or video would have without one set by hand, read from the file now.
pub fn automatic_date(full: &Path, rel: &str) -> Option<(i64, &'static str)> {
    let meta = std::fs::metadata(full).ok()?;
    let mtime = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as i64);
    let taken = match files::kind_of(rel) {
        "image" => read_photo(full),
        "video" => read_video(full),
        _ => Taken::default(),
    };
    Some(dated(&taken, rel, mtime))
}

/// A date in a file's name: the day, and the time when the name has one too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NameDate {
    /// Seconds since 1970 of that wall clock, treated as UTC (like a camera's date).
    pub at: i64,
    pub has_time: bool,
}

impl NameDate {
    fn day(self) -> chrono::NaiveDate {
        chrono::DateTime::from_timestamp(self.at, 0).map_or_else(chrono::NaiveDate::default, |at| at.date_naive())
    }
}

/// The date that phones, cameras, and apps write into file names: `IMG-20210201-WA0003`,
/// `IMG_20210201_134502`, `PXL_20210201_134502123`, `Screenshot_2021-02-01-13-45-02`,
/// `WhatsApp Image 2021-02-01 at 13.45.02`, `20210201134502`, or just `2021-02-01`. Strict on
/// purpose: the date must stand on its own (a whole run of 8, 14, or 17 digits, or
/// year-month-day with one kind of separator), fall between 1990 and `today`, and be a real day;
/// longer or shorter runs of digits (counters, phone numbers, timestamps) are not dates.
pub fn date_in_name(name: &str, today: chrono::NaiveDate) -> Option<NameDate> {
    let stem = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && ext.len() <= 5 && ext.chars().all(|c| c.is_ascii_alphanumeric()) => stem,
        _ => name,
    };
    // Runs of digits and of everything else, in turn.
    let mut runs: Vec<&str> = Vec::new();
    let mut start = 0;
    for (at, c) in stem.char_indices().skip(1) {
        let before = stem[..at].chars().next_back().is_some_and(|b| b.is_ascii_digit());
        if before != c.is_ascii_digit() {
            runs.push(&stem[start..at]);
            start = at;
        }
    }
    if !stem.is_empty() {
        runs.push(&stem[start..]);
    }
    fn digits<'a>(run: Option<&&'a str>) -> Option<&'a str> {
        run.filter(|run| run.bytes().all(|b| b.is_ascii_digit())).copied()
    }
    let number = |text: &str| text.parse::<u32>().ok();
    // Letters right against the digits are fine ("IMG20210201"), unless more digits come before
    // them in the same word ("a3f20210201…", a hash).
    let stands_alone = |before: Option<&&str>, earlier: Option<&&str>| match before {
        None => true,
        Some(text) => !text.chars().all(|c| c.is_ascii_alphabetic()) || earlier.is_none(),
    };
    for index in 0..runs.len() {
        let Some(run) = digits(runs.get(index)) else { continue };
        let before = index.checked_sub(1).and_then(|i| runs.get(i));
        let earlier = index.checked_sub(2).and_then(|i| runs.get(i));
        if !stands_alone(before, earlier) {
            continue;
        }
        let (date, mut next) = match run.len() {
            8 | 14 | 17 => ((number(&run[..4])?, number(&run[4..6])?, number(&run[6..8])?), index + 1),
            4 => {
                let sep = runs.get(index + 1).copied().filter(|sep| matches!(*sep, "-" | "_" | "."));
                match (sep, digits(runs.get(index + 2)), runs.get(index + 3), digits(runs.get(index + 4))) {
                    (Some(sep), Some(month), Some(&again), Some(day)) if again == sep && month.len() == 2 && day.len() == 2 => {
                        ((number(run)?, number(month)?, number(day)?), index + 5)
                    }
                    _ => continue,
                }
            }
            _ => continue,
        };
        let (year, month, day) = date;
        let Some(on) = chrono::NaiveDate::from_ymd_opt(year as i32, month, day) else { continue };
        if year < 1990 || on > today.succ_opt().unwrap_or(today) {
            continue;
        }
        // The time: in the same run (YYYYMMDDhhmmss[mmm]), or after a separator as hhmmss[mmm]
        // or hh-mm-ss (hh.mm.ss, hh_mm_ss), with an optional AM or PM.
        let mut time = None;
        if run.len() >= 14 {
            time = Some((number(&run[8..10])?, number(&run[10..12])?, number(&run[12..14])?));
        } else if runs.get(next).is_some_and(|sep| matches!(sep.trim(), "" | "-" | "_" | "." | "T" | "at" | "_at_" | "-at-")) && runs.get(next).is_some_and(|sep| sep.len() <= 4) {
            let first = digits(runs.get(next + 1));
            match first {
                Some(clock) if clock.len() == 6 || clock.len() == 9 => {
                    time = Some((number(&clock[..2])?, number(&clock[2..4])?, number(&clock[4..6])?));
                    next += 2;
                }
                Some(hour) if hour.len() <= 2 => {
                    let sep = runs.get(next + 2).copied().filter(|sep| matches!(*sep, "-" | "_" | "." | ":"));
                    if let (Some(sep), Some(minute), Some(&again), Some(second)) = (sep, digits(runs.get(next + 3)), runs.get(next + 4), digits(runs.get(next + 5))) {
                        if again == sep && minute.len() == 2 && second.len() == 2 {
                            time = Some((number(hour)?, number(minute)?, number(second)?));
                            next += 6;
                        }
                    }
                }
                _ => {}
            }
            // "1.45.02 PM" (with a space or a narrow one before it).
            if let (Some((hour, minute, second)), Some(rest)) = (time, runs.get(next)) {
                let rest = rest.trim_start_matches([' ', '\u{202f}', '\u{a0}']).to_ascii_uppercase();
                if (1..=12).contains(&hour) && (rest.starts_with("AM") || rest.starts_with("PM")) {
                    let hour = match (hour, rest.starts_with("PM")) {
                        (12, false) => 0,
                        (12, true) => 12,
                        (hour, true) => hour + 12,
                        (hour, false) => hour,
                    };
                    time = Some((hour, minute, second));
                }
            }
        }
        // Nor may letters and more digits follow straight on ("20210201ab3f…").
        let after = runs.get(next).is_some_and(|text| text.chars().all(|c| c.is_ascii_alphabetic())) && runs.get(next + 1).is_some();
        if after && time.is_none() {
            continue;
        }
        let time = time.filter(|(hour, minute, second)| *hour <= 23 && *minute <= 59 && *second <= 59);
        let (hour, minute, second) = time.unwrap_or((0, 0, 0));
        let at = on.and_hms_opt(hour, minute, second)?.and_utc().timestamp();
        return Some(NameDate { at, has_time: time.is_some() });
    }
    None
}

#[derive(Default)]
struct Taken {
    /// When the photo was taken, as written by the camera: a local time with no time zone,
    /// stored as if it were UTC so it shows the same wherever it is viewed.
    at: Option<i64>,
    /// What `at` is: "exif" (a camera's local time, the default) or "utc" (a real instant).
    source: Option<&'static str>,
    width: Option<i64>,
    height: Option<i64>,
    /// Where it was taken (latitude, longitude).
    place: Option<(f64, f64)>,
}

/// When and where a video was filmed, as the file says (MP4 and QuickTime).
fn read_video(path: &Path) -> Taken {
    let filmed = crate::video_meta::read(path);
    let (at, source) = match (filmed.local, filmed.utc) {
        (Some(local), _) => (Some(local), Some("exif")),
        (None, Some(utc)) => (Some(utc), Some("utc")),
        (None, None) => (None, None),
    };
    Taken { at, source, place: filmed.place, ..Taken::default() }
}

/// The date taken and the size the photo shows at (after the camera's rotation).
fn read_photo(path: &Path) -> Taken {
    let mut taken = Taken::default();
    let mut turned = false;
    if let Ok(file) = std::fs::File::open(path) {
        let mut reader = std::io::BufReader::new(file);
        if let Ok(exif) = exif::Reader::new().read_from_container(&mut reader) {
            for tag in [exif::Tag::DateTimeOriginal, exif::Tag::DateTimeDigitized, exif::Tag::DateTime] {
                if let Some(field) = exif.get_field(tag, exif::In::PRIMARY) {
                    if let exif::Value::Ascii(ref parts) = field.value {
                        taken.at = parts.first().and_then(|bytes| parse_exif_date(bytes));
                    }
                }
                if taken.at.is_some() {
                    break;
                }
            }
            let number = |tag| exif.get_field(tag, exif::In::PRIMARY).and_then(|field| field.value.get_uint(0)).map(i64::from);
            taken.place = crate::photo_details::gps_position(&exif);
            taken.width = number(exif::Tag::PixelXDimension);
            taken.height = number(exif::Tag::PixelYDimension);
            // Orientations 5 to 8 turn the picture a quarter: it shows the other way round.
            turned = matches!(number(exif::Tag::Orientation), Some(5..=8));
        }
    }
    // The picture's own size first: the EXIF size is the camera's, and a resized copy keeps it.
    if let Ok((width, height)) = image::image_dimensions(path) {
        taken.width = Some(width.into());
        taken.height = Some(height.into());
    }
    if turned {
        std::mem::swap(&mut taken.width, &mut taken.height);
    }
    // RAW and JPEG XL: the size of the picture inside, already upright.
    let extension = path.extension().map(|ext| ext.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if crate::photo_formats::handles(&extension) {
        if let Some((width, height)) = crate::photo_formats::shown_size(path) {
            taken.width = Some(width.into());
            taken.height = Some(height.into());
        }
    }
    taken
}

/// "2024:07:15 18:30:02" as seconds since 1970, treating the time as UTC.
fn parse_exif_date(bytes: &[u8]) -> Option<i64> {
    let text = std::str::from_utf8(bytes).ok()?.trim();
    let number = |range: std::ops::Range<usize>| text.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13).unwrap_or(0), number(14..16).unwrap_or(0), number(17..19).unwrap_or(0));
    if year < 1900 || !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second)
}

/// Days since 1970-01-01 (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn rows(conn: &Connection) -> Vec<(String, i64, String)> {
        let mut stmt = conn.prepare("SELECT path, taken_at, date_source FROM media_index ORDER BY path").unwrap();
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap().map(Result::unwrap).collect()
    }

    /// A tiny JPEG with an EXIF DateTimeOriginal of 2021:06:05 14:30:00.
    fn jpeg_with_date() -> Vec<u8> {
        let mut buffer = Vec::new();
        image::RgbImage::new(4, 3).write_to(&mut std::io::Cursor::new(&mut buffer), image::ImageFormat::Jpeg).unwrap();
        // Build an APP1 Exif segment by hand: TIFF header, IFD0 with an ExifIFD pointer,
        // and an Exif IFD holding DateTimeOriginal.
        let date = b"2021:06:05 14:30:00\0";
        let mut tiff = Vec::new();
        tiff.extend_from_slice(b"II*\0");
        tiff.extend_from_slice(&8u32.to_le_bytes());
        // IFD0: one entry (ExifIFDPointer 0x8769), next IFD 0.
        tiff.extend_from_slice(&1u16.to_le_bytes());
        tiff.extend_from_slice(&0x8769u16.to_le_bytes());
        tiff.extend_from_slice(&4u16.to_le_bytes());
        tiff.extend_from_slice(&1u32.to_le_bytes());
        tiff.extend_from_slice(&26u32.to_le_bytes());
        tiff.extend_from_slice(&0u32.to_le_bytes());
        // Exif IFD at 26: DateTimeOriginal (0x9003), ASCII, 20 bytes at offset 44.
        tiff.extend_from_slice(&1u16.to_le_bytes());
        tiff.extend_from_slice(&0x9003u16.to_le_bytes());
        tiff.extend_from_slice(&2u16.to_le_bytes());
        tiff.extend_from_slice(&(date.len() as u32).to_le_bytes());
        tiff.extend_from_slice(&44u32.to_le_bytes());
        tiff.extend_from_slice(&0u32.to_le_bytes());
        tiff.extend_from_slice(date);
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend_from_slice(&tiff);
        let mut out = vec![0xFF, 0xD8, 0xFF, 0xE1];
        out.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&app1);
        out.extend_from_slice(&buffer[2..]);
        out
    }

    #[test]
    fn scans_photos_and_skips_what_does_not_belong() {
        let root = Scratch(std::env::temp_dir().join(format!("ownnas-media-{}", rand::random::<u64>())));
        let lib = root.0.join("library");
        for dir in ["trip", ".hidden", ".ownnas-trash/1", "Photos Library.photoslibrary/originals", "docs"] {
            std::fs::create_dir_all(lib.join(dir)).unwrap();
        }
        std::fs::write(lib.join("trip/beach.jpg"), jpeg_with_date()).unwrap();
        std::fs::write(lib.join("trip/clip.mp4"), b"not really a video").unwrap();
        std::fs::write(lib.join("docs/notes.txt"), b"x").unwrap();
        std::fs::write(lib.join(".hidden/secret.jpg"), jpeg_with_date()).unwrap();
        std::fs::write(lib.join(".ownnas-trash/1/old.jpg"), jpeg_with_date()).unwrap();
        std::fs::write(lib.join("Photos Library.photoslibrary/originals/p.jpg"), jpeg_with_date()).unwrap();
        let before = std::fs::read(lib.join("trip/beach.jpg")).unwrap();

        let conn = crate::db::open(&root.0.join("data/ownnas.db")).unwrap();
        scan(&conn, &lib, &|_, _| {}).unwrap();
        let found = rows(&conn);
        assert_eq!(found.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(), ["trip/beach.jpg", "trip/clip.mp4"]);
        assert_eq!(found[0].1, 1_622_903_400, "2021-06-05 14:30:00");
        assert_eq!(found[0].2, "exif");
        assert_eq!(found[1].2, "file");
        let size: (Option<i64>, Option<i64>) = conn
            .query_row("SELECT width, height FROM media_index WHERE path = 'trip/beach.jpg'", [], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap();
        assert_eq!(size, (Some(4), Some(3)));
        assert_eq!(std::fs::read(lib.join("trip/beach.jpg")).unwrap(), before, "photos are only read");

        // Unchanged files are not read again; removed ones leave the index.
        let read = std::cell::Cell::new(0);
        scan(&conn, &lib, &|_, r| read.set(r)).unwrap();
        assert_eq!(read.get(), 0);
        // Its description and faces go with it.
        let clip = crate::smart::Pending { path: "trip/clip.mp4".into(), kind: "video".into(), size: 1, mtime: 1 };
        crate::smart::store_description(&conn, &clip, &[1.0, 0.0]).unwrap();
        conn.execute("INSERT INTO faces(path, x, y, w, h, score, vector, created_at) VALUES('trip/clip.mp4', 0, 0, 1, 1, 1, x'', 0)", []).unwrap();
        std::fs::remove_file(lib.join("trip/clip.mp4")).unwrap();
        scan(&conn, &lib, &|_, _| {}).unwrap();
        assert_eq!(rows(&conn).len(), 1);
        let left: i64 = conn.query_row("SELECT (SELECT count(*) FROM image_vectors) + (SELECT count(*) FROM faces)", [], |row| row.get(0)).unwrap();
        assert_eq!(left, 0);
    }

    #[test]
    fn speeds_rest_and_share_the_work() {
        let worked = Duration::from_millis(100);
        assert_eq!(Speed::Fast.rest(worked), Duration::ZERO);
        assert_eq!(Speed::Medium.rest(worked), Duration::from_millis(100));
        assert_eq!(Speed::Slow.rest(worked), Duration::from_millis(300));
        assert_eq!(Speed::Slow.rest(Duration::from_secs(60)), Duration::from_secs(5), "rests are capped");
        assert_eq!(Speed::Slow.workers(), 1);
        assert!(Speed::Medium.workers() <= Speed::Fast.workers());
        assert_eq!(Speed::parse("medium"), Some(Speed::Medium));
        assert_eq!(Speed::parse("turbo"), None);
        let indexer = Indexer::default();
        assert_eq!(indexer.speed(), Speed::Medium, "medium unless chosen otherwise");
        indexer.set_speed(Speed::Slow);
        assert_eq!(indexer.status().speed, Some(Speed::Slow));
    }

    #[test]
    fn dates_in_file_names() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 11, 21).unwrap();
        let at = |y, mo, d, h, mi, s| chrono::NaiveDate::from_ymd_opt(y, mo, d).unwrap().and_hms_opt(h, mi, s).unwrap().and_utc().timestamp();
        let day = |y, mo, d| Some(NameDate { at: at(y, mo, d, 0, 0, 0), has_time: false });
        let time = |y, mo, d, h, mi, s| Some(NameDate { at: at(y, mo, d, h, mi, s), has_time: true });
        let found = |name: &str| date_in_name(name, today);
        // Phones, cameras, and apps.
        assert_eq!(found("IMG-20210201-WA0003.jpg"), day(2021, 2, 1));
        assert_eq!(found("VID-20210201-WA0001.mp4"), day(2021, 2, 1));
        assert_eq!(found("IMG_20210201_134502.jpg"), time(2021, 2, 1, 13, 45, 2));
        assert_eq!(found("PXL_20210201_134502123.jpg"), time(2021, 2, 1, 13, 45, 2));
        assert_eq!(found("PXL_20210201_134502123.MP.jpg"), time(2021, 2, 1, 13, 45, 2));
        assert_eq!(found("Screenshot_2021-02-01-13-45-02.png"), time(2021, 2, 1, 13, 45, 2));
        assert_eq!(found("Screenshot_20210201-134502.png"), time(2021, 2, 1, 13, 45, 2));
        assert_eq!(found("WhatsApp Image 2021-02-01 at 13.45.02.jpeg"), time(2021, 2, 1, 13, 45, 2));
        assert_eq!(found("Screen Shot 2021-02-01 at 1.45.02 PM.png"), time(2021, 2, 1, 13, 45, 2));
        assert_eq!(found("Screenshot 2021-02-01 at 12.05.00\u{202f}AM.png"), time(2021, 2, 1, 0, 5, 0));
        assert_eq!(found("photo_2021-02-01_13-45-02.jpg"), time(2021, 2, 1, 13, 45, 2));
        assert_eq!(found("signal-2021-02-01-134502.jpg"), time(2021, 2, 1, 13, 45, 2));
        assert_eq!(found("20210201_134502.jpg"), time(2021, 2, 1, 13, 45, 2));
        assert_eq!(found("20210201134502.jpg"), time(2021, 2, 1, 13, 45, 2));
        assert_eq!(found("IMG20210201134502.jpg"), time(2021, 2, 1, 13, 45, 2));
        assert_eq!(found("VID_20210201_134502~2.mp4"), time(2021, 2, 1, 13, 45, 2));
        // Just a date in the name.
        assert_eq!(found("20210201.jpg"), day(2021, 2, 1));
        assert_eq!(found("Beach 2021-02-01.jpg"), day(2021, 2, 1));
        assert_eq!(found("trip_2021.02.01_3.jpg"), day(2021, 2, 1));
        assert_eq!(found("IMG_20210201_999999.jpg"), day(2021, 2, 1), "not a time: only the day");
        // Not dates: impossible days, too early or later than today, other runs of digits.
        assert_eq!(found("IMG-20210230-WA0001.jpg"), None);
        assert_eq!(found("IMG-20211301-WA0001.jpg"), None);
        assert_eq!(found("IMG-19891231-WA0001.jpg"), None);
        assert_eq!(found("IMG-20270101-WA0001.jpg"), None);
        assert_eq!(found("2021-02-01_99.jpg"), day(2021, 2, 1));
        assert_eq!(found("2021-02_01.jpg"), None, "mixed separators");
        assert_eq!(found("DSC01234.JPG"), None);
        assert_eq!(found("IMG_4021.HEIC"), None);
        assert_eq!(found("1612183502123.jpg"), None, "a timestamp in milliseconds");
        assert_eq!(found("FB_IMG_1612183502123.jpg"), None);
        assert_eq!(found("202102011.jpg"), None, "nine digits");
        assert_eq!(found("+3120210201.jpg"), None, "a phone number");
        assert_eq!(found("a3f20210201bc.jpg"), None, "inside a hash");
        assert_eq!(found("20210201ab3f.jpg"), None);
        assert_eq!(found("e3b0c442 98fc1c14.jpg"), None);
        assert_eq!(found("holiday.jpg"), None);
        assert_eq!(found(""), None);
        // A date the day after today still counts (time zones).
        assert_eq!(found("IMG_20261122_010000.jpg"), time(2026, 11, 22, 1, 0, 0));
    }

    #[test]
    fn exif_dates() {
        assert_eq!(parse_exif_date(b"1970:01:01 00:00:00"), Some(0));
        assert_eq!(parse_exif_date(b"2000:02:29 12:00:00"), Some(951_825_600));
        assert_eq!(parse_exif_date(b"0000:00:00 00:00:00"), None);
        assert_eq!(parse_exif_date(b"garbage"), None);
    }
}
