use std::fs::File;
use std::io::Read;
use std::path::Path;

use serde::Serialize;

const TEXT_LIMIT: usize = 256 * 1024;
const ARCHIVE_LIMIT: usize = 500;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextPreview {
    pub content: String,
    pub truncated: bool,
    pub binary: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveEntry {
    pub name: String,
    pub size: u64,
    pub dir: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Inspection {
    pub text: Option<TextPreview>,
    pub archive: Option<Vec<ArchiveEntry>>,
    pub archive_truncated: bool,
    pub note: Option<String>,
}

pub fn inspect(path: &Path, name: &str) -> Inspection {
    let kind = crate::files::kind_of(name);
    if kind == "archive" {
        return inspect_archive(path, name);
    }
    if kind == "text" || kind == "file" {
        return Inspection {
            text: Some(read_text(path)),
            archive: None,
            archive_truncated: false,
            note: None,
        };
    }
    Inspection {
        text: None,
        archive: None,
        archive_truncated: false,
        note: None,
    }
}

fn read_text(path: &Path) -> TextPreview {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(_) => {
            return TextPreview {
                content: String::new(),
                truncated: false,
                binary: true,
            };
        }
    };
    let mut buf = vec![0u8; TEXT_LIMIT + 1];
    let read = file.read(&mut buf).unwrap_or(0);
    let truncated = read > TEXT_LIMIT;
    let size = read.min(TEXT_LIMIT);
    let sample = &buf[..size];
    if sample.contains(&0) || std::str::from_utf8(sample).is_err() {
        return TextPreview {
            content: String::new(),
            truncated: false,
            binary: true,
        };
    }
    TextPreview {
        content: String::from_utf8_lossy(sample).to_string(),
        truncated,
        binary: false,
    }
}

fn inspect_archive(path: &Path, name: &str) -> Inspection {
    match read_archive(path, name) {
        Ok((entries, truncated)) => Inspection {
            text: None,
            archive: Some(entries),
            archive_truncated: truncated,
            note: None,
        },
        Err(note) => Inspection {
            text: None,
            archive: None,
            archive_truncated: false,
            note: Some(note),
        },
    }
}

fn read_archive(path: &Path, name: &str) -> Result<(Vec<ArchiveEntry>, bool), String> {
    let (entries, more) = crate::archives::list(path, name, ARCHIVE_LIMIT)?;
    Ok((entries.into_iter().map(|entry| ArchiveEntry { name: entry.name, size: entry.size, dir: entry.dir }).collect(), more))
}
