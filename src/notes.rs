//! Notes: quick notes and checklists kept as plain Markdown files in one library folder
//! (`Notes` unless set otherwise), so they open in any editor (Obsidian included), sync, and
//! keep their history like every other file. Nothing about a note lives anywhere but its file:
//! colour and pinning are front matter (`color: yellow`, `pinned: true`), labels are OwnNAS tags
//! and `#hashtags`, and links are `[[wiki links]]`. Archived notes are in `Archive/` inside it.
//!
//! This reads and changes the files only; the web API checks who may.

use serde::Serialize;
use std::path::{Path, PathBuf};

pub const DEFAULT_FOLDER: &str = "Notes";
pub const SETTING: &str = "notes_folder";
pub const ARCHIVE: &str = "Archive";
pub const DAILY: &str = "Daily";
pub const TEMPLATES: &str = "Templates";
pub const ATTACHMENTS: &str = "attachments";
/// Notes listed at most, newest first.
const MOST_NOTES: usize = 3000;
/// The most of a note read for its card.
const MOST_READ: u64 = 256 * 1024;
const EXCERPT: usize = 600;
const MOST_ITEMS: usize = 50;
pub const COLORS: [&str; 10] = ["red", "orange", "yellow", "green", "teal", "blue", "purple", "pink", "brown", "gray"];

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    /// The line in the file, from 0.
    pub line: usize,
    pub done: bool,
    pub text: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub path: String,
    pub title: String,
    pub excerpt: String,
    pub items: Vec<Item>,
    /// How many checklist items there are in all (only the first ones are in `items`).
    pub item_count: usize,
    pub done_count: usize,
    pub color: Option<String>,
    pub pinned: bool,
    pub archived: bool,
    /// When to be reminded, "2026-11-08 09:00" (this computer's time), from `remind:`.
    pub remind: Option<String>,
    /// `#hashtags` in the text (OwnNAS tags are added by the caller).
    pub hashtags: Vec<String>,
    /// `[[wiki links]]`: the names linked to.
    pub links: Vec<String>,
    /// Seconds since 1970.
    pub modified: i64,
    pub size: u64,
}

/// The front matter's lines, and where the text after it starts.
fn front_matter(text: &str) -> (Vec<&str>, usize) {
    let mut lines = text.split_inclusive('\n');
    let Some(first) = lines.next() else { return (Vec::new(), 0) };
    if first.trim_end() != "---" {
        return (Vec::new(), 0);
    }
    let mut at = first.len();
    let mut found = Vec::new();
    for line in lines {
        at += line.len();
        if line.trim_end() == "---" {
            return (found, at);
        }
        found.push(line.trim_end_matches(['\n', '\r']));
    }
    (Vec::new(), 0)
}

fn matter_value<'a>(matter: &[&'a str], key: &str) -> Option<&'a str> {
    matter.iter().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        (name.trim().eq_ignore_ascii_case(key)).then(|| value.trim().trim_matches(['"', '\'']))
    })
}

/// A checklist line (`- [ ] milk`, `* [x] eggs`): done, and its text.
pub fn checklist_item(line: &str) -> Option<(bool, &str)> {
    let rest = line.trim_start().strip_prefix("- ").or_else(|| line.trim_start().strip_prefix("* "))?;
    let done = match rest.get(..3)? {
        "[ ]" => false,
        "[x]" | "[X]" => true,
        _ => return None,
    };
    Some((done, rest[3..].trim()))
}

fn hashtags(text: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut in_code = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            continue;
        }
        let chars: Vec<char> = line.chars().collect();
        for (i, c) in chars.iter().enumerate() {
            if *c != '#' || (i > 0 && !chars[i - 1].is_whitespace() && chars[i - 1] != '(') {
                continue;
            }
            let tag: String = chars[i + 1..].iter().take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '/')).collect();
            // A heading's "# " has a space, and "#1" is a number, not a tag.
            if tag.chars().any(char::is_alphabetic) && !found.iter().any(|seen| seen.eq_ignore_ascii_case(&tag)) {
                found.push(tag);
            }
        }
    }
    found
}

/// The names `[[wiki links]]` point to (without `|alias` or `#heading`).
pub fn links(text: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("[[") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("]]") else { break };
        let inner = &after[..end];
        let target = inner.split(['|', '#']).next().unwrap_or_default().trim();
        if !target.is_empty() && !inner.contains('\n') && !found.iter().any(|seen| seen.eq_ignore_ascii_case(target)) {
            found.push(target.to_string());
        }
        rest = &after[end + 2..];
    }
    found
}

