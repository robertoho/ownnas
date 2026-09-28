use std::fs::{self, Metadata};
use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::Serialize;
use sha2::{Digest, Sha256};

pub const MAX_LIST: usize = 5_000;

#[derive(Debug)]
pub enum FileError {
    Forbidden,
    NotFound,
    NotADirectory,
    IsADirectory,
    AlreadyExists,
    InvalidName,
    Rejected(&'static str),
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub name: String,
    pub path: String,
    pub dir: bool,
    pub kind: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub bytes: u64,
    pub files: u64,
    pub truncated: bool,
}

pub fn search(root: &Path, rel: &str, query: &str) -> Result<Vec<SearchHit>, FileError> {
    let query = query.trim().to_lowercase();
    if query.chars().count() < 2 {
        return Err(FileError::Rejected("Type at least 2 characters"));
    }
    let start = resolve(root, rel)?;
    if !start.full.is_dir() {
        return Err(FileError::NotADirectory);
    }
    let root = root.canonicalize().map_err(map_io)?;
    let mut hits = Vec::new();
    let mut seen = 0usize;
    walk_search(&root, &start.full, &start.rel, &query, &mut hits, &mut seen);
    Ok(hits)
}

fn walk_search(
    root: &Path,
    dir: &Path,
    rel: &str,
    query: &str,
    hits: &mut Vec<SearchHit>,
    seen: &mut usize,
) {
    if hits.len() >= 100 || *seen >= 8_000 {
        return;
    }
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for item in entries {
        if hits.len() >= 100 || *seen >= 8_000 {
            return;
        }
        let Ok(item) = item else { continue };
        *seen += 1;
        let name = item.file_name().to_string_lossy().to_string();
        let child = item.path();
        let canon = match child.canonicalize() {
            Ok(path) if path.starts_with(root) => path,
            _ => continue,
        };
        let child_rel = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        let dir = canon.is_dir();
        if name.to_lowercase().contains(query) {
            hits.push(SearchHit {
                kind: if dir {
                    "folder".to_string()
                } else {
                    kind_of(&name).to_string()
                },
                name,
                path: child_rel.clone(),
                dir,
            });
        }
        if dir {
            walk_search(root, &canon, &child_rel, query, hits, seen);
        }
    }
}

pub fn usage(root: &Path, rel: &str) -> Result<Usage, FileError> {
    let start = resolve(root, rel)?;
    if !start.full.is_dir() {
        return Err(FileError::NotADirectory);
    }
    let root = root.canonicalize().map_err(map_io)?;
    let mut usage = Usage {
        bytes: 0,
        files: 0,
        truncated: false,
    };
    walk_usage(&root, &start.full, &mut usage);
    Ok(usage)
}

fn walk_usage(root: &Path, dir: &Path, usage: &mut Usage) {
    if usage.files >= 20_000 {
        usage.truncated = true;
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else { return };
    for item in entries {
        if usage.files >= 20_000 {
            usage.truncated = true;
            return;
        }
        let Ok(item) = item else { continue };
        let canon = match item.path().canonicalize() {
            Ok(path) if path.starts_with(root) => path,
            _ => continue,
        };
        if canon.is_dir() {
            walk_usage(root, &canon, usage);
        } else {
            usage.files += 1;
            usage.bytes += canon.metadata().map(|meta| meta.len()).unwrap_or(0);
        }
    }
}

pub fn duplicate(root: &Path, rel: &str) -> Result<String, FileError> {
    let src = resolve(root, rel)?;
    if src.rel.is_empty() {
        return Err(FileError::Forbidden);
    }
    let parent = src.full.parent().ok_or(FileError::Forbidden)?;
    let name = src
        .full
        .file_name()
        .ok_or(FileError::InvalidName)?;
    let dest = unique_path(parent.join(name));
    let mut copied = 0usize;
    copy_path(&src.full, &dest, &mut copied)?;
    let file_name = dest
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .ok_or(FileError::InvalidName)?;
    Ok(match parent_rel(&src.rel) {
        Some(parent_rel) if !parent_rel.is_empty() => format!("{parent_rel}/{file_name}"),
        _ => file_name,
    })
}

pub fn move_entry(root: &Path, rel: &str, dest_rel: &str) -> Result<String, FileError> {
    let src = resolve(root, rel)?;
    if src.rel.is_empty() {
        return Err(FileError::Forbidden);
    }
    let dest_dir = resolve(root, dest_rel)?;
    if !dest_dir.full.is_dir() {
        return Err(FileError::NotADirectory);
    }
    if dest_dir.full.starts_with(&src.full) {
        return Err(FileError::Rejected("A folder cannot be moved inside itself"));
    }
    if src.full.parent() == Some(dest_dir.full.as_path()) {
        return Err(FileError::Rejected("That item is already in this folder"));
    }
    let name = src.full.file_name().ok_or(FileError::InvalidName)?;
    let target = unique_path(dest_dir.full.join(name));
    if fs::rename(&src.full, &target).is_err() {
        let mut copied = 0usize;
        copy_path(&src.full, &target, &mut copied)?;
        if src.full.is_dir() {
            fs::remove_dir_all(&src.full).map_err(map_io)?;
        } else {
            fs::remove_file(&src.full).map_err(map_io)?;
        }
    }
    let file_name = target
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .ok_or(FileError::InvalidName)?;
    Ok(if dest_dir.rel.is_empty() {
        file_name
    } else {
        format!("{}/{file_name}", dest_dir.rel)
    })
}

pub fn sha256_file(root: &Path, rel: &str) -> Result<String, FileError> {
    let resolved = resolve(root, rel)?;
    if resolved.full.is_dir() {
        return Err(FileError::IsADirectory);
    }
    let meta = fs::metadata(&resolved.full).map_err(map_io)?;
    if meta.len() > 2 * 1024 * 1024 * 1024 {
        return Err(FileError::Rejected("That file is too large to hash here"));
    }
    let mut file = fs::File::open(&resolved.full).map_err(map_io)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let read = io::Read::read(&mut file, &mut buf).map_err(|_| FileError::Io("Could not read the file"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub fn write_zip(root: &Path, rel: &str, output: &Path) -> Result<String, FileError> {
    let start = resolve(root, rel)?;
    if !start.full.is_dir() {
        return Err(FileError::NotADirectory);
    }
    let root = root.canonicalize().map_err(map_io)?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).map_err(map_io)?;
    }
    let file = fs::File::create(output).map_err(map_io)?;
    let mut zip = zip::ZipWriter::new(file);
    let mut count = 0usize;
    zip_tree(&root, &start.full, "", &mut zip, &mut count)?;
    zip.finish()
        .map_err(|_| FileError::Io("Could not finish the zip"))?;
    let label = if start.rel.is_empty() {
        root_label(&root)
    } else {
        start
            .full
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "folder".to_string())
    };
    Ok(format!("{label}.zip"))
}

fn zip_tree(
    root: &Path,
    dir: &Path,
    prefix: &str,
    zip: &mut zip::ZipWriter<fs::File>,
    count: &mut usize,
) -> Result<(), FileError> {
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for item in fs::read_dir(dir).map_err(map_io)? {
        let item = item.map_err(map_io)?;
        if *count >= 2_000 {
            return Err(FileError::Rejected("That folder is too large to download as one zip"));
        }
        let name = item.file_name().to_string_lossy().to_string();
        let canon = match item.path().canonicalize() {
            Ok(path) if path.starts_with(root) => path,
            _ => continue,
        };
        let zip_name = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{}", item.file_name().to_string_lossy())
        };
        *count += 1;
        if canon.is_dir() {
            zip.add_directory(format!("{zip_name}/"), options)
                .map_err(|_| FileError::Io("Could not build the zip"))?;
            zip_tree(root, &canon, &zip_name, zip, count)?;
        } else {
            zip.start_file(&zip_name, options)
                .map_err(|_| FileError::Io("Could not build the zip"))?;
            let mut input = fs::File::open(&canon).map_err(map_io)?;
            io::copy(&mut input, zip).map_err(|_| FileError::Io("Could not build the zip"))?;
        }
    }
    Ok(())
}

fn copy_path(src: &Path, dest: &Path, copied: &mut usize) -> Result<(), FileError> {
    if *copied >= 2_000 {
        return Err(FileError::Rejected("That item is too large to copy in one step"));
    }
    if src.is_dir() {
        fs::create_dir(dest).map_err(map_io)?;
        for item in fs::read_dir(src).map_err(map_io)? {
            let item = item.map_err(map_io)?;
            *copied += 1;
            copy_path(&item.path(), &dest.join(item.file_name()), copied)?;
        }
    } else {
        *copied += 1;
        fs::copy(src, dest).map_err(|_| FileError::Io("Could not copy the file"))?;
    }
    Ok(())
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

    #[test]
    fn searches_duplicates_and_measures() {
        let root = scratch();
        fs::write(root.join("sub").join("Alpha Note.txt"), b"abc").unwrap();
        let hits = search(&root, "", "alpha").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "Alpha Note.txt");
        let copy = duplicate(&root, "sub/Alpha Note.txt").unwrap();
        assert!(copy.contains("Alpha Note"));
        assert_ne!(copy, "sub/Alpha Note.txt");
        let usage = usage(&root, "").unwrap();
        assert!(usage.bytes >= 3);
        let _ = fs::remove_dir_all(&root);
    }
}
