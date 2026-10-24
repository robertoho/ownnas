//! The songs in a folder and its subfolders, for the music player: in folder order, each folder's
//! songs by name with numbers compared as numbers ("2 Song" before "10 Song"), as music players
//! list an album.

use std::cmp::Ordering;
use std::path::Path;

use serde::Serialize;

use crate::files::{self, FileError};

/// A folder with more songs than this plays the first ones.
pub const MAX_SONGS: usize = 20_000;

#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Song {
    pub path: String,
    pub name: String,
    pub size: u64,
}

/// Every song under `rel`, skipping hidden folders, the Trash, Apple Photos libraries, and
/// links (which could lead out of the library or round in circles).
pub fn songs_in(root: &Path, rel: &str) -> Result<(Vec<Song>, bool), FileError> {
    let resolved = files::resolve(root, rel)?;
    if !resolved.full.is_dir() {
        return Err(FileError::NotADirectory);
    }
    let mut songs = Vec::new();
    let mut truncated = false;
    walk(&resolved.full, &resolved.rel, &mut songs, &mut truncated);
    Ok((songs, truncated))
}

fn walk(dir: &Path, rel: &str, songs: &mut Vec<Song>, truncated: &mut bool) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    let mut folders = Vec::new();
    let mut here = Vec::new();
    for item in read.flatten() {
        let name = item.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let Ok(kind) = item.file_type() else { continue };
        let path = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
        if kind.is_dir() {
            if files::is_under_trash(&path) || name.ends_with(".photoslibrary") {
                continue;
            }
            folders.push((name, path));
        } else if kind.is_file() && files::kind_of(&name) == "audio" {
            let size = item.metadata().map(|m| m.len()).unwrap_or(0);
            here.push(Song { path, name, size });
        }
    }
    here.sort_by(|a, b| natural(&a.name, &b.name));
    for song in here {
        if songs.len() >= MAX_SONGS {
            *truncated = true;
            return;
        }
        songs.push(song);
    }
    folders.sort_by(|a, b| natural(&a.0, &b.0));
    for (name, path) in folders {
        if *truncated {
            return;
        }
        walk(&dir.join(&name), &path, songs, truncated);
    }
}

/// Compares names with runs of digits as numbers, ignoring case.
pub fn natural(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut digits = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        digits.push(c);
                        it.next();
                    }
                    digits
                };
                let (left, right) = (take(&mut a), take(&mut b));
                let (l, r) = (left.trim_start_matches('0'), right.trim_start_matches('0'));
                let order = l.len().cmp(&r.len()).then_with(|| l.cmp(r));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                let order = x.to_lowercase().cmp(y.to_lowercase());
                if order != Ordering::Equal {
                    return order;
                }
                a.next();
                b.next();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_sort_as_numbers() {
        let mut names = vec!["10 Ten.mp3", "2 Two.mp3", "1 One.mp3", "b.mp3", "A.mp3"];
        names.sort_by(|a, b| natural(a, b));
        assert_eq!(names, ["1 One.mp3", "2 Two.mp3", "10 Ten.mp3", "A.mp3", "b.mp3"]);
    }

    #[test]
    fn walks_folders_in_order_and_skips_what_is_not_music() {
        let root = std::env::temp_dir().join(format!("ownnas-music-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for path in ["Music/Album B/02 b.mp3", "Music/Album B/1 a.flac", "Music/Album A/song.ogg", "Music/top.m4a", "Music/cover.jpg", "Music/.hidden/x.mp3", ".ownnas-trash/old.mp3"] {
            let full = root.join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, b"x").unwrap();
        }
        let (songs, truncated) = songs_in(&root, "Music").unwrap();
        let paths: Vec<_> = songs.iter().map(|s| s.path.as_str()).collect();
        assert_eq!(paths, ["Music/top.m4a", "Music/Album A/song.ogg", "Music/Album B/1 a.flac", "Music/Album B/02 b.mp3"]);
        assert!(!truncated);
        let (all, _) = songs_in(&root, "").unwrap();
        assert_eq!(all.len(), 4, "the Trash and hidden folders are skipped");
        let _ = std::fs::remove_dir_all(&root);
    }
}
