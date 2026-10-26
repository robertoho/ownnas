//! Looking inside archives without unpacking them: what they hold, and one file from inside.
//! Zip (and the formats built on it: comics, office files, EPUB), tar (plain, gzip, bzip2, xz),
//! 7-Zip (`sevenz-rust2`, pure Rust), and RAR (the `unrar` library, built in). And unpacking
//! them (or a single gzip, bzip2, or xz file) into the folder they are in.

use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};

use crate::files::{self, CancelFlag, FileError};

use serde::Serialize;

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub name: String,
    pub size: u64,
    pub dir: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Format {
    Zip,
    Tar,
    TarGz,
    TarBz2,
    TarXz,
    SevenZip,
    Rar,
    /// A CD or DVD image (ISO 9660, with Joliet's long names when it has them).
    Iso,
}

/// The archive format a file's name says, including formats that are zips inside (office
/// documents, EPUB, comics) so their parts can be read.
pub fn format(name: &str) -> Option<Format> {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
        return Some(Format::TarGz);
    }
    if lower.ends_with(".tar.bz2") || lower.ends_with(".tbz2") || lower.ends_with(".tbz") {
        return Some(Format::TarBz2);
    }
    if lower.ends_with(".tar.xz") || lower.ends_with(".txz") {
        return Some(Format::TarXz);
    }
    match lower.rsplit_once('.').map_or("", |(_, ext)| ext) {
        "tar" => Some(Format::Tar),
        "zip" | "jar" | "cbz" | "epub" | "docx" | "xlsx" | "pptx" | "ppsx" | "odt" | "ods" | "odp" | "pages" | "numbers" | "key" | "apk" | "kmz" => Some(Format::Zip),
        "7z" | "cb7" => Some(Format::SevenZip),
        "rar" | "cbr" => Some(Format::Rar),
        "iso" => Some(Format::Iso),
        _ => None,
    }
}

/// What the archive holds: at most `limit` entries, and whether there were more.
pub fn list(path: &Path, name: &str, limit: usize) -> Result<(Vec<Entry>, bool), String> {
    let format = format(name).ok_or("OwnNAS does not read this kind of archive")?;
    let mut entries = Vec::new();
    let mut more = false;
    let mut push = |entry: Entry| {
        if entries.len() >= limit {
            more = true;
            false
        } else {
            entries.push(entry);
            true
        }
    };
    match format {
        Format::Zip => {
            let mut zip = open_zip(path)?;
            for index in 0..zip.len() {
                let Ok(file) = zip.by_index_raw(index) else { continue };
                if !push(Entry { name: file.name().to_string(), size: file.size(), dir: file.is_dir() }) {
                    break;
                }
            }
        }
        Format::Tar | Format::TarGz | Format::TarBz2 | Format::TarXz => {
            let mut archive = tar::Archive::new(tar_reader(path, format)?);
            for entry in archive.entries().map_err(|_| "Could not read the tar file".to_string())? {
                let entry = entry.map_err(|_| "Could not read the tar file".to_string())?;
                let name = entry.path().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
                if !push(Entry { name, size: entry.size(), dir: entry.header().entry_type().is_dir() }) {
                    break;
                }
            }
        }
        Format::SevenZip => {
            let reader = sevenz_rust2::ArchiveReader::open(path, sevenz_rust2::Password::empty()).map_err(|err| seven_zip_error(&err))?;
            for file in &reader.archive().files {
                if !push(Entry { name: file.name.clone(), size: file.size, dir: file.is_directory }) {
                    break;
                }
            }
        }
        Format::Iso => {
            for (name, _, size, dir) in iso_entries(path, limit + 1)? {
                if !push(Entry { name, size, dir }) {
                    break;
                }
            }
        }
        Format::Rar => {
            let archive = unrar::Archive::new(path).open_for_listing().map_err(|_| "Could not read the RAR file".to_string())?;
            for header in archive {
                let Ok(header) = header else { break };
                if !push(Entry { name: header.filename.to_string_lossy().replace('\\', "/"), size: header.unpacked_size, dir: header.is_directory() }) {
                    break;
                }
            }
        }
    }
    Ok((entries, more))
}

