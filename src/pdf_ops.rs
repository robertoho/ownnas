//! PDF page tools: extract, split, merge, rotate (via lopdf).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use lopdf::{Document, Object, ObjectId};
use serde::Serialize;

use crate::files::{self, FileError};

const MAX_PDF_BYTES: u64 = 120 * 1024 * 1024;
const MAX_MERGE_FILES: usize = 40;
const MAX_SPLIT_PAGES: usize = 250;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfInfo {
    pub path: String,
    pub pages: u32,
    pub bytes: u64,
}

fn ensure_pdf(root: &Path, rel: &str) -> Result<files::Resolved, FileError> {
    let target = files::resolve(root, rel)?;
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
    if files::kind_of(&name) != "pdf" {
        return Err(FileError::Rejected("Only PDF files are supported"));
    }
    let meta = fs::metadata(&target.full).map_err(files::map_io)?;
    if meta.len() > MAX_PDF_BYTES {
        return Err(FileError::Rejected("PDF is too large (120 MB max)"));
    }
    Ok(target)
}

fn load_pdf(path: &Path) -> Result<Document, FileError> {
    Document::load(path).map_err(|_| FileError::Rejected("Could not open the PDF (encrypted or unsupported)"))
}

fn load_pdf_mem(bytes: &[u8]) -> Result<Document, FileError> {
    Document::load_mem(bytes).map_err(|_| FileError::Rejected("Could not open the PDF (encrypted or unsupported)"))
}

fn stem_of(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "document".to_string())
}

fn parent_rel_of(rel: &str) -> String {
    rel.rsplit_once('/')
        .map(|(p, _)| p.to_string())
        .unwrap_or_default()
}

fn join_rel(parent_rel: &str, name: &str) -> String {
    if parent_rel.is_empty() {
        name.to_string()
    } else {
        format!("{parent_rel}/{name}")
    }
}

fn tmp_beside(parent: &Path, tag: &str) -> PathBuf {
    parent.join(format!(
        ".ownnas-pdf-{tag}-{}-{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ))
}

fn save_doc_atomic(doc: &mut Document, dest: &Path) -> Result<(), FileError> {
    let parent = dest.parent().ok_or(FileError::Forbidden)?;
    let tmp = tmp_beside(parent, "save");
    {
        let mut file = fs::File::create(&tmp).map_err(files::map_io)?;
        doc.save_to(&mut file)
            .map_err(|_| FileError::Rejected("Could not write the PDF"))?;
        file.flush().map_err(files::map_io)?;
    }
    fs::rename(&tmp, dest).map_err(|err| {
        let _ = fs::remove_file(&tmp);
        files::map_io(err)
    })?;
    Ok(())
}

fn dest_name(dest: &Path, fallback: &str) -> String {
    dest.file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| fallback.to_string())
}

/// Parses specs like `1,3,5-8` (1-based). Empty means all pages when `allow_all`.
pub fn parse_page_spec(spec: &str, page_count: u32, allow_all: bool) -> Result<Vec<u32>, FileError> {
    if page_count == 0 {
        return Err(FileError::Rejected("This PDF has no pages"));
    }
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        if allow_all {
            return Ok((1..=page_count).collect());
        }
        return Err(FileError::Rejected("Enter at least one page number"));
    }
    let mut pages = BTreeSet::new();
    for part in trimmed.split(|c: char| c == ',' || c == ';' || c.is_whitespace()) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((a, b)) = part.split_once('-') {
            let start: u32 = a
                .trim()
                .parse()
                .map_err(|_| FileError::Rejected("Invalid page range"))?;
            let end: u32 = b
                .trim()
                .parse()
                .map_err(|_| FileError::Rejected("Invalid page range"))?;
            if start == 0 || end == 0 || start > end {
                return Err(FileError::Rejected("Invalid page range"));
            }
            if end > page_count {
                return Err(FileError::Rejected("Page number is past the end of the PDF"));
            }
            for n in start..=end {
                pages.insert(n);
            }
        } else {
            let n: u32 = part
                .parse()
                .map_err(|_| FileError::Rejected("Invalid page number"))?;
            if n == 0 {
                return Err(FileError::Rejected("Page numbers start at 1"));
            }
            if n > page_count {
                return Err(FileError::Rejected("Page number is past the end of the PDF"));
            }
            pages.insert(n);
        }
    }
    if pages.is_empty() {
        return Err(FileError::Rejected("Enter at least one page number"));
    }
    Ok(pages.into_iter().collect())
}

