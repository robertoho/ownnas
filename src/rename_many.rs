//! Renaming many files at once: by a pattern (`{YYYY}-{MM}-{DD} {hh}.{mm}.{ss}`, `Trip {n:3}`,
//! `{folder} {name}`), by finding and replacing words in the names, and by changing their case.
//! The extension stays. A plan is made first, to be shown; names that would clash get a number.
//! Carrying it out renames in two steps (first to temporary names), so files can swap names.

use crate::files::{self, FileError};
use crate::sort_folder::DateFrom;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;

pub const MOST_FILES: usize = 20_000;

#[derive(Debug, Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Rules {
    /// The new name without its extension; `{name}` is the old one. Empty keeps the old name.
    #[serde(default)]
    pub pattern: String,
    #[serde(default)]
    pub find: String,
    #[serde(default)]
    pub replace: String,
    /// "lower", "upper", "title", or empty to keep it.
    #[serde(default)]
    pub case: String,
    /// The first number for {n}.
    #[serde(default = "one")]
    pub start: i64,
}

fn one() -> i64 {
    1
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Rename {
    pub from: String,
    pub to: String,
}

fn split_name(name: &str) -> (&str, &str) {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && ext.len() <= 10 && !ext.contains(' ') => (stem, ext),
        _ => (name, ""),
    }
}

fn clean(name: &str) -> String {
    let cleaned: String = name.chars().map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { '-' } else { c }).collect();
    cleaned.trim().trim_end_matches('.').trim().to_string()
}

fn title_case(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut start = true;
    for c in text.chars() {
        if start {
            out.extend(c.to_uppercase());
        } else {
            out.extend(c.to_lowercase());
        }
        start = !c.is_alphanumeric() && c != '\'';
    }
    out
}

/// Replaces every `find` in `text`, letters matched in any case.
fn replace_any_case(text: &str, find: &str, replace: &str) -> String {
    if find.is_empty() {
        return text.to_string();
    }
    let lower_text = text.to_lowercase();
    let lower_find = find.to_lowercase();
    // Only when lowering keeps the byte positions (true for nearly every name).
    if lower_text.len() != text.len() {
        return text.replace(find, replace);
    }
    let mut out = String::new();
    let mut at = 0;
    while let Some(found) = lower_text[at..].find(&lower_find) {
        out.push_str(&text[at..at + found]);
        out.push_str(replace);
        at += found + find.len();
    }
    out.push_str(&text[at..]);
    out
}

/// A new name (with its extension) for one file. `index` counts from 0 in the order given;
/// `when` is its date as calendar seconds.
pub fn new_name(rules: &Rules, name: &str, folder: &str, index: usize, when: i64) -> String {
    let (stem, ext) = split_name(name);
    let mut base = if rules.pattern.trim().is_empty() {
        stem.to_string()
    } else {
        let (year, month, day) = date_parts(when);
        let rest = when.rem_euclid(86_400);
        let number = rules.start + index as i64;
        let mut text = rules
            .pattern
            .replace("{name}", stem)
            .replace("{folder}", folder)
            .replace("{YYYY}", &format!("{year:04}"))
            .replace("{MM}", &format!("{month:02}"))
            .replace("{DD}", &format!("{day:02}"))
            .replace("{hh}", &format!("{:02}", rest / 3600))
            .replace("{mm}", &format!("{:02}", rest / 60 % 60))
            .replace("{ss}", &format!("{:02}", rest % 60));
        // {n} and {n:3} (three digits at least).
        while let Some(start) = text.find("{n") {
            let Some(end) = text[start..].find('}').map(|end| start + end) else { break };
            let inner = &text[start + 2..end];
            let digits = inner.strip_prefix(':').and_then(|digits| digits.parse::<usize>().ok()).unwrap_or(if inner.is_empty() { 1 } else { 0 });
            if digits == 0 && !inner.is_empty() {
                break;
            }
            text.replace_range(start..=end, &format!("{number:0digits$}"));
        }
        text
    };
    base = replace_any_case(&base, &rules.find, &rules.replace);
    base = match rules.case.as_str() {
        "lower" => base.to_lowercase(),
        "upper" => base.to_uppercase(),
        "title" => title_case(&base),
        _ => base,
    };
    let base = clean(&base);
    let base = if base.is_empty() { stem.to_string() } else { base };
    if ext.is_empty() { base } else { format!("{base}.{ext}") }
}

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

