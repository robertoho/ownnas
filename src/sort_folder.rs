//! Sorting a folder's files into sub-folders by a pattern: by date (`{YYYY}/{MM}`,
//! `{YYYY}-{MM}`, `{YYYY}/{MM}/{DD}`, `{MMMM}` for the month's name), by kind (`{type}`), by
//! extension (`{ext}`) or by first letter (`{first}`), or any mix with other words
//! ("Photos {YYYY}"). A plan is made first, to be shown; carrying it out moves the files.
//!
//! Hidden files, folders, and the Trash are left alone; a file already where the pattern puts it
//! stays. A name already taken where a file goes gets a number, as moves always do.

use crate::files::{self, FileError};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

/// Files looked at, at most, in one go.
pub const MOST_FILES: usize = 50_000;
const TOKENS: [&str; 8] = ["{YYYY}", "{MM}", "{DD}", "{MMMM}", "{MMM}", "{type}", "{ext}", "{first}"];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DateFrom {
    /// The date a photo or video was taken, or else when the file was last changed.
    Taken,
    Modified,
    Created,
}

#[derive(Debug, Serialize, serde::Deserialize, Clone, PartialEq)]
pub struct Move {
    pub from: String,
    /// The folder it goes into, from the library's top.
    pub to: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub moves: Vec<Move>,
    /// The folders the files go into, with how many each.
    pub folders: BTreeMap<String, usize>,
    /// Files already where the pattern puts them.
    pub in_place: usize,
    pub looked_at: usize,
    pub truncated: bool,
}

/// Year, month, day, from seconds as a calendar date (already in the right time zone).
fn date_parts(seconds: i64) -> (i64, u32, u32) {
    let days = seconds.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

fn kind_folder(name: &str) -> &'static str {
    if crate::documents::has_text(name) {
        return "Documents";
    }
    match files::kind_of(name) {
        "image" | "svg" => "Pictures",
        "video" => "Videos",
        "audio" => "Music",
        "pdf" | "text" | "spreadsheet" => "Documents",
        "archive" => "Archives",
        _ => "Other",
    }
}

/// A name part made safe for a folder name.
fn clean(part: &str) -> String {
    let cleaned: String = part.chars().map(|c| if matches!(c, '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { '-' } else { c }).collect();
    cleaned.trim().trim_start_matches('.').trim().to_string()
}

pub fn check_pattern(pattern: &str) -> Result<(), String> {
    if !TOKENS.iter().any(|token| pattern.contains(token)) {
        return Err("The pattern needs at least one of {YYYY}, {MM}, {DD}, {MMMM}, {type}, {ext} or {first}".into());
    }
    if pattern.split('/').any(|part| part.trim() == ".." || part.trim() == ".") {
        return Err("The pattern cannot leave the folder".into());
    }
    if pattern.len() > 200 {
        return Err("That pattern is too long".into());
    }
    Ok(())
}

/// The folder (relative to the one being sorted) a file goes into. `when` is its date as a
/// calendar date in seconds; `months` the month names to use for {MMMM} (and their first three
/// letters for {MMM}).
pub fn target(pattern: &str, name: &str, when: i64, months: &[String]) -> Option<String> {
    let (year, month, day) = date_parts(when);
    let month_name = months.get(month as usize - 1).cloned().unwrap_or_else(|| format!("{month:02}"));
    let extension = name.rsplit_once('.').filter(|(stem, _)| !stem.is_empty()).map(|(_, ext)| ext.to_lowercase()).unwrap_or_else(|| "no extension".into());
    let first = name.chars().next().map(|c| if c.is_alphabetic() { c.to_uppercase().collect() } else { "#".to_string() }).unwrap_or_else(|| "#".into());
    let filled = pattern
        .replace("{YYYY}", &format!("{year:04}"))
        .replace("{MMMM}", &month_name)
        .replace("{MMM}", &month_name.chars().take(3).collect::<String>())
        .replace("{MM}", &format!("{month:02}"))
        .replace("{DD}", &format!("{day:02}"))
        .replace("{type}", kind_folder(name))
        .replace("{ext}", &extension)
        .replace("{first}", &first);
    let parts: Vec<String> = filled.split('/').map(clean).filter(|part| !part.is_empty()).collect();
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// The files to sort: in `folder` only, or also in the folders inside it; library paths with
/// their place on disk.
fn files_in(root: &Path, folder: &str, recursive: bool) -> Result<(Vec<(String, std::path::PathBuf)>, bool), FileError> {
    let start = files::resolve(root, folder)?;
    if !start.full.is_dir() {
        return Err(FileError::NotADirectory);
    }
    let mut found = Vec::new();
    let mut pending = vec![(start.rel.clone(), start.full.clone())];
    while let Some((rel, dir)) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let path = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            if files::entry_hidden(&entry.path(), &name) || files::is_under_trash(&path) || files::is_trash_folder(&entry.path()) {
                continue;
            }
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                if recursive {
                    pending.push((path, entry.path()));
                }
            } else if kind.is_file() {
                if found.len() == MOST_FILES {
                    return Ok((found, true));
                }
                found.push((path, entry.path()));
            }
        }
    }
    found.sort();
    Ok((found, false))
}