/// What a note shows on its card, from its text.
pub fn read_note(path: &str, stem: &str, text: &str, modified: i64, size: u64, archived: bool) -> Note {
    let (matter, body_at) = front_matter(text);
    let body = &text[body_at..];
    let mut title = matter_value(&matter, "title").map(str::to_string);
    let mut excerpt = String::new();
    let mut items = Vec::new();
    let (mut item_count, mut done_count) = (0, 0);
    let first_line = text[..body_at].matches('\n').count();
    let mut title_taken = title.is_some();
    for (index, line) in body.lines().enumerate() {
        if let Some((done, item)) = checklist_item(line) {
            item_count += 1;
            done_count += usize::from(done);
            if items.len() < MOST_ITEMS {
                items.push(Item { line: first_line + index, done, text: item.to_string() });
            }
            continue;
        }
        let trimmed = line.trim();
        if !title_taken && !trimmed.is_empty() {
            title_taken = true;
            if let Some(heading) = trimmed.strip_prefix("# ") {
                title = Some(heading.trim().to_string());
                continue;
            }
        }
        if excerpt.chars().count() < EXCERPT && !(excerpt.is_empty() && trimmed.is_empty()) {
            excerpt.push_str(line);
            excerpt.push('\n');
        }
    }
    let excerpt: String = excerpt.trim_end().chars().take(EXCERPT).collect();
    let color = matter_value(&matter, "color").map(str::to_ascii_lowercase).filter(|color| COLORS.contains(&color.as_str()));
    let pinned = matter_value(&matter, "pinned").is_some_and(|value| value.eq_ignore_ascii_case("true") || value == "yes");
    let remind = matter_value(&matter, "remind").and_then(reminder_time);
    Note {
        path: path.to_string(),
        title: title.unwrap_or_else(|| stem.to_string()),
        excerpt,
        items,
        item_count,
        done_count,
        color,
        pinned,
        archived,
        remind,
        hashtags: hashtags(body),
        links: links(body),
        modified,
        size,
    }
}

/// The notes in `folder` (a library folder, `full` its place on disk): every `.md` file in it
/// and the folders inside, apart from hidden ones, attachments and templates. Newest first.
pub fn list(full: &Path, folder: &str) -> Vec<Note> {
    let mut found = Vec::new();
    walk(full, folder, false, &mut found);
    found.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.path.cmp(&b.path)));
    found.truncate(MOST_NOTES);
    found
}

fn walk(dir: &Path, rel: &str, archived: bool, found: &mut Vec<Note>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let path = format!("{rel}/{name}");
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            if name == ATTACHMENTS || name == TEMPLATES {
                continue;
            }
            walk(&entry.path(), &path, archived || name == ARCHIVE, found);
            continue;
        }
        let Some(stem) = name.strip_suffix(".md").or_else(|| name.strip_suffix(".MD")).or_else(|| name.strip_suffix(".markdown")) else { continue };
        let Ok(meta) = entry.metadata() else { continue };
        let mut text = String::new();
        if let Ok(file) = std::fs::File::open(entry.path()) {
            use std::io::Read;
            let _ = file.take(MOST_READ).read_to_string(&mut text);
        }
        let modified = meta.modified().ok().and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |time| time.as_secs() as i64);
        found.push(read_note(&path, stem, &text, modified, meta.len(), archived));
    }
}

/// A file name for a new note, from its first line ("Groceries.md"), or the time.
pub fn name_for(text: &str, now: &str) -> String {
    let first = text.lines().map(str::trim).find(|line| !line.is_empty() && *line != "---").unwrap_or_default();
    let first = checklist_item(first).map_or(first, |(_, item)| item).trim_start_matches('#').trim();
    let mut name: String = first.chars().filter(|c| !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '[' | ']' | '#' | '^') && !c.is_control()).collect();
    name = name.trim().trim_start_matches('.').trim().to_string();
    if name.chars().count() > 60 {
        name = name.chars().take(60).collect::<String>().trim_end().to_string();
    }
    if name.is_empty() {
        name = format!("Note {now}");
    }
    name
}