/// What renaming `paths` would do: the renames (only names that change), in the order given.
/// Clashes, with each other or with files already there, get " 2", " 3"…
pub fn plan(root: &Path, paths: &[String], rules: &Rules, from: DateFrom, taken: &HashMap<String, i64>) -> Result<Vec<Rename>, FileError> {
    if paths.len() > MOST_FILES {
        return Err(FileError::Rejected("Choose fewer files"));
    }
    let offset = chrono::Local::now().offset().local_minus_utc() as i64;
    let moving: HashSet<&str> = paths.iter().map(String::as_str).collect();
    let mut taken_names: HashSet<String> = HashSet::new();
    let mut renames = Vec::new();
    for (index, path) in paths.iter().enumerate() {
        let found = files::resolve(root, path)?;
        if found.full.is_dir() {
            return Err(FileError::IsADirectory);
        }
        let (parent, name) = found.rel.rsplit_once('/').map_or(("", found.rel.as_str()), |(parent, name)| (parent, name));
        let folder = parent.rsplit('/').next().unwrap_or_default();
        let meta = std::fs::metadata(&found.full).map_err(|_| FileError::NotFound)?;
        let seconds = |time: std::io::Result<std::time::SystemTime>| time.ok().and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok()).map(|time| time.as_secs() as i64 + offset);
        let when = match from {
            DateFrom::Taken => taken.get(&found.rel).copied().or_else(|| seconds(meta.modified())),
            DateFrom::Name => crate::sort_folder::date_from_name(name).or_else(|| taken.get(&found.rel).copied()).or_else(|| seconds(meta.modified())),
            DateFrom::Modified => seconds(meta.modified()),
            DateFrom::Created => seconds(meta.created()).or_else(|| seconds(meta.modified())),
        }
        .unwrap_or(0);
        let wanted = new_name(rules, name, folder, index, when);
        // A free name: not taken by another rename, nor by a file that is not being renamed.
        let free = |candidate: &str, taken_names: &HashSet<String>| {
            let rel = if parent.is_empty() { candidate.to_string() } else { format!("{parent}/{candidate}") };
            let key = rel.to_lowercase();
            !taken_names.contains(&key) && (candidate == name || moving.contains(rel.as_str()) || !found.full.with_file_name(candidate).exists())
        };
        let mut chosen = wanted.clone();
        let (stem, ext) = split_name(&wanted);
        let mut n = 2;
        while !free(&chosen, &taken_names) {
            chosen = if ext.is_empty() { format!("{stem} {n}") } else { format!("{stem} {n}.{ext}") };
            n += 1;
        }
        let to = if parent.is_empty() { chosen.clone() } else { format!("{parent}/{chosen}") };
        taken_names.insert(to.to_lowercase());
        if chosen != name {
            renames.push(Rename { from: found.rel.clone(), to });
        }
    }
    Ok(renames)
}

