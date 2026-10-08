use std::collections::HashSet;
use std::fs::{self, Metadata};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use serde::Serialize;
use sha2::{Digest, Sha256};

pub const MAX_LIST: usize = 5_000;
const ARCHIVE_DIR: &str = ".ownnas-archive";
const ARCHIVE_MARK: &str = ".ownnas-id";
const LEGACY_ARCHIVE_DIR: &str = "Archive";
const LEGACY_ARCHIVE_MARK: &str = ".ownnas-archive";
const ARCHIVE_LOG: &str = "archive.log";
pub const TRASH_DIR: &str = ".ownnas-trash";
const TRASH_MARK: &str = ".ownnas-trash-id";
const LEGACY_TRASH_DIR: &str = "Trash";
const LEGACY_TRASH_MARK: &str = ".ownnas-trash";
const TRASH_META_DIR: &str = ".meta";
const TRASH_LOG: &str = "trash.log";

#[derive(Debug)]
pub enum FileError {
    Forbidden,
    NotFound,
    NotADirectory,
    IsADirectory,
    AlreadyExists,
    InvalidName,
    Cancelled,
    Rejected(&'static str),
    Io(&'static str),
}

/// Cooperative cancel flag for long walks (compress, usage, duplicates, zip).
#[derive(Clone, Default)]
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    fn check(&self) -> Result<(), FileError> {
        if self.is_cancelled() {
            Err(FileError::Cancelled)
        } else {
            Ok(())
        }
    }
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
    pub created: i64,
    pub kind: String,
    pub thumb: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Cached recursive folder size when still valid.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measured_size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measured_truncated: Option<bool>,
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
        let child_path = item.path();
        if name.is_empty()
            || name == "."
            || name == ".."
            || name == ARCHIVE_MARK
            || name == TRASH_MARK
            || name == TRASH_META_DIR
            || name == TRASH_LOG
            || name == ARCHIVE_LOG
            || (name == LEGACY_ARCHIVE_MARK && !child_path.is_dir())
            || (name == LEGACY_TRASH_MARK && !child_path.is_dir())
        {
            continue;
        }
        let special = child_path.is_dir()
            && (is_archive_folder(&child_path) || is_trash_folder(&child_path));
        // Dotfolders are normally hidden, but OwnNAS Trash/Archive stay visible as special folders.
        if !show_hidden && !special && entry_hidden(&child_path, &name) {
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
        let kind = if dir {
            if is_trash_folder(&child_path) {
                "trash-folder".to_string()
            } else if is_archive_folder(&child_path) {
                "archive-folder".to_string()
            } else {
                "folder".to_string()
            }
        } else {
            kind_of(&name).to_string()
        };
        let original = if is_under_trash(&path) && !is_trash_root(&path) {
            trash_original(root, &name)
        } else {
            None
        };
        entries.push(Entry {
            kind,
            thumb: !dir && can_thumb(&name, ffmpeg),
            name,
            path,
            dir,
            size: if dir { 0 } else { meta.len() },
            modified: modified_secs(&meta),
            created: created_secs(&meta),
            original,
            tags: Vec::new(),
            measured_size: None,
            measured_truncated: None,
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

/// Creates a set of folders from an uploaded directory tree. Existing folders are
/// accepted so merged folder uploads can safely include paths already on disk.
pub fn ensure_upload_dirs(root: &Path, parent_rel: &str, names: &[String]) -> Result<(), FileError> {
    let root = root.canonicalize().map_err(map_io)?;
    let parent = resolve(&root, parent_rel)?;
    if !parent.full.is_dir() {
        return Err(FileError::NotADirectory);
    }

    for name in names {
        if name
            .split(['/', '\\'])
            .any(|part| part == "." || part == "..")
        {
            return Err(FileError::InvalidName);
        }
        let parts = clean_parts(name)?;
        if parts.is_empty() || parts.iter().any(|part| !valid_new_component(part)) {
            return Err(FileError::InvalidName);
        }
        let mut cursor = parent.full.clone();
        for part in parts {
            let next = cursor.join(part);
            if next.exists() {
                let canon = next.canonicalize().map_err(map_io)?;
                if !canon.starts_with(&root) {
                    return Err(FileError::Forbidden);
                }
                if !canon.is_dir() {
                    return Err(FileError::AlreadyExists);
                }
                cursor = canon;
                continue;
            }
            match fs::create_dir(&next) {
                Ok(()) => cursor = next,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    let canon = next.canonicalize().map_err(map_io)?;
                    if !canon.starts_with(&root) {
                        return Err(FileError::Forbidden);
                    }
                    if !canon.is_dir() {
                        return Err(FileError::AlreadyExists);
                    }
                    cursor = canon;
                }
                Err(error) => return Err(map_io(error)),
            }
        }
    }
    Ok(())
}

/// Creates a new empty-ish file of a supported kind in `parent_rel`.
/// Returns the relative path of the created file.
pub fn create_file(root: &Path, parent_rel: &str, name: &str, kind: &str) -> Result<String, FileError> {
    let ext = match kind {
        "md" | "csv" | "txt" | "json" | "html" | "docx" | "odt" => kind,
        _ => return Err(FileError::Rejected("Unsupported file type")),
    };
    let name = finalize_new_filename(name, ext)?;
    if !valid_new_component(&name) {
        return Err(FileError::InvalidName);
    }
    let parent = resolve(root, parent_rel)?;
    if !parent.full.is_dir() {
        return Err(FileError::NotADirectory);
    }
    let dest = parent.full.join(&name);
    if dest.exists() {
        return Err(FileError::AlreadyExists);
    }
    let stem = Path::new(&name)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "Untitled".to_string());
    let body = match ext {
        "md" => format!("# {stem}\n\n"),
        "csv" => String::new(),
        "txt" => String::new(),
        "json" => "{\n  \n}\n".to_string(),
        "html" => format!(
            "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n  <meta charset=\"utf-8\">\n  <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n  <title>{stem}</title>\n</head>\n<body>\n  <h1>{stem}</h1>\n</body>\n</html>\n"
        ),
        _ => String::new(),
    };
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&dest)
        .map_err(|err| {
            if err.kind() == io::ErrorKind::AlreadyExists {
                FileError::AlreadyExists
            } else {
                map_io(err)
            }
        })?;
    if ext == "docx" { file.write_all(&blank_docx()?).map_err(map_io)?; }
    else if ext == "odt" { file.write_all(&blank_odt()?).map_err(map_io)?; }
    else { file.write_all(body.as_bytes()).map_err(map_io)?; }
    Ok(if parent.rel.is_empty() {
        name
    } else {
        format!("{}/{}", parent.rel, name)
    })
}

/// Overwrites a text file with UTF-8 content (Markdown, CSV, source, etc.).
pub fn write_text_file(root: &Path, rel: &str, content: &str) -> Result<(), FileError> {
    const MAX_BYTES: usize = 1024 * 1024;
    if content.len() > MAX_BYTES {
        return Err(FileError::Rejected(
            "This file is too large to save in the editor (1 MB max)",
        ));
    }
    if content.contains('\0') {
        return Err(FileError::Rejected("Binary content cannot be saved here"));
    }
    let target = resolve(root, rel)?;
    if target.rel.is_empty() {
        return Err(FileError::Forbidden);
    }
    if target.full.is_dir() {
        return Err(FileError::IsADirectory);
    }
    let name = target
        .full
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    if kind_of(&name) != "text" {
        return Err(FileError::Rejected("Only text files can be edited"));
    }
    let parent = target.full.parent().ok_or(FileError::Forbidden)?;
    let tmp = parent.join(format!(
        ".ownnas-write-{}-{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    fs::write(&tmp, content.as_bytes()).map_err(map_io)?;
    if target.full.exists() {
        fs::remove_file(&target.full).map_err(map_io)?;
    }
    if let Err(err) = fs::rename(&tmp, &target.full) {
        let _ = fs::remove_file(&tmp);
        return Err(map_io(err));
    }
    Ok(())
}

pub fn rename_entry(root: &Path, rel: &str, new_name: &str) -> Result<String, FileError> {
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
    fs::rename(&src.full, &dest).map_err(map_io)?;
    Ok(match parent_rel(&src.rel) {
        Some(parent) if !parent.is_empty() => format!("{parent}/{new_name}"),
        _ => new_name.to_string(),
    })
}

pub fn remove_entry(root: &Path, rel: &str) -> Result<(), FileError> {
    let target = resolve(root, rel)?;
    if target.rel.is_empty() {
        return Err(FileError::Forbidden);
    }
    if is_trash_root(&target.rel) {
        return Err(FileError::Rejected(
            "Empty the Trash instead of deleting the Trash folder",
        ));
    }
    if target.full.is_dir() {
        fs::remove_dir_all(&target.full).map_err(map_io)
    } else {
        fs::remove_file(&target.full).map_err(map_io)
    }?;
    if is_under_trash(&target.rel) {
        let _ = remove_trash_meta(root, &target.rel);
    }
    Ok(())
}

/// Moves an item into the shared-folder Trash. Items already in Trash are removed for good.
/// Returns `(action, new_path)` where `new_path` is set when the item was moved into Trash.
pub fn trash_or_delete(root: &Path, rel: &str) -> Result<(&'static str, Option<String>), FileError> {
    if is_under_trash(rel) {
        remove_entry(root, rel)?;
        Ok(("deleted", None))
    } else {
        let path = trash_entry(root, rel)?;
        Ok(("trashed", Some(path)))
    }
}

pub fn trash_entry(root: &Path, rel: &str) -> Result<String, FileError> {
    let src = resolve(root, rel)?;
    if src.rel.is_empty() {
        return Err(FileError::Forbidden);
    }
    if is_trash_folder(&src.full) || is_trash_root(&src.rel) {
        return Err(FileError::Rejected(
            "The Trash folder cannot be moved into itself",
        ));
    }
    if is_under_trash(&src.rel) {
        return Err(FileError::Rejected("That item is already in the Trash"));
    }
    let trash = ensure_trash_dir(root)?;
    let name = src
        .full
        .file_name()
        .ok_or(FileError::InvalidName)?
        .to_string_lossy()
        .to_string();
    let dest = unique_path(trash.join(&name));
    let saved_as = dest
        .file_name()
        .map(|item| item.to_string_lossy().to_string())
        .ok_or(FileError::InvalidName)?;
    if fs::rename(&src.full, &dest).is_err() {
        let mut copied = 0usize;
        copy_path(&src.full, &dest, &mut copied)?;
        if src.full.is_dir() {
            fs::remove_dir_all(&src.full).map_err(map_io)?;
        } else {
            fs::remove_file(&src.full).map_err(map_io)?;
        }
    }
    write_trash_meta(root, &saved_as, &src.rel, &name)?;
    append_trash_log(root, &trash, &src.rel, &saved_as)?;
    Ok(format!("{TRASH_DIR}/{saved_as}"))
}

pub fn restore_entry(root: &Path, rel: &str) -> Result<String, FileError> {
    let src = resolve(root, rel)?;
    if !is_under_trash(&src.rel) || is_trash_root(&src.rel) {
        return Err(FileError::Rejected("Only items in the Trash can be restored"));
    }
    if src.rel.matches('/').count() != 1 {
        return Err(FileError::Rejected("Restore items from the Trash root"));
    }
    let saved_as = src
        .full
        .file_name()
        .ok_or(FileError::InvalidName)?
        .to_string_lossy()
        .to_string();
    let meta = read_trash_meta(root, &saved_as)?
        .ok_or_else(|| FileError::Rejected("That Trash item has no restore information"))?;
    let original = clean_parts(&meta.original)?;
    if original.is_empty() {
        return Err(FileError::Forbidden);
    }
    let root_canon = root.canonicalize().map_err(map_io)?;
    let mut dest = root_canon.clone();
    for (index, part) in original.iter().enumerate() {
        if !valid_new_component(part) {
            return Err(FileError::InvalidName);
        }
        let is_last = index + 1 == original.len();
        dest.push(part);
        if is_last {
            break;
        }
        if dest.exists() {
            if !dest.is_dir() {
                return Err(FileError::AlreadyExists);
            }
        } else {
            fs::create_dir(&dest).map_err(map_io)?;
        }
    }
    let dest = unique_path(dest);
    if !dest.starts_with(&root_canon) {
        return Err(FileError::Forbidden);
    }
    if fs::rename(&src.full, &dest).is_err() {
        let mut copied = 0usize;
        copy_path(&src.full, &dest, &mut copied)?;
        if src.full.is_dir() {
            fs::remove_dir_all(&src.full).map_err(map_io)?;
        } else {
            fs::remove_file(&src.full).map_err(map_io)?;
        }
    }
    let _ = remove_trash_meta(root, &src.rel);
    let file_name = dest
        .file_name()
        .map(|item| item.to_string_lossy().to_string())
        .ok_or(FileError::InvalidName)?;
    let parent = meta
        .original
        .rsplit_once('/')
        .map(|(parent, _)| parent.to_string())
        .unwrap_or_default();
    Ok(if parent.is_empty() {
        file_name
    } else {
        format!("{parent}/{file_name}")
    })
}

pub fn empty_trash(root: &Path) -> Result<u32, FileError> {
    let trash = ensure_trash_dir(root)?;
    let mut removed = 0u32;
    for item in fs::read_dir(&trash).map_err(map_io)? {
        let item = item.map_err(map_io)?;
        let name = item.file_name().to_string_lossy().to_string();
        if name == TRASH_MARK || name == TRASH_META_DIR || name == TRASH_LOG {
            continue;
        }
        let path = item.path();
        if path.is_dir() {
            fs::remove_dir_all(&path).map_err(map_io)?;
        } else {
            fs::remove_file(&path).map_err(map_io)?;
        }
        removed += 1;
    }
    let meta_dir = trash.join(TRASH_META_DIR);
    if meta_dir.is_dir() {
        for item in fs::read_dir(&meta_dir).map_err(map_io)? {
            let item = item.map_err(map_io)?;
            let _ = fs::remove_file(item.path());
        }
    }
    Ok(removed)
}

#[derive(Serialize, serde::Deserialize)]
struct TrashMeta {
    original: String,
    name: String,
    saved_as: String,
    deleted_at: i64,
}

#[derive(Clone, Copy)]
pub enum UploadChoice {
    Overwrite,
    KeepBoth,
    ArchiveOlder { incoming_modified: i64 },
    ArchiveExisting,
    Ignore,
}

#[derive(Clone, Copy)]
pub struct ExistingFile {
    pub dir: bool,
    pub modified: i64,
    pub size: u64,
}

pub enum PreparedUpload {
    Write(PathBuf),
    /// The upload itself is the older file, so it is stored in `.ownnas-archive`.
    /// The log line is written after the bytes are saved.
    StoreInArchive { path: PathBuf, original_name: String },
    Skip,
    Ask(ExistingFile),
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadStatus {
    pub name: String,
    pub exists: bool,
    pub dir: bool,
    pub modified: i64,
    pub size: u64,
}

struct LocatedUpload {
    root: PathBuf,
    full: PathBuf,
    pending_dirs: Vec<PathBuf>,
    existing: Option<ExistingFile>,
}

/// Reports whether an upload name already exists. Does not create folders.
pub fn upload_status(root: &Path, dir_rel: &str, filename: &str) -> Result<UploadStatus, FileError> {
    let located = locate_upload(root, dir_rel, filename)?;
    let existing = located.existing.unwrap_or(ExistingFile {
        dir: false,
        modified: 0,
        size: 0,
    });
    Ok(UploadStatus {
        name: filename.to_string(),
        exists: located.existing.is_some(),
        dir: existing.dir,
        modified: existing.modified,
        size: existing.size,
    })
}

/// Chooses where an uploaded file will be written.
/// Intermediate folders in `filename` (from a folder upload) are created only when writing.
/// Without a choice, an existing file is returned as `Ask` so the caller can prompt.
pub fn prepare_upload(
    root: &Path,
    dir_rel: &str,
    filename: &str,
    choice: Option<UploadChoice>,
) -> Result<PreparedUpload, FileError> {
    let located = locate_upload(root, dir_rel, filename)?;
    let Some(existing) = located.existing else {
        create_dirs(&located.pending_dirs)?;
        return Ok(PreparedUpload::Write(located.full));
    };
    let Some(choice) = choice else {
        return Ok(PreparedUpload::Ask(existing));
    };
    if existing.dir && !matches!(choice, UploadChoice::KeepBoth | UploadChoice::Ignore) {
        return Err(FileError::Rejected(
            "A folder with that name already exists. Keep both or ignore it.",
        ));
    }
    match choice {
        UploadChoice::Ignore => Ok(PreparedUpload::Skip),
        UploadChoice::KeepBoth => {
            create_dirs(&located.pending_dirs)?;
            Ok(PreparedUpload::Write(unique_path(located.full)))
        }
        UploadChoice::Overwrite => {
            create_dirs(&located.pending_dirs)?;
            Ok(PreparedUpload::Write(located.full))
        }
        UploadChoice::ArchiveExisting => {
            move_to_archive(&located.root, &located.full)?;
            Ok(PreparedUpload::Write(located.full))
        }
        UploadChoice::ArchiveOlder { incoming_modified } => {
            if existing.modified > incoming_modified {
                let dest = archived_copy_path(&located.root, &located.full)?;
                let original_name = located
                    .full
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| "file".to_string());
                Ok(PreparedUpload::StoreInArchive {
                    path: dest,
                    original_name,
                })
            } else {
                move_to_archive(&located.root, &located.full)?;
                Ok(PreparedUpload::Write(located.full))
            }
        }
    }
}

fn locate_upload(root: &Path, dir_rel: &str, filename: &str) -> Result<LocatedUpload, FileError> {
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
    let mut pending_dirs = Vec::new();
    for (index, part) in extra.iter().enumerate() {
        let is_last = index + 1 == extra.len();
        let next = cursor.join(part);
        if next.exists() {
            let canon = next.canonicalize().map_err(map_io)?;
            if !canon.starts_with(&root) {
                return Err(FileError::Forbidden);
            }
            if is_last {
                let meta = canon.metadata().map_err(map_io)?;
                let dir = canon.is_dir();
                return Ok(LocatedUpload {
                    root,
                    full: canon,
                    pending_dirs,
                    existing: Some(ExistingFile {
                        dir,
                        modified: modified_secs(&meta),
                        size: if dir { 0 } else { meta.len() },
                    }),
                });
            }
            if !canon.is_dir() {
                return Err(FileError::AlreadyExists);
            }
            cursor = canon;
            continue;
        }
        if is_last {
            return Ok(LocatedUpload {
                root,
                full: next,
                pending_dirs,
                existing: None,
            });
        }
        pending_dirs.push(next.clone());
        cursor = next;
    }
    Err(FileError::InvalidName)
}

fn create_dirs(dirs: &[PathBuf]) -> Result<(), FileError> {
    for dir in dirs {
        if dir.exists() {
            if !dir.is_dir() {
                return Err(FileError::AlreadyExists);
            }
            continue;
        }
        fs::create_dir(dir).map_err(map_io)?;
    }
    Ok(())
}

fn ensure_archive_dir(root: &Path, parent: &Path) -> Result<PathBuf, FileError> {
    let archive = parent.join(ARCHIVE_DIR);
    let legacy = parent.join(LEGACY_ARCHIVE_DIR);
    if !archive.exists() && legacy.is_dir() && is_archive_folder(&legacy) {
        fs::rename(&legacy, &archive).map_err(map_io)?;
    }
    if archive.exists() {
        let canon = archive.canonicalize().map_err(map_io)?;
        if !canon.starts_with(root) || !canon.is_dir() {
            return Err(FileError::Rejected("Could not archive into that folder"));
        }
        write_archive_mark(&canon)?;
        return Ok(canon);
    }
    fs::create_dir(&archive).map_err(map_io)?;
    let canon = archive.canonicalize().map_err(map_io)?;
    if !canon.starts_with(root) {
        let _ = fs::remove_dir(&archive);
        return Err(FileError::Forbidden);
    }
    if let Err(err) = write_archive_mark(&canon) {
        let _ = fs::remove_dir_all(&canon);
        return Err(err);
    }
    Ok(canon)
}

fn write_archive_mark(archive: &Path) -> Result<(), FileError> {
    let mark = archive.join(ARCHIVE_MARK);
    if mark.is_file() {
        return Ok(());
    }
    // Older builds used `.ownnas-archive` as the marker file inside `Archive/`.
    let legacy_mark = archive.join(LEGACY_ARCHIVE_MARK);
    if legacy_mark.is_file() {
        return Ok(());
    }
    fs::write(
        &mark,
        "OwnNAS archive folder\nThis file marks the folder as an OwnNAS Archive.\n",
    )
    .map_err(map_io)
}

fn is_trash_folder(path: &Path) -> bool {
    if !path.is_dir() {
        return false;
    }
    let name = path.file_name().and_then(|name| name.to_str());
    if matches!(name, Some(ARCHIVE_DIR) | Some(LEGACY_ARCHIVE_DIR)) {
        return false;
    }
    if matches!(name, Some(TRASH_DIR) | Some(LEGACY_TRASH_DIR)) {
        return true;
    }
    path.join(TRASH_MARK).is_file() || path.join(LEGACY_TRASH_MARK).is_file()
}

fn is_archive_folder(path: &Path) -> bool {
    if !path.is_dir() {
        return false;
    }
    let name = path.file_name().and_then(|name| name.to_str());
    if matches!(name, Some(TRASH_DIR) | Some(LEGACY_TRASH_DIR)) {
        return false;
    }
    if matches!(name, Some(ARCHIVE_DIR) | Some(LEGACY_ARCHIVE_DIR)) {
        return true;
    }
    path.join(ARCHIVE_MARK).is_file() || path.join(LEGACY_ARCHIVE_MARK).is_file()
}

fn ensure_trash_dir(root: &Path) -> Result<PathBuf, FileError> {
    let root = root.canonicalize().map_err(map_io)?;
    let trash = root.join(TRASH_DIR);
    let legacy = root.join(LEGACY_TRASH_DIR);
    if !trash.exists() && legacy.is_dir() && is_trash_folder(&legacy) {
        fs::rename(&legacy, &trash).map_err(map_io)?;
    }
    if trash.exists() {
        let canon = trash.canonicalize().map_err(map_io)?;
        if !canon.starts_with(&root) || !canon.is_dir() {
            return Err(FileError::Rejected("Could not open the Trash folder"));
        }
        write_trash_mark(&canon)?;
        fs::create_dir_all(canon.join(TRASH_META_DIR)).map_err(map_io)?;
        return Ok(canon);
    }
    fs::create_dir(&trash).map_err(map_io)?;
    let canon = trash.canonicalize().map_err(map_io)?;
    if !canon.starts_with(&root) {
        let _ = fs::remove_dir(&trash);
        return Err(FileError::Forbidden);
    }
    write_trash_mark(&canon)?;
    fs::create_dir_all(canon.join(TRASH_META_DIR)).map_err(map_io)?;
    Ok(canon)
}

fn write_trash_mark(trash: &Path) -> Result<(), FileError> {
    let mark = trash.join(TRASH_MARK);
    if mark.is_file() {
        return Ok(());
    }
    // Older builds used `.ownnas-trash` as the marker file inside `Trash/`.
    let legacy_mark = trash.join(LEGACY_TRASH_MARK);
    if legacy_mark.is_file() {
        return Ok(());
    }
    fs::write(
        &mark,
        "OwnNAS trash folder\nThis file marks the folder as the OwnNAS Trash.\n",
    )
    .map_err(map_io)
}

fn is_trash_root(rel: &str) -> bool {
    rel == TRASH_DIR || rel == LEGACY_TRASH_DIR
}

fn is_under_trash(rel: &str) -> bool {
    is_trash_root(rel)
        || rel.starts_with(&format!("{TRASH_DIR}/"))
        || rel.starts_with(&format!("{LEGACY_TRASH_DIR}/"))
}

/// Renames legacy `Trash` / root `Archive` folders to the dotted OwnNAS names.
/// Returns `(from, to)` pairs for database path rewrites.
pub fn migrate_legacy_dirs(root: &Path) -> Result<Vec<(String, String)>, FileError> {
    let root = root.canonicalize().map_err(map_io)?;
    let mut moved = Vec::new();
    let trash = root.join(TRASH_DIR);
    let legacy_trash = root.join(LEGACY_TRASH_DIR);
    if !trash.exists() && legacy_trash.is_dir() && is_trash_folder(&legacy_trash) {
        fs::rename(&legacy_trash, &trash).map_err(map_io)?;
        moved.push((LEGACY_TRASH_DIR.to_string(), TRASH_DIR.to_string()));
    }
    Ok(moved)
}

fn trash_meta_path(root: &Path, saved_as: &str) -> Result<PathBuf, FileError> {
    let trash = ensure_trash_dir(root)?;
    let mut hasher = Sha256::new();
    hasher.update(saved_as.as_bytes());
    Ok(trash
        .join(TRASH_META_DIR)
        .join(format!("{}.json", hex::encode(hasher.finalize()))))
}

fn write_trash_meta(root: &Path, saved_as: &str, original: &str, name: &str) -> Result<(), FileError> {
    let path = trash_meta_path(root, saved_as)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(map_io)?;
    }
    let meta = TrashMeta {
        original: original.to_string(),
        name: name.to_string(),
        saved_as: saved_as.to_string(),
        deleted_at: now_unix(),
    };
    let body = serde_json::to_vec_pretty(&meta).map_err(|_| FileError::Io("Could not write trash metadata"))?;
    fs::write(path, body).map_err(map_io)
}

fn read_trash_meta(root: &Path, saved_as: &str) -> Result<Option<TrashMeta>, FileError> {
    let path = trash_meta_path(root, saved_as)?;
    if !path.is_file() {
        return Ok(None);
    }
    let body = fs::read(path).map_err(map_io)?;
    let meta = serde_json::from_slice(&body).map_err(|_| FileError::Io("Could not read trash metadata"))?;
    Ok(Some(meta))
}

fn remove_trash_meta(root: &Path, rel: &str) -> Result<(), FileError> {
    let saved_as = rel
        .rsplit_once('/')
        .map(|(_, name)| name)
        .unwrap_or(rel);
    let path = trash_meta_path(root, saved_as)?;
    if path.exists() {
        fs::remove_file(path).map_err(map_io)?;
    }
    Ok(())
}

fn trash_original(root: &Path, saved_as: &str) -> Option<String> {
    read_trash_meta(root, saved_as)
        .ok()
        .flatten()
        .map(|meta| meta.original)
}

fn append_trash_log(root: &Path, trash: &Path, original: &str, saved_as: &str) -> Result<(), FileError> {
    let log_path = trash.join(TRASH_LOG);
    if log_path.exists() {
        let canon = log_path.canonicalize().map_err(map_io)?;
        if !canon.starts_with(root) || canon.is_dir() {
            return Err(FileError::Rejected("Could not update the trash log"));
        }
    }
    let detail = if original.ends_with(saved_as) && original != saved_as {
        format!("{original} as {saved_as}")
    } else if original == saved_as || original.ends_with(&format!("/{saved_as}")) {
        original.to_string()
    } else {
        format!("{original} as {saved_as}")
    };
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(map_io)?;
    writeln!(file, "{}  {detail}", utc_stamp())
        .map_err(|_| FileError::Io("Could not update the trash log"))?;
    Ok(())
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn move_to_archive(root: &Path, existing: &Path) -> Result<(), FileError> {
    let parent = existing.parent().ok_or(FileError::Forbidden)?;
    let archive = ensure_archive_dir(root, parent)?;
    let name = existing.file_name().ok_or(FileError::InvalidName)?;
    let original_name = name.to_string_lossy().to_string();
    let dest = archive_destination(&archive, name);
    let saved_as = dest
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .ok_or(FileError::InvalidName)?;
    fs::rename(existing, &dest).map_err(map_io)?;
    append_archive_log(root, &archive, &original_name, &saved_as)
}

fn archived_copy_path(root: &Path, existing: &Path) -> Result<PathBuf, FileError> {
    let parent = existing.parent().ok_or(FileError::Forbidden)?;
    let archive = ensure_archive_dir(root, parent)?;
    let name = existing.file_name().ok_or(FileError::InvalidName)?;
    Ok(archive_destination(&archive, name))
}

fn archive_destination(archive: &Path, name: &std::ffi::OsStr) -> PathBuf {
    let path = unique_path(archive.join(name));
    match path.file_name().and_then(|item| item.to_str()) {
        Some(ARCHIVE_LOG) => unique_path(archive.join("archive (2).log")),
        Some(ARCHIVE_MARK) | Some(LEGACY_ARCHIVE_MARK) => {
            unique_path(archive.join("ownnas-archive-mark (2)"))
        }
        _ => path,
    }
}

/// Appends one archived file to `.ownnas-archive/archive.log`.
pub fn record_archived_file(
    root: &Path,
    archived_path: &Path,
    original_name: &str,
) -> Result<(), FileError> {
    let root = root.canonicalize().map_err(map_io)?;
    let archived_path = archived_path.canonicalize().map_err(map_io)?;
    if !archived_path.starts_with(&root) || archived_path.is_dir() {
        return Err(FileError::Forbidden);
    }
    let archive = archived_path.parent().ok_or(FileError::Forbidden)?;
    let saved_as = archived_path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .ok_or(FileError::InvalidName)?;
    append_archive_log(&root, archive, original_name, &saved_as)
}

fn append_archive_log(
    root: &Path,
    archive: &Path,
    original_name: &str,
    saved_as: &str,
) -> Result<(), FileError> {
    let log_path = archive.join(ARCHIVE_LOG);
    if log_path.exists() {
        let canon = log_path.canonicalize().map_err(map_io)?;
        if !canon.starts_with(root) || canon.is_dir() {
            return Err(FileError::Rejected("Could not update the archive log"));
        }
    }
    let original_name = one_line(original_name);
    let saved_as = one_line(saved_as);
    let detail = if original_name == saved_as {
        original_name
    } else {
        format!("{original_name} saved as {saved_as}")
    };
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(map_io)?;
    writeln!(file, "{}  {detail}", utc_stamp())
        .map_err(|_| FileError::Io("Could not update the archive log"))?;
    Ok(())
}

fn one_line(name: &str) -> String {
    name.replace(['\n', '\r'], " ")
}

fn utc_stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    format_utc(secs)
}

fn format_utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let tod = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}Z",
        hour = tod / 3600,
        minute = (tod % 3600) / 60,
        second = tod % 60
    )
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let year = if month <= 2 { y + 1 } else { y };
    (year, month, day)
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
        "xlsx" => "spreadsheet",
        "zip" | "jar" | "cbz" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "7z" | "rar" => "archive",
        "txt" | "md" | "markdown" | "json" | "csv" | "tsv" | "log" | "xml" | "yaml" | "yml"
        | "toml" | "ini" | "conf" | "cfg" | "env" | "nfo" | "properties" | "html" | "htm"
        | "css" | "js" | "mjs" | "ts" | "tsx" | "jsx" | "py" | "rs" | "go" | "java" | "c"
        | "h" | "cpp" | "hpp" | "cs" | "sh" | "bash" | "zsh" | "ps1" | "sql" | "rb" | "php"
        | "lua" | "vue" | "svelte" => "text",
        "dxf" | "dwg" => "cad2d",
        "obj" | "stl" | "gltf" | "glb" | "ply" | "3mf" | "stp" | "step" | "iges" | "igs" => {
            "model3d"
        }
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

pub(crate) fn valid_new_component(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name.len() <= 200
        && !name.ends_with('.')
        && !name.ends_with(' ')
        && !name.contains(['/', '\\', ':', '\0'])
        && !name.chars().any(|c| c.is_control())
}

fn finalize_new_filename(name: &str, ext: &str) -> Result<String, FileError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(FileError::InvalidName);
    }
    let want = format!(".{ext}");
    let lower = name.to_lowercase();
    if lower.ends_with(&want) {
        return Ok(name.to_string());
    }
    if let Some(idx) = name.rfind('.') {
        if idx > 0 {
            return Ok(format!("{}{want}", &name[..idx]));
        }
    }
    Ok(format!("{name}{want}"))
}