/// What sorting `folder` would do. `taken` gives a photo's or video's date taken, as a calendar
/// date in seconds, when the library knows it.
pub fn plan(root: &Path, folder: &str, pattern: &str, from: DateFrom, recursive: bool, months: &[String], taken: &HashMap<String, i64>) -> Result<Plan, FileError> {
    let (found, truncated) = files_in(root, folder, recursive)?;
    let base = files::resolve(root, folder)?.rel;
    let offset = chrono::Local::now().offset().local_minus_utc() as i64;
    let mut plan = Plan { moves: Vec::new(), folders: BTreeMap::new(), in_place: 0, looked_at: found.len(), truncated };
    for (path, full) in found {
        let name = path.rsplit('/').next().unwrap_or_default().to_string();
        let meta = std::fs::metadata(&full).ok();
        let seconds = |time: Option<std::io::Result<std::time::SystemTime>>| {
            time.and_then(Result::ok).and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok()).map(|time| time.as_secs() as i64 + offset)
        };
        let modified = seconds(meta.as_ref().map(std::fs::Metadata::modified));
        let when = match from {
            DateFrom::Taken => taken.get(&path).copied().or(modified),
            DateFrom::Modified => modified,
            DateFrom::Created => seconds(meta.as_ref().map(std::fs::Metadata::created)).or(modified),
        };
        let Some(inside) = target(pattern, &name, when.unwrap_or(0), months) else { continue };
        let to = if base.is_empty() { inside } else { format!("{base}/{inside}") };
        let parent = path.rsplit_once('/').map_or("", |(parent, _)| parent);
        if parent == to {
            plan.in_place += 1;
            continue;
        }
        *plan.folders.entry(to.clone()).or_default() += 1;
        plan.moves.push(Move { from: path, to });
    }
    Ok(plan)
}

/// Makes a folder and the ones above it, inside the library.
fn make_folders(root: &Path, rel: &str) -> Result<(), FileError> {
    let mut at = String::new();
    for part in rel.split('/').filter(|part| !part.is_empty()) {
        match files::make_dir(root, &at, part) {
            Ok(()) | Err(FileError::AlreadyExists) => {}
            Err(err) => return Err(err),
        }
        at = if at.is_empty() { part.to_string() } else { format!("{at}/{part}") };
    }
    Ok(())
}

fn why(err: FileError) -> String {
    match err {
        FileError::NotFound => "It is no longer there".into(),
        FileError::AlreadyExists => "Something with that name is in the way".into(),
        FileError::InvalidName => "That name cannot be used".into(),
        FileError::Forbidden => "Not allowed".into(),
        FileError::Rejected(why) | FileError::Io(why) => why.into(),
        other => format!("{other:?}"),
    }
}

