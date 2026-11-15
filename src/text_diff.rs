//! What changed between two versions of a file, line by line, with the changed words marked:
//! for text files (notes, code, CSV…) and for the words of documents, spreadsheets,
//! presentations, and PDFs, which are compared as their text, a paragraph to a line.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use similar::{Algorithm, ChangeTag, TextDiff};

/// Text files larger than this are not compared.
const MAX_TEXT_FILE: u64 = 8 * 1024 * 1024;
/// Documents larger than this are not compared.
const MAX_DOCUMENT: u64 = 100 * 1024 * 1024;
/// Lines of unchanged text shown around each change.
const CONTEXT: usize = 3;
/// Lines sent at most; past them the comparison says it stopped.
const MOST_LINES: usize = 20_000;
/// A comparison that takes longer than this settles for a coarser answer.
const TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Diff {
    pub identical: bool,
    /// Said when the texts are the same but the files are not (line endings, a document's layout).
    pub note: Option<String>,
    pub added: usize,
    pub removed: usize,
    pub hunks: Vec<Hunk>,
    /// Not every change is in `hunks`: there were too many lines.
    pub truncated: bool,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Hunk {
    pub old_start: usize,
    pub new_start: usize,
    pub lines: Vec<Line>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Line {
    /// "same", "del" (only in the older one), or "add" (only in the newer one).
    pub kind: &'static str,
    /// Line numbers, from 1.
    pub old: Option<usize>,
    pub new: Option<usize>,
    /// The line in pieces: `true` for the words that changed.
    pub parts: Vec<(bool, String)>,
}

/// The text of a file to compare. `name` gives the kind (the file itself may be a version's
/// copy, which has no extension), and `scratch` is a folder for a copy with that name.
pub fn text_of(path: &Path, name: &str, scratch: &Path) -> Result<String, String> {
    let size = std::fs::metadata(path).map_err(|_| "The file is not there any more".to_string())?.len();
    let extension = name.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase()).unwrap_or_default();
    let document = extension == "pdf" || crate::documents::TEXT_FORMATS.contains(&extension.as_str());
    if document {
        if size > MAX_DOCUMENT {
            return Err(format!("Files over {} MB are not compared", MAX_DOCUMENT / 1024 / 1024));
        }
        // The readers go by the extension, which a version's copy does not have.
        let named = Named::new(path, &extension, scratch)?;
        let text = if extension == "pdf" { crate::content_index::pdf_text(&named.path) } else { crate::documents::plain_text(&named.path) };
        return text.map(|text| tidy_document(&text)).ok_or_else(|| "The words in this file could not be read".to_string());
    }
    if size > MAX_TEXT_FILE {
        return Err(format!("Text files over {} MB are not compared", MAX_TEXT_FILE / 1024 / 1024));
    }
    let bytes = std::fs::read(path).map_err(|_| "The file could not be read".to_string())?;
    if bytes.iter().take(8192).any(|&b| b == 0) {
        return Err("Only text files and documents can be compared".into());
    }
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        // Latin-1 and the like: shown as best it can be, rather than not at all.
        Err(err) => String::from_utf8_lossy(err.as_bytes()).into_owned(),
    };
    Ok(text.strip_prefix('\u{feff}').map(str::to_string).unwrap_or(text))
}