/// One file from inside the archive, if it is at most `max` bytes.
pub fn read(path: &Path, name: &str, wanted: &str, max: u64) -> Result<Vec<u8>, String> {
    let format = format(name).ok_or("OwnNAS does not read this kind of archive")?;
    let too_big = || format!("This file is larger than {} MB", max / (1024 * 1024));
    match format {
        Format::Zip => {
            let mut zip = open_zip(path)?;
            let file = zip.by_name(wanted).map_err(|_| "This file is not in the archive".to_string())?;
            if file.size() > max {
                return Err(too_big());
            }
            read_limited(file, max)
        }
        Format::Tar | Format::TarGz | Format::TarBz2 | Format::TarXz => {
            let mut archive = tar::Archive::new(tar_reader(path, format)?);
            for entry in archive.entries().map_err(|_| "Could not read the tar file".to_string())? {
                let entry = entry.map_err(|_| "Could not read the tar file".to_string())?;
                let matches = entry.path().is_ok_and(|p| p.to_string_lossy() == wanted);
                if matches {
                    if entry.size() > max {
                        return Err(too_big());
                    }
                    return read_limited(entry, max);
                }
            }
            Err("This file is not in the archive".into())
        }
        Format::SevenZip => {
            let mut reader = sevenz_rust2::ArchiveReader::open(path, sevenz_rust2::Password::empty()).map_err(|err| seven_zip_error(&err))?;
            let size = reader.archive().files.iter().find(|f| f.name == wanted).map(|f| f.size).ok_or("This file is not in the archive")?;
            if size > max {
                return Err(too_big());
            }
            reader.read_file(wanted).map_err(|err| seven_zip_error(&err))
        }
        Format::Iso => {
            let (_, extent, size, _) = iso_entries(path, usize::MAX)?.into_iter().find(|(name, _, _, dir)| !dir && name == wanted).ok_or("This file is not in the disc image")?;
            if size > max {
                return Err(too_big());
            }
            let mut file = File::open(path).map_err(|_| "Could not open the disc image".to_string())?;
            std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(extent * ISO_SECTOR)).map_err(|_| "Could not read the disc image".to_string())?;
            read_limited(file.take(size), max)
        }
        Format::Rar => {
            let mut archive = unrar::Archive::new(path).open_for_processing().map_err(|_| "Could not read the RAR file".to_string())?;
            while let Some(header) = archive.read_header().map_err(|_| "Could not read the RAR file".to_string())? {
                let entry = header.entry();
                if entry.filename.to_string_lossy().replace('\\', "/") == wanted {
                    if entry.unpacked_size > max {
                        return Err(too_big());
                    }
                    let (data, _rest) = header.read().map_err(|_| "This file could not be unpacked (is the archive protected by a password?)".to_string())?;
                    return Ok(data);
                }
                archive = header.skip().map_err(|_| "Could not read the RAR file".to_string())?;
            }
            Err("This file is not in the archive".into())
        }
    }
}

/// An archive with more entries than this is not unpacked.
const MAX_ENTRIES: usize = 200_000;
/// Space left free on the disk after unpacking.
const SPARE_BYTES: u64 = 512 * 1024 * 1024;

/// Whether `extract` unpacks this file: the archives `format` knows (but not the documents built
/// on zip), and single gzip, bzip2, or xz files.
pub fn extractable(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let ext = lower.rsplit_once('.').map_or("", |(_, ext)| ext);
    match format(name) {
        Some(Format::Zip) => matches!(ext, "zip" | "cbz"),
        Some(_) => true,
        None => single_compressed(&lower).is_some(),
    }
}

