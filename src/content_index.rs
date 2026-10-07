//! Cached text extraction for content search (PDF text + optional Tesseract OCR).

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant, UNIX_EPOCH};

use lopdf::content::Content;
use lopdf::{Document, Object};
use sha2::{Digest, Sha256};

const MAX_PDF_BYTES: u64 = 80 * 1024 * 1024;
const MAX_PDF_PAGES: usize = 80;
const MAX_PDF_TEXT: usize = 2 * 1024 * 1024;
const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;
const MAX_OCR_TEXT: usize = 512 * 1024;
const OCR_TIMEOUT: Duration = Duration::from_secs(20);
const PDFTOTEXT_TIMEOUT: Duration = Duration::from_secs(30);
/// Bump when extraction logic changes so stale empty caches are ignored.
const PDF_CACHE_KIND: &str = "pdf-v3";
const OCR_CACHE_KIND: &str = "ocr-v1";

pub fn tesseract_available() -> bool {
    Command::new("tesseract")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

pub fn pdftotext_available() -> bool {
    static CACHED: OnceLock<bool> = OnceLock::new();
    *CACHED.get_or_init(|| {
        // poppler's pdftotext prints version to stderr and may exit non-zero.
        Command::new("pdftotext")
            .arg("-v")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .is_ok()
    })
}

pub fn ensure_cache_dir(data_dir: &Path) -> Result<PathBuf, String> {
    let dir = data_dir.join("text-cache");
    fs::create_dir_all(&dir).map_err(|_| "Could not create the text cache".to_string())?;
    Ok(dir)
}

fn cache_path(cache_root: &Path, source: &Path, meta: &fs::Metadata, kind: &str) -> Option<PathBuf> {
    let mut hasher = Sha256::new();
    hasher.update(source.to_string_lossy().as_bytes());
    hasher.update(b"|");
    hasher.update(kind.as_bytes());
    hasher.update(b"|");
    let modified = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    hasher.update(modified.to_le_bytes());
    hasher.update(meta.len().to_le_bytes());
    let digest = hex::encode(hasher.finalize());
    let shard = &digest[..2];
    Some(cache_root.join(shard).join(format!("{digest}.txt")))
}

fn read_cache(path: &Path) -> Option<Option<String>> {
    if !path.is_file() {
        return None;
    }
    let mut file = File::open(path).ok()?;
    let mut buf = String::new();
    file.read_to_string(&mut buf).ok()?;
    // Empty cache file means "indexed, no extractable text".
    Some(if buf.is_empty() {
        Some(String::new())
    } else {
        Some(buf)
    })
}

/// Cache lookup only (no PDF parse / OCR).
pub fn cached_text(
    path: &Path,
    kind: &str,
    cache_root: &Path,
    ocr: bool,
) -> Option<String> {
    let meta = fs::metadata(path).ok()?;
    let cache_kind = match kind {
        "pdf" => PDF_CACHE_KIND,
        "image" if ocr => OCR_CACHE_KIND,
        _ => return None,
    };
    let dest = cache_path(cache_root, path, &meta, cache_kind)?;
    read_cache(&dest)?
}

fn write_cache(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let tmp = path.with_extension("tmp");
    if fs::write(&tmp, text.as_bytes()).is_ok() {
        if path.is_file() {
            let _ = fs::remove_file(path);
        }
        if fs::rename(&tmp, path).is_err() {
            let _ = fs::remove_file(&tmp);
        }
    }
}

/// Returns extracted text for content search. `None` means skip (unsupported / failed).
/// Empty string means indexed with no text.
/// The bool is `true` when extraction did real work (cache miss).
pub fn extract_for_search(
    path: &Path,
    kind: &str,
    cache_root: &Path,
    ocr: bool,
) -> Option<(String, bool)> {
    let meta = fs::metadata(path).ok()?;
    if meta.len() == 0 {
        return Some((String::new(), false));
    }
    let cache_kind = match kind {
        "pdf" => PDF_CACHE_KIND,
        "image" if ocr => OCR_CACHE_KIND,
        _ => return None,
    };
    let dest = cache_path(cache_root, path, &meta, cache_kind)?;
    if let Some(cached) = read_cache(&dest) {
        return cached.map(|text| (text, false));
    }
    let text = match kind {
        "pdf" => extract_pdf_text(path, &meta),
        "image" => ocr_image(path, &meta),
        _ => None,
    };
    match text {
        Some(text) => {
            write_cache(&dest, &text);
            Some((text, true))
        }
        None => {
            // Cache failures as empty so we do not hammer bad files every search.
            write_cache(&dest, "");
            Some((String::new(), true))
        }
    }
}

fn truncate_text(mut text: String, max: usize) -> String {
    if text.len() > max {
        text.truncate(max);
        while !text.is_char_boundary(text.len()) {
            text.pop();
        }
    }
    text
}

fn extract_pdf_text(path: &Path, meta: &fs::Metadata) -> Option<String> {
    if meta.len() > MAX_PDF_BYTES {
        return None;
    }
    // Prefer poppler's pdftotext when available — matches “selectable text” much better
    // than lopdf for modern fonts / ToUnicode maps.
    if let Some(text) = pdftotext_extract(path) {
        if !text.trim().is_empty() {
            return Some(truncate_text(text, MAX_PDF_TEXT));
        }
    }
    let text = lopdf_extract_text(path).unwrap_or_default();
    Some(truncate_text(text, MAX_PDF_TEXT))
}

fn pdftotext_extract(path: &Path) -> Option<String> {
    if !pdftotext_available() {
        return None;
    }
    let mut child = Command::new("pdftotext")
        .args(["-q", "-enc", "UTF-8", "-layout"])
        .arg(path)
        .arg("-")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) => return None,
            Ok(None) if start.elapsed() > PDFTOTEXT_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(30)),
            Err(_) => return None,
        }
    }
    let mut out = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        out = String::from_utf8_lossy(&buf).into_owned();
    }
    Some(out)
}

