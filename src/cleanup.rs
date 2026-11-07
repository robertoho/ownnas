//! Tidying a folder: its largest files, the folders left empty, photos whose date is wrong while
//! their name says when they were taken, and what differs between it and another folder (a
//! backup, a copy). Only lists; changes go through the usual moves to the Trash, apart from
//! setting a file's date from its name.

use crate::files::{self, FileError};
use rusqlite::{params, Connection};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

/// Files looked at, at most, by a walk.
const MOST_FILES: usize = 100_000;

#[derive(Debug, Serialize)]
pub struct Sized {
    pub path: String,
    pub size: u64,
    pub modified: i64,
}

/// The largest files under `folder`, from the file index (biggest first).
pub fn large_files(conn: &Connection, folder: &str, limit: usize) -> Result<Vec<Sized>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT path, size, mtime FROM file_index WHERE dir = 0 AND size > 0
               AND (?1 = '' OR (path >= ?1 || '/' AND path < ?1 || '0')) AND path NOT LIKE '.%' AND path NOT LIKE '%/.%'
             ORDER BY size DESC LIMIT ?2",
        )
        .map_err(|_| "Could not read the file index".to_string())?;
    let rows = stmt
        .query_map(params![folder, limit as i64 * 2], |row| Ok(Sized { path: row.get(0)?, size: row.get::<_, i64>(1)? as u64, modified: row.get::<_, i64>(2)? / 1_000_000_000 }))
        .map_err(|_| "Could not read the file index".to_string())?;
    Ok(rows.filter_map(Result::ok).filter(|file| !files::is_under_trash(&file.path)).take(limit).collect())
}

/// Files that do not count when deciding if a folder is empty (left by systems, not people).
fn leftover(name: &str) -> bool {
    matches!(name, ".DS_Store" | "Thumbs.db" | "desktop.ini" | ".localized") || name.starts_with("._")
}

/// Folders under `folder` with no files in them (or only system leftovers), the outermost
/// ones only (an empty folder's empty folders go with it).
pub fn empty_folders(root: &Path, folder: &str) -> Result<Vec<String>, FileError> {
    let start = files::resolve(root, folder)?;
    if !start.full.is_dir() {
        return Err(FileError::NotADirectory);
    }
    let mut found = Vec::new();
    let mut seen = 0usize;
    // Whether a folder holds nothing; records the empty ones inside it that are not.
    fn walk(dir: &Path, rel: &str, found: &mut Vec<String>, seen: &mut usize) -> bool {
        let Ok(entries) = std::fs::read_dir(dir) else { return false };
        let mut empty = true;
        let mut empty_children = Vec::new();
        for entry in entries.flatten() {
            *seen += 1;
            if *seen > MOST_FILES {
                return false;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let Ok(kind) = entry.file_type() else { empty = false; continue };
            if kind.is_dir() {
                let path = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
                if name.starts_with('.') || files::is_under_trash(&path) || files::is_trash_folder(&entry.path()) {
                    empty = false;
                    continue;
                }
                if walk(&entry.path(), &path, found, seen) {
                    empty_children.push(path);
                } else {
                    empty = false;
                }
            } else if !leftover(&name) {
                empty = false;
            }
        }
        if !empty {
            found.extend(empty_children);
        }
        empty
    }
    if walk(&start.full, &start.rel, &mut found, &mut seen) && !start.rel.is_empty() {
        // The folder itself is empty: say so, rather than listing what is inside it.
        found = vec![start.rel.clone()];
    }
    found.sort();
    Ok(found)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DateFix {
    pub path: String,
    /// The file's date now, and the one its name gives (calendar seconds).
    pub now: i64,
    pub from_name: i64,
}

/// Photos and videos under `folder` without a date taken, whose name gives a date more than a
/// day away from the file's date: the file's date is then probably when it was copied.
pub fn date_fixes(conn: &Connection, root: &Path, folder: &str) -> Result<Vec<DateFix>, String> {
    let offset = chrono::Local::now().offset().local_minus_utc() as i64;
    let mut stmt = conn
        .prepare(
            "SELECT f.path, f.mtime FROM file_index f LEFT JOIN media_index m ON m.path = f.path
             WHERE f.dir = 0 AND f.kind IN ('image', 'video') AND (m.date_source IS NULL OR m.date_source <> 'exif')
               AND (?1 = '' OR (f.path >= ?1 || '/' AND f.path < ?1 || '0')) AND f.path NOT LIKE '.%' AND f.path NOT LIKE '%/.%'
             ORDER BY f.path LIMIT ?2",
        )
        .map_err(|_| "Could not read the file index".to_string())?;
    let rows = stmt.query_map(params![folder, MOST_FILES as i64], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))).map_err(|_| "Could not read the file index".to_string())?;
    let mut fixes = Vec::new();
    for (path, mtime) in rows.filter_map(Result::ok) {
        if files::is_under_trash(&path) || !root.join(&path).is_file() {
            continue;
        }
        let name = path.rsplit('/').next().unwrap_or_default();
        let Some(from_name) = crate::sort_folder::date_from_name(name) else { continue };
        let now = mtime / 1_000_000_000 + offset;
        if (now - from_name).abs() > 86_400 {
            fixes.push(DateFix { path, now, from_name });
        }
    }
    Ok(fixes)
}

