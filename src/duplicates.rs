//! Files with the same contents, found through the search index: files of the same size are
//! compared by a quick fingerprint (their first and last 64 KB), and those still alike by their
//! whole SHA-256, kept in `file_hashes` so an unchanged file is never read twice.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::UNIX_EPOCH;

use rusqlite::{params, Connection};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::files::{self, CancelFlag, FileError};

/// The most groups listed.
const MAX_GROUPS: usize = 2_000;
const EDGE: u64 = 64 * 1024;

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Copy {
    pub path: String,
    /// Seconds since 1970.
    pub modified: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    pub size: u64,
    pub sha256: String,
    pub copies: Vec<Copy>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub groups: Vec<Group>,
    /// Files looked at (those sharing a size with another).
    pub compared: u64,
    /// Room the extra copies take.
    pub wasted: u64,
    /// More groups than listed.
    pub truncated: bool,
}

/// Duplicates under `rel`, biggest waste first. The Trash, archives of replaced uploads, and
/// photo-library packages are left out.
pub fn find(conn: &Connection, root: &Path, rel: &str, cancel: &CancelFlag) -> Result<Report, FileError> {
    let root = root.canonicalize().map_err(|_| FileError::NotFound)?;
    let start = files::resolve(&root, rel)?;
    if !start.full.is_dir() {
        return Err(FileError::NotADirectory);
    }
    let mut stmt = conn
        .prepare(
            "SELECT path, size FROM file_index
             WHERE dir = 0 AND size > 0 AND (?1 = '' OR substr(path, 1, length(?1) + 1) = ?1 || '/')
               AND size IN (SELECT size FROM file_index WHERE dir = 0 AND size > 0
                            AND (?1 = '' OR substr(path, 1, length(?1) + 1) = ?1 || '/')
                            GROUP BY size HAVING count(*) > 1)",
        )
        .map_err(|_| FileError::Rejected("Could not read the search index"))?;
    let rows: Vec<(String, i64)> = stmt
        .query_map(params![start.rel], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))
        .map_err(|_| FileError::Rejected("Could not read the search index"))?
        .filter_map(Result::ok)
        .filter(|(path, _)| !skipped(path))
        .collect();
    let mut by_size: HashMap<u64, Vec<String>> = HashMap::new();
    for (path, size) in rows {
        by_size.entry(size as u64).or_default().push(path);
    }
    let compared = by_size.values().filter(|paths| paths.len() > 1).map(|paths| paths.len() as u64).sum();
    // Biggest first, so a cap leaves out the smallest.
    let mut sizes: Vec<u64> = by_size.iter().filter(|(_, paths)| paths.len() > 1).map(|(size, _)| *size).collect();
    sizes.sort_unstable_by(|a, b| b.cmp(a));
    let mut groups = Vec::new();
    let mut truncated = false;
    'sizes: for size in sizes {
        let paths = &by_size[&size];
        let mut by_quick: HashMap<[u8; 32], Vec<&String>> = HashMap::new();
        for path in paths {
            cancel.check()?;
            if let Some(quick) = quick_print(&root.join(path), size) {
                by_quick.entry(quick).or_default().push(path);
            }
        }
        for alike in by_quick.into_values().filter(|alike| alike.len() > 1) {
            let mut by_hash: HashMap<String, Vec<Copy>> = HashMap::new();
            for path in alike {
                cancel.check()?;
                let full = root.join(path);
                let Ok(meta) = std::fs::metadata(&full) else { continue };
                if let Some(hash) = sha256(conn, path, &full, &meta, cancel)? {
                    let modified = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as i64);
                    by_hash.entry(hash).or_default().push(Copy { path: path.clone(), modified });
                }
            }
            for (sha256, mut copies) in by_hash.into_iter().filter(|(_, copies)| copies.len() > 1) {
                if groups.len() >= MAX_GROUPS {
                    truncated = true;
                    break 'sizes;
                }
                copies.sort_by(|a, b| a.path.cmp(&b.path));
                groups.push(Group { size, sha256, copies });
            }
        }
    }
    groups.sort_by(|a, b| waste(b).cmp(&waste(a)).then(a.copies[0].path.cmp(&b.copies[0].path)));
    let wasted = groups.iter().map(waste).sum();
    Ok(Report { groups, compared, wasted, truncated })
}