fn lopdf_extract_text(path: &Path) -> Option<String> {
    let doc = Document::load(path).ok()?;
    let pages: Vec<u32> = doc.get_pages().keys().copied().take(MAX_PDF_PAGES).collect();
    if pages.is_empty() {
        return Some(String::new());
    }
    // Prefer the high-level API when it works for the whole set.
    if let Ok(text) = doc.extract_text(&pages) {
        if !text.trim().is_empty() {
            return Some(text);
        }
    }
    // Fall back to per-page + raw string scrape so one bad font does not wipe everything.
    let mut out = String::new();
    for page in &pages {
        if let Ok(chunk) = doc.extract_text(&[*page]) {
            if !chunk.is_empty() {
                out.push_str(&chunk);
                if !out.ends_with('\n') {
                    out.push('\n');
                }
                continue;
            }
        }
        if let Some(raw) = scrape_page_strings(&doc, *page) {
            out.push_str(&raw);
            if !out.ends_with('\n') {
                out.push('\n');
            }
        }
    }
    Some(out)
}

fn scrape_page_strings(doc: &Document, page_number: u32) -> Option<String> {
    let page_id = *doc.get_pages().get(&page_number)?;
    let content_data = doc.get_page_content(page_id).ok()?;
    let content = Content::decode(&content_data).ok()?;
    let mut out = String::new();
    for operation in &content.operations {
        match operation.operator.as_ref() {
            "Tj" | "'" | "\"" => {
                for operand in &operation.operands {
                    push_pdf_string(&mut out, operand);
                }
            }
            "TJ" => {
                for operand in &operation.operands {
                    match operand {
                        Object::Array(items) => {
                            for item in items {
                                push_pdf_string(&mut out, item);
                                if let Object::Integer(i) = item {
                                    if *i < -80 {
                                        out.push(' ');
                                    }
                                }
                            }
                        }
                        _ => push_pdf_string(&mut out, operand),
                    }
                }
            }
            "ET" | "Td" | "TD" | "T*" => {
                if !out.ends_with('\n') && !out.ends_with(' ') {
                    out.push(' ');
                }
            }
            _ => {}
        }
    }
    if out.trim().is_empty() {
        None
    } else {
        Some(out)
    }
}