/// Sets a file's date to the calendar seconds given (in this computer's time zone).
pub fn set_date(root: &Path, rel: &str, calendar_seconds: i64) -> Result<(), FileError> {
    let found = files::resolve(root, rel)?;
    if !found.full.is_file() {
        return Err(FileError::NotFound);
    }
    let offset = chrono::Local::now().offset().local_minus_utc() as i64;
    let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs((calendar_seconds - offset).max(0) as u64);
    std::fs::File::options().write(true).open(&found.full).and_then(|file| file.set_modified(when)).map_err(|_| FileError::Io("Could not change the file's date"))
}

#[derive(Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Comparison {
    /// Paths relative to the folders compared.
    pub only_in_first: Vec<String>,
    pub only_in_second: Vec<String>,
    /// In both, but with different sizes (or, when contents are compared, different contents).
    pub different: Vec<String>,
    pub same: usize,
    pub truncated: bool,
}

/// Every file under a folder, by its path inside it: (size, place on disk).
fn tree(dir: &Path) -> (BTreeMap<String, (u64, std::path::PathBuf)>, bool) {
    let mut found = BTreeMap::new();
    let mut pending = vec![(String::new(), dir.to_path_buf())];
    while let Some((rel, at)) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&at) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || leftover(&name) {
                continue;
            }
            let path = if rel.is_empty() { name } else { format!("{rel}/{name}") };
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                if !files::is_trash_folder(&entry.path()) {
                    pending.push((path, entry.path()));
                }
            } else if kind.is_file() {
                if found.len() == MOST_FILES {
                    return (found, true);
                }
                let size = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
                found.insert(path, (size, entry.path()));
            }
        }
    }
    (found, false)
}

fn same_contents(a: &Path, b: &Path) -> bool {
    use std::io::Read;
    let (Ok(mut first), Ok(mut second)) = (std::fs::File::open(a), std::fs::File::open(b)) else { return false };
    let (mut one, mut two) = (vec![0u8; 1 << 16], vec![0u8; 1 << 16]);
    loop {
        let (Ok(read_one), Ok(read_two)) = (first.read(&mut one), second.read(&mut two)) else { return false };
        if read_one != read_two || one[..read_one] != two[..read_two] {
            return false;
        }
        if read_one == 0 {
            return true;
        }
    }
}

/// What differs between two folders, by the paths of their files; `contents` also reads files
/// of the same size to compare them byte by byte.
pub fn compare(root: &Path, first: &str, second: &str, contents: bool, cancel: &files::CancelFlag) -> Result<Comparison, FileError> {
    let (a, b) = (files::resolve(root, first)?, files::resolve(root, second)?);
    if !a.full.is_dir() || !b.full.is_dir() {
        return Err(FileError::NotADirectory);
    }
    if a.full.starts_with(&b.full) || b.full.starts_with(&a.full) {
        return Err(FileError::Rejected("Choose two folders that are not inside each other"));
    }
    let ((left, cut_left), (right, cut_right)) = (tree(&a.full), tree(&b.full));
    let mut out = Comparison { truncated: cut_left || cut_right, ..Comparison::default() };
    for (path, (size, place)) in &left {
        if cancel.is_cancelled() {
            return Err(FileError::Cancelled);
        }
        match right.get(path) {
            None => out.only_in_first.push(path.clone()),
            Some((other_size, _)) if other_size != size => out.different.push(path.clone()),
            Some((_, other_place)) if contents && !same_contents(place, other_place) => out.different.push(path.clone()),
            Some(_) => out.same += 1,
        }
    }
    out.only_in_second = right.keys().filter(|path| !left.contains_key(*path)).cloned().collect();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_folders_and_comparisons() {
        let dir = std::env::temp_dir().join(format!("ownnas-cleanup-{}", rand::random::<u64>()));
        for folder in ["A/empty/inner", "A/only-ds", "A/full", "B/full"] {
            std::fs::create_dir_all(dir.join(folder)).unwrap();
        }
        std::fs::write(dir.join("A/only-ds/.DS_Store"), "x").unwrap();
        std::fs::write(dir.join("A/full/same.txt"), "same").unwrap();
        std::fs::write(dir.join("B/full/same.txt"), "same").unwrap();
        std::fs::write(dir.join("A/full/changed.txt"), "one").unwrap();
        std::fs::write(dir.join("B/full/changed.txt"), "two").unwrap();
        std::fs::write(dir.join("A/full/left.txt"), "l").unwrap();
        std::fs::write(dir.join("B/full/right.txt"), "r").unwrap();
        let root = dir.canonicalize().unwrap();
        assert_eq!(empty_folders(&root, "A").unwrap(), ["A/empty", "A/only-ds"]);
        let cancel = files::CancelFlag::new();
        let quick = compare(&root, "A/full", "B/full", false, &cancel).unwrap();
        assert_eq!((quick.only_in_first.as_slice(), quick.only_in_second.as_slice(), quick.different.len(), quick.same), (&["left.txt".to_string()][..], &["right.txt".to_string()][..], 0, 2), "same sizes look the same");
        let careful = compare(&root, "A/full", "B/full", true, &cancel).unwrap();
        assert_eq!((careful.different.as_slice(), careful.same), (&["changed.txt".to_string()][..], 1));
        assert!(compare(&root, "A", "A/full", false, &cancel).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