pub fn pdf_info(root: &Path, rel: &str) -> Result<PdfInfo, FileError> {
    let target = ensure_pdf(root, rel)?;
    let meta = fs::metadata(&target.full).map_err(files::map_io)?;
    let doc = load_pdf(&target.full)?;
    let pages = doc.get_pages().len() as u32;
    Ok(PdfInfo {
        path: target.rel,
        pages,
        bytes: meta.len(),
    })
}

fn keep_only_pages(doc: &mut Document, keep: &[u32]) -> Result<(), FileError> {
    if keep.is_empty() {
        return Err(FileError::Rejected("No pages selected"));
    }
    let existing: BTreeSet<u32> = doc.get_pages().keys().copied().collect();
    for page in keep {
        if !existing.contains(page) {
            return Err(FileError::Rejected("Page number is past the end of the PDF"));
        }
    }
    let keep_set: BTreeSet<u32> = keep.iter().copied().collect();
    let to_delete: Vec<u32> = existing.into_iter().filter(|p| !keep_set.contains(p)).collect();
    if !to_delete.is_empty() {
        doc.delete_pages(&to_delete);
    }
    let _ = doc.prune_objects();
    Ok(())
}

/// Extracts pages into one PDF (`combined`) or one file per page (`separate`).
pub fn pdf_extract(
    root: &Path,
    rel: &str,
    pages_spec: &str,
    mode: &str,
) -> Result<Vec<String>, FileError> {
    let target = ensure_pdf(root, rel)?;
    let bytes = fs::read(&target.full).map_err(files::map_io)?;
    let probe = load_pdf_mem(&bytes)?;
    let page_count = probe.get_pages().len() as u32;
    let pages = parse_page_spec(pages_spec, page_count, false)?;
    let parent = target.full.parent().ok_or(FileError::Forbidden)?;
    let parent_rel = parent_rel_of(&target.rel);
    let stem = stem_of(&target.full);
    let mode = mode.trim().to_ascii_lowercase();

    match mode.as_str() {
        "combined" | "one" | "single" => {
            let mut doc = load_pdf_mem(&bytes)?;
            keep_only_pages(&mut doc, &pages)?;
            let label = if pages.len() == 1 {
                format!("{stem}-p{}.pdf", pages[0])
            } else {
                format!("{stem}-pages.pdf")
            };
            if !files::valid_new_component(&label) {
                return Err(FileError::InvalidName);
            }
            let dest = files::unique_path(parent.join(&label));
            let name = dest_name(&dest, &label);
            save_doc_atomic(&mut doc, &dest)?;
            Ok(vec![join_rel(&parent_rel, &name)])
        }
        "separate" | "each" | "split" => {
            if pages.len() > MAX_SPLIT_PAGES {
                return Err(FileError::Rejected("Too many pages to export individually"));
            }
            let mut created = Vec::with_capacity(pages.len());
            for page in &pages {
                let mut doc = load_pdf_mem(&bytes)?;
                keep_only_pages(&mut doc, &[*page])?;
                let label = format!("{stem}-p{page}.pdf");
                if !files::valid_new_component(&label) {
                    return Err(FileError::InvalidName);
                }
                let dest = files::unique_path(parent.join(&label));
                let name = dest_name(&dest, &label);
                save_doc_atomic(&mut doc, &dest)?;
                created.push(join_rel(&parent_rel, &name));
            }
            Ok(created)
        }
        _ => Err(FileError::Rejected("Use combined or separate export mode")),
    }
}

/// Writes every page of a PDF as its own sibling file.
pub fn pdf_split_all(root: &Path, rel: &str) -> Result<Vec<String>, FileError> {
    let info = pdf_info(root, rel)?;
    if info.pages == 0 {
        return Err(FileError::Rejected("This PDF has no pages"));
    }
    if info.pages > MAX_SPLIT_PAGES as u32 {
        return Err(FileError::Rejected("Too many pages to split (250 max)"));
    }
    let all = format!("1-{}", info.pages);
    pdf_extract(root, rel, &all, "separate")
}

/// Merges PDFs in selection order into one sibling file.
pub fn pdf_merge(root: &Path, rels: &[String]) -> Result<String, FileError> {
    if rels.len() < 2 {
        return Err(FileError::Rejected("Select at least two PDFs to join"));
    }
    if rels.len() > MAX_MERGE_FILES {
        return Err(FileError::Rejected("Too many PDFs to join at once"));
    }
    let mut resolved = Vec::with_capacity(rels.len());
    for rel in rels {
        resolved.push(ensure_pdf(root, rel)?);
    }
    let parent_rel = shared_parent(&resolved)?;
    let parent = files::resolve(root, &parent_rel)?;
    if !parent.full.is_dir() {
        return Err(FileError::NotADirectory);
    }
    let label = merge_label(&resolved);
    let archive_name = format!("{label}-joined.pdf");
    if !files::valid_new_component(&archive_name) {
        return Err(FileError::InvalidName);
    }
    let dest = files::unique_path(parent.full.join(&archive_name));
    let dest_name = dest_name(&dest, &archive_name);

    let mut documents = Vec::with_capacity(resolved.len());
    for item in &resolved {
        documents.push(load_pdf(&item.full)?);
    }
    let mut merged = merge_documents(documents)?;
    save_doc_atomic(&mut merged, &dest)?;
    Ok(join_rel(&parent_rel, &dest_name))
}