fn waste(group: &Group) -> u64 {
    group.size * (group.copies.len() as u64 - 1)
}

/// Copies not worth listing: in the Trash, in an archive of replaced uploads, or inside a photo
/// library (whose originals sit next to its own previews).
fn skipped(path: &str) -> bool {
    files::is_under_trash(path) || path.split('/').any(|part| part == ".ownnas-archive") || crate::file_index::inside_package(path)
}

/// SHA-256 of the file's size and its first and last 64 KB: different files almost always differ.
fn quick_print(full: &Path, size: u64) -> Option<[u8; 32]> {
    let mut file = std::fs::File::open(full).ok()?;
    let mut hasher = Sha256::new();
    hasher.update(size.to_le_bytes());
    let mut buf = vec![0u8; EDGE.min(size) as usize];
    file.read_exact(&mut buf).ok()?;
    hasher.update(&buf);
    if size > EDGE {
        file.seek(SeekFrom::Start(size - EDGE.min(size))).ok()?;
        file.read_exact(&mut buf).ok()?;
        hasher.update(&buf);
    }
    Some(hasher.finalize().into())
}

/// The whole file's SHA-256, from the cache while its size and time are unchanged.
pub fn sha256(conn: &Connection, rel: &str, full: &Path, meta: &std::fs::Metadata, cancel: &CancelFlag) -> Result<Option<String>, FileError> {
    let mtime = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as i64);
    if let Some(hash) = crate::db::cached_hash(conn, rel, meta.len(), mtime) {
        return Ok(Some(hash));
    }
    let Ok(mut file) = std::fs::File::open(full) else { return Ok(None) };
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        cancel.check()?;
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => hasher.update(&buf[..n]),
            Err(_) => return Ok(None),
        }
    }
    let hash = hex::encode(hasher.finalize());
    let _ = crate::db::store_hash(conn, rel, meta.len(), mtime, &hash);
    Ok(Some(hash))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_copies_by_contents_not_names_or_sizes() {
        let dir = std::env::temp_dir().join(format!("ownnas-dupes-{}-{}", std::process::id(), rand::random::<u32>()));
        let lib = dir.join("lib");
        std::fs::create_dir_all(lib.join("a/deep")).unwrap();
        std::fs::create_dir_all(lib.join(".ownnas-trash")).unwrap();
        let big: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let mut other = big.clone();
        other[150_000] ^= 1; // Same size, same edges: only the whole hash tells them apart.
        std::fs::write(lib.join("one.bin"), &big).unwrap();
        std::fs::write(lib.join("a/deep/copy.bin"), &big).unwrap();
        std::fs::write(lib.join("a/almost.bin"), &other).unwrap();
        std::fs::write(lib.join(".ownnas-trash/old.bin"), &big).unwrap();
        std::fs::write(lib.join("x.txt"), "same").unwrap();
        std::fs::write(lib.join("y.txt"), "diff").unwrap();
        let conn = crate::db::open(&dir.join("ownnas.db")).unwrap();
        let root = lib.canonicalize().unwrap();
        for (path, size) in [("one.bin", 300_000), ("a/deep/copy.bin", 300_000), ("a/almost.bin", 300_000), (".ownnas-trash/old.bin", 300_000), ("x.txt", 4), ("y.txt", 4)] {
            conn.execute(
                "INSERT INTO file_index(path, parent, name_lower, dir, size, mtime, kind) VALUES (?1, '', ?1, 0, ?2, 0, 'file')",
                params![path, size],
            )
            .unwrap();
        }
        let report = find(&conn, &root, "", &CancelFlag::new()).unwrap();
        assert_eq!(report.groups.len(), 1);
        let paths: Vec<&str> = report.groups[0].copies.iter().map(|copy| copy.path.as_str()).collect();
        assert_eq!(paths, ["a/deep/copy.bin", "one.bin"], "not the near-copy, nor the one in the Trash");
        assert_eq!(report.wasted, 300_000);
        assert!(find(&conn, &root, "a", &CancelFlag::new()).unwrap().groups.is_empty(), "only under the folder");
        let _ = std::fs::remove_dir_all(dir);
    }
}
