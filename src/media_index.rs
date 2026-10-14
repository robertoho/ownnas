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
}

/// Runs scans in a background thread: at start, every half hour, and when asked to.
pub struct Indexer {
    status: Mutex<Status>,
    wake: Mutex<bool>,
    signal: Condvar,
}

impl Default for Indexer {
    fn default() -> Self {
        Self { status: Mutex::new(Status::default()), wake: Mutex::new(false), signal: Condvar::new() }
    }
}

impl Indexer {
    pub fn status(&self) -> Status {
        self.status.lock().unwrap_or_else(|err| err.into_inner()).clone()
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
        let result = open(db_path).and_then(|conn| scan(&conn, root, &|seen, read| {
            let mut status = self.status.lock().unwrap_or_else(|err| err.into_inner());
            status.seen = seen;
            status.read = read;
        }).map(|_| count(&conn)));
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
        // A few at a time: fast, without taking the whole machine.
        let workers = std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).clamp(1, 4));
        std::thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(|| loop {
                    let index = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some((path, kind)) = paths.get(index) else { break };
                    let name = path.rsplit('/').next().unwrap_or(path);
                    // A new scan was asked for: stop here, it will start this again.
                    if *self.wake.lock().unwrap_or_else(|err| err.into_inner()) {
                        break;
                    }
                    if files::can_thumb(name, ffmpeg) {
                        let _ = crate::thumbs::ensure(&root.join(path), thumbs, kind == "video");
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

fn open(db_path: &Path) -> Result<Connection, String> {
    let conn = Connection::open(db_path).map_err(|err| format!("Could not open the database: {err}"))?;
    conn.busy_timeout(Duration::from_secs(10)).map_err(|err| err.to_string())?;
    conn.pragma_update(None, "foreign_keys", "ON").map_err(|err| err.to_string())?;
    Ok(conn)
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
pub fn scan(conn: &Connection, root: &Path, progress: &dyn Fn(u64, u64)) -> Result<(), String> {
    let known: HashMap<String, (i64, i64)> = {
        let mut stmt = conn.prepare("SELECT path, size, mtime FROM media_index").map_err(|err| err.to_string())?;
        let rows = stmt
            .query_map([], |row| Ok((row.get::<_, String>(0)?, (row.get(1)?, row.get(2)?))))
            .map_err(|err| err.to_string())?;
        rows.filter_map(Result::ok).collect()
    };
    let mut seen = HashSet::new();
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
            let size = meta.len() as i64;
            let mtime = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as i64);
            seen.insert(rel.clone());
            if known.get(&rel) != Some(&(size, mtime)) {
                pending.push(Found { rel, full: entry.path(), kind, size, mtime });
                if pending.len() >= BATCH {
                    read += pending.len() as u64;
                    store(conn, std::mem::take(&mut pending))?;
                }
            }
            if seen.len() % 500 == 0 {
                progress(seen.len() as u64, read + pending.len() as u64);
            }
        }
    }
    read += pending.len() as u64;
    store(conn, pending)?;
    // Forget what is gone (moves made by OwnNAS have already updated their rows).
    let gone: Vec<&String> = known.keys().filter(|path| !seen.contains(*path) && !files::is_under_trash(path)).collect();
    for chunk in gone.chunks(BATCH) {
        let tx = conn.unchecked_transaction().map_err(|err| err.to_string())?;
        for path in chunk {
            tx.execute("DELETE FROM media_index WHERE path = ?1", [path]).map_err(|err| err.to_string())?;
        }
        tx.commit().map_err(|err| err.to_string())?;
    }
    progress(seen.len() as u64, read);
    Ok(())
}

fn store(conn: &Connection, found: Vec<Found>) -> Result<(), String> {
    if found.is_empty() {
        return Ok(());
    }
    // Reading dates happens before the transaction, so the database is locked only to write.
    let rows: Vec<(Found, Taken)> = found.into_iter().map(|item| {
        let taken = if item.kind == "image" { read_photo(&item.full) } else { Taken::default() };
        (item, taken)
    }).collect();
    let now = crate::db::now_secs();
    let tx = conn.unchecked_transaction().map_err(|err| err.to_string())?;
    for (item, taken) in rows {
        let (taken_at, source) = match taken.at {
            Some(at) => (at, "exif"),
            None => (item.mtime, "file"),
        };
        tx.execute(
            "INSERT OR REPLACE INTO media_index(path, kind, size, mtime, taken_at, date_source, width, height, indexed_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![item.rel, item.kind, item.size, item.mtime, taken_at, source, taken.width, taken.height, now],
        )
        .map_err(|err| err.to_string())?;
    }
    tx.commit().map_err(|err| err.to_string())
}

#[derive(Default)]
struct Taken {
    /// When the photo was taken, as written by the camera: a local time with no time zone,
    /// stored as if it were UTC so it shows the same wherever it is viewed.
    at: Option<i64>,
    width: Option<i64>,
    height: Option<i64>,
}

fn read_photo(path: &Path) -> Taken {
    let mut taken = Taken::default();
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
            taken.width = number(exif::Tag::PixelXDimension);
            taken.height = number(exif::Tag::PixelYDimension);
        }
    }
    if taken.width.is_none() {
        if let Ok((width, height)) = image::image_dimensions(path) {
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
        std::fs::remove_file(lib.join("trip/clip.mp4")).unwrap();
        scan(&conn, &lib, &|_, _| {}).unwrap();
        assert_eq!(rows(&conn).len(), 1);
    }

    #[test]
    fn exif_dates() {
        assert_eq!(parse_exif_date(b"1970:01:01 00:00:00"), Some(0));
        assert_eq!(parse_exif_date(b"2000:02:29 12:00:00"), Some(951_825_600));
        assert_eq!(parse_exif_date(b"0000:00:00 00:00:00"), None);
        assert_eq!(parse_exif_date(b"garbage"), None);
    }
}
