//! How full the library's disk is, and how much of it the library (and its Trash) and OwnNAS's
//! own data (database, thumbnails, caches) take. The disk is asked directly; the folders are
//! measured in the background, since a large library takes a while to walk, and the last
//! measurement is kept so it shows at once after a restart.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

/// Where the last measurement is kept between restarts.
const SETTING: &str = "storage_measure";
/// A measurement older than this is redone when someone looks.
pub const STALE_SECS: i64 = 6 * 3600;

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Disk {
    pub total: u64,
    /// What OwnNAS can still write (the disk may keep some for the system).
    pub available: u64,
    pub used: u64,
}

/// The disk a path is on.
pub fn disk(path: &Path) -> Option<Disk> {
    let stats = fs4::statvfs(path).ok()?;
    let total = stats.total_space();
    let free = stats.free_space();
    Some(Disk { total, available: stats.available_space(), used: total.saturating_sub(free) })
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Measure {
    /// The whole library, Trash included.
    pub bytes: u64,
    pub files: u64,
    pub trash_bytes: u64,
    pub trash_files: u64,
    /// OwnNAS's own folder: the database, thumbnails, and caches.
    pub data_bytes: u64,
    pub measured_at: i64,
    pub took_ms: u64,
}

#[derive(Default)]
pub struct Storage {
    last: Mutex<Option<Measure>>,
    running: AtomicBool,
    /// Bytes counted so far by the measurement under way.
    seen: AtomicU64,
}

impl Storage {
    pub fn last(&self) -> Option<Measure> {
        self.last.lock().unwrap_or_else(|err| err.into_inner()).clone()
    }

    pub fn running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    pub fn seen(&self) -> u64 {
        self.seen.load(Ordering::Relaxed)
    }

    /// Takes the measurement kept from before a restart.
    pub fn load(&self, db_path: &Path) {
        let Ok(conn) = crate::db::open(db_path) else { return };
        let kept = crate::db::get_setting(&conn, SETTING).ok().flatten().and_then(|text| serde_json::from_str::<Measure>(&text).ok());
        *self.last.lock().unwrap_or_else(|err| err.into_inner()) = kept;
    }

    /// Whether the last measurement is missing or old.
    pub fn stale(&self) -> bool {
        self.last().is_none_or(|measure| crate::db::now_secs() - measure.measured_at > STALE_SECS)
    }

    /// Measures the library and OwnNAS's data in the background, unless that is under way.
    pub fn measure(self: &Arc<Self>, root: PathBuf, db_path: PathBuf) {
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        self.seen.store(0, Ordering::Relaxed);
        let storage = self.clone();
        std::thread::spawn(move || {
            let started = std::time::Instant::now();
            let mut measure = Measure::default();
            walk(&root, &root, &mut measure, &storage.seen);
            let data = db_path.parent().map(Path::to_path_buf).unwrap_or_default();
            // Counted once when OwnNAS keeps its data inside the library.
            if !data.as_os_str().is_empty() && !data.starts_with(&root) {
                let mut own = Measure::default();
                walk(&data, &data, &mut own, &AtomicU64::new(0));
                measure.data_bytes = own.bytes;
            }
            measure.measured_at = crate::db::now_secs();
            measure.took_ms = started.elapsed().as_millis() as u64;
            if let (Ok(conn), Ok(text)) = (crate::db::open(&db_path), serde_json::to_string(&measure)) {
                let _ = crate::db::set_setting(&conn, SETTING, Some(&text));
            }
            *storage.last.lock().unwrap_or_else(|err| err.into_inner()) = Some(measure);
            storage.running.store(false, Ordering::SeqCst);
        });
    }
}

/// Adds up the files under `dir` (links are not followed: they may lead anywhere, or loop).
fn walk(root: &Path, dir: &Path, measure: &mut Measure, seen: &AtomicU64) {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let Ok(meta) = entry.path().symlink_metadata() else { continue };
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                stack.push(entry.path());
                continue;
            }
            let size = meta.len();
            measure.bytes += size;
            measure.files += 1;
            seen.fetch_add(size, Ordering::Relaxed);
            let in_trash = entry.path().strip_prefix(root).ok().and_then(|rel| rel.components().next()).is_some_and(|first| first.as_os_str() == crate::files::TRASH_DIR);
            if in_trash {
                measure.trash_bytes += size;
                measure.trash_files += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_the_library_and_its_trash() {
        let root = std::env::temp_dir().join(format!("ownnas-storage-{}-{}", std::process::id(), rand::random::<u64>()));
        std::fs::create_dir_all(root.join("photos")).unwrap();
        std::fs::create_dir_all(root.join(crate::files::TRASH_DIR)).unwrap();
        std::fs::write(root.join("photos/a.jpg"), vec![0u8; 1000]).unwrap();
        std::fs::write(root.join("b.txt"), vec![0u8; 24]).unwrap();
        std::fs::write(root.join(crate::files::TRASH_DIR).join("old.jpg"), vec![0u8; 500]).unwrap();
        let mut measure = Measure::default();
        let seen = AtomicU64::new(0);
        walk(&root, &root, &mut measure, &seen);
        assert_eq!((measure.bytes, measure.files), (1524, 3));
        assert_eq!((measure.trash_bytes, measure.trash_files), (500, 1));
        assert_eq!(seen.load(Ordering::Relaxed), 1524);
        let disk = disk(&root).unwrap();
        assert!(disk.total > 0 && disk.used <= disk.total && disk.available <= disk.total);
        let _ = std::fs::remove_dir_all(root);
    }
}