/// Carries out renames: each file first to a temporary name, then to its new one, so names can
/// be swapped. The renames made, and the ones that failed with why.
pub fn apply(root: &Path, renames: &[Rename]) -> (Vec<Rename>, Vec<(String, String)>) {
    let mut parked = Vec::new();
    let mut failed = Vec::new();
    for (index, item) in renames.iter().enumerate() {
        let temporary = format!(".ownnas-renaming-{index}-{}", item.from.rsplit('/').next().unwrap_or_default());
        match files::rename_entry(root, &item.from, &temporary) {
            Ok(path) => parked.push((item, path)),
            Err(err) => failed.push((item.from.clone(), crate::sort_folder::why(err))),
        }
    }
    let mut made = Vec::new();
    for (item, parked_at) in parked {
        let name = item.to.rsplit('/').next().unwrap_or_default();
        match files::rename_entry(root, &parked_at, name) {
            Ok(path) => made.push(Rename { from: item.from.clone(), to: path }),
            Err(err) => {
                // Back to the old name, rather than left under the temporary one.
                let old = item.from.rsplit('/').next().unwrap_or_default();
                let _ = files::rename_entry(root, &parked_at, old);
                failed.push((item.from.clone(), crate::sort_folder::why(err)));
            }
        }
    }
    (made, failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(pattern: &str) -> Rules {
        Rules { pattern: pattern.into(), start: 1, ..Rules::default() }
    }

    #[test]
    fn names_from_patterns() {
        let when = 1_709_994_720; // 2024-03-09 14:32:00
        assert_eq!(new_name(&rules("{YYYY}-{MM}-{DD} {hh}.{mm}.{ss}"), "IMG_1234.JPG", "Trip", 0, when), "2024-03-09 14.32.00.JPG");
        assert_eq!(new_name(&rules("Trip {n:3}"), "a.jpg", "x", 4, when), "Trip 005.jpg");
        assert_eq!(new_name(&rules("{n} - {name}"), "a.jpg", "x", 0, when), "1 - a.jpg");
        assert_eq!(new_name(&rules("{folder} {name}"), "a.jpg", "Rome", 0, when), "Rome a.jpg");
        assert_eq!(new_name(&rules("a/b:c"), "x.txt", "f", 0, when), "a-b-c.txt");
        assert_eq!(new_name(&rules(""), "Makefile", "f", 0, when), "Makefile");
    }

    #[test]
    fn find_replace_and_case() {
        let replace = Rules { find: "img_".into(), replace: "Photo ".into(), ..Rules::default() };
        assert_eq!(new_name(&replace, "IMG_1234.jpg", "f", 0, 0), "Photo 1234.jpg");
        let title = Rules { case: "title".into(), ..Rules::default() };
        assert_eq!(new_name(&title, "my holiday in rome.JPG", "f", 0, 0), "My Holiday In Rome.JPG");
        let lower = Rules { case: "lower".into(), ..Rules::default() };
        assert_eq!(new_name(&lower, "README.TXT", "f", 0, 0), "readme.TXT");
    }

    #[test]
    fn clashes_get_numbers_and_swaps_work() {
        let dir = std::env::temp_dir().join(format!("ownnas-rename-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, text) in [("a.txt", "A"), ("b.txt", "B"), ("taken.txt", "T"), ("c.txt", "C")] {
            std::fs::write(dir.join(name), text).unwrap();
        }
        let root = dir.canonicalize().unwrap();
        // Everything to "taken": the file not being renamed keeps its name.
        let plan_taken = plan(&root, &["a.txt".into(), "b.txt".into()], &rules("taken"), DateFrom::Modified, &HashMap::new()).unwrap();
        assert_eq!(plan_taken.iter().map(|item| item.to.as_str()).collect::<Vec<_>>(), ["taken 2.txt", "taken 3.txt"]);
        // a and b swap names.
        let swap = vec![Rename { from: "a.txt".into(), to: "b.txt".into() }, Rename { from: "b.txt".into(), to: "a.txt".into() }];
        let (made, failed) = apply(&root, &swap);
        assert!(failed.is_empty(), "{failed:?}");
        assert_eq!(made.len(), 2);
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "B");
        assert_eq!(std::fs::read_to_string(root.join("b.txt")).unwrap(), "A");
        // Renaming among themselves: a→b and b→c is planned without numbers.
        let chain = plan(&root, &["a.txt".into(), "b.txt".into()], &rules("{n:1}"), DateFrom::Modified, &HashMap::new()).unwrap();
        assert_eq!(chain.iter().map(|item| item.to.as_str()).collect::<Vec<_>>(), ["1.txt", "2.txt"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