pub(crate) fn unique_path(path: PathBuf) -> PathBuf {
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

fn created_secs(meta: &Metadata) -> i64 {
    meta.created()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_else(|| modified_secs(meta))
}

/// Size and modified time for annotation fingerprints.
pub fn entry_stats(root: &Path, rel: &str) -> Result<(u64, i64), FileError> {
    let resolved = resolve(root, rel)?;
    let meta = fs::metadata(&resolved.full).map_err(map_io)?;
    Ok((meta.len(), modified_secs(&meta)))
}

fn entry_hidden(path: &Path, name: &str) -> bool {
    if name.starts_with('.') {
        return true;
    }
    #[cfg(not(windows))]
    let _ = path;
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

pub(crate) fn map_io(err: io::Error) -> FileError {
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
    pub size: u64,
    pub modified: i64,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_content: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub bytes: u64,
    pub files: u64,
    pub truncated: bool,
    pub cached: bool,
    pub fingerprint: String,
    pub max_mtime: i64,
}

pub struct SearchOptions {
    pub ocr: bool,
    pub cache_dir: PathBuf,
}

#[derive(Clone, Copy)]
enum SearchPhase {
    /// Names + text files + PDFs (cached). No image OCR.
    Fast,
    /// Image OCR only (uses cache when available).
    Ocr,
}

pub fn search(
    root: &Path,
    rel: &str,
    query: &str,
    options: &SearchOptions,
) -> Result<Vec<SearchHit>, FileError> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Err(FileError::Rejected("Enter a search"));
    }
    let start = resolve(root, rel)?;
    if !start.full.is_dir() {
        return Err(FileError::NotADirectory);
    }
    let root = root.canonicalize().map_err(map_io)?;
    let mut hits = Vec::new();
    // Reserve results for directory names before file/content matches can fill
    // the result cap. This keeps nested folder-name searches useful in busy
    // libraries where many files happen to match first.
    let mut folder_seen = 0usize;
    walk_folder_search(
        &root,
        &start.full,
        &start.rel,
        &query,
        &mut hits,
        &mut folder_seen,
    );
    let mut seen = 0usize;
    // Phase 1: cheap content (text + PDF cache). This is what makes a single-PDF library feel snappy.
    walk_search(
        &root,
        &start.full,
        &start.rel,
        &query,
        &mut hits,
        &mut seen,
        options,
        SearchPhase::Fast,
        &mut 0,
    );
    // Phase 2: OCR images only if enabled and we still have room for more hits.
    // First OCR runs are slow; results are cached under ownnas-data/text-cache/.
    if options.ocr && query.chars().count() >= 2 && hits.len() < 100 {
        let mut ocr_budget = 12usize;
        let mut ocr_seen = 0usize;
        walk_search(
            &root,
            &start.full,
            &start.rel,
            &query,
            &mut hits,
            &mut ocr_seen,
            options,
            SearchPhase::Ocr,
            &mut ocr_budget,
        );
    }
    Ok(hits)
}