/// A document's words as lines: one per paragraph, without the blank ones.
fn tidy_document(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines().map(str::trim_end).filter(|line| !line.trim().is_empty()) {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// A file under a name with the extension its reader wants: a hard link, or a copy.
struct Named {
    path: PathBuf,
    made: bool,
}

impl Named {
    fn new(path: &Path, extension: &str, scratch: &Path) -> Result<Self, String> {
        if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case(extension)) {
            return Ok(Self { path: path.to_path_buf(), made: false });
        }
        std::fs::create_dir_all(scratch).map_err(|_| "Could not prepare the comparison".to_string())?;
        let unique = format!("{}-{}.{extension}", std::process::id(), rand::random::<u64>());
        let named = scratch.join(unique);
        if std::fs::hard_link(path, &named).is_err() {
            std::fs::copy(path, &named).map_err(|_| "Could not prepare the comparison".to_string())?;
        }
        Ok(Self { path: named, made: true })
    }
}

impl Drop for Named {
    fn drop(&mut self) {
        if self.made {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// What changed from `old` to `new`.
pub fn diff(old: &str, new: &str) -> Diff {
    let (old_lines, new_lines) = (old.replace("\r\n", "\n"), new.replace("\r\n", "\n"));
    if old_lines == new_lines {
        let note = (old != new).then(|| "Only the line endings differ.".to_string());
        return Diff { identical: true, note, added: 0, removed: 0, hunks: Vec::new(), truncated: false };
    }
    let diff = TextDiff::configure().algorithm(Algorithm::Patience).timeout(TIMEOUT).diff_lines(&old_lines, &new_lines);
    let (mut added, mut removed, mut sent, mut truncated) = (0, 0, 0, false);
    let mut hunks = Vec::new();
    for group in diff.grouped_ops(CONTEXT) {
        let Some(first) = group.first() else { continue };
        let mut hunk = Hunk { old_start: first.old_range().start + 1, new_start: first.new_range().start + 1, lines: Vec::new() };
        for op in &group {
            for change in diff.iter_inline_changes(op) {
                let kind = match change.tag() {
                    ChangeTag::Equal => "same",
                    ChangeTag::Delete => {
                        removed += 1;
                        "del"
                    }
                    ChangeTag::Insert => {
                        added += 1;
                        "add"
                    }
                };
                if sent >= MOST_LINES {
                    truncated = true;
                    continue;
                }
                sent += 1;
                let mut parts: Vec<(bool, String)> = Vec::new();
                for (changed, text) in change.iter_strings_lossy() {
                    let text = text.trim_end_matches(['\n', '\r']);
                    if text.is_empty() {
                        continue;
                    }
                    // Neighbouring pieces of the same kind are one.
                    match parts.last_mut() {
                        Some((last, joined)) if *last == changed => joined.push_str(text),
                        _ => parts.push((changed, text.to_string())),
                    }
                }
                // A line changed as a whole marks nothing in particular.
                if parts.iter().all(|(changed, _)| *changed) {
                    parts.iter_mut().for_each(|(changed, _)| *changed = false);
                }
                hunk.lines.push(Line { kind, old: change.old_index().map(|i| i + 1), new: change.new_index().map(|i| i + 1), parts });
            }
        }
        if !hunk.lines.is_empty() {
            hunks.push(hunk);
        }
    }
    Diff { identical: false, note: None, added, removed, hunks, truncated }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_and_words_that_changed() {
        let old = "# Shopping\n\n- milk\n- bread\n- eggs\n";
        let new = "# Shopping\n\n- milk\n- brown bread\n- eggs\n- coffee\n";
        let d = diff(old, new);
        assert!(!d.identical);
        assert_eq!((d.added, d.removed), (2, 1));
        assert_eq!(d.hunks.len(), 1);
        let lines = &d.hunks[0].lines;
        let del = lines.iter().find(|l| l.kind == "del").unwrap();
        assert_eq!(del.old, Some(4));
        let add = lines.iter().find(|l| l.kind == "add" && l.new == Some(4)).unwrap();
        assert!(add.parts.iter().any(|(changed, text)| *changed && text.contains("brown")), "{:?}", add.parts);
        let coffee = lines.iter().find(|l| l.new == Some(6)).unwrap();
        assert_eq!(coffee.parts, vec![(false, "- coffee".to_string())], "a new line marks no words");
    }

    #[test]
    fn far_apart_changes_are_separate_hunks() {
        let old: String = (1..=40).map(|n| format!("line {n}\n")).collect();
        let new = old.replace("line 2\n", "line two\n").replace("line 38\n", "line thirty-eight\n");
        let d = diff(&old, &new);
        assert_eq!(d.hunks.len(), 2);
        assert_eq!(d.hunks[1].old_start, 35);
        assert!(d.hunks[0].lines.iter().all(|l| l.old.unwrap_or(0) <= 5 || l.kind != "same"));
    }

    #[test]
    fn line_endings_alone_are_no_change() {
        let d = diff("a\nb\n", "a\r\nb\r\n");
        assert!(d.identical);
        assert_eq!(d.note.as_deref(), Some("Only the line endings differ."));
        assert!(diff("same", "same").note.is_none());
    }

    #[test]
    fn documents_are_compared_by_their_words() {
        let dir = std::env::temp_dir().join(format!("ownnas-diff-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).unwrap();
        // A version's copy has no extension: the name says what it is.
        let copy = dir.join("12345");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&copy).unwrap());
        zip.start_file("word/document.xml", zip::write::SimpleFileOptions::default()).unwrap();
        std::io::Write::write_all(
            &mut zip,
            br#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>Dear Ann,</w:t></w:r></w:p><w:p></w:p><w:p><w:r><w:t>See you on Monday.</w:t></w:r></w:p></w:body></w:document>"#,
        )
        .unwrap();
        zip.finish().unwrap();
        let text = text_of(&copy, "letter.docx", &dir.join("scratch")).unwrap();
        assert_eq!(text, "Dear Ann,\nSee you on Monday.\n");
        assert_eq!(std::fs::read_dir(dir.join("scratch")).unwrap().count(), 0, "the named copy is removed");
        std::fs::write(dir.join("photo"), [0xff, 0xd8, 0, 0, 1]).unwrap();
        assert!(text_of(&dir.join("photo"), "photo.jpg", &dir).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