fn push_pdf_string(out: &mut String, object: &Object) {
    match object {
        Object::String(bytes, _) => {
            if bytes.len() >= 2 && bytes[0] == 0xfe && bytes[1] == 0xff {
                // UTF-16BE BOM
                let chars = bytes[2..]
                    .chunks(2)
                    .filter_map(|pair| {
                        if pair.len() == 2 {
                            Some(char::from_u32(u32::from(u16::from_be_bytes([pair[0], pair[1]])))?)
                        } else {
                            None
                        }
                    })
                    .collect::<String>();
                out.push_str(&chars);
            } else if bytes.iter().any(|b| *b == 0) && bytes.len() % 2 == 0 {
                // Likely UTF-16BE without BOM
                let chars = bytes
                    .chunks(2)
                    .filter_map(|pair| {
                        if pair.len() == 2 {
                            Some(char::from_u32(u32::from(u16::from_be_bytes([pair[0], pair[1]])))?)
                        } else {
                            None
                        }
                    })
                    .collect::<String>();
                out.push_str(&chars);
            } else {
                out.push_str(&String::from_utf8_lossy(bytes));
            }
        }
        Object::Array(items) => {
            for item in items {
                push_pdf_string(out, item);
            }
        }
        _ => {}
    }
}

fn ocr_image(path: &Path, meta: &fs::Metadata) -> Option<String> {
    if meta.len() > MAX_IMAGE_BYTES {
        return None;
    }
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    // Leptonica/tesseract-friendly formats OwnNAS already treats as images.
    let ok_ext = name.ends_with(".jpg")
        || name.ends_with(".jpeg")
        || name.ends_with(".png")
        || name.ends_with(".tif")
        || name.ends_with(".tiff")
        || name.ends_with(".webp")
        || name.ends_with(".gif")
        || name.ends_with(".bmp");
    if !ok_ext {
        return None;
    }
    let mut child = Command::new("tesseract")
        .arg(path)
        .arg("stdout")
        .arg("-l")
        .arg("eng")
        .arg("--psm")
        .arg("3")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) => return None,
            Ok(None) if start.elapsed() > OCR_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(40)),
            Err(_) => return None,
        }
    }
    let mut out = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        out = String::from_utf8_lossy(&buf).into_owned();
    }
    Some(truncate_text(out, MAX_OCR_TEXT))
}

/// True if `needle` appears in `haystack`, allowing PDF-style letter spacing
/// (e.g. "H e l l o" still matches "hello").
pub fn text_matches(haystack: &str, needle: &str) -> bool {
    let hay = haystack.to_lowercase();
    let needle = needle.to_lowercase();
    if needle.is_empty() {
        return false;
    }
    if hay.contains(&needle) {
        return true;
    }
    let compact_hay: String = hay.chars().filter(|c| !c.is_whitespace()).collect();
    let compact_needle: String = needle.chars().filter(|c| !c.is_whitespace()).collect();
    !compact_needle.is_empty() && compact_hay.contains(&compact_needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::content::{Content, Operation};
    use lopdf::{dictionary, Object, Stream};

    fn make_pdf(path: &Path, label: &str) {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Courier",
        });
        let resources_id = doc.add_object(dictionary! {
            "Font" => dictionary! { "F1" => font_id },
        });
        let content = Content {
            operations: vec![
                Operation::new("BT", vec![]),
                Operation::new("Tf", vec!["F1".into(), 24.into()]),
                Operation::new("Td", vec![72.into(), 720.into()]),
                Operation::new("Tj", vec![Object::string_literal(label)]),
                Operation::new("ET", vec![]),
            ],
        };
        let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
            "Resources" => resources_id,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
        });
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page_id.into()],
                "Count" => 1,
            }),
        );
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        doc.trailer.set("Root", catalog_id);
        doc.save(path).unwrap();
    }

    #[test]
    fn pdf_text_is_cached() {
        let root = std::env::temp_dir().join(format!("ownnas-text-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let pdf = root.join("note.pdf");
        make_pdf(&pdf, "UniqueSearchTokenXYZ");
        let cache = root.join("cache");
        fs::create_dir_all(&cache).unwrap();
        let text = extract_for_search(&pdf, "pdf", &cache, false).unwrap().0;
        assert!(
            text_matches(&text, "uniquesearchtokenxyz"),
            "extracted={text:?}"
        );
        let meta = fs::metadata(&pdf).unwrap();
        let dest = cache_path(&cache, &pdf, &meta, PDF_CACHE_KIND).unwrap();
        assert!(dest.is_file());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn spaced_letters_still_match() {
        assert!(text_matches("H e l l o   World", "hello"));
        assert!(!text_matches("H e l l o", "help"));
    }
}