/// Unpacks the archive at `path` into the folder it is in: a single file or folder at its top
/// goes there as it is, more than one into a new folder named after the archive (as Finder and
/// Windows do). Nothing is overwritten: a name already taken gets a number. Links, devices,
/// and paths leading out of the folder are left out, and unpacking stops before the disk fills.
/// Returns the new file or folder.
pub fn extract(path: &Path, cancel: &CancelFlag) -> Result<PathBuf, FileError> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if !extractable(&name) {
        return Err(FileError::Rejected("OwnNAS cannot unpack this kind of file"));
    }
    let parent = path.parent().ok_or(FileError::Forbidden)?;
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let tmp = parent.join(format!(".ownnas-extract-{}-{nanos}", std::process::id()));
    std::fs::create_dir(&tmp).map_err(files::map_io)?;
    let free = fs4::available_space(parent).unwrap_or(u64::MAX);
    let mut out = Unpack { dir: tmp.clone(), left: free.saturating_sub(SPARE_BYTES), entries: 0, cancel: cancel.clone() };
    let unpacked = unpack(path, &name, &mut out).and_then(|()| place(&tmp, parent, &name));
    if unpacked.is_err() {
        let _ = std::fs::remove_dir_all(&tmp);
    }
    unpacked
}

/// Where unpacked entries go, and what is left of the disk and the entry count.
struct Unpack {
    dir: PathBuf,
    left: u64,
    entries: usize,
    cancel: CancelFlag,
}

const NO_SPACE: FileError = FileError::Rejected("There is not enough free space on the disk to unpack this archive");

impl Unpack {
    /// The place for an entry inside the folder, or `None` for one that would lead outside it.
    fn target(&mut self, name: &str) -> Result<Option<PathBuf>, FileError> {
        self.cancel.check()?;
        self.entries += 1;
        if self.entries > MAX_ENTRIES {
            return Err(FileError::Rejected("This archive holds too many files to unpack"));
        }
        let mut target = self.dir.clone();
        let mut parts = 0;
        for part in name.split(['/', '\\']) {
            match part {
                "" | "." => {}
                ".." => return Ok(None),
                _ if parts == 0 && part.len() == 2 && part.ends_with(':') => return Ok(None),
                // Finder's copies of resource forks, which are of no use here.
                "__MACOSX" if parts == 0 => return Ok(None),
                // macOS's hidden companions of files (AppleDouble) and folder settings.
                _ if part.starts_with("._") || part == ".DS_Store" => return Ok(None),
                _ => {
                    let clean: String = part.chars().map(|c| if c == ':' || c.is_control() { '_' } else { c }).collect();
                    target.push(if files::valid_new_component(&clean) { clean } else { "_".into() });
                    parts += 1;
                }
            }
        }
        Ok((parts > 0).then_some(target))
    }

    /// Checks before writing `size` bytes that they fit.
    fn reserve(&self, size: u64) -> Result<(), FileError> {
        if size > self.left { Err(NO_SPACE) } else { Ok(()) }
    }

    fn folder(&mut self, name: &str) -> Result<(), FileError> {
        if let Some(target) = self.target(name)? {
            std::fs::create_dir_all(target).map_err(files::map_io)?;
        }
        Ok(())
    }

    /// Writes one file, keeping its date and whether it can be run when the archive says.
    fn file(&mut self, name: &str, reader: &mut dyn Read, modified: Option<std::time::SystemTime>, mode: Option<u32>) -> Result<(), FileError> {
        let Some(target) = self.target(name)? else { return Ok(()) };
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(files::map_io)?;
        }
        let mut file = File::create(&target).map_err(files::map_io)?;
        let mut buffer = vec![0u8; 256 * 1024];
        loop {
            self.cancel.check()?;
            let read = reader.read(&mut buffer).map_err(|_| FileError::Rejected("The archive is damaged, or protected by a password"))?;
            if read == 0 {
                break;
            }
            self.left = self.left.checked_sub(read as u64).ok_or(NO_SPACE)?;
            file.write_all(&buffer[..read]).map_err(files::map_io)?;
        }
        if let Some(modified) = modified {
            let _ = file.set_modified(modified);
        }
        #[cfg(unix)]
        if mode.is_some_and(|mode| mode & 0o111 != 0) {
            use std::os::unix::fs::PermissionsExt;
            let _ = file.set_permissions(std::fs::Permissions::from_mode(0o755));
        }
        #[cfg(not(unix))]
        let _ = mode;
        Ok(())
    }
}