/// Rotates selected pages (or all when `pages_spec` is empty) by 90/180/270 degrees.
/// Writes a sibling file; original is kept.
pub fn pdf_rotate(
    root: &Path,
    rel: &str,
    pages_spec: &str,
    degrees: i64,
) -> Result<String, FileError> {
    if degrees % 90 != 0 || degrees == 0 {
        return Err(FileError::Rejected("Rotation must be 90, 180, or 270 degrees"));
    }
    let degrees = ((degrees % 360) + 360) % 360;
    if degrees == 0 {
        return Err(FileError::Rejected("Rotation must be 90, 180, or 270 degrees"));
    }
    let target = ensure_pdf(root, rel)?;
    let mut doc = load_pdf(&target.full)?;
    let page_count = doc.get_pages().len() as u32;
    let pages = parse_page_spec(pages_spec, page_count, true)?;
    let page_map = doc.get_pages();
    for page in &pages {
        let page_id = *page_map
            .get(page)
            .ok_or(FileError::Rejected("Page number is past the end of the PDF"))?;
        let page_dict = doc
            .get_object_mut(page_id)
            .and_then(|obj| obj.as_dict_mut())
            .map_err(|_| FileError::Rejected("Could not update a PDF page"))?;
        let current = page_dict
            .get(b"Rotate")
            .and_then(|obj| obj.as_i64())
            .unwrap_or(0);
        page_dict.set("Rotate", (current + degrees) % 360);
    }
    let parent = target.full.parent().ok_or(FileError::Forbidden)?;
    let parent_rel = parent_rel_of(&target.rel);
    let stem = stem_of(&target.full);
    let label = format!("{stem}-rotated.pdf");
    if !files::valid_new_component(&label) {
        return Err(FileError::InvalidName);
    }
    let dest = files::unique_path(parent.join(&label));
    let name = dest_name(&dest, &label);
    save_doc_atomic(&mut doc, &dest)?;
    Ok(join_rel(&parent_rel, &name))
}

fn shared_parent(items: &[files::Resolved]) -> Result<String, FileError> {
    let mut parent: Option<String> = None;
    for item in items {
        let p = parent_rel_of(&item.rel);
        match &parent {
            None => parent = Some(p),
            Some(existing) if *existing == p => {}
            _ => {
                return Err(FileError::Rejected(
                    "Join PDFs that live in the same folder",
                ))
            }
        }
    }
    Ok(parent.unwrap_or_default())
}

fn merge_label(items: &[files::Resolved]) -> String {
    if items.len() == 2 {
        let a = stem_of(&items[0].full);
        let b = stem_of(&items[1].full);
        let combined = format!("{a}+{b}");
        if combined.len() <= 80 && files::valid_new_component(&format!("{combined}-joined.pdf")) {
            return combined;
        }
    }
    format!("{}-pdfs", items.len())
}