/// Carries out the moves; returns the moves made (with where each file ended up) and the ones
/// that failed, with why.
pub fn apply(root: &Path, moves: &[Move]) -> (Vec<Move>, Vec<(String, String)>) {
    let mut made = Vec::new();
    let mut failed = Vec::new();
    for item in moves {
        let outcome = make_folders(root, &item.to).and_then(|()| files::move_entry(root, &item.from, &item.to));
        match outcome {
            Ok(path) => made.push(Move { from: item.from.clone(), to: path }),
            Err(err) => failed.push((item.from.clone(), why(err))),
        }
    }
    (made, failed)
}

/// Puts files back where they were (`to` is where each one is now); folders left empty go.
pub fn undo(root: &Path, moves: &[Move]) -> (usize, Vec<(String, String)>) {
    let mut back = 0;
    let mut failed = Vec::new();
    let mut emptied: Vec<String> = Vec::new();
    for item in moves.iter().rev() {
        match files::move_exact(root, &item.to, &item.from) {
            Ok(_) => {
                back += 1;
                if let Some((parent, _)) = item.to.rsplit_once('/') {
                    emptied.push(parent.to_string());
                }
            }
            Err(err) => failed.push((item.to.clone(), why(err))),
        }
    }
    // The deepest first, so a folder made of empty folders goes too.
    emptied.sort_by_key(|path| std::cmp::Reverse(path.matches('/').count()));
    emptied.dedup();
    for folder in emptied {
        let mut at = folder;
        loop {
            let Ok(found) = files::resolve(root, &at) else { break };
            if found.rel.is_empty() || std::fs::remove_dir(&found.full).is_err() {
                break;
            }
            match at.rsplit_once('/') {
                Some((parent, _)) => at = parent.to_string(),
                None => break,
            }
        }
    }
    (back, failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn months() -> Vec<String> {
        ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"].map(String::from).to_vec()
    }

    #[test]
    fn patterns_fill_in() {
        let march = 1_709_942_400; // 2024-03-09
        assert_eq!(target("{YYYY}/{MM}", "a.jpg", march, &months()).as_deref(), Some("2024/03"));
        assert_eq!(target("{YYYY}-{MM}", "a.jpg", march, &months()).as_deref(), Some("2024-03"));
        assert_eq!(target("{YYYY}/{MM}/{DD}", "a.jpg", march, &months()).as_deref(), Some("2024/03/09"));
        assert_eq!(target("{YYYY}/{MM} {MMMM}", "a.jpg", march, &months()).as_deref(), Some("2024/03 March"));
        assert_eq!(target("{MMM} {YYYY}", "a.jpg", march, &months()).as_deref(), Some("Mar 2024"));
        assert_eq!(target("{type}", "a.JPG", march, &months()).as_deref(), Some("Pictures"));
        assert_eq!(target("{type}", "report.pdf", march, &months()).as_deref(), Some("Documents"));
        assert_eq!(target("{type}", "letter.docx", march, &months()).as_deref(), Some("Documents"));
        assert_eq!(target("{type}", "song.mp3", march, &months()).as_deref(), Some("Music"));
        assert_eq!(target("{ext}", "a.JPG", march, &months()).as_deref(), Some("jpg"));
        assert_eq!(target("{ext}", "Makefile", march, &months()).as_deref(), Some("no extension"));
        assert_eq!(target("{first}", "zebra.txt", march, &months()).as_deref(), Some("Z"));
        assert_eq!(target("{first}", "2024.txt", march, &months()).as_deref(), Some("#"));
        assert_eq!(target("Photos {YYYY}/{MM}", "a.jpg", march, &months()).as_deref(), Some("Photos 2024/03"));
        assert_eq!(target("{YYYY}//:{MM}", "a.jpg", march, &months()).as_deref(), Some("2024/-03"));
    }

    #[test]
    fn patterns_are_checked() {
        assert!(check_pattern("{YYYY}/{MM}").is_ok());
        assert!(check_pattern("Photos").is_err(), "no token");
        assert!(check_pattern("../{YYYY}").is_err());
    }
}