fn unpack(path: &Path, name: &str, out: &mut Unpack) -> Result<(), FileError> {
    let damaged = |_| FileError::Rejected("The archive is damaged or not complete");
    let Some(format) = format(name) else {
        let lower = name.to_ascii_lowercase();
        let (inner, kind) = single_compressed(&lower).ok_or(FileError::Rejected("OwnNAS cannot unpack this kind of file"))?;
        let file = BufReader::new(File::open(path).map_err(files::map_io)?);
        let mut reader: Box<dyn Read> = match kind {
            "gz" => Box::new(flate2::read::MultiGzDecoder::new(file)),
            "bz2" => Box::new(bzip2::read::MultiBzDecoder::new(file)),
            _ => Box::new(xz2::read::XzDecoder::new_multi_decoder(file)),
        };
        let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        return out.file(&name[..inner], &mut reader, modified, None);
    };
    match format {
        Format::Zip => {
            let mut zip = open_zip(path).map_err(|_| FileError::Rejected("Could not read the zip file"))?;
            out.reserve((0..zip.len()).filter_map(|i| zip.by_index_raw(i).ok().map(|f| f.size())).sum())?;
            for index in 0..zip.len() {
                let mut file = zip.by_index(index).map_err(|err| match err {
                    zip::result::ZipError::UnsupportedArchive(_) => FileError::Rejected("This zip is protected by a password, which OwnNAS cannot open"),
                    _ => FileError::Rejected("The zip file is damaged"),
                })?;
                let entry = file.name().to_string();
                if file.is_dir() {
                    out.folder(&entry)?;
                } else if !file.is_symlink() {
                    let modified = file.last_modified().and_then(zip_time);
                    let mode = file.unix_mode();
                    out.file(&entry, &mut file, modified, mode)?;
                }
            }
        }
        Format::Tar | Format::TarGz | Format::TarBz2 | Format::TarXz => {
            let mut archive = tar::Archive::new(tar_reader(path, format).map_err(|_| FileError::Rejected("Could not open the archive"))?);
            for entry in archive.entries().map_err(damaged)? {
                let mut entry = entry.map_err(damaged)?;
                let Ok(entry_path) = entry.path().map(|p| p.to_string_lossy().into_owned()) else { continue };
                let header = entry.header();
                let (kind, modified, mode) = (header.entry_type(), header.mtime().ok(), header.mode().ok());
                if kind.is_dir() {
                    out.folder(&entry_path)?;
                } else if kind.is_file() || kind == tar::EntryType::Continuous {
                    let modified = modified.map(|secs| std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs));
                    out.file(&entry_path, &mut entry, modified, mode)?;
                }
            }
        }
        Format::SevenZip => {
            let mut reader = sevenz_rust2::ArchiveReader::open(path, sevenz_rust2::Password::empty()).map_err(|err| seven_zip_fault(&err))?;
            out.reserve(reader.archive().files.iter().map(|f| f.size).sum())?;
            let mut failed = None;
            let result = reader.for_each_entries(|entry, data| {
                let done = if entry.is_directory {
                    out.folder(&entry.name)
                } else if entry.is_anti_item {
                    Ok(())
                } else {
                    let modified = entry.has_last_modified_date.then(|| entry.last_modified_date.into());
                    out.file(&entry.name, data, modified, None)
                };
                match done {
                    Ok(()) => Ok(true),
                    Err(err) => {
                        failed = Some(err);
                        Ok(false)
                    }
                }
            });
            if let Some(err) = failed {
                return Err(err);
            }
            result.map_err(|err| seven_zip_fault(&err))?;
        }
        Format::Iso => {
            let entries = iso_entries(path, MAX_ENTRIES + 1).map_err(|_| FileError::Rejected("Could not read the disc image"))?;
            out.reserve(entries.iter().filter(|e| !e.3).map(|e| e.2).sum())?;
            let mut image = File::open(path).map_err(files::map_io)?;
            for (entry, extent, size, dir) in entries {
                if dir {
                    out.folder(&entry)?;
                } else {
                    std::io::Seek::seek(&mut image, std::io::SeekFrom::Start(extent * ISO_SECTOR)).map_err(files::map_io)?;
                    out.file(&entry, &mut (&mut image).take(size), None, None)?;
                }
            }
        }
        Format::Rar => {
            let rar_fault = |_| FileError::Rejected("Could not read the RAR file (is it protected by a password, or one part of several?)");
            let mut archive = unrar::Archive::new(path).open_for_processing().map_err(rar_fault)?;
            while let Some(header) = archive.read_header().map_err(rar_fault)? {
                let entry = header.entry();
                let entry_name = entry.filename.to_string_lossy().into_owned();
                let (dir, size) = (entry.is_directory(), entry.unpacked_size);
                let target = out.target(&entry_name)?;
                archive = match target {
                    Some(target) if dir => {
                        std::fs::create_dir_all(&target).map_err(files::map_io)?;
                        header.skip().map_err(rar_fault)?
                    }
                    Some(target) => {
                        out.reserve(size)?;
                        out.left -= size;
                        if let Some(parent) = target.parent() {
                            std::fs::create_dir_all(parent).map_err(files::map_io)?;
                        }
                        header.extract_to(&target).map_err(rar_fault)?
                    }
                    None => header.skip().map_err(rar_fault)?,
                };
            }
        }
    }
    Ok(())
}