/// `dir/name.md`, or `dir/name 2.md` and so on when it is taken.
pub fn free_path(dir: &Path, name: &str) -> PathBuf {
    let mut candidate = dir.join(format!("{name}.md"));
    let mut n = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{name} {n}.md"));
        n += 1;
    }
    candidate
}

/// A reminder's time as "YYYY-MM-DD HH:MM", from that or "YYYY-MM-DDTHH:MM" (what a browser's
/// date and time field gives), or a day alone (9:00 then).
pub fn reminder_time(text: &str) -> Option<String> {
    let text = text.trim().replace('T', " ");
    let (day, time) = text.split_once(' ').unwrap_or((&text, "09:00"));
    let date = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()?;
    let time = chrono::NaiveTime::parse_from_str(time.get(..5).unwrap_or(time), "%H:%M").ok()?;
    Some(format!("{} {}", date.format("%Y-%m-%d"), time.format("%H:%M")))
}

/// The text with one checklist line ticked or not. `expected` is the item's text as shown, so a
/// file changed meanwhile is not ticked in the wrong place.
pub fn tick(text: &str, line: usize, expected: &str, done: bool) -> Result<String, &'static str> {
    let mut out = String::with_capacity(text.len());
    let mut changed = false;
    for (index, piece) in text.split_inclusive('\n').enumerate() {
        if index == line {
            let (_, item) = checklist_item(piece).ok_or("That line is no longer a checklist item")?;
            if item != expected.trim() {
                return Err("The note changed meanwhile: look again");
            }
            let open = piece.find('[').ok_or("That line is no longer a checklist item")?;
            out.push_str(&piece[..open]);
            out.push_str(if done { "[x]" } else { "[ ]" });
            out.push_str(&piece[open + 3..]);
            changed = true;
        } else {
            out.push_str(piece);
        }
    }
    if changed { Ok(out) } else { Err("That line is no longer there") }
}

/// The text with front matter keys set (`Some`) or taken out (`None`); front matter with nothing
/// left goes.
pub fn set_matter(text: &str, changes: &[(&str, Option<&str>)]) -> String {
    let (matter, body_at) = front_matter(text);
    let mut lines: Vec<String> = matter.iter().map(|line| line.to_string()).collect();
    for (key, value) in changes {
        let at = lines.iter().position(|line| line.split_once(':').is_some_and(|(name, _)| name.trim().eq_ignore_ascii_case(key)));
        match (at, value) {
            (Some(at), Some(value)) => lines[at] = format!("{key}: {value}"),
            (None, Some(value)) => lines.push(format!("{key}: {value}")),
            (Some(at), None) => {
                lines.remove(at);
            }
            (None, None) => {}
        }
    }
    let body = &text[body_at..];
    if lines.is_empty() {
        return body.to_string();
    }
    format!("---\n{}\n---\n{body}", lines.join("\n"))
}