fn content_snippet(haystack: &str, needle: &str) -> Option<String> {
    if !crate::content_index::text_matches(haystack, needle) {
        return None;
    }
    let lower = haystack.to_lowercase();
    let needle_l = needle.to_lowercase();
    let idx = lower.find(&needle_l).unwrap_or_else(|| {
        let compact_hay: String = lower.chars().filter(|c| !c.is_whitespace()).collect();
        let compact_needle: String = needle_l.chars().filter(|c| !c.is_whitespace()).collect();
        let Some(compact_idx) = compact_hay.find(&compact_needle) else {
            return 0;
        };
        let target_chars = compact_hay[..compact_idx].chars().count();
        let mut kept_chars = 0usize;
        for (byte_idx, ch) in haystack.char_indices() {
            if ch.is_whitespace() {
                continue;
            }
            if kept_chars == target_chars {
                return byte_idx;
            }
            kept_chars += 1;
        }
        0
    });
    let start = haystack[..idx]
        .char_indices()
        .rev()
        .nth(40)
        .map(|(i, _)| i)
        .unwrap_or(0);
    let end = haystack[idx..]
        .char_indices()
        .nth(80)
        .map(|(i, _)| idx + i)
        .unwrap_or(haystack.len());
    let mut snippet = haystack[start..end].replace('\n', " ").replace('\r', " ");
    while snippet.contains("  ") {
        snippet = snippet.replace("  ", " ");
    }
    let mut out = snippet.trim().to_string();
    if start > 0 {
        out.insert_str(0, "…");
    }
    if end < haystack.len() {
        out.push('…');
    }
    Some(out)
}