/// For `name.gz`, `name.bz2`, and `name.xz` (not tar files): the length of `name`, and the kind.
fn single_compressed(lower: &str) -> Option<(usize, &'static str)> {
    let (stem, ext) = lower.rsplit_once('.')?;
    let kind = match ext {
        "gz" => "gz",
        "bz2" => "bz2",
        "xz" => "xz",
        _ => return None,
    };
    (!stem.is_empty() && !stem.ends_with(".tar")).then_some((stem.len(), kind))
}

/// Moves what was unpacked into `tmp` beside the archive. Returns where it went.
fn place(tmp: &Path, parent: &Path, archive: &str) -> Result<PathBuf, FileError> {
    let mut top: Vec<_> = std::fs::read_dir(tmp).map_err(files::map_io)?.flatten().collect();
    if top.is_empty() {
        return Err(FileError::Rejected("There was nothing in the archive to unpack"));
    }
    if top.len() == 1 {
        let only = top.remove(0);
        let dest = unique_name(parent, &only.file_name().to_string_lossy());
        std::fs::rename(only.path(), &dest).map_err(files::map_io)?;
        let _ = std::fs::remove_dir(tmp);
        return Ok(dest);
    }
    let dest = unique_name(parent, archive_stem(archive));
    std::fs::rename(tmp, &dest).map_err(files::map_io)?;
    Ok(dest)
}

/// `name` in `parent`, or "name 2", "name 3"… (before the extension) when it is taken.
fn unique_name(parent: &Path, name: &str) -> PathBuf {
    let first = parent.join(name);
    if std::fs::symlink_metadata(&first).is_err() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, format!(".{ext}")),
        _ => (name, String::new()),
    };
    (2..)
        .map(|n| parent.join(format!("{stem} {n}{ext}")))
        .find(|candidate| std::fs::symlink_metadata(candidate).is_err())
        .unwrap_or(first)
}

/// The archive's name without what makes it an archive: "photos" for "photos.tar.gz".
fn archive_stem(name: &str) -> &str {
    let lower = name.to_ascii_lowercase();
    for ext in [".tar.gz", ".tar.bz2", ".tar.xz"] {
        if lower.ends_with(ext) && lower.len() > ext.len() {
            return &name[..name.len() - ext.len()];
        }
    }
    match name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => name,
    }
}

/// A zip's date, which is in the local time of whoever made it.
fn zip_time(time: zip::DateTime) -> Option<std::time::SystemTime> {
    use chrono::TimeZone;
    let date = chrono::NaiveDate::from_ymd_opt(time.year().into(), time.month().into(), time.day().into())?;
    let at = date.and_hms_opt(time.hour().into(), time.minute().into(), time.second().into())?;
    chrono::Local.from_local_datetime(&at).earliest().map(Into::into)
}

fn seven_zip_fault(err: &sevenz_rust2::Error) -> FileError {
    if seven_zip_locked(err) {
        FileError::Rejected("This 7-Zip archive is protected by a password, which OwnNAS cannot open")
    } else {
        FileError::Rejected("The 7-Zip file is damaged")
    }
}

const ISO_SECTOR: u64 = 2048;

