use std::fs::{self, Metadata};
use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::Serialize;

pub const MAX_LIST: usize = 5_000;

#[derive(Debug)]
pub enum FileError {
    Forbidden,
    NotFound,
    NotADirectory,
    IsADirectory,
    AlreadyExists,
    InvalidName,
    Io(&'static str),
}

pub struct Resolved {
    pub rel: String,
    pub full: PathBuf,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub dir: bool,
    pub size: u64,
    pub modified: i64,
    pub kind: String,
    pub thumb: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listing {
    pub path: String,
    pub parent: Option<String>,
    pub root_name: String,
    pub entries: Vec<Entry>,
    pub truncated: bool,
    pub total: usize,
}

pub fn prepare_paths(root: &Path, data: &Path) -> Result<(PathBuf, PathBuf), String> {
    if !root.exists() {
        return Err(format!("Shared folder does not exist: {}", root.display()));
    }
    let root = root
        .canonicalize()
        .map_err(|_| format!("Could not open the shared folder: {}", root.display()))?;
    if !root.is_dir() {
        return Err("The shared path is not a folder".into());
    }
    fs::create_dir_all(data)
        .map_err(|_| format!("Could not create the data folder: {}", data.display()))?;
    let data = data
        .canonicalize()
        .map_err(|_| format!("Could not open the data folder: {}", data.display()))?;
    if !data.is_dir() {
        return Err("The data path is not a folder".into());
    }
    if data.starts_with(&root) || root.starts_with(&data) {
        return Err(
            "Keep the data folder outside the shared folder. The SQLite database and thumbnail cache must not live inside the files you are sharing."
                .into(),
        );
    }
    Ok((root, data))
}

pub fn root_label(root: &Path) -> String {
    root.file_name()
        .map(|s| s.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Root".to_string())
}

pub fn resolve(root: &Path, rel: &str) -> Result<Resolved, FileError> {
    let root = root.canonicalize().map_err(map_io)?;
    let parts = clean_parts(rel)?;
    let mut full = root.clone();
    for part in &parts {
        full.push(part);
        if full.is_symlink() {
            let canon = full.canonicalize().map_err(map_io)?;
            if !canon.starts_with(&root) {
                return Err(FileError::Forbidden);
            }
            full = canon;
        }
    }
    if !full.exists() {
        return Err(FileError::NotFound);
    }
    let canon = full.canonicalize().map_err(map_io)?;
    if !canon.starts_with(&root) {
        return Err(FileError::Forbidden);
    }
    Ok(Resolved {
        rel: parts.join("/"),
        full: canon,
    })
}

pub fn list_dir(root: &Path, rel: &str, show_hidden: bool, ffmpeg: bool) -> Result<Listing, FileError> {
    let resolved = resolve(root, rel)?;
    if !resolved.full.is_dir() {
        return Err(FileError::NotADirectory);
    }
    let mut entries = Vec::new();
    for item in fs::read_dir(&resolved.full).map_err(map_io)? {
        let item = match item {
            Ok(item) => item,
            Err(_) => continue,
        };
        let name = item.file_name().to_string_lossy().to_string();
        if name.is_empty() || name == "." || name == ".." {
            continue;
        }
        let child_path = item.path();
        if !show_hidden && entry_hidden(&child_path, &name) {
            continue;
        }
        let meta = match item.metadata() {
            Ok(meta) => meta,
            Err(_) => match std::fs::symlink_metadata(&child_path) {
                Ok(meta) => meta,
                Err(_) => continue,
            },
        };
        let dir = meta.is_dir();
        let path = if resolved.rel.is_empty() {
            name.clone()
        } else {
            format!("{}/{}", resolved.rel, name)
        };
        entries.push(Entry {
            kind: if dir {
                "folder".to_string()
            } else {
                kind_of(&name).to_string()
            },
            thumb: !dir && can_thumb(&name, ffmpeg),
            name,
            path,
            dir,
            size: if dir { 0 } else { meta.len() },
            modified: modified_secs(&meta),
        });
    }
    entries.sort_by(|a, b| {
        b.dir
            .cmp(&a.dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    let total = entries.len();
    let truncated = total > MAX_LIST;
    if truncated {
        entries.truncate(MAX_LIST);
    }
    let parent = parent_rel(&resolved.rel);
    Ok(Listing {
        path: resolved.rel,
        parent,
        root_name: root_label(root),
        entries,
        truncated,
        total,
    })
}

pub fn make_dir(root: &Path, parent_rel: &str, name: &str) -> Result<(), FileError> {
    if !valid_new_component(name) {
        return Err(FileError::InvalidName);
    }
    let parent = resolve(root, parent_rel)?;
    if !parent.full.is_dir() {
        return Err(FileError::NotADirectory);
    }
    let dest = parent.full.join(name);
    if dest.exists() {
        return Err(FileError::AlreadyExists);
    }
    fs::create_dir(&dest).map_err(map_io)
}

pub fn rename_entry(root: &Path, rel: &str, new_name: &str) -> Result<(), FileError> {
    if !valid_new_component(new_name) {
        return Err(FileError::InvalidName);
    }
    let src = resolve(root, rel)?;
    if src.rel.is_empty() {
        return Err(FileError::Forbidden);
    }
    let parent = src.full.parent().ok_or(FileError::Forbidden)?;
    let dest = parent.join(new_name);
    if dest.exists() {
        return Err(FileError::AlreadyExists);
    }
    fs::rename(&src.full, &dest).map_err(map_io)
}

pub fn remove_entry(root: &Path, rel: &str) -> Result<(), FileError> {
    let target = resolve(root, rel)?;
    if target.rel.is_empty() {
        return Err(FileError::Forbidden);
    }
    if target.full.is_dir() {
        fs::remove_dir_all(&target.full).map_err(map_io)
    } else {
        fs::remove_file(&target.full).map_err(map_io)
    }
}

/// Builds a destination path inside `dir_rel` for an uploaded file.
/// Intermediate folders in `filename` (from a folder upload) are created.
/// An existing file gets a numeric suffix so nothing is overwritten.
pub fn prepare_upload(root: &Path, dir_rel: &str, filename: &str) -> Result<PathBuf, FileError> {
    let root = root.canonicalize().map_err(map_io)?;
    let dir = resolve(&root, dir_rel)?;
    if !dir.full.is_dir() {
        return Err(FileError::NotADirectory);
    }
    let extra = clean_parts(filename)?;
    if extra.is_empty() || extra.iter().any(|part| !valid_new_component(part)) {
        return Err(FileError::InvalidName);
    }
    let mut cursor = dir.full;
    for (index, part) in extra.iter().enumerate() {
        let is_last = index + 1 == extra.len();
        cursor.push(part);
        if cursor.exists() {
            let canon = cursor.canonicalize().map_err(map_io)?;
            if !canon.starts_with(&root) {
                return Err(FileError::Forbidden);
            }
            if is_last {
                if canon.is_dir() {
                    return Err(FileError::AlreadyExists);
                }
                return Ok(unique_path(canon));
            }
            if !canon.is_dir() {
                return Err(FileError::AlreadyExists);
            }
            cursor = canon;
        } else if !is_last {
            fs::create_dir(&cursor).map_err(map_io)?;
        }
    }
    Ok(cursor)
}

pub fn kind_of(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".tar.gz")
        || lower.ends_with(".tgz")
        || lower.ends_with(".tar.bz2")
        || lower.ends_with(".tar.xz")
    {
        return "archive";
    }
    match extension(&lower).as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "ico" | "heic"
        | "heif" | "avif" | "jfif" => "image",
        "svg" | "svgz" => "svg",
        "mp4" | "m4v" | "webm" | "mkv" | "mov" | "avi" | "ogv" | "mpeg" | "mpg" | "wmv" => "video",
        "mp3" | "wav" | "flac" | "ogg" | "opus" | "m4a" | "aac" | "wma" => "audio",
        "pdf" => "pdf",
        "zip" | "jar" | "cbz" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "7z" | "rar" => "archive",
        "txt" | "md" | "markdown" | "json" | "csv" | "tsv" | "log" | "xml" | "yaml" | "yml"
        | "toml" | "ini" | "conf" | "cfg" | "html" | "htm" | "css" | "js" | "mjs" | "ts"
        | "tsx" | "jsx" | "py" | "rs" | "go" | "java" | "c" | "h" | "cpp" | "hpp" | "cs"
        | "sh" | "bash" | "zsh" | "ps1" | "sql" | "rb" | "php" | "lua" | "vue" | "svelte" => "text",
        _ => "file",
    }
}

pub fn can_thumb(name: &str, ffmpeg: bool) -> bool {
    match kind_of(name) {
        "image" => !matches!(
            extension(&name.to_ascii_lowercase()).as_str(),
            "svg" | "svgz" | "heic" | "heif" | "avif" | "jxl"
        ),
        "video" => ffmpeg,
        _ => false,
    }
}

pub fn content_disposition(name: &str, attachment: bool) -> String {
    let kind = if attachment { "attachment" } else { "inline" };
    let safe_name = if name.chars().any(|c| c == '\r' || c == '\n' || c == '"') {
        "download"
    } else {
        name
    };
    let fallback: String = safe_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._- ".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let fallback = if fallback.trim().is_empty() {
        "download".to_string()
    } else {
        fallback
    };
    format!(
        "{kind}; filename=\"{fallback}\"; filename*=UTF-8''{}",
        percent_encode(safe_name)
    )
}

pub fn active_content(name: &str) -> bool {
    matches!(
        extension(&name.to_ascii_lowercase()).as_str(),
        "svg" | "svgz" | "html" | "htm" | "xhtml" | "xml" | "xsl" | "js" | "mjs"
    )
}

fn percent_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn extension(lower_name: &str) -> String {
    Path::new(lower_name)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_string()
}

fn parent_rel(rel: &str) -> Option<String> {
    if rel.is_empty() {
        return None;
    }
    match rel.rfind('/') {
        Some(index) => Some(rel[..index].to_string()),
        None => Some(String::new()),
    }
}

fn clean_parts(rel: &str) -> Result<Vec<String>, FileError> {
    let rel = rel.trim().replace('\\', "/");
    if rel.contains('\0') || rel.contains(':') || rel.starts_with('/') {
        return Err(FileError::Forbidden);
    }
    let mut parts = Vec::new();
    for comp in rel.split('/') {
        if comp.is_empty() || comp == "." {
            continue;
        }
        if comp == ".." {
            if parts.pop().is_none() {
                return Err(FileError::Forbidden);
            }
            continue;
        }
        parts.push(comp.to_string());
    }
    Ok(parts)
}

fn valid_new_component(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name.len() <= 200
        && !name.ends_with('.')
        && !name.ends_with(' ')
        && !name.contains(['/', '\\', ':', '\0'])
        && !name.chars().any(|c| c.is_control())
}

fn unique_path(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let ext = path
        .extension()
        .map(|ext| format!(".{}", ext.to_string_lossy()))
        .unwrap_or_default();
    let parent = path.parent().unwrap_or(Path::new("."));
    for index in 2..10_000 {
        let candidate = parent.join(format!("{stem} ({index}){ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    path
}

fn modified_secs(meta: &Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn entry_hidden(path: &Path, name: &str) -> bool {
    if name.starts_with('.') {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        if let Ok(meta) = path.symlink_metadata() {
            if meta.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0 {
                return true;
            }
        }
    }
    false
}

fn map_io(err: io::Error) -> FileError {
    match err.kind() {
        io::ErrorKind::NotFound => FileError::NotFound,
        io::ErrorKind::PermissionDenied => FileError::Io("Permission denied"),
        io::ErrorKind::AlreadyExists => FileError::AlreadyExists,
        _ => FileError::Io("The file operation failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "ownnas-files-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(path.join("sub")).unwrap();
        fs::write(path.join("sub").join("note.txt"), b"hello").unwrap();
        path.canonicalize().unwrap()
    }

    #[test]
    fn rejects_paths_that_leave_the_share() {
        let root = scratch();
        assert!(resolve(&root, "..").is_err());
        assert!(resolve(&root, "sub/../../etc").is_err());
        assert!(resolve(&root, "/etc/passwd").is_err());
        assert!(resolve(&root, "C:/Windows").is_err());
        assert!(resolve(&root, "sub\\..\\..\\secret").is_err());
        assert_eq!(resolve(&root, "sub/../sub").unwrap().rel, "sub");
        assert!(resolve(&root, "sub/note.txt").is_ok());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn lists_children() {
        let root = scratch();
        let listing = list_dir(&root, "", false, false).unwrap();
        assert_eq!(listing.entries.len(), 1);
        assert_eq!(listing.entries[0].name, "sub");
        let nested = list_dir(&root, "sub", false, false).unwrap();
        assert_eq!(nested.entries[0].kind, "text");
        let _ = fs::remove_dir_all(&root);
    }
}
