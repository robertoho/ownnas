use std::fs::File;
use std::io::Read;
use std::path::Path;

use flate2::read::GzDecoder;
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
    if archive_format(name).is_some() || kind == "archive" {
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
    match archive_format(name) {
        Some(ArchiveKind::Zip) => read_zip(path),
        Some(ArchiveKind::Tar) => read_tar(path, false),
        Some(ArchiveKind::TarGz) => read_tar(path, true),
        None => Err("This archive can be downloaded. OwnNAS does not list this compression format.".into()),
    }
}

enum ArchiveKind {
    Zip,
    Tar,
    TarGz,
}

fn archive_format(name: &str) -> Option<ArchiveKind> {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
        return Some(ArchiveKind::TarGz);
    }
    if lower.ends_with(".tar") {
        return Some(ArchiveKind::Tar);
    }
    match Path::new(&lower)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
    {
        "zip" | "jar" | "cbz" => Some(ArchiveKind::Zip),
        _ => None,
    }
}

fn read_zip(path: &Path) -> Result<(Vec<ArchiveEntry>, bool), String> {
    let file = File::open(path).map_err(|_| "Could not open the archive".to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|_| "Could not read the zip file".to_string())?;
    let total = archive.len();
    let take = total.min(ARCHIVE_LIMIT);
    let mut entries = Vec::with_capacity(take);
    for index in 0..take {
        let entry = archive
            .by_index(index)
            .map_err(|_| "Could not read an entry in the zip file".to_string())?;
        entries.push(ArchiveEntry {
            name: entry.name().to_string(),
            size: entry.size(),
            dir: entry.is_dir(),
        });
    }
    Ok((entries, total > ARCHIVE_LIMIT))
}

fn read_tar(path: &Path, gzip: bool) -> Result<(Vec<ArchiveEntry>, bool), String> {
    let file = File::open(path).map_err(|_| "Could not open the archive".to_string())?;
    if gzip {
        let decoder = GzDecoder::new(file);
        collect_tar(tar::Archive::new(decoder))
    } else {
        collect_tar(tar::Archive::new(file))
    }
}

fn collect_tar<R: Read>(mut archive: tar::Archive<R>) -> Result<(Vec<ArchiveEntry>, bool), String> {
    let mut entries = Vec::new();
    let mut truncated = false;
    let iter = archive
        .entries()
        .map_err(|_| "Could not read the tar file".to_string())?;
    for entry in iter {
        if entries.len() >= ARCHIVE_LIMIT {
            truncated = true;
            break;
        }
        let entry = entry.map_err(|_| "Could not read an entry in the tar file".to_string())?;
        let name = entry
            .path()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "(unreadable name)".to_string());
        entries.push(ArchiveEntry {
            name,
            size: entry.size(),
            dir: entry.header().entry_type().is_dir(),
        });
    }
    Ok((entries, truncated))
}