/// Every file and folder on an ISO 9660 disc image, as (path, first sector, size, folder),
/// up to `limit`. Joliet's names (long, any language) are used when the disc has them.
fn iso_entries(path: &Path, limit: usize) -> Result<Vec<(String, u64, u64, bool)>, String> {
    use std::io::Seek;
    let mut file = File::open(path).map_err(|_| "Could not open the disc image".to_string())?;
    let mut read_at = |sector: u64, length: usize| -> Result<Vec<u8>, String> {
        file.seek(std::io::SeekFrom::Start(sector * ISO_SECTOR)).map_err(|_| "Could not read the disc image".to_string())?;
        let mut bytes = vec![0; length];
        file.read_exact(&mut bytes).map_err(|_| "Could not read the disc image".to_string())?;
        Ok(bytes)
    };
    // The volume descriptors, from sector 16: the primary one, and Joliet's (a supplementary
    // one whose escape sequence names UCS-2).
    let mut root: Option<(u64, u64, bool)> = None;
    for sector in 16..64 {
        let descriptor = read_at(sector, 2048)?;
        if &descriptor[1..6] != b"CD001" {
            return Err("This is not a CD or DVD image (or it uses only the UDF format)".into());
        }
        let kind = descriptor[0];
        let record = &descriptor[156..190];
        let extent = u64::from(u32::from_le_bytes(record[2..6].try_into().unwrap()));
        let length = u64::from(u32::from_le_bytes(record[10..14].try_into().unwrap()));
        if kind == 1 && root.is_none() {
            root = Some((extent, length, false));
        }
        if kind == 2 && matches!(&descriptor[88..91], b"%/@" | b"%/C" | b"%/E") {
            root = Some((extent, length, true));
        }
        if kind == 255 {
            break;
        }
    }
    let (extent, length, joliet) = root.ok_or("This disc image has no file list")?;
    let mut found = Vec::new();
    let mut folders = vec![(String::new(), extent, length)];
    let mut visited = 0;
    while let Some((prefix, extent, length)) = folders.pop() {
        visited += 1;
        if visited > 100_000 || length > 64 * 1024 * 1024 {
            break;
        }
        let data = read_at(extent, length as usize)?;
        let mut at = 0;
        while at < data.len() {
            let size = usize::from(data[at]);
            if size == 0 {
                // Records do not cross sectors: the rest of this one is empty.
                at = (at / ISO_SECTOR as usize + 1) * ISO_SECTOR as usize;
                continue;
            }
            let Some(record) = data.get(at..at + size) else { break };
            at += size;
            let name_len = usize::from(record[32]);
            let Some(raw) = record.get(33..33 + name_len) else { continue };
            // "." and ".." are a single byte, 0 or 1.
            if name_len == 1 && raw[0] <= 1 {
                continue;
            }
            let mut name = if joliet {
                String::from_utf16_lossy(&raw.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect::<Vec<_>>())
            } else {
                String::from_utf8_lossy(raw).into_owned()
            };
            if let Some((base, _)) = name.rsplit_once(';') {
                name = base.trim_end_matches('.').to_string();
            }
            let entry_extent = u64::from(u32::from_le_bytes(record[2..6].try_into().unwrap()));
            let entry_size = u64::from(u32::from_le_bytes(record[10..14].try_into().unwrap()));
            let dir = record[25] & 0x02 != 0;
            let full = format!("{prefix}{name}");
            if dir {
                found.push((format!("{full}/"), entry_extent, 0, true));
                folders.push((format!("{full}/"), entry_extent, entry_size));
            } else {
                found.push((full, entry_extent, entry_size, false));
            }
            if found.len() >= limit {
                return Ok(found);
            }
        }
    }
    Ok(found)
}

fn open_zip(path: &Path) -> Result<zip::ZipArchive<BufReader<File>>, String> {
    let file = File::open(path).map_err(|_| "Could not open the archive".to_string())?;
    zip::ZipArchive::new(BufReader::new(file)).map_err(|_| "Could not read the zip file".to_string())
}

fn tar_reader(path: &Path, format: Format) -> Result<Box<dyn Read>, String> {
    let file = BufReader::new(File::open(path).map_err(|_| "Could not open the archive".to_string())?);
    Ok(match format {
        Format::TarGz => Box::new(flate2::read::GzDecoder::new(file)),
        Format::TarBz2 => Box::new(bzip2::read::BzDecoder::new(file)),
        Format::TarXz => Box::new(xz2::read::XzDecoder::new(file)),
        _ => Box::new(file),
    })
}

