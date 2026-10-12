//! Local folder scanning and file operations.
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::model::{meta_dir, Node, Stamp, META_DIR};

/// Prefix of partial downloads; never synced.
const DOWNLOAD_PREFIX: &str = ".ownnas-dl-";

/// Names that are never synced in either direction: OS clutter, editor lock files, and
/// OwnNAS's own temporary files.
pub fn ignored(name: &str) -> bool {
    name == META_DIR
        || name.starts_with(DOWNLOAD_PREFIX)
        || name.starts_with(".ownnas-part-")
        || matches!(name, ".DS_Store" | "Thumbs.db" | "desktop.ini" | ".directory")
        || name.starts_with("~$")
        || (name.starts_with(".~lock.") && name.ends_with('#'))
}

/// True if any component of a relative path is ignored.
pub fn ignored_path(rel: &str) -> bool {
    rel.split('/').any(ignored)
}

pub fn full_path(root: &Path, rel: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    for part in rel.split('/') {
        path.push(part);
    }
    path
}

fn millis(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

pub fn node_of(meta: &fs::Metadata) -> Node {
    if meta.is_dir() {
        Node::dir()
    } else {
        Node::file(meta.len(), meta.modified().map(millis).unwrap_or(0))
    }
}

/// Current state of one local path, ignoring symlinks.
pub fn stat(root: &Path, rel: &str) -> Option<Node> {
    let meta = fs::symlink_metadata(full_path(root, rel)).ok()?;
    if meta.file_type().is_symlink() {
        return None;
    }
    Some(node_of(&meta))
}

/// Walks the synced folder. Symlinks and ignored names are skipped.
pub fn scan(root: &Path) -> Result<BTreeMap<String, Node>, String> {
    let mut out = BTreeMap::new();
    let mut stack = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, rel)) = stack.pop() {
        let items = fs::read_dir(&dir).map_err(|err| format!("Could not read {}: {err}", dir.display()))?;
        for item in items.flatten() {
            let Some(name) = item.file_name().to_str().map(str::to_string) else {
                eprintln!("skip: {} has a name that is not valid Unicode", item.path().display());
                continue;
            };
            if ignored(&name) {
                continue;
            }
            let Ok(meta) = fs::symlink_metadata(item.path()) else { continue };
            if meta.file_type().is_symlink() {
                continue;
            }
            let path = if rel.is_empty() { name } else { format!("{rel}/{name}") };
            if meta.is_dir() {
                stack.push((item.path(), path.clone()));
                out.insert(path, Node::dir());
            } else if meta.is_file() {
                out.insert(path, node_of(&meta));
            }
        }
    }
    Ok(out)
}

pub fn sha256(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Writes `reader` to a temporary file next to `rel`, then moves it into place, but only if
/// the local path still matches `expect` (so an edit made during the download is kept).
/// Returns the local stamp of the written file, or `Ok(None)` if the local file changed.
pub fn write_download(
    root: &Path,
    rel: &str,
    mut reader: impl Read,
    mtime: i64,
    expect: Option<Node>,
) -> io::Result<Option<Stamp>> {
    let dest = full_path(root, rel);
    let parent = dest.parent().ok_or_else(|| io::Error::other("invalid path"))?;
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!("{DOWNLOAD_PREFIX}{:016x}", rand::random::<u64>()));
    let result = (|| {
        let mut file = fs::File::create(&temp)?;
        io::copy(&mut reader, &mut file)?;
        file.flush()?;
        file.set_modified(UNIX_EPOCH + Duration::from_millis(mtime.max(0) as u64))?;
        file.sync_all()?;
        drop(file);
        if stat(root, rel) != expect {
            return Ok(None);
        }
        fs::rename(&temp, &dest)?;
        Ok(stat(root, rel).filter(|node| !node.dir).map(|node| node.stamp))
    })();
    if !matches!(result, Ok(Some(_))) {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Moves a local file or empty folder out of the synced tree into a dated safety folder
/// under `.ownnas-sync/deleted` instead of destroying it.
pub fn remove(root: &Path, rel: &str, session: &str) -> io::Result<()> {
    let full = full_path(root, rel);
    let meta = fs::symlink_metadata(&full)?;
    if meta.is_dir() {
        return fs::remove_dir(&full);
    }
    let dest = full_path(&meta_dir(root).join("deleted").join(session), rel);
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(&full, &dest)
}

/// Deletes safety copies older than `keep_days`.
pub fn prune_deleted(root: &Path, keep_days: u64) {
    let dir = meta_dir(root).join("deleted");
    let Ok(items) = fs::read_dir(&dir) else { return };
    let cutoff = SystemTime::now() - Duration::from_secs(keep_days * 86400);
    for item in items.flatten() {
        let old = item.metadata().and_then(|meta| meta.modified()).map(|time| time < cutoff);
        if old.unwrap_or(false) {
            let _ = fs::remove_dir_all(item.path());
        }
    }
}

/// Name for the local copy kept when both sides changed: `report (conflict host 2026-10-12 0930).pdf`.
pub fn conflict_name(rel: &str, host: &str, now: SystemTime) -> String {
    let (dir, name) = match rel.rsplit_once('/') {
        Some((dir, name)) => (Some(dir), name),
        None => (None, rel),
    };
    let (stem, ext) = match name.rfind('.') {
        Some(index) if index > 0 => (&name[..index], &name[index..]),
        _ => (name, ""),
    };
    let secs = now.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let (y, m, d) = civil_from_days(secs.div_euclid(86400));
    let tod = secs.rem_euclid(86400);
    let file = format!(
        "{stem} (conflict {host} {y:04}-{m:02}-{d:02} {:02}{:02}{:02}){ext}",
        tod / 3600,
        tod % 3600 / 60,
        tod % 60
    );
    match dir {
        Some(dir) => format!("{dir}/{file}"),
        None => file,
    }
}

/// Days since 1970-01-01 to a UTC calendar date (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflict_names_keep_extension_and_folder() {
        let at = UNIX_EPOCH + Duration::from_secs(1_760_261_400); // 2025-10-12 09:30:00 UTC
        assert_eq!(
            conflict_name("docs/report.final.pdf", "laptop", at),
            "docs/report.final (conflict laptop 2025-10-12 093000).pdf"
        );
        assert_eq!(conflict_name(".bashrc", "pc", at), ".bashrc (conflict pc 2025-10-12 093000)");
        assert_eq!(conflict_name("Makefile", "pc", at), "Makefile (conflict pc 2025-10-12 093000)");
    }

    #[test]
    fn ignores_clutter_and_internal_files() {
        assert!(ignored(".DS_Store"));
        assert!(ignored("~$report.docx"));
        assert!(ignored(".~lock.sheet.ods#"));
        assert!(ignored(".ownnas-sync"));
        assert!(!ignored(".gitignore"));
        assert!(ignored_path("a/.DS_Store"));
        assert!(!ignored_path("a/b.txt"));
    }
}