fn merge_documents(documents: Vec<Document>) -> Result<Document, FileError> {
    let mut max_id = 1u32;
    let mut documents_pages: BTreeMap<ObjectId, Object> = BTreeMap::new();
    let mut documents_objects: BTreeMap<ObjectId, Object> = BTreeMap::new();
    let mut document = Document::with_version("1.5");

    for mut doc in documents {
        doc.renumber_objects_with(max_id);
        max_id = doc.max_id + 1;
        for (object_id, object) in doc.get_pages().into_values().filter_map(|id| {
            doc.get_object(id)
                .ok()
                .map(|object| (id, object.to_owned()))
        }) {
            documents_pages.insert(object_id, object);
        }
        documents_objects.extend(doc.objects);
    }

    if documents_pages.is_empty() {
        return Err(FileError::Rejected("No pages found to join"));
    }

    let mut catalog_object: Option<(ObjectId, Object)> = None;
    let mut pages_object: Option<(ObjectId, Object)> = None;

    for (object_id, object) in documents_objects {
        match object.type_name().unwrap_or("") {
            "Catalog" => {
                catalog_object = Some((
                    catalog_object.map(|(id, _)| id).unwrap_or(object_id),
                    object,
                ));
            }
            "Pages" => {
                if let Ok(dictionary) = object.as_dict() {
                    let mut dictionary = dictionary.clone();
                    if let Some((_, ref existing)) = pages_object {
                        if let Ok(old) = existing.as_dict() {
                            dictionary.extend(old);
                        }
                    }
                    pages_object = Some((
                        pages_object.map(|(id, _)| id).unwrap_or(object_id),
                        Object::Dictionary(dictionary),
                    ));
                }
            }
            "Page" | "Outlines" | "Outline" => {}
            _ => {
                document.objects.insert(object_id, object);
            }
        }
    }

    let (pages_id, pages_obj) = pages_object.ok_or(FileError::Rejected("Pages root not found"))?;
    let (catalog_id, catalog_obj) =
        catalog_object.ok_or(FileError::Rejected("Catalog root not found"))?;

    for (object_id, object) in &documents_pages {
        if let Ok(dictionary) = object.as_dict() {
            let mut dictionary = dictionary.clone();
            dictionary.set("Parent", pages_id);
            document
                .objects
                .insert(*object_id, Object::Dictionary(dictionary));
        }
    }

    if let Ok(dictionary) = pages_obj.as_dict() {
        let mut dictionary = dictionary.clone();
        dictionary.set("Count", documents_pages.len() as u32);
        dictionary.set(
            "Kids",
            documents_pages
                .keys()
                .map(|id| Object::Reference(*id))
                .collect::<Vec<_>>(),
        );
        document
            .objects
            .insert(pages_id, Object::Dictionary(dictionary));
    }

    if let Ok(dictionary) = catalog_obj.as_dict() {
        let mut dictionary = dictionary.clone();
        dictionary.set("Pages", pages_id);
        dictionary.remove(b"Outlines");
        document
            .objects
            .insert(catalog_id, Object::Dictionary(dictionary));
    }

    document.trailer.set("Root", catalog_id);
    document.max_id = document.objects.len() as u32;
    document.renumber_objects();
    Ok(document)
}


#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{dictionary, Document, Object, Stream};
    use lopdf::content::{Content, Operation};
    use std::fs;

    fn make_pdf(path: &Path, pages: u32) {
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
        let mut kids = Vec::new();
        for i in 0..pages {
            let content = Content {
                operations: vec![
                    Operation::new("BT", vec![]),
                    Operation::new("Tf", vec!["F1".into(), 24.into()]),
                    Operation::new("Td", vec![72.into(), 720.into()]),
                    Operation::new("Tj", vec![Object::string_literal(format!("Page {}", i + 1))]),
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
            kids.push(page_id.into());
        }
        let pages_dict = dictionary! {
            "Type" => "Pages",
            "Kids" => kids,
            "Count" => pages as i64,
        };
        doc.objects.insert(pages_id, Object::Dictionary(pages_dict));
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        doc.trailer.set("Root", catalog_id);
        doc.save(path).unwrap();
    }

    #[test]
    fn extract_merge_rotate() {
        let root = std::env::temp_dir().join(format!("ownnas-pdf-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        make_pdf(&root.join("a.pdf"), 3);
        make_pdf(&root.join("b.pdf"), 2);

        let info = pdf_info(&root, "a.pdf").unwrap();
        assert_eq!(info.pages, 3);

        let extracted = pdf_extract(&root, "a.pdf", "2", "separate").unwrap();
        assert_eq!(extracted.len(), 1);
        assert_eq!(pdf_info(&root, &extracted[0]).unwrap().pages, 1);

        let combined = pdf_extract(&root, "a.pdf", "1-2", "combined").unwrap();
        assert_eq!(combined.len(), 1);
        assert_eq!(pdf_info(&root, &combined[0]).unwrap().pages, 2);

        let split = pdf_split_all(&root, "b.pdf").unwrap();
        assert_eq!(split.len(), 2);

        let merged = pdf_merge(&root, &vec!["a.pdf".into(), "b.pdf".into()]).unwrap();
        assert_eq!(pdf_info(&root, &merged).unwrap().pages, 5);

        let rotated = pdf_rotate(&root, "a.pdf", "1", 90).unwrap();
        assert!(root.join(rotated.split('/').last().unwrap()).is_file());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn parse_pages() {
        assert_eq!(parse_page_spec("1,3,5-6", 6, false).unwrap(), vec![1, 3, 5, 6]);
        assert_eq!(parse_page_spec("", 3, true).unwrap(), vec![1, 2, 3]);
        assert!(parse_page_spec("9", 3, false).is_err());
    }
}
