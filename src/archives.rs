//! Looking inside archives without unpacking them: what they hold, and one file from inside.
//! Zip (and the formats built on it: comics, office files, EPUB), tar (plain, gzip, bzip2, xz),
//! 7-Zip (`sevenz-rust2`, pure Rust), and RAR (the `unrar` library, built in).

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

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
        "zip" | "jar" | "cbz" | "epub" | "docx" | "xlsx" | "pptx" | "ppsx" | "odt" | "ods" | "odp" | "pages" | "numbers" | "key" | "apk" => Some(Format::Zip),
        "7z" | "cb7" => Some(Format::SevenZip),
        "rar" | "cbr" => Some(Format::Rar),
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

fn seven_zip_error(err: &sevenz_rust2::Error) -> String {
    let text = err.to_string().to_lowercase();
    if text.contains("password") {
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

        let (entries, more) = list(&zip_path, "book.cbz", 0).unwrap();
        assert!(entries.is_empty() && more);
        assert_eq!(format("x.docx"), Some(Format::Zip));
        assert_eq!(format("x.gz"), None);
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