fn read_limited(reader: impl Read, max: u64) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    reader.take(max + 1).read_to_end(&mut bytes).map_err(|_| "This file could not be unpacked (is the archive protected by a password?)".to_string())?;
    if bytes.len() as u64 > max {
        return Err("This file is too large".into());
    }
    Ok(bytes)
}

/// Whether 7-Zip's error means the archive is encrypted (AES is not built in, so an encrypted
/// archive reads as one in an unknown format).
fn seven_zip_locked(err: &sevenz_rust2::Error) -> bool {
    match err {
        sevenz_rust2::Error::PasswordRequired | sevenz_rust2::Error::MaybeBadPassword(_) => true,
        sevenz_rust2::Error::UnsupportedCompressionMethod(method) => method.starts_with("AES"),
        _ => err.to_string().to_lowercase().contains("password"),
    }
}

fn seven_zip_error(err: &sevenz_rust2::Error) -> String {
    if seven_zip_locked(err) {
        "This 7-Zip archive is protected by a password".into()
    } else {
        "Could not read the 7-Zip file".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn lists_and_reads_zip_tar_and_7z() {
        let dir = std::env::temp_dir().join(format!("ownnas-archives-{}-{}", std::process::id(), rand::random::<u32>()));
        std::fs::create_dir_all(dir.join("src/pages")).unwrap();
        std::fs::write(dir.join("src/pages/001.txt"), b"first page").unwrap();
        std::fs::write(dir.join("src/readme.md"), b"hello").unwrap();

        let zip_path = dir.join("book.cbz");
        let mut zip = zip::ZipWriter::new(File::create(&zip_path).unwrap());
        zip.start_file("pages/001.txt", zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(b"first page").unwrap();
        zip.finish().unwrap();
        assert_eq!(list(&zip_path, "book.cbz", 10).unwrap().0, [Entry { name: "pages/001.txt".into(), size: 10, dir: false }]);
        assert_eq!(read(&zip_path, "book.cbz", "pages/001.txt", 100).unwrap(), b"first page");
        assert!(read(&zip_path, "book.cbz", "pages/001.txt", 5).is_err());

        let tar_path = dir.join("files.tar.bz2");
        let mut builder = tar::Builder::new(bzip2::write::BzEncoder::new(File::create(&tar_path).unwrap(), bzip2::Compression::fast()));
        builder.append_path_with_name(dir.join("src/readme.md"), "readme.md").unwrap();
        builder.into_inner().unwrap().finish().unwrap();
        assert_eq!(list(&tar_path, "files.tar.bz2", 10).unwrap().0.len(), 1);
        assert_eq!(read(&tar_path, "files.tar.bz2", "readme.md", 100).unwrap(), b"hello");

        // A tiny ISO 9660 image: its volume descriptor, root folder, and one file.
        let iso_path = dir.join("disc.iso");
        let mut iso = vec![0u8; 2048 * 20];
        let record = |extent: u32, size: u32, flags: u8, name: &[u8]| {
            let mut r = vec![0u8; 33 + name.len() + (name.len() + 1) % 2];
            r[0] = r.len() as u8;
            r[2..6].copy_from_slice(&extent.to_le_bytes());
            r[10..14].copy_from_slice(&size.to_le_bytes());
            r[25] = flags;
            r[32] = name.len() as u8;
            r[33..33 + name.len()].copy_from_slice(name);
            r
        };
        let pvd = 16 * 2048;
        iso[pvd] = 1;
        iso[pvd + 1..pvd + 6].copy_from_slice(b"CD001");
        let root = record(18, 2048, 2, &[0]);
        iso[pvd + 156..pvd + 156 + root.len()].copy_from_slice(&root);
        iso[17 * 2048] = 255;
        iso[17 * 2048 + 1..17 * 2048 + 6].copy_from_slice(b"CD001");
        let mut folder = record(18, 2048, 2, &[0]);
        folder.extend(record(18, 2048, 2, &[1]));
        folder.extend(record(19, 5, 0, b"HELLO.TXT;1"));
        iso[18 * 2048..18 * 2048 + folder.len()].copy_from_slice(&folder);
        iso[19 * 2048..19 * 2048 + 5].copy_from_slice(b"hello");
        std::fs::write(&iso_path, &iso).unwrap();
        assert_eq!(list(&iso_path, "disc.iso", 10).unwrap().0, [Entry { name: "HELLO.TXT".into(), size: 5, dir: false }]);
        assert_eq!(read(&iso_path, "disc.iso", "HELLO.TXT", 100).unwrap(), b"hello");

        let (entries, more) = list(&zip_path, "book.cbz", 0).unwrap();
        assert!(entries.is_empty() && more);
        assert_eq!(format("x.docx"), Some(Format::Zip));
        assert_eq!(format("x.gz"), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn extracts_beside_the_archive_and_keeps_inside_the_folder() {
        let dir = std::env::temp_dir().join(format!("ownnas-extract-{}-{}", std::process::id(), rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        let cancel = CancelFlag::new();

        // Two things at the top, one of them trying to get out: a new folder named after it.
        let zip_path = dir.join("photos.zip");
        let mut zip = zip::ZipWriter::new(File::create(&zip_path).unwrap());
        for (name, body) in [("a.txt", "a"), ("sub/b.txt", "b"), ("../escape.txt", "x"), ("__MACOSX/._a.txt", "x"), ("sub/._b.txt", "x")] {
            zip.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        let made = extract(&zip_path, &cancel).unwrap();
        assert_eq!(made, dir.join("photos"));
        assert_eq!(std::fs::read(made.join("sub/b.txt")).unwrap(), b"b");
        assert!(!dir.join("escape.txt").exists() && !made.join("__MACOSX").exists() && !made.join("sub/._b.txt").exists());
        // Again: a new name, nothing overwritten.
        assert_eq!(extract(&zip_path, &cancel).unwrap(), dir.join("photos 2"));

        // One folder at the top goes beside the archive as it is.
        std::fs::create_dir_all(dir.join("src/project")).unwrap();
        std::fs::write(dir.join("src/project/readme.md"), b"hello").unwrap();
        let tar_path = dir.join("project.tar.gz");
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(File::create(&tar_path).unwrap(), flate2::Compression::fast()));
        builder.append_dir_all("project", dir.join("src/project")).unwrap();
        builder.into_inner().unwrap().finish().unwrap();
        assert_eq!(extract(&tar_path, &cancel).unwrap(), dir.join("project"));
        assert_eq!(std::fs::read(dir.join("project/readme.md")).unwrap(), b"hello");

        // A single compressed file.
        let gz_path = dir.join("notes.txt.gz");
        let mut gz = flate2::write::GzEncoder::new(File::create(&gz_path).unwrap(), flate2::Compression::fast());
        gz.write_all(b"notes").unwrap();
        gz.finish().unwrap();
        assert_eq!(extract(&gz_path, &cancel).unwrap(), dir.join("notes.txt"));
        assert_eq!(std::fs::read(dir.join("notes.txt")).unwrap(), b"notes");

        assert!(extractable("a.7z") && extractable("a.tar.xz") && extractable("a.bz2") && !extractable("a.docx") && !extractable(".gz"));
        assert_eq!(archive_stem("Photos.Tar.GZ"), "Photos");
        let leftovers = std::fs::read_dir(&dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with(".ownnas-extract")).count();
        assert_eq!(leftovers, 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// `OWNNAS_ARCHIVE_SAMPLES=dir`: lists and reads the first file of each real archive there.
    #[test]
    fn real_samples() {
        let Ok(dir) = std::env::var("OWNNAS_ARCHIVE_SAMPLES") else { return };
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let listed = list(&entry.path(), &name, 50);
            let first = listed.as_ref().ok().and_then(|(entries, _)| entries.iter().find(|e| !e.dir).map(|e| e.name.clone()));
            let read_back = first.as_ref().map(|wanted| read(&entry.path(), &name, wanted, 50 * 1024 * 1024).map(|b| b.len()));
            println!("{name}: {:?} first {first:?} -> {read_back:?}", listed.map(|(e, more)| (e.len(), more)));
        }
    }
}