/// The text with `[[old]]` links (also `[[old|shown]]` and `[[old#part]]`, in any case) pointing
/// to `new`; `None` when there are none.
pub fn relink(text: &str, old: &str, new: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut changed = false;
    while let Some(start) = rest.find("[[") {
        out.push_str(&rest[..start + 2]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("]]") else {
            rest = after;
            break;
        };
        let inner = &after[..end];
        let cut = inner.find(['|', '#']).unwrap_or(inner.len());
        if !inner.contains('\n') && inner[..cut].trim().eq_ignore_ascii_case(old.trim()) {
            out.push_str(new);
            out.push_str(&inner[cut..]);
            changed = true;
        } else {
            out.push_str(inner);
        }
        out.push_str("]]");
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    changed.then_some(out)
}

/// After a note was renamed from `old` to `new` (file names without `.md`): points the links in
/// the notes of `folder` (on disk `full`) to the new name. `keep` is called with each note's
/// place and library path before it is changed (to keep a version). The notes changed.
pub fn relink_folder(full: &Path, folder: &str, old: &str, new: &str, keep: &dyn Fn(&Path, &str)) -> Vec<String> {
    if old.eq_ignore_ascii_case(new) {
        return Vec::new();
    }
    let mut changed = Vec::new();
    for note in list(full, folder) {
        let Some(inside) = note.path.strip_prefix(folder).map(|rest| rest.trim_start_matches('/')) else { continue };
        let place = full.join(inside);
        let Ok(text) = std::fs::read_to_string(&place) else { continue };
        if let Some(relinked) = relink(&text, old, new) {
            keep(&place, &note.path);
            if std::fs::write(&place, relinked).is_ok() {
                changed.push(note.path);
            }
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTE: &str = "---\ncolor: Yellow\npinned: true\n---\n# Groceries\n\nFor the weekend #home #shopping, see [[Recipes|food]] and [[Plans#June]].\n- [ ] milk\n- [x] eggs\n  * [ ] bread\n## Later\n#1 is not a tag\n";

    #[test]
    fn a_note_reads_as_its_card() {
        let note = read_note("Notes/Groceries.md", "Groceries", NOTE, 5, 10, false);
        assert_eq!(note.title, "Groceries");
        assert_eq!(note.color.as_deref(), Some("yellow"));
        assert!(note.pinned);
        assert_eq!(note.items, [
            Item { line: 7, done: false, text: "milk".into() },
            Item { line: 8, done: true, text: "eggs".into() },
            Item { line: 9, done: false, text: "bread".into() },
        ]);
        assert_eq!((note.item_count, note.done_count), (3, 1));
        assert_eq!(note.hashtags, ["home", "shopping"]);
        assert_eq!(note.links, ["Recipes", "Plans"]);
        assert!(note.excerpt.starts_with("For the weekend"), "{}", note.excerpt);
        assert!(!note.excerpt.contains("milk"), "items show as items");
        let plain = read_note("Notes/x.md", "x", "just words\n", 0, 0, false);
        assert_eq!((plain.title.as_str(), plain.excerpt.as_str()), ("x", "just words"));
    }

    #[test]
    fn ticking_changes_only_that_line() {
        let ticked = tick(NOTE, 7, "milk", true).unwrap();
        assert!(ticked.contains("- [x] milk\n- [x] eggs\n"));
        assert_eq!(ticked.len(), NOTE.len());
        assert_eq!(tick(&ticked, 9, "bread", true).unwrap().matches("[x]").count(), 3, "indented items too");
        assert!(tick(NOTE, 7, "butter", true).is_err(), "changed meanwhile");
        assert!(tick(NOTE, 4, "Groceries", true).is_err(), "not an item");
        assert!(tick(NOTE, 99, "milk", true).is_err());
    }

    #[test]
    fn front_matter_is_set_and_taken_out() {
        let set = set_matter(NOTE, &[("color", Some("blue")), ("pinned", None)]);
        assert!(set.starts_with("---\ncolor: blue\n---\n# Groceries"), "{set}");
        let none = set_matter(&set, &[("color", None)]);
        assert!(none.starts_with("# Groceries"), "{none}");
        assert_eq!(set_matter("plain\n", &[("pinned", Some("true"))]), "---\npinned: true\n---\nplain\n");
    }

    #[test]
    fn links_follow_a_rename() {
        let text = "See [[groceries]], [[Groceries|the list]] and [[Groceries#Weekend]], not [[Groceries old]] or [[Other]].";
        assert_eq!(
            relink(text, "Groceries", "Shopping").as_deref(),
            Some("See [[Shopping]], [[Shopping|the list]] and [[Shopping#Weekend]], not [[Groceries old]] or [[Other]].")
        );
        assert_eq!(relink("no links here", "Groceries", "Shopping"), None);
        assert_eq!(relink("[[broken", "broken", "x"), None);
    }

    #[test]
    fn reminders_read_in_several_ways() {
        assert_eq!(reminder_time("2026-11-08 09:30").as_deref(), Some("2026-11-08 09:30"));
        assert_eq!(reminder_time("2026-11-08T18:05").as_deref(), Some("2026-11-08 18:05"));
        assert_eq!(reminder_time("2026-11-08").as_deref(), Some("2026-11-08 09:00"));
        assert_eq!(reminder_time("tomorrow"), None);
        let note = read_note("Notes/x.md", "x", "---\nremind: 2026-11-08T07:00\n---\nCall Ann\n", 0, 0, false);
        assert_eq!(note.remind.as_deref(), Some("2026-11-08 07:00"));
    }

    #[test]
    fn names_come_from_the_first_line() {
        assert_eq!(name_for("# Ideas: big/small?\nmore", "x"), "Ideas bigsmall");
        assert_eq!(name_for("- [ ] buy milk\n- [ ] eggs", "x"), "buy milk");
        assert_eq!(name_for("   \n", "2026-11-07 10.30"), "Note 2026-11-07 10.30");
        assert_eq!(name_for(".hidden", "x"), "hidden");
    }
}