fn text_file_contains(path: &Path, query: &str) -> Option<String> {
    const MAX: u64 = 256 * 1024;
    let meta = fs::metadata(path).ok()?;
    if meta.len() == 0 || meta.len() > MAX {
        return None;
    }
    let mut file = fs::File::open(path).ok()?;
    let mut buf = Vec::with_capacity(meta.len() as usize);
    std::io::Read::take(&mut file, MAX)
        .read_to_end(&mut buf)
        .ok()?;
    if buf.contains(&0) {
        return None;
    }
    let text = String::from_utf8_lossy(&buf);
    if !crate::content_index::text_matches(&text, query) {
        return None;
    }
    content_snippet(&text, query)
}

fn indexed_contains(
    path: &Path,
    kind: &str,
    query: &str,
    options: &SearchOptions,
    ocr_budget: &mut usize,
) -> Option<String> {
    if kind == "image" && !options.ocr {
        return None;
    }
    let text = if kind == "image" && *ocr_budget == 0 {
        crate::content_index::cached_text(path, kind, &options.cache_dir, options.ocr)?
    } else {
        let (text, did_work) = crate::content_index::extract_for_search(
            path,
            kind,
            &options.cache_dir,
            options.ocr,
        )?;
        if kind == "image" && did_work {
            *ocr_budget = ocr_budget.saturating_sub(1);
        }
        text
    };
    if text.is_empty() || !crate::content_index::text_matches(&text, query) {
        return None;
    }
    content_snippet(&text, query)
}

fn walk_folder_search(
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
        if !canon.is_dir() {
            continue;
        }
        let child_rel = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        if name.to_lowercase().contains(query) {
            if let Ok(meta) = canon.metadata() {
                hits.push(SearchHit {
                    kind: if is_trash_folder(&canon) {
                        "trash-folder".to_string()
                    } else if is_archive_folder(&canon) {
                        "archive-folder".to_string()
                    } else {
                        "folder".to_string()
                    },
                    name: name.clone(),
                    path: child_rel.clone(),
                    dir: true,
                    size: 0,
                    modified: modified_secs(&meta),
                    matched_tag: None,
                    matched_content: None,
                });
            }
        }
        walk_folder_search(root, &canon, &child_rel, query, hits, seen);
    }
}

fn walk_search(
    root: &Path,
    dir: &Path,
    rel: &str,
    query: &str,
    hits: &mut Vec<SearchHit>,
    seen: &mut usize,
    options: &SearchOptions,
    phase: SearchPhase,
    ocr_budget: &mut usize,
) {
    if hits.len() >= 100 || *seen >= 8_000 {
        return;
    }
    if matches!(phase, SearchPhase::Ocr) && *ocr_budget == 0 {
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
        if matches!(phase, SearchPhase::Ocr) && *ocr_budget == 0 {
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
        let kind = if dir {
            if is_trash_folder(&canon) {
                "trash-folder".to_string()
            } else if is_archive_folder(&canon) {
                "archive-folder".to_string()
            } else {
                "folder".to_string()
            }
        } else {
            kind_of(&name).to_string()
        };
        let name_hit = matches!(phase, SearchPhase::Fast) && name.to_lowercase().contains(query);
        let content_hit = if !dir && !name_hit && query.chars().count() >= 2 {
            match (phase, kind.as_str()) {
                (SearchPhase::Fast, "text") => text_file_contains(&canon, query),
                (SearchPhase::Fast, "pdf") => {
                    indexed_contains(&canon, "pdf", query, options, ocr_budget)
                }
                (SearchPhase::Ocr, "image") => {
                    indexed_contains(&canon, "image", query, options, ocr_budget)
                }
                _ => None,
            }
        } else {
            None
        };
        if name_hit || content_hit.is_some() {
            // Avoid duplicate paths if OCR phase somehow overlaps (should not).
            if !hits.iter().any(|hit| hit.path == child_rel) {
                let Ok(meta) = canon.metadata() else { continue };
                hits.push(SearchHit {
                    kind,
                    name,
                    path: child_rel.clone(),
                    dir,
                    size: if dir { 0 } else { meta.len() },
                    modified: modified_secs(&meta),
                    matched_tag: None,
                    matched_content: content_hit,
                });
            }
        }
        if dir {
            walk_search(
                root,
                &canon,
                &child_rel,
                query,
                hits,
                seen,
                options,
                phase,
                ocr_budget,
            );
        }
    }
}

/// Cheap stamp of a folder's immediate children (name/size/mtime) + directory mtime.
/// External adds/removes/renames, or edits of direct children, change this.
pub fn folder_stamp(root: &Path, rel: &str) -> Result<String, FileError> {
    let start = resolve(root, rel)?;
    if !start.full.is_dir() {
        return Err(FileError::NotADirectory);
    }
    let mut hasher = Sha256::new();
    let dir_meta = fs::metadata(&start.full).map_err(map_io)?;
    hasher.update(modified_secs(&dir_meta).to_le_bytes());
    hasher.update(created_secs(&dir_meta).to_le_bytes());
    let mut children = Vec::new();
    if let Ok(entries) = fs::read_dir(&start.full) {
        for item in entries.flatten() {
            let name = item.file_name().to_string_lossy().to_string();
            if name == "." || name == ".." {
                continue;
            }
            let meta = match item.metadata() {
                Ok(meta) => meta,
                Err(_) => continue,
            };
            children.push((
                name,
                meta.is_dir(),
                if meta.is_dir() { 0 } else { meta.len() },
                modified_secs(&meta),
            ));
        }
    }
    children.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    hasher.update((children.len() as u64).to_le_bytes());
    for (name, is_dir, size, modified) in children {
        hasher.update(name.as_bytes());
        hasher.update([u8::from(is_dir)]);
        hasher.update(size.to_le_bytes());
        hasher.update(modified.to_le_bytes());
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Recursively measures a folder. When `slow` is set, the walk yields briefly
/// so background listing updates do not pin a CPU core.
pub fn usage(root: &Path, rel: &str) -> Result<Usage, FileError> {
    usage_paced(root, rel, false, &CancelFlag::new())
}

pub fn usage_paced(
    root: &Path,
    rel: &str,
    slow: bool,
    cancel: &CancelFlag,
) -> Result<Usage, FileError> {
    let start = resolve(root, rel)?;
    if !start.full.is_dir() {
        return Err(FileError::NotADirectory);
    }
    let fingerprint = folder_stamp(root, rel)?;
    let root = root.canonicalize().map_err(map_io)?;
    let mut usage = Usage {
        bytes: 0,
        files: 0,
        truncated: false,
        cached: false,
        fingerprint,
        max_mtime: 0,
    };
    let mut since_yield = 0u32;
    walk_usage(&root, &start.full, &mut usage, slow, &mut since_yield, cancel)?;
    Ok(usage)
}

fn walk_usage(
    root: &Path,
    dir: &Path,
    usage: &mut Usage,
    slow: bool,
    since_yield: &mut u32,
    cancel: &CancelFlag,
) -> Result<(), FileError> {
    cancel.check()?;
    if usage.files >= 20_000 {
        usage.truncated = true;
        return Ok(());
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(());
    };
    for item in entries {
        cancel.check()?;
        if usage.files >= 20_000 {
            usage.truncated = true;
            return Ok(());
        }
        let Ok(item) = item else { continue };
        let canon = match item.path().canonicalize() {
            Ok(path) if path.starts_with(root) => path,
            _ => continue,
        };
        if canon.is_dir() {
            walk_usage(root, &canon, usage, slow, since_yield, cancel)?;
        } else {
            usage.files += 1;
            let meta = canon.metadata().ok();
            usage.bytes += meta.as_ref().map(|m| m.len()).unwrap_or(0);
            if let Some(meta) = meta.as_ref() {
                usage.max_mtime = usage.max_mtime.max(modified_secs(meta));
            }
            if slow {
                *since_yield += 1;
                if *since_yield >= 48 {
                    *since_yield = 0;
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
        }
        if usage.truncated {
            return Ok(());
        }
    }
    Ok(())
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

pub fn move_exact(root: &Path, rel: &str, dest_rel: &str) -> Result<String, FileError> {
    let src = resolve(root, rel)?;
    if src.rel.is_empty() { return Err(FileError::Forbidden); }
    let parts = clean_parts(dest_rel)?;
    let name = parts.last().ok_or(FileError::InvalidName)?;
    if !valid_new_component(name) { return Err(FileError::InvalidName); }
    let parent = resolve(root, &parts[..parts.len()-1].join("/"))?;
    if !parent.full.is_dir() { return Err(FileError::NotADirectory); }
    if parent.full.starts_with(&src.full) { return Err(FileError::Rejected("A folder cannot be moved inside itself")); }
    let target = parent.full.join(name);
    if target.exists() { return Err(FileError::Rejected("The original location is occupied. Undo would overwrite an item.")); }
    fs::rename(&src.full, &target).map_err(map_io)?;
    Ok(parts.join("/"))
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

pub fn copy_entry(root: &Path, rel: &str, dest_rel: &str) -> Result<String, FileError> {
    let src = resolve(root, rel)?;
    if src.rel.is_empty() {
        return Err(FileError::Forbidden);
    }
    let dest_dir = resolve(root, dest_rel)?;
    if !dest_dir.full.is_dir() {
        return Err(FileError::NotADirectory);
    }
    if dest_dir.full.starts_with(&src.full) {
        return Err(FileError::Rejected("A folder cannot be copied inside itself"));
    }
    let name = src.full.file_name().ok_or(FileError::InvalidName)?;
    let target = unique_path(dest_dir.full.join(name));
    let mut copied = 0usize;
    copy_path(&src.full, &target, &mut copied)?;
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

/// Overwrites an image file with raw bytes (used by the preview crop/rotate editor).
pub fn write_image_bytes(root: &Path, rel: &str, bytes: &[u8]) -> Result<(), FileError> {
    const MAX_BYTES: usize = 25 * 1024 * 1024;
    if bytes.is_empty() {
        return Err(FileError::Rejected("Empty image data"));
    }
    if bytes.len() > MAX_BYTES {
        return Err(FileError::Rejected("Edited image is too large to save (25 MB max)"));
    }
    let target = resolve(root, rel)?;
    if target.rel.is_empty() {
        return Err(FileError::Forbidden);
    }
    if target.full.is_dir() {
        return Err(FileError::IsADirectory);
    }
    let name = target
        .full
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    if kind_of(&name) != "image" {
        return Err(FileError::Rejected("Only raster images can be saved from the editor"));
    }
    let parent = target.full.parent().ok_or(FileError::Forbidden)?;
    let tmp = parent.join(format!(
        ".ownnas-img-{}-{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    fs::write(&tmp, bytes).map_err(map_io)?;
    fs::rename(&tmp, &target.full).map_err(|err| {
        let _ = fs::remove_file(&tmp);
        map_io(err)
    })?;
    Ok(())
}

/// Converts a raster image to another format, writing a sibling file (keeps the original).
pub fn convert_image(root: &Path, rel: &str, format: &str) -> Result<String, FileError> {
    let (ext, image_format) = match format.trim().to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => ("jpg", image::ImageFormat::Jpeg),
        "png" => ("png", image::ImageFormat::Png),
        "webp" => ("webp", image::ImageFormat::WebP),
        "gif" => ("gif", image::ImageFormat::Gif),
        "bmp" => ("bmp", image::ImageFormat::Bmp),
        "tif" | "tiff" => ("tiff", image::ImageFormat::Tiff),
        _ => return Err(FileError::Rejected("Unsupported image format")),
    };
    let target = resolve(root, rel)?;
    if target.rel.is_empty() {
        return Err(FileError::Forbidden);
    }
    if target.full.is_dir() {
        return Err(FileError::IsADirectory);
    }
    let name = target
        .full
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    if kind_of(&name) != "image" {
        return Err(FileError::Rejected("Only raster images can be converted"));
    }
    let current_ext = extension(&name.to_ascii_lowercase());
    if current_ext == ext || (current_ext == "jpeg" && ext == "jpg") || (current_ext == "tif" && ext == "tiff") {
        return Err(FileError::Rejected("That file is already in this format"));
    }
    let meta = fs::metadata(&target.full).map_err(map_io)?;
    if meta.len() > 40 * 1024 * 1024 {
        return Err(FileError::Rejected("Image is too large to convert (40 MB max)"));
    }
    let orientation = read_image_orientation(&target.full);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(25_000);
    limits.max_image_height = Some(25_000);
    limits.max_alloc = Some(256 * 1024 * 1024);
    let mut reader = image::ImageReader::open(&target.full)
        .map_err(|_| FileError::Io("Could not open the image"))?
        .with_guessed_format()
        .map_err(|_| FileError::Rejected("Could not recognize the image format"))?;
    reader.limits(limits);
    let decoded = reader
        .decode()
        .map_err(|_| FileError::Rejected("Could not decode the image"))?;
    if decoded.width() == 0 || decoded.height() == 0 {
        return Err(FileError::Rejected("The image is empty"));
    }
    let oriented = apply_image_orientation(decoded, orientation);
    let stem = Path::new(&name)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "image".to_string());
    let parent = target.full.parent().ok_or(FileError::Forbidden)?;
    let dest = unique_path(parent.join(format!("{stem}.{ext}")));
    let dest_name = dest
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| format!("{stem}.{ext}"));
    let tmp = parent.join(format!(
        ".ownnas-convert-{}-{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    oriented
        .save_with_format(&tmp, image_format)
        .map_err(|_| FileError::Rejected("Could not encode the converted image"))?;
    fs::rename(&tmp, &dest).map_err(|err| {
        let _ = fs::remove_file(&tmp);
        map_io(err)
    })?;
    Ok(if target.rel.contains('/') {
        let parent_rel = target.rel.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
        if parent_rel.is_empty() {
            dest_name
        } else {
            format!("{parent_rel}/{dest_name}")
        }
    } else {
        dest_name
    })
}

fn read_image_orientation(path: &Path) -> u32 {
    let Ok(file) = fs::File::open(path) else {
        return 1;
    };
    let mut reader = io::BufReader::new(file);
    let Ok(exif) = exif::Reader::new().read_from_container(&mut reader) else {
        return 1;
    };
    exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY)
        .and_then(|field| field.value.get_uint(0))
        .unwrap_or(1)
}

fn apply_image_orientation(img: image::DynamicImage, orientation: u32) -> image::DynamicImage {
    match orientation {
        2 => img.fliph(),
        3 => img.rotate180(),
        4 => img.flipv(),
        5 => img.rotate90().fliph(),
        6 => img.rotate90(),
        7 => img.rotate270().fliph(),
        8 => img.rotate270(),
        _ => img,
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateGroup {
    pub size: u64,
    pub sha256: String,
    pub paths: Vec<String>,
}

/// Finds likely duplicates under `rel` by size then SHA-256.
pub fn find_duplicates(
    root: &Path,
    rel: &str,
    cancel: &CancelFlag,
) -> Result<Vec<DuplicateGroup>, FileError> {
    let start = resolve(root, rel)?;
    if !start.full.is_dir() {
        return Err(FileError::NotADirectory);
    }
    let root_canon = root.canonicalize().map_err(map_io)?;
    let mut by_size: std::collections::HashMap<u64, Vec<(String, PathBuf)>> =
        std::collections::HashMap::new();
    let mut seen = 0usize;
    collect_files_for_dupes(
        &root_canon,
        &start.full,
        &start.rel,
        &mut by_size,
        &mut seen,
        cancel,
    )?;
    let mut groups = Vec::new();
    for (size, entries) in by_size {
        cancel.check()?;
        if size == 0 || entries.len() < 2 {
            continue;
        }
        let mut by_hash: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for (path, full) in entries {
            cancel.check()?;
            if groups.len() >= 40 {
                break;
            }
            let Ok(meta) = fs::metadata(&full) else { continue };
            if meta.len() > 200 * 1024 * 1024 {
                continue;
            }
            let Ok(mut file) = fs::File::open(&full) else { continue };
            let mut hasher = Sha256::new();
            let mut buf = [0u8; 64 * 1024];
            let mut ok = true;
            loop {
                cancel.check()?;
                match file.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => hasher.update(&buf[..n]),
                    Err(_) => {
                        ok = false;
                        break;
                    }
                }
            }
            if !ok {
                continue;
            }
            let digest = hex::encode(hasher.finalize());
            by_hash.entry(digest).or_default().push(path);
        }
        for (sha256, paths) in by_hash {
            if paths.len() >= 2 {
                groups.push(DuplicateGroup { size, sha256, paths });
            }
            if groups.len() >= 40 {
                break;
            }
        }
        if groups.len() >= 40 {
            break;
        }
    }
    groups.sort_by(|a, b| b.size.cmp(&a.size).then(b.paths.len().cmp(&a.paths.len())));
    Ok(groups)
}

fn collect_files_for_dupes(
    root: &Path,
    dir: &Path,
    rel: &str,
    by_size: &mut std::collections::HashMap<u64, Vec<(String, PathBuf)>>,
    seen: &mut usize,
    cancel: &CancelFlag,
) -> Result<(), FileError> {
    cancel.check()?;
    if *seen >= 6_000 {
        return Ok(());
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(());
    };
    for item in entries {
        cancel.check()?;
        if *seen >= 6_000 {
            return Ok(());
        }
        let Ok(item) = item else { continue };
        *seen += 1;
        let name = item.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let child = item.path();
        let Ok(canon) = child.canonicalize() else { continue };
        if !canon.starts_with(root) {
            continue;
        }
        let child_rel = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        if canon.is_dir() {
            if is_trash_folder(&canon) || is_archive_folder(&canon) {
                continue;
            }
            collect_files_for_dupes(root, &canon, &child_rel, by_size, seen, cancel)?;
            continue;
        }
        let Ok(meta) = fs::metadata(&canon) else { continue };
        by_size
            .entry(meta.len())
            .or_default()
            .push((child_rel, canon));
    }
    Ok(())
}

pub fn write_zip(
    root: &Path,
    rel: &str,
    output: &Path,
    cancel: &CancelFlag,
) -> Result<String, FileError> {
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
    zip_tree(&root, &start.full, "", &mut zip, &mut count, cancel)?;
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

/// Builds a zip for one or more selected files and folders.
pub fn write_zip_selection(
    root: &Path,
    rels: &[String],
    output: &Path,
    cancel: &CancelFlag,
) -> Result<String, FileError> {
    if rels.is_empty() {
        return Err(FileError::Rejected("Choose at least one item to download"));
    }
    if rels.len() > 500 {
        return Err(FileError::Rejected("Too many items selected for one download"));
    }
    if rels.len() == 1 {
        let one = resolve(root, &rels[0])?;
        if one.rel.is_empty() {
            return write_zip(root, "", output, cancel);
        }
        if one.full.is_dir() {
            return write_zip(root, &rels[0], output, cancel);
        }
    }
    let root = root.canonicalize().map_err(map_io)?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).map_err(map_io)?;
    }
    let file = fs::File::create(output).map_err(map_io)?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut count = 0usize;
    let mut used = HashSet::new();
    let mut first_name = None;
    for rel in rels {
        cancel.check()?;
        let resolved = resolve(&root, rel)?;
        if resolved.rel.is_empty() {
            return Err(FileError::Rejected("Download selected items instead of the whole library root"));
        }
        let base = resolved
            .full
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .ok_or(FileError::InvalidName)?;
        if first_name.is_none() {
            first_name = Some(base.clone());
        }
        let zip_name = unique_zip_name(&mut used, &base);
        count += 1;
        if count > 2_000 {
            return Err(FileError::Rejected("That selection is too large to download as one zip"));
        }
        if resolved.full.is_dir() {
            zip.add_directory(format!("{zip_name}/"), options)
                .map_err(|_| FileError::Io("Could not build the zip"))?;
            zip_tree(&root, &resolved.full, &zip_name, &mut zip, &mut count, cancel)?;
        } else {
            zip.start_file(&zip_name, options)
                .map_err(|_| FileError::Io("Could not build the zip"))?;
            let mut input = fs::File::open(&resolved.full).map_err(map_io)?;
            io::copy(&mut input, &mut zip).map_err(|_| FileError::Io("Could not build the zip"))?;
        }
    }
    zip.finish()
        .map_err(|_| FileError::Io("Could not finish the zip"))?;
    let label = if rels.len() == 1 {
        first_name.unwrap_or_else(|| "download".to_string())
    } else {
        format!("ownnas-{}-items", rels.len())
    };
    Ok(format!("{label}.zip"))
}

fn unique_zip_name(used: &mut HashSet<String>, name: &str) -> String {
    if used.insert(name.to_string()) {
        return name.to_string();
    }
    let path = Path::new(name);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let ext = path
        .extension()
        .map(|ext| format!(".{}", ext.to_string_lossy()))
        .unwrap_or_default();
    for index in 2..10_000 {
        let candidate = format!("{stem} ({index}){ext}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
    }
    name.to_string()
}

fn zip_tree(
    root: &Path,
    dir: &Path,
    prefix: &str,
    zip: &mut zip::ZipWriter<fs::File>,
    count: &mut usize,
    cancel: &CancelFlag,
) -> Result<(), FileError> {
    cancel.check()?;
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for item in fs::read_dir(dir).map_err(map_io)? {
        cancel.check()?;
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
            zip_tree(root, &canon, &zip_name, zip, count, cancel)?;
        } else {
            zip.start_file(&zip_name, options)
                .map_err(|_| FileError::Io("Could not build the zip"))?;
            let mut input = fs::File::open(&canon).map_err(map_io)?;
            io::copy(&mut input, zip).map_err(|_| FileError::Io("Could not build the zip"))?;
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum ArchiveFormat {
    Zip,
    Tar,
    TarGz,
    TarXz,
}

impl ArchiveFormat {
    fn parse(value: &str) -> Result<Self, FileError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "zip" => Ok(Self::Zip),
            "tar" => Ok(Self::Tar),
            "tar.gz" | "tgz" => Ok(Self::TarGz),
            "tar.xz" | "txz" => Ok(Self::TarXz),
            "rar" | "7z" | "zipx" => Err(FileError::Rejected(
                "That archive format cannot be created here (use ZIP or TAR.GZ)",
            )),
            _ => Err(FileError::Rejected("Unsupported archive format")),
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Zip => "zip",
            Self::Tar => "tar",
            Self::TarGz => "tar.gz",
            Self::TarXz => "tar.xz",
        }
    }
}

/// Compresses selected files/folders into a new archive beside them in the library.
pub fn compress_selection(
    root: &Path,
    rels: &[String],
    format: &str,
    cancel: &CancelFlag,
) -> Result<String, FileError> {
    let format = ArchiveFormat::parse(format)?;
    if rels.is_empty() {
        return Err(FileError::Rejected("Choose at least one item to compress"));
    }
    if rels.len() > 500 {
        return Err(FileError::Rejected("Too many items selected for one archive"));
    }
    cancel.check()?;
    let parent_rel = shared_parent_rel(rels)?;
    let parent = resolve(root, &parent_rel)?;
    if !parent.full.is_dir() {
        return Err(FileError::NotADirectory);
    }
    let label = archive_label(root, rels)?;
    let archive_name = format!("{label}.{}", format.extension());
    if !valid_new_component(&archive_name) {
        return Err(FileError::InvalidName);
    }
    let dest = unique_path(parent.full.join(&archive_name));
    let dest_name = dest
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or(archive_name);
    let tmp = parent.full.join(format!(
        ".ownnas-compress-{}-{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let built = match format {
        ArchiveFormat::Zip => write_zip_selection(root, rels, &tmp, cancel).map(|_| ()),
        ArchiveFormat::Tar | ArchiveFormat::TarGz | ArchiveFormat::TarXz => {
            write_tar_selection(root, rels, &tmp, format, cancel)
        }
    };
    if let Err(err) = built {
        let _ = fs::remove_file(&tmp);
        return Err(err);
    }
    if cancel.is_cancelled() {
        let _ = fs::remove_file(&tmp);
        return Err(FileError::Cancelled);
    }
    fs::rename(&tmp, &dest).map_err(|err| {
        let _ = fs::remove_file(&tmp);
        map_io(err)
    })?;
    Ok(if parent_rel.is_empty() {
        dest_name
    } else {
        format!("{parent_rel}/{dest_name}")
    })
}

fn shared_parent_rel(rels: &[String]) -> Result<String, FileError> {
    let mut parent: Option<String> = None;
    for rel in rels {
        let trimmed = rel.trim().trim_start_matches('/').to_string();
        if trimmed.is_empty() {
            return Err(FileError::Rejected("Compress items inside a folder, not the library root"));
        }
        if trimmed.contains("..") {
            return Err(FileError::Forbidden);
        }
        let this_parent = trimmed
            .rsplit_once('/')
            .map(|(p, _)| p.to_string())
            .unwrap_or_default();
        match &parent {
            None => parent = Some(this_parent),
            Some(existing) if existing == &this_parent => {}
            Some(_) => {
                return Err(FileError::Rejected(
                    "Select items from the same folder to compress together",
                ))
            }
        }
    }
    Ok(parent.unwrap_or_default())
}

fn archive_label(root: &Path, rels: &[String]) -> Result<String, FileError> {
    if rels.len() == 1 {
        let resolved = resolve(root, &rels[0])?;
        let name = resolved
            .full
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "Archive".to_string());
        // Avoid "photo.jpg.zip" feeling odd for files — use stem for files, full name for dirs.
        if resolved.full.is_dir() {
            Ok(name)
        } else {
            let stem = Path::new(&name)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or(name);
            Ok(if stem.is_empty() { "Archive".into() } else { stem })
        }
    } else {
        Ok(format!("Archive ({} items)", rels.len()))
    }
}

fn write_tar_selection(
    root: &Path,
    rels: &[String],
    output: &Path,
    format: ArchiveFormat,
    cancel: &CancelFlag,
) -> Result<(), FileError> {
    let root = root.canonicalize().map_err(map_io)?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).map_err(map_io)?;
    }
    let file = fs::File::create(output).map_err(map_io)?;
    let mut count = 0usize;
    let mut used = HashSet::new();
    match format {
        ArchiveFormat::Tar => {
            let mut builder = tar::Builder::new(file);
            append_selection_to_tar(&root, rels, &mut builder, &mut used, &mut count, cancel)?;
            builder
                .finish()
                .map_err(|_| FileError::Io("Could not finish the archive"))?;
        }
        ArchiveFormat::TarGz => {
            let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
            let mut builder = tar::Builder::new(encoder);
            append_selection_to_tar(&root, rels, &mut builder, &mut used, &mut count, cancel)?;
            let encoder = builder
                .into_inner()
                .map_err(|_| FileError::Io("Could not finish the archive"))?;
            encoder
                .finish()
                .map_err(|_| FileError::Io("Could not finish the archive"))?;
        }
        ArchiveFormat::TarXz => {
            let encoder = xz2::write::XzEncoder::new(file, 6);
            let mut builder = tar::Builder::new(encoder);
            append_selection_to_tar(&root, rels, &mut builder, &mut used, &mut count, cancel)?;
            let encoder = builder
                .into_inner()
                .map_err(|_| FileError::Io("Could not finish the archive"))?;
            encoder
                .finish()
                .map_err(|_| FileError::Io("Could not finish the archive"))?;
        }
        ArchiveFormat::Zip => unreachable!(),
    }
    Ok(())
}

fn append_selection_to_tar<W: Write>(
    root: &Path,
    rels: &[String],
    builder: &mut tar::Builder<W>,
    used: &mut HashSet<String>,
    count: &mut usize,
    cancel: &CancelFlag,
) -> Result<(), FileError> {
    for rel in rels {
        cancel.check()?;
        let resolved = resolve(root, rel)?;
        if resolved.rel.is_empty() {
            return Err(FileError::Rejected(
                "Compress items inside a folder, not the library root",
            ));
        }
        let base = resolved
            .full
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .ok_or(FileError::InvalidName)?;
        let archive_name = unique_zip_name(used, &base);
        *count += 1;
        if *count > 2_000 {
            return Err(FileError::Rejected("That selection is too large to compress"));
        }
        if resolved.full.is_dir() {
            tar_append_tree(root, &resolved.full, &archive_name, builder, count, cancel)?;
        } else {
            builder
                .append_path_with_name(&resolved.full, &archive_name)
                .map_err(|_| FileError::Io("Could not build the archive"))?;
        }
    }
    Ok(())
}

fn tar_append_tree<W: Write>(
    root: &Path,
    dir: &Path,
    prefix: &str,
    builder: &mut tar::Builder<W>,
    count: &mut usize,
    cancel: &CancelFlag,
) -> Result<(), FileError> {
    cancel.check()?;
    // Include the directory node itself when prefix is non-empty.
    if !prefix.is_empty() {
        builder
            .append_dir(prefix, dir)
            .map_err(|_| FileError::Io("Could not build the archive"))?;
    }
    for item in fs::read_dir(dir).map_err(map_io)? {
        cancel.check()?;
        let item = item.map_err(map_io)?;
        if *count >= 2_000 {
            return Err(FileError::Rejected("That selection is too large to compress"));
        }
        let name = item.file_name().to_string_lossy().to_string();
        let canon = match item.path().canonicalize() {
            Ok(path) if path.starts_with(root) => path,
            _ => continue,
        };
        let entry_name = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{}", item.file_name().to_string_lossy())
        };
        *count += 1;
        if canon.is_dir() {
            tar_append_tree(root, &canon, &entry_name, builder, count, cancel)?;
        } else {
            builder
                .append_path_with_name(&canon, &entry_name)
                .map_err(|_| FileError::Io("Could not build the archive"))?;
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
    fn upload_directories_merge_existing_tree_and_reject_files() {
        let root = scratch();
        fs::create_dir_all(root.join("sub/Folder/existing")).unwrap();
        fs::write(root.join("sub/Folder/existing/old.txt"), b"old").unwrap();
        let names = vec!["Folder".to_string(), "Folder/empty/nested".to_string()];
        ensure_upload_dirs(&root, "sub", &names).unwrap();
        assert!(root.join("sub/Folder/existing/old.txt").is_file());
        assert!(root.join("sub/Folder/empty/nested").is_dir());

        fs::write(root.join("sub/blocked"), b"file").unwrap();
        assert!(matches!(
            ensure_upload_dirs(&root, "sub", &["blocked/child".to_string()]),
            Err(FileError::AlreadyExists)
        ));
        assert!(matches!(
            ensure_upload_dirs(&root, "sub", &["../outside".to_string()]),
            Err(FileError::InvalidName)
        ));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn creates_markdown_and_csv_files() {
        let root = scratch();
        let md = create_file(&root, "sub", "Notes", "md").unwrap();
        assert_eq!(md, "sub/Notes.md");
        let text = fs::read_to_string(root.join("sub").join("Notes.md")).unwrap();
        assert!(text.starts_with("# Notes\n"));
        let csv = create_file(&root, "sub", "data.csv", "csv").unwrap();
        assert_eq!(csv, "sub/data.csv");
        assert!(root.join("sub").join("data.csv").exists());
        assert!(matches!(
            create_file(&root, "sub", "Notes", "md"),
            Err(FileError::AlreadyExists)
        ));
        assert!(matches!(
            create_file(&root, "sub", "x", "unsupported"),
            Err(FileError::Rejected(_))
        ));
        write_text_file(&root, "sub/Notes.md", "# Updated\n\nbody\n").unwrap();
        assert_eq!(
            fs::read_to_string(root.join("sub").join("Notes.md")).unwrap(),
            "# Updated\n\nbody\n"
        );
        assert!(matches!(
            write_text_file(&root, "sub/Notes.md", &"x".repeat(1024 * 1024 + 1)),
            Err(FileError::Rejected(_))
        ));
        let _ = fs::remove_dir_all(&root);
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
    fn undo_moves_preserves_conflicts_and_restores_trash() {
        let root = scratch();
        let moved = move_entry(&root, "sub/note.txt", "").unwrap();
        fs::write(root.join("sub/note.txt"), b"new item").unwrap();
        assert!(move_exact(&root, &moved, "sub/note.txt").is_err());
        assert_eq!(fs::read(root.join("sub/note.txt")).unwrap(), b"new item");
        assert_eq!(fs::read(root.join(&moved)).unwrap(), b"hello");
        fs::remove_file(root.join("sub/note.txt")).unwrap();
        assert_eq!(move_exact(&root, &moved, "sub/note.txt").unwrap(), "sub/note.txt");
        let trashed = trash_entry(&root, "sub/note.txt").unwrap();
        move_exact(&root, &trashed, "sub/note.txt").unwrap();
        assert_eq!(fs::read(root.join("sub/note.txt")).unwrap(), b"hello");
        assert!(move_exact(&root, "sub/note.txt", "../escape.txt").is_err());
        assert!(move_exact(&root, "sub", "sub/inside").is_err());
        fs::remove_dir_all(root).unwrap();
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
    fn search_matches_partial_file_and_folder_names() {
        let root = scratch();
        fs::create_dir_all(root.join("MyProjectsArchive/ClientFiles/QuarterlyReports"))
            .unwrap();
        fs::write(root.join("MyProjectsArchive/AnnualReport.DOCX"), b"binary fixture").unwrap();
        fs::write(root.join("content.txt"), b"single character Z in content").unwrap();
        let options = SearchOptions {
            ocr: false,
            cache_dir: root.join(".text-cache"),
        };
        let folders = search(&root, "", "PROJECTS", &options).unwrap();
        assert!(folders.iter().any(|hit| hit.dir && hit.name == "MyProjectsArchive"));
        let nested_folders = search(&root, "", "quarterlyrep", &options).unwrap();
        assert!(nested_folders.iter().any(|hit| {
            hit.dir && hit.path == "MyProjectsArchive/ClientFiles/QuarterlyReports"
        }));
        let files = search(&root, "", "ualrep", &options).unwrap();
        assert!(files.iter().any(|hit| !hit.dir && hit.name == "AnnualReport.DOCX"));
        let one_character = search(&root, "", "R", &options).unwrap();
        assert!(one_character.iter().any(|hit| hit.dir && hit.name == "MyProjectsArchive"));
        assert!(one_character.iter().any(|hit| hit.name == "AnnualReport.DOCX"));
        assert!(
            search(&root, "", "Z", &options).unwrap().is_empty(),
            "single character searches only match names"
        );
        assert!(search(&root, "", " ", &options).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn recursive_folder_name_hits_are_not_displaced_by_file_matches() {
        let root = scratch();
        for index in 0..110 {
            fs::write(root.join(format!("matching-file-{index:03}.bin")), b"fixture").unwrap();
        }
        let nested = root.join("sub/deep/matching-folder");
        fs::create_dir_all(&nested).unwrap();
        let options = SearchOptions {
            ocr: false,
            cache_dir: root.join(".text-cache"),
        };

        let hits = search(&root, "", "matching", &options).unwrap();

        assert!(hits.len() <= 100);
        assert!(hits
            .iter()
            .any(|hit| hit.dir && hit.path == "sub/deep/matching-folder"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn searches_duplicates_and_measures() {
        let root = scratch();
        fs::write(root.join("sub").join("Alpha Note.txt"), b"abc").unwrap();
        let hits = search(
            &root,
            "",
            "alpha",
            &SearchOptions {
                ocr: false,
                cache_dir: root.join(".text-cache"),
            },
        )
        .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "Alpha Note.txt");
        let copy = duplicate(&root, "sub/Alpha Note.txt").unwrap();
        assert!(copy.contains("Alpha Note"));
        assert_ne!(copy, "sub/Alpha Note.txt");
        fs::create_dir(root.join("other")).unwrap();
        let pasted = copy_entry(&root, "sub/note.txt", "other").unwrap();
        assert_eq!(pasted, "other/note.txt");
        assert_eq!(fs::read(root.join("other").join("note.txt")).unwrap(), b"hello");
        assert_eq!(fs::read(root.join("sub").join("note.txt")).unwrap(), b"hello");
        let again = copy_entry(&root, "sub/note.txt", "other").unwrap();
        assert_ne!(again, "other/note.txt");
        assert!(again.starts_with("other/note"));
        let usage = usage(&root, "").unwrap();
        assert!(usage.bytes >= 3);
        let _ = fs::remove_dir_all(&root);
    }

    fn set_mtime(path: &Path, secs: u64) {
        let file = fs::File::options().write(true).open(path).unwrap();
        file.set_modified(UNIX_EPOCH + std::time::Duration::from_secs(secs))
            .unwrap();
    }

    #[test]
    fn upload_conflict_choices() {
        let root = scratch();
        let note = root.join("sub").join("note.txt");
        set_mtime(&note, 1_000);
        assert!(matches!(
            prepare_upload(&root, "sub", "note.txt", None).unwrap(),
            PreparedUpload::Ask(_)
        ));

        let ignored = prepare_upload(&root, "sub", "note.txt", Some(UploadChoice::Ignore)).unwrap();
        assert!(matches!(ignored, PreparedUpload::Skip));
        assert_eq!(fs::read(&note).unwrap(), b"hello");

        let kept = prepare_upload(&root, "sub", "note.txt", Some(UploadChoice::KeepBoth)).unwrap();
        match kept {
            PreparedUpload::Write(path) => assert!(path.ends_with("note (2).txt")),
            _ => panic!("keep both should choose a new path"),
        }

        set_mtime(&note, 1_000);
        let older = prepare_upload(
            &root,
            "sub",
            "note.txt",
            Some(UploadChoice::ArchiveOlder { incoming_modified: 2_000 }),
        )
        .unwrap();
        match older {
            PreparedUpload::Write(path) => assert_eq!(path, note),
            _ => panic!("newer upload should keep the original name"),
        }
        assert!(root.join("sub").join(".ownnas-archive").join("note.txt").is_file());
        assert!(root.join("sub").join(".ownnas-archive").join(".ownnas-id").is_file());
        let listing = list_dir(&root, "sub", true, false).unwrap();
        let archive = listing
            .entries
            .iter()
            .find(|entry| entry.name == ".ownnas-archive")
            .unwrap();
        assert_eq!(archive.kind, "archive-folder");
        // Special folders stay visible even when Hidden is off.
        let visible = list_dir(&root, "sub", false, false).unwrap();
        let visible_archive = visible
            .entries
            .iter()
            .find(|entry| entry.name == ".ownnas-archive")
            .unwrap();
        assert_eq!(visible_archive.kind, "archive-folder");
        let hidden_listing = list_dir(&root, "sub/.ownnas-archive", true, false).unwrap();
        assert!(!hidden_listing.entries.iter().any(|entry| entry.name == ".ownnas-id"));
        fs::write(&note, b"new").unwrap();
        set_mtime(&note, 5_000);

        let newer_existing = prepare_upload(
            &root,
            "sub",
            "note.txt",
            Some(UploadChoice::ArchiveOlder { incoming_modified: 3_000 }),
        )
        .unwrap();
        match &newer_existing {
            PreparedUpload::StoreInArchive { path, original_name } => {
                assert!(path.starts_with(root.join("sub").join(".ownnas-archive")));
                assert_ne!(path, &note);
                assert_eq!(original_name, "note.txt");
                fs::write(path, b"old-upload").unwrap();
                record_archived_file(&root, path, original_name).unwrap();
            }
            _ => panic!("older upload should be archived"),
        }
        assert_eq!(fs::read(&note).unwrap(), b"new");

        fs::write(&note, b"stay").unwrap();
        let replaced = prepare_upload(&root, "sub", "note.txt", Some(UploadChoice::ArchiveExisting)).unwrap();
        match replaced {
            PreparedUpload::Write(path) => assert_eq!(path, note),
            _ => panic!("archive existing should free the original name"),
        }
        assert!(!note.exists());
        assert!(root.join("sub").join(".ownnas-archive").join("note (3).txt").is_file());
        let log = fs::read_to_string(root.join("sub").join(".ownnas-archive").join("archive.log")).unwrap();
        let lines: Vec<_> = log.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].ends_with("  note.txt"));
        assert!(lines[1].ends_with("  note.txt saved as note (2).txt"));
        assert!(lines[2].ends_with("  note.txt saved as note (3).txt"));
        assert_eq!(format_utc(1_577_836_800), "2020-01-01 00:00:00Z");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn trash_restore_and_empty() {
        let root = scratch();
        fs::write(root.join("sub").join("keep.txt"), b"keep").unwrap();
        let trashed = trash_entry(&root, "sub/keep.txt").unwrap();
        assert_eq!(trashed, ".ownnas-trash/keep.txt");
        assert!(!root.join("sub").join("keep.txt").exists());
        assert!(root.join(".ownnas-trash").join(".ownnas-trash-id").is_file());
        let root_listing = list_dir(&root, "", false, false).unwrap();
        let trash_card = root_listing
            .entries
            .iter()
            .find(|entry| entry.kind == "trash-folder")
            .unwrap();
        assert_eq!(trash_card.name, ".ownnas-trash");
        let listing = list_dir(&root, ".ownnas-trash", false, false).unwrap();
        let item = listing.entries.iter().find(|entry| entry.name == "keep.txt").unwrap();
        assert_eq!(item.original.as_deref(), Some("sub/keep.txt"));
        let restored = restore_entry(&root, ".ownnas-trash/keep.txt").unwrap();
        assert_eq!(restored, "sub/keep.txt");
        assert_eq!(fs::read(root.join("sub").join("keep.txt")).unwrap(), b"keep");
        trash_entry(&root, "sub/keep.txt").unwrap();
        assert_eq!(empty_trash(&root).unwrap(), 1);
        assert!(list_dir(&root, ".ownnas-trash", true, false).unwrap().entries.is_empty());

        // Legacy `Trash/` is renamed when the dotted trash folder is absent.
        let _ = fs::remove_dir_all(root.join(".ownnas-trash"));
        fs::write(root.join("sub").join("old.txt"), b"old").unwrap();
        fs::create_dir(root.join("Trash")).unwrap();
        fs::write(
            root.join("Trash").join(".ownnas-trash"),
            b"OwnNAS trash folder\n",
        )
        .unwrap();
        fs::rename(root.join("sub").join("old.txt"), root.join("Trash").join("old.txt")).unwrap();
        let moved = migrate_legacy_dirs(&root).unwrap();
        assert_eq!(moved, vec![("Trash".to_string(), ".ownnas-trash".to_string())]);
        assert!(root.join(".ownnas-trash").join("old.txt").is_file());
        assert!(!root.join("Trash").exists());
        let _ = fs::remove_dir_all(&root);
    }
}

/// Save an existing DOCX package using a temporary file.
/// The hash guards against overwriting edits made since the document was opened.
pub fn write_docx_bytes(root: &Path, rel: &str, bytes: &[u8], expected_hash: &str) -> Result<String, FileError> {
    static SAVE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = SAVE_LOCK.lock().map_err(|_| FileError::Io("Document save lock failed"))?;
    if bytes.is_empty() || bytes.len() > 25 * 1024 * 1024 {
        return Err(FileError::Rejected("Document must be between 1 byte and 25 MB"));
    }
    let target = resolve(root, rel)?;
    if !target.full.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("docx")) || is_under_trash(&target.rel) {
        return Err(FileError::Rejected("Only DOCX files outside Trash can be edited"));
    }
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|_| FileError::Rejected("Invalid DOCX package"))?;
    if archive.len() > 4096 { return Err(FileError::Rejected("Too many document parts")); }
    let mut total = 0u64;
    for i in 0..archive.len() {
        let part = archive.by_index(i).map_err(|_| FileError::Rejected("Invalid document part"))?;
        total = total.saturating_add(part.size());
        if total > 100 * 1024 * 1024 { return Err(FileError::Rejected("Document expands beyond 100 MB")); }
    }
    for name in ["[Content_Types].xml", "word/document.xml"] {
        let mut part = archive.by_name(name).map_err(|_| FileError::Rejected("Missing DOCX document part"))?;
        let mut data = String::new();
        part.read_to_string(&mut data).map_err(|_| FileError::Rejected("Invalid document XML"))?;
        if !data.contains('<') { return Err(FileError::Rejected("Invalid document XML")); }
    }
    let old = fs::read(&target.full).map_err(map_io)?;
    if hex::encode(Sha256::digest(&old)) != expected_hash {
        return Err(FileError::Rejected("Document changed on the server. Reopen it before saving."));
    }
    let parent = target.full.parent().ok_or(FileError::Forbidden)?;
    let tmp = parent.join(format!(".ownnas-docx-{}-{}.tmp", std::process::id(), rand::random::<u64>()));
    let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&tmp).map_err(map_io)?;
    let result = (|| {
        file.write_all(bytes).map_err(map_io)?;
        file.sync_all().map_err(map_io)?;
        drop(file);
        fs::rename(&tmp, &target.full).map_err(map_io)?;
        Ok(hex::encode(Sha256::digest(bytes)))
    })();
    if result.is_err() { let _ = fs::remove_file(&tmp); }
    result
}

/// Save an existing XLSX package, validating its workbook parts and guarding against stale writes.
pub fn write_xlsx_bytes(root: &Path, rel: &str, bytes: &[u8], expected_hash: &str) -> Result<String, FileError> {
    static SAVE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = SAVE_LOCK.lock().map_err(|_| FileError::Io("Spreadsheet save lock failed"))?;
    if bytes.is_empty() || bytes.len() > 25 * 1024 * 1024 {
        return Err(FileError::Rejected("Spreadsheet must be between 1 byte and 25 MB"));
    }
    let target = resolve(root, rel)?;
    if !target.full.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("xlsx")) || is_under_trash(&target.rel) {
        return Err(FileError::Rejected("Only XLSX files outside Trash can be edited"));
    }
    validate_xlsx_package(bytes)?;
    let old = fs::read(&target.full).map_err(map_io)?;
    if hex::encode(Sha256::digest(&old)) != expected_hash {
        return Err(FileError::Rejected("Spreadsheet changed on the server. Reopen it before saving."));
    }
    let parent = target.full.parent().ok_or(FileError::Forbidden)?;
    let tmp = parent.join(format!(".ownnas-xlsx-{}-{}.tmp", std::process::id(), rand::random::<u64>()));
    let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&tmp).map_err(map_io)?;
    let result = (|| {
        file.write_all(bytes).map_err(map_io)?;
        file.sync_all().map_err(map_io)?;
        drop(file);
        fs::rename(&tmp, &target.full).map_err(map_io)?;
        Ok(hex::encode(Sha256::digest(bytes)))
    })();
    if result.is_err() { let _ = fs::remove_file(&tmp); }
    result
}

fn validate_xlsx_package(bytes: &[u8]) -> Result<(), FileError> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|_| FileError::Rejected("Invalid XLSX package"))?;
    if archive.len() > 4096 { return Err(FileError::Rejected("Too many spreadsheet parts")); }
    let mut total = 0u64;
    let mut sheet_part = None;
    for index in 0..archive.len() {
        let part = archive.by_index(index).map_err(|_| FileError::Rejected("Invalid spreadsheet part"))?;
        total = total.saturating_add(part.size());
        if total > 100 * 1024 * 1024 { return Err(FileError::Rejected("Spreadsheet expands beyond 100 MB")); }
        let name = part.name();
        if name.starts_with("_xmlsignatures/") {
            return Err(FileError::Rejected("Digitally signed spreadsheets cannot be edited"));
        }
        if name.starts_with("xl/worksheets/") && name.ends_with(".xml") && sheet_part.is_none() {
            sheet_part = Some(name.to_string());
        }
    }
    let sheet_part = sheet_part.ok_or(FileError::Rejected("Spreadsheet has no worksheets"))?;
    for (name, expected) in [
        ("[Content_Types].xml", "Types"),
        ("xl/workbook.xml", "workbook"),
        ("xl/_rels/workbook.xml.rels", "Relationships"),
    ] {
        let mut part = archive.by_name(name).map_err(|_| FileError::Rejected("Missing XLSX workbook part"))?;
        let mut data = String::new();
        part.read_to_string(&mut data).map_err(|_| FileError::Rejected("Invalid XLSX XML"))?;
        if data.contains("<!DOCTYPE") || data.contains("<!ENTITY") || !data.contains(expected) {
            return Err(FileError::Rejected("Invalid XLSX workbook XML"));
        }
    }
    let mut worksheet = archive.by_name(&sheet_part).map_err(|_| FileError::Rejected("Missing XLSX worksheet"))?;
    let mut data = String::new();
    worksheet.read_to_string(&mut data).map_err(|_| FileError::Rejected("Invalid XLSX worksheet XML"))?;
    if data.contains("<!DOCTYPE") || data.contains("<!ENTITY") || !data.contains("worksheet") {
        return Err(FileError::Rejected("Invalid XLSX worksheet XML"));
    }
    Ok(())
}

fn blank_docx() -> Result<Vec<u8>, FileError> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, data) in [
        ("[Content_Types].xml", r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#),
        ("_rels/.rels", r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#),
        ("word/document.xml", r#"<?xml version="1.0" encoding="UTF-8"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t xml:space="preserve"></w:t></w:r></w:p><w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440"/></w:sectPr></w:body></w:document>"#),
    ] {
        zip.start_file(name, zip::write::SimpleFileOptions::default()).map_err(|_| FileError::Io("Could not create document"))?;
        zip.write_all(data.as_bytes()).map_err(map_io)?;
    }
    Ok(zip.finish().map_err(|_| FileError::Io("Could not create document"))?.into_inner())
}

/// Save an existing OpenDocument Text package, guarding against stale editor writes.
pub fn write_odt_bytes(root: &Path, rel: &str, bytes: &[u8], expected_hash: &str) -> Result<String, FileError> {
    static SAVE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = SAVE_LOCK.lock().map_err(|_| FileError::Io("Document save lock failed"))?;
    if bytes.is_empty() || bytes.len() > 25 * 1024 * 1024 {
        return Err(FileError::Rejected("Document must be between 1 byte and 25 MB"));
    }
    let target = resolve(root, rel)?;
    if !target.full.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("odt")) || is_under_trash(&target.rel) {
        return Err(FileError::Rejected("Only ODT files outside Trash can be edited"));
    }
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|_| FileError::Rejected("Invalid ODT package"))?;
    if archive.len() > 4096 { return Err(FileError::Rejected("Too many document parts")); }
    let first = archive.by_index(0).map_err(|_| FileError::Rejected("Invalid ODT package"))?;
    if first.name() != "mimetype" || first.compression() != zip::CompressionMethod::Stored {
        return Err(FileError::Rejected("ODT mimetype entry must be first and uncompressed"));
    }
    drop(first);
    if bytes.len() < 30 || &bytes[0..4] != b"PK\x03\x04" || u16::from_le_bytes([bytes[8], bytes[9]]) != 0 {
        return Err(FileError::Rejected("ODT mimetype entry must be first and uncompressed"));
    }
    let name_len = u16::from_le_bytes([bytes[26], bytes[27]]) as usize;
    let extra_len = u16::from_le_bytes([bytes[28], bytes[29]]) as usize;
    let data_len = u32::from_le_bytes([bytes[18], bytes[19], bytes[20], bytes[21]]) as usize;
    let data_start = 30usize.saturating_add(name_len).saturating_add(extra_len);
    if extra_len != 0 || bytes.get(30..30 + name_len) != Some(b"mimetype".as_slice())
        || bytes.get(data_start..data_start.saturating_add(data_len)) != Some(b"application/vnd.oasis.opendocument.text".as_slice()) {
        return Err(FileError::Rejected("ODT mimetype header is invalid"));
    }
    let mut total = 0u64;
    for i in 0..archive.len() {
        let mut part = archive.by_index(i).map_err(|_| FileError::Rejected("Invalid document part"))?;
        total = total.saturating_add(part.size());
        if total > 100 * 1024 * 1024 { return Err(FileError::Rejected("Document expands beyond 100 MB")); }
        if part.name() == "mimetype" {
            let mut value = String::new();
            part.read_to_string(&mut value).map_err(|_| FileError::Rejected("Invalid ODT mimetype"))?;
            if value != "application/vnd.oasis.opendocument.text" { return Err(FileError::Rejected("Invalid ODT mimetype")); }
        }
    }
    for name in ["content.xml", "META-INF/manifest.xml"] {
        let mut part = archive.by_name(name).map_err(|_| FileError::Rejected("Missing ODT document part"))?;
        let mut data = String::new();
        part.read_to_string(&mut data).map_err(|_| FileError::Rejected("Invalid ODT XML"))?;
        if data.contains("<!DOCTYPE") || data.contains("<!ENTITY") || !data.contains('<') {
            return Err(FileError::Rejected("Invalid ODT XML"));
        }
        if name == "content.xml" && (!data.contains("document-content") || !data.contains("office:text")) {
            return Err(FileError::Rejected("Invalid ODT content document"));
        }
    }
    if archive.file_names().any(|name| name.to_ascii_lowercase().starts_with("meta-inf/") && name.to_ascii_lowercase().contains("signatures")) {
        return Err(FileError::Rejected("Digitally signed documents cannot be edited"));
    }
    let old = fs::read(&target.full).map_err(map_io)?;
    if hex::encode(Sha256::digest(&old)) != expected_hash {
        return Err(FileError::Rejected("Document changed on the server. Reopen it before saving."));
    }
    let parent = target.full.parent().ok_or(FileError::Forbidden)?;
    let tmp = parent.join(format!(".ownnas-odt-{}-{}.tmp", std::process::id(), rand::random::<u64>()));
    let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&tmp).map_err(map_io)?;
    let result = (|| {
        file.write_all(bytes).map_err(map_io)?;
        file.sync_all().map_err(map_io)?;
        drop(file);
        fs::rename(&tmp, &target.full).map_err(map_io)?;
        Ok(hex::encode(Sha256::digest(bytes)))
    })();
    if result.is_err() { let _ = fs::remove_file(&tmp); }
    result
}

fn blank_odt() -> Result<Vec<u8>, FileError> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    zip.start_file("mimetype", zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored))
        .map_err(|_| FileError::Io("Could not create document"))?;
    zip.write_all(b"application/vnd.oasis.opendocument.text").map_err(map_io)?;
    for (name, data) in [
        ("content.xml", r#"<?xml version="1.0" encoding="UTF-8"?><office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" office:version="1.3"><office:automatic-styles/><office:body><office:text><text:p/></office:text></office:body></office:document-content>"#),
        ("styles.xml", r#"<?xml version="1.0" encoding="UTF-8"?><office:document-styles xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" office:version="1.3"><office:font-face-decls/><office:styles/><office:automatic-styles/><office:master-styles/></office:document-styles>"#),
        ("meta.xml", r#"<?xml version="1.0" encoding="UTF-8"?><office:document-meta xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" office:version="1.3"><office:meta/></office:document-meta>"#),
        ("settings.xml", r#"<?xml version="1.0" encoding="UTF-8"?><office:document-settings xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" office:version="1.3"><office:settings/></office:document-settings>"#),
        ("META-INF/manifest.xml", r#"<?xml version="1.0" encoding="UTF-8"?><manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3"><manifest:file-entry manifest:full-path="/" manifest:media-type="application/vnd.oasis.opendocument.text"/><manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/><manifest:file-entry manifest:full-path="styles.xml" manifest:media-type="text/xml"/><manifest:file-entry manifest:full-path="meta.xml" manifest:media-type="text/xml"/><manifest:file-entry manifest:full-path="settings.xml" manifest:media-type="text/xml"/></manifest:manifest>"#),
    ] {
        zip.start_file(name, zip::write::SimpleFileOptions::default()).map_err(|_| FileError::Io("Could not create document"))?;
        zip.write_all(data.as_bytes()).map_err(map_io)?;
    }
    Ok(zip.finish().map_err(|_| FileError::Io("Could not create document"))?.into_inner())
}

#[cfg(test)]
mod file_kind_tests {
    use super::kind_of;

    #[test]
    fn classifies_cad_drawings_separately_from_mesh_models() {
        assert_eq!(kind_of("plan.dxf"), "cad2d");
        assert_eq!(kind_of("Plan.DWG"), "cad2d");
        assert_eq!(kind_of("print.3mf"), "model3d");
        assert_eq!(kind_of("budget.XLSX"), "spreadsheet");
    }
}

#[cfg(test)]
mod document_package_tests {
    use super::*;
    #[test]
    fn saves_docx_and_rejects_stale_or_invalid_updates() {
        let root = std::env::temp_dir().join(format!("ownnas-docx-{}", rand::random::<u64>()));
        fs::create_dir(&root).unwrap();
        let rel = create_file(&root, "", "Document", "docx").unwrap();
        let original = fs::read(root.join(&rel)).unwrap();
        let hash = hex::encode(Sha256::digest(&original));
        assert!(write_docx_bytes(&root, &rel, b"bad zip", &hash).is_err());
        assert_eq!(fs::read(root.join(&rel)).unwrap(), original);
        assert!(write_docx_bytes(&root, &rel, &original, "stale").is_err());
        assert_eq!(write_docx_bytes(&root, &rel, &original, &hash).unwrap(), hash);
        assert!(write_docx_bytes(&root, "../escape.docx", &original, &hash).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn creates_and_saves_odt_with_required_mimetype_header() {
        let root = std::env::temp_dir().join(format!("ownnas-odt-{}", rand::random::<u64>()));
        fs::create_dir(&root).unwrap();
        let rel = create_file(&root, "", "Document", "odt").unwrap();
        let original = fs::read(root.join(&rel)).unwrap();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&original)).unwrap();
        let first = archive.by_index(0).unwrap();
        assert_eq!(first.name(), "mimetype");
        assert_eq!(first.compression(), zip::CompressionMethod::Stored);
        drop(first);
        let hash = hex::encode(Sha256::digest(&original));
        assert!(write_odt_bytes(&root, &rel, b"bad zip", &hash).is_err());
        assert_eq!(fs::read(root.join(&rel)).unwrap(), original);
        assert!(write_odt_bytes(&root, &rel, &original, "stale").is_err());
        assert_eq!(write_odt_bytes(&root, &rel, &original, &hash).unwrap(), hash);
        assert!(write_odt_bytes(&root, "../escape.odt", &original, &hash).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn saves_xlsx_with_stale_write_and_package_validation() {
        let root = std::env::temp_dir().join(format!("ownnas-xlsx-{}", rand::random::<u64>()));
        fs::create_dir(&root).unwrap();
        let rel = "budget.xlsx";
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, xml) in [
            ("[Content_Types].xml", "<Types/>") ,
            ("xl/workbook.xml", "<workbook/>") ,
            ("xl/_rels/workbook.xml.rels", "<Relationships/>") ,
            ("xl/worksheets/sheet1.xml", "<worksheet><sheetData/></worksheet>"),
        ] {
            zip.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(xml.as_bytes()).unwrap();
        }
        let original = zip.finish().unwrap().into_inner();
        fs::write(root.join(rel), &original).unwrap();
        let hash = hex::encode(Sha256::digest(&original));
        assert!(write_xlsx_bytes(&root, rel, b"bad zip", &hash).is_err());
        assert_eq!(fs::read(root.join(rel)).unwrap(), original);
        assert!(write_xlsx_bytes(&root, rel, &original, "stale").is_err());
        assert_eq!(write_xlsx_bytes(&root, rel, &original, &hash).unwrap(), hash);
        assert!(write_xlsx_bytes(&root, "../escape.xlsx", &original, &hash).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
