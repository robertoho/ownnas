//! Previews of office files that have no viewer of their own, as plain structure the web page
//! draws: the text, headings, tables, and pictures of a document; the slides of a
//! presentation; the cells of a spreadsheet. Everything is read in Rust; files are only read.
//!
//! - PowerPoint (.pptx) and OpenDocument presentations (.odp): slides with their title, text,
//!   tables, pictures, and speaker notes.
//! - Word 97–2003 (.doc) and PowerPoint 97–2003 (.ppt): their text, from the compound file.
//! - Rich Text (.rtf): its text, in paragraphs.
//! - Excel 97–2003 (.xls), binary Excel (.xlsb), and OpenDocument spreadsheets (.ods), via
//!   `calamine`.
//! - Apple Pages, Numbers, and Keynote: the preview picture (or PDF) they keep inside.
//!
//! Pictures are named by their path inside the file, and served by `/api/archive/entry`.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use serde::Serialize;

/// The largest file read whole.
const MAX_FILE: u64 = 200 * 1024 * 1024;
/// The largest part read from inside a zip.
const MAX_PART: u64 = 64 * 1024 * 1024;
/// Spreadsheets show at most this many rows and columns of each sheet.
const MAX_ROWS: usize = 2000;
const MAX_COLUMNS: usize = 60;

#[derive(Debug, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Block {
    Heading { level: u8, text: String },
    Paragraph { text: String, level: u8 },
    Table { rows: Vec<Vec<String>> },
    Image { entry: String },
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Slide {
    pub title: Option<String>,
    pub blocks: Vec<Block>,
    pub notes: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sheet {
    pub name: String,
    pub rows: Vec<Vec<String>>,
    pub total_rows: usize,
    pub total_columns: usize,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum View {
    Document { blocks: Vec<Block>, note: Option<String> },
    Slides { slides: Vec<Slide>, note: Option<String> },
    Sheets { sheets: Vec<Sheet> },
    /// A picture (or a PDF) inside the file stands for it.
    Picture { entry: String, pdf: bool, note: String },
}

pub fn view(path: &Path) -> Result<View, String> {
    let size = std::fs::metadata(path).map_err(|_| "Could not read the file".to_string())?.len();
    if path.is_dir() {
        return Err("This document is a folder: open it to see what is inside".into());
    }
    if size > MAX_FILE {
        return Err("This file is too large to preview".into());
    }
    let extension = path.extension().map(|ext| ext.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    match extension.as_str() {
        "pptx" | "ppsx" => pptx(path),
        "odp" => odp(path),
        "doc" => doc(path),
        "ppt" | "pps" => ppt(path),
        "rtf" => {
            let bytes = std::fs::read(path).map_err(|_| "Could not read the file".to_string())?;
            Ok(View::Document { blocks: paragraphs(&rtf_text(&bytes)), note: None })
        }
        "xls" | "xlsb" | "ods" => spreadsheet(path),
        "pages" | "numbers" | "key" => iwork(path),
        _ => Err("This format is not previewed".into()),
    }
}

/// Text split into paragraphs, blank ones left out.
fn paragraphs(text: &str) -> Vec<Block> {
    text.split('\n')
        .map(|line| line.trim_end())
        .filter(|line| !line.trim().is_empty())
        .map(|line| Block::Paragraph { text: line.to_string(), level: 0 })
        .collect()
}

// ---------------------------------------------------------------------------
// Zip-based formats
// ---------------------------------------------------------------------------

type Zip = zip::ZipArchive<std::io::BufReader<std::fs::File>>;

fn open_zip(path: &Path) -> Result<Zip, String> {
    let file = std::fs::File::open(path).map_err(|_| "Could not read the file".to_string())?;
    zip::ZipArchive::new(std::io::BufReader::new(file)).map_err(|_| "This file is damaged or not in the format its name says".to_string())
}

fn read_part(zip: &mut Zip, name: &str) -> Option<String> {
    let mut part = zip.by_name(name).ok()?;
    if part.size() > MAX_PART {
        return None;
    }
    let mut text = String::new();
    part.read_to_string(&mut text).ok()?;
    Some(text)
}

/// `target` (a relationship's target) as a path inside the zip, from the part in `dir`.
fn join_part(dir: &str, target: &str) -> String {
    if let Some(absolute) = target.strip_prefix('/') {
        return absolute.to_string();
    }
    let mut parts: Vec<&str> = dir.split('/').filter(|p| !p.is_empty()).collect();
    for piece in target.split('/') {
        match piece {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            other => parts.push(other),
        }
    }
    parts.join("/")
}

/// A part's relationships: id → (type, target inside the zip).
fn relationships(zip: &mut Zip, part: &str) -> HashMap<String, (String, String)> {
    let (dir, name) = part.rsplit_once('/').unwrap_or(("", part));
    let rels_path = if dir.is_empty() { format!("_rels/{name}.rels") } else { format!("{dir}/_rels/{name}.rels") };
    let Some(xml) = read_part(zip, &rels_path) else { return HashMap::new() };
    let Ok(doc) = roxmltree::Document::parse(&xml) else { return HashMap::new() };
    doc.descendants()
        .filter(|n| n.has_tag_name("Relationship"))
        .filter_map(|n| {
            let id = n.attribute("Id")?;
            let target = n.attribute("Target")?;
            let kind = n.attribute("Type").unwrap_or("").rsplit('/').next().unwrap_or("").to_string();
            let resolved = if n.attribute("TargetMode") == Some("External") { String::new() } else { join_part(dir, target) };
            Some((id.to_string(), (kind, resolved)))
        })
        .collect()
}

const REL_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// The text of a DrawingML paragraph (`a:p`).
fn drawing_text(paragraph: roxmltree::Node) -> String {
    let mut text = String::new();
    for node in paragraph.descendants() {
        match node.tag_name().name() {
            "t" if node.is_element() => text.push_str(node.text().unwrap_or("")),
            "br" => text.push('\n'),
            "tab" => text.push('\t'),
            _ => {}
        }
    }
    text
}

fn pptx(path: &Path) -> Result<View, String> {
    let mut zip = open_zip(path)?;
    let presentation = read_part(&mut zip, "ppt/presentation.xml").ok_or("This presentation has no slides list")?;
    let doc = roxmltree::Document::parse(&presentation).map_err(|_| "This presentation could not be read".to_string())?;
    let rels = relationships(&mut zip, "ppt/presentation.xml");
    let order: Vec<String> = doc
        .descendants()
        .filter(|n| n.has_tag_name("sldId"))
        .filter_map(|n| n.attribute((REL_NS, "id")))
        .filter_map(|id| rels.get(id).map(|(_, target)| target.clone()))
        .collect();
    let mut slides = Vec::new();
    for part in order {
        let Some(xml) = read_part(&mut zip, &part) else { continue };
        let Ok(doc) = roxmltree::Document::parse(&xml) else { continue };
        let slide_rels = relationships(&mut zip, &part);
        let mut slide = Slide::default();
        for shape in doc.descendants().filter(|n| n.is_element() && matches!(n.tag_name().name(), "sp" | "pic" | "graphicFrame")) {
            match shape.tag_name().name() {
                "sp" => {
                    let placeholder = shape.descendants().find(|n| n.has_tag_name("ph")).and_then(|n| n.attribute("type")).unwrap_or("");
                    let lines: Vec<(String, u8)> = shape
                        .descendants()
                        .filter(|n| n.has_tag_name("p") && n.tag_name().namespace().is_some_and(|ns| ns.contains("drawingml")))
                        .map(|p| {
                            let level = p.children().find(|n| n.has_tag_name("pPr")).and_then(|n| n.attribute("lvl")).and_then(|l| l.parse().ok()).unwrap_or(0);
                            (drawing_text(p), level)
                        })
                        .filter(|(text, _)| !text.trim().is_empty())
                        .collect();
                    if lines.is_empty() {
                        continue;
                    }
                    if matches!(placeholder, "title" | "ctrTitle") && slide.title.is_none() {
                        slide.title = Some(lines.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>().join(" "));
                    } else {
                        slide.blocks.extend(lines.into_iter().map(|(text, level)| Block::Paragraph { text, level }));
                    }
                }
                "pic" => {
                    let embed = shape.descendants().find(|n| n.has_tag_name("blip")).and_then(|n| n.attribute((REL_NS, "embed")));
                    if let Some((_, target)) = embed.and_then(|id| slide_rels.get(id)) {
                        if !target.is_empty() {
                            slide.blocks.push(Block::Image { entry: target.clone() });
                        }
                    }
                }
                _ => {
                    if let Some(table) = shape.descendants().find(|n| n.has_tag_name("tbl")) {
                        let rows: Vec<Vec<String>> = table
                            .children()
                            .filter(|n| n.has_tag_name("tr"))
                            .map(|row| {
                                row.children()
                                    .filter(|n| n.has_tag_name("tc"))
                                    .map(|cell| cell.descendants().filter(|n| n.has_tag_name("p")).map(drawing_text).collect::<Vec<_>>().join("\n"))
                                    .collect()
                            })
                            .collect();
                        if !rows.is_empty() {
                            slide.blocks.push(Block::Table { rows });
                        }
                    }
                }
            }
        }
        // Speaker notes: the notes page's body text.
        if let Some((_, notes_part)) = slide_rels.values().find(|(kind, _)| kind == "notesSlide") {
            if let Some(notes) = read_part(&mut zip, notes_part).and_then(|xml| {
                let doc = roxmltree::Document::parse(&xml).ok()?;
                let body = doc.descendants().filter(|n| n.has_tag_name("sp")).find(|sp| sp.descendants().any(|n| n.has_tag_name("ph") && n.attribute("type") == Some("body")))?;
                let text: Vec<String> = body.descendants().filter(|n| n.has_tag_name("p") && n.tag_name().namespace().is_some_and(|ns| ns.contains("drawingml"))).map(drawing_text).collect();
                Some(text.join("\n").trim().to_string())
            }) {
                if !notes.is_empty() {
                    slide.notes = Some(notes);
                }
            }
        }
        slides.push(slide);
    }
    Ok(View::Slides { slides, note: None })
}

/// The text of an OpenDocument element: spaces (`text:s`), tabs, and line breaks included.
fn odf_text(node: roxmltree::Node) -> String {
    let mut out = String::new();
    for child in node.children() {
        if child.is_text() {
            out.push_str(child.text().unwrap_or(""));
            continue;
        }
        match child.tag_name().name() {
            "s" => out.push_str(&" ".repeat(child.attribute((ODF_TEXT, "c")).and_then(|c| c.parse().ok()).unwrap_or(1))),
            "tab" => out.push('\t'),
            "line-break" => out.push('\n'),
            "note" | "annotation" => {}
            _ => out.push_str(&odf_text(child)),
        }
    }
    out
}

const ODF_TEXT: &str = "urn:oasis:names:tc:opendocument:xmlns:text:1.0";
const ODF_PRESENTATION: &str = "urn:oasis:names:tc:opendocument:xmlns:presentation:1.0";
const XLINK: &str = "http://www.w3.org/1999/xlink";

/// The paragraphs, headings, list items, tables, and pictures inside an OpenDocument element.
fn odf_blocks(node: roxmltree::Node, blocks: &mut Vec<Block>, depth: u8) {
    for child in node.children().filter(|n| n.is_element()) {
        match child.tag_name().name() {
            "h" => {
                let level = child.attribute((ODF_TEXT, "outline-level")).and_then(|l| l.parse().ok()).unwrap_or(1);
                blocks.push(Block::Heading { level, text: odf_text(child) });
            }
            "p" => {
                let text = odf_text(child);
                if !text.trim().is_empty() {
                    blocks.push(Block::Paragraph { text, level: depth });
                }
                // Pictures can be anchored inside a paragraph.
                for frame in child.descendants().filter(|n| n.has_tag_name("image")) {
                    if let Some(href) = frame.attribute((XLINK, "href")).filter(|h| !h.contains("://")) {
                        blocks.push(Block::Image { entry: href.trim_start_matches("./").to_string() });
                    }
                }
            }
            "list" => odf_blocks(child, blocks, depth + 1),
            "table" => {
                let rows: Vec<Vec<String>> = child
                    .descendants()
                    .filter(|n| n.has_tag_name("table-row"))
                    .map(|row| row.children().filter(|n| n.has_tag_name("table-cell")).map(|cell| cell.children().filter(|n| n.is_element()).map(odf_text).collect::<Vec<_>>().join("\n")).collect())
                    .collect();
                if !rows.is_empty() {
                    blocks.push(Block::Table { rows });
                }
            }
            "image" => {
                if let Some(href) = child.attribute((XLINK, "href")).filter(|h| !h.contains("://")) {
                    blocks.push(Block::Image { entry: href.trim_start_matches("./").to_string() });
                }
            }
            // Speaker notes are read separately.
            "notes" => {}
            _ => odf_blocks(child, blocks, depth),
        }
    }
}

fn odp(path: &Path) -> Result<View, String> {
    let mut zip = open_zip(path)?;
    let content = read_part(&mut zip, "content.xml").ok_or("This presentation could not be read")?;
    let doc = roxmltree::Document::parse(&content).map_err(|_| "This presentation could not be read".to_string())?;
    let mut slides = Vec::new();
    for page in doc.descendants().filter(|n| n.has_tag_name("page")) {
        let mut slide = Slide::default();
        for frame in page.children().filter(|n| n.is_element()) {
            if frame.has_tag_name("notes") {
                let mut notes = Vec::new();
                odf_blocks(frame, &mut notes, 0);
                let text: Vec<String> = notes.into_iter().filter_map(|b| if let Block::Paragraph { text, .. } = b { Some(text) } else { None }).collect();
                if !text.is_empty() {
                    slide.notes = Some(text.join("\n"));
                }
                continue;
            }
            let class = frame.attribute((ODF_PRESENTATION, "class")).unwrap_or("");
            if class == "title" && slide.title.is_none() {
                let text = frame.descendants().filter(|n| n.has_tag_name("p")).map(odf_text).collect::<Vec<_>>().join(" ");
                if !text.trim().is_empty() {
                    slide.title = Some(text);
                    continue;
                }
            }
            odf_blocks(frame, &mut slide.blocks, 0);
        }
        slides.push(slide);
    }
    Ok(View::Slides { slides, note: None })
}

fn iwork(path: &Path) -> Result<View, String> {
    let mut zip = open_zip(path).map_err(|_| "This Apple document could not be opened".to_string())?;
    for (entry, pdf) in [("preview.jpg", false), ("QuickLook/Preview.pdf", true), ("preview-web.jpg", false), ("QuickLook/Thumbnail.jpg", false), ("Thumbnails/thumbnail.jpg", false)] {
        if zip.by_name(entry).is_ok() {
            return Ok(View::Picture {
                entry: entry.to_string(),
                pdf,
                note: if pdf { "The preview the app saved with the document.".into() } else { "The first page, as the app saved it with the document. Open it in Pages, Numbers, or Keynote to see the rest.".into() },
            });
        }
    }
    Err("This Apple document has no preview inside".into())
}

// ---------------------------------------------------------------------------
// Spreadsheets
// ---------------------------------------------------------------------------

fn spreadsheet(path: &Path) -> Result<View, String> {
    use calamine::Reader;
    let mut workbook = calamine::open_workbook_auto(path).map_err(|_| "This spreadsheet could not be read".to_string())?;
    let mut sheets = Vec::new();
    for name in workbook.sheet_names() {
        let Ok(range) = workbook.worksheet_range(&name) else { continue };
        let (total_rows, total_columns) = range.get_size();
        let rows = range
            .rows()
            .take(MAX_ROWS)
            .map(|row| row.iter().take(MAX_COLUMNS).map(cell_text).collect::<Vec<_>>())
            .collect();
        sheets.push(Sheet { name, rows, total_rows, total_columns });
    }
    Ok(View::Sheets { sheets })
}

fn cell_text(cell: &calamine::Data) -> String {
    match cell {
        calamine::Data::Float(value) if value.fract() == 0.0 && value.abs() < 1e15 => format!("{}", *value as i64),
        calamine::Data::DateTime(date) if date.is_datetime() => excel_date(date.as_f64()),
        calamine::Data::Error(_) => "#ERROR".into(),
        other => other.to_string(),
    }
}

/// An Excel date (days since 30 December 1899) as "2024-07-15" or "2024-07-15 18:30".
fn excel_date(serial: f64) -> String {
    let days = serial.floor() as i64;
    let (year, month, day) = civil_from_days(days - 25569);
    let minutes = ((serial - serial.floor()) * 1440.0).round() as i64;
    if minutes == 0 {
        format!("{year:04}-{month:02}-{day:02}")
    } else {
        format!("{year:04}-{month:02}-{day:02} {:02}:{:02}", minutes / 60 % 24, minutes % 60)
    }
}

/// Days since 1970-01-01 as a date (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

// ---------------------------------------------------------------------------
// Word and PowerPoint 97–2003 (compound files)
// ---------------------------------------------------------------------------

fn open_compound(path: &Path) -> Result<cfb::CompoundFile<std::fs::File>, String> {
    cfb::open(path).map_err(|_| "This file is damaged, or is a newer format with an old name".to_string())
}

fn read_stream(file: &mut cfb::CompoundFile<std::fs::File>, name: &str) -> Option<Vec<u8>> {
    let mut stream = file.open_stream(name).ok()?;
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn doc(path: &Path) -> Result<View, String> {
    // Some ".doc" files are really RTF or HTML, saved under the old name.
    let head = {
        let mut head = [0u8; 8];
        let mut file = std::fs::File::open(path).map_err(|_| "Could not read the file".to_string())?;
        let read = file.read(&mut head).unwrap_or(0);
        head[..read].to_vec()
    };
    if head.starts_with(b"{\\rtf") {
        let bytes = std::fs::read(path).map_err(|_| "Could not read the file".to_string())?;
        return Ok(View::Document { blocks: paragraphs(&rtf_text(&bytes)), note: None });
    }
    let mut file = open_compound(path)?;
    let word = read_stream(&mut file, "WordDocument").ok_or("This is not a Word document")?;
    let text = word_text(&mut file, &word).ok_or("The text of this Word document could not be read")?;
    Ok(View::Document { blocks: paragraphs(&text), note: Some("Text only: this older Word format is shown without its layout and pictures.".into()) })
}

/// The main text of a Word 97–2003 document, through its piece table.
fn word_text(file: &mut cfb::CompoundFile<std::fs::File>, word: &[u8]) -> Option<String> {
    if u16_at(word, 0)? != 0xA5EC {
        return None;
    }
    let flags = u16_at(word, 0x0A)?;
    if flags & 0x0100 != 0 {
        // Encrypted.
        return None;
    }
    let table_name = if flags & 0x0200 != 0 { "1Table" } else { "0Table" };
    let table = read_stream(file, table_name)?;
    // The FIB: a fixed part, then counted arrays of shorts, longs, and offset/length pairs.
    let csw = usize::from(u16_at(word, 32)?);
    let longs_at = 32 + 2 + csw * 2;
    let cslw = usize::from(u16_at(word, longs_at)?);
    let ccp_text = u32_at(word, longs_at + 2 + 3 * 4)? as usize;
    let pairs_at = longs_at + 2 + cslw * 4 + 2;
    let fc_clx = u32_at(word, pairs_at + 33 * 8)? as usize;
    let lcb_clx = u32_at(word, pairs_at + 33 * 8 + 4)? as usize;
    let clx = table.get(fc_clx..fc_clx + lcb_clx)?;
    // Skip the formatting (Prc) entries to reach the piece table (Pcdt).
    let mut at = 0;
    while *clx.get(at)? == 0x01 {
        at += 3 + usize::from(u16_at(clx, at + 1)?);
    }
    if *clx.get(at)? != 0x02 {
        return None;
    }
    let lcb = u32_at(clx, at + 1)? as usize;
    let plc = clx.get(at + 5..at + 5 + lcb)?;
    let pieces = (lcb.checked_sub(4)?) / 12;
    let mut text = String::new();
    for piece in 0..pieces {
        let cp_start = u32_at(plc, piece * 4)? as usize;
        let cp_end = u32_at(plc, (piece + 1) * 4)? as usize;
        if cp_start >= ccp_text {
            break;
        }
        let count = cp_end.min(ccp_text).saturating_sub(cp_start);
        let pcd = (pieces + 1) * 4 + piece * 8;
        let fc = u32_at(plc, pcd + 2)?;
        if fc & 0x4000_0000 != 0 {
            // 8-bit text (Windows-1252), at half the offset.
            let start = ((fc & !0x4000_0000) / 2) as usize;
            for &byte in word.get(start..start + count)? {
                text.push(crate::media_stream::windows_1252(byte));
            }
        } else {
            let start = fc as usize;
            let units: Vec<u16> = word.get(start..start + count * 2)?.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            text.push_str(&String::from_utf16_lossy(&units));
        }
    }
    Some(clean_word_text(&text))
}

/// Word's special characters as plain text: paragraph and cell marks become breaks, fields keep
/// their result and lose their code, other marks go.
fn clean_word_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    // Inside a field's code (between its start and separator marks): hidden.
    let mut field_codes: Vec<bool> = Vec::new();
    for c in text.chars() {
        match c {
            '\u{13}' => field_codes.push(true),
            '\u{14}' => {
                if let Some(code) = field_codes.last_mut() {
                    *code = false;
                }
            }
            '\u{15}' => {
                field_codes.pop();
            }
            _ if field_codes.last() == Some(&true) => {}
            '\r' | '\u{0C}' => out.push('\n'),
            // A table cell ends with one mark, a row with two.
            '\u{07}' if out.ends_with('\t') => {
                out.pop();
                out.push('\n');
            }
            '\u{07}' => out.push('\t'),
            '\u{0B}' => out.push('\n'),
            '\u{1E}' => out.push('-'),
            '\u{A0}' => out.push(' '),
            c if (c as u32) < 0x20 && c != '\t' => {}
            c => out.push(c),
        }
    }
    out
}

fn ppt(path: &Path) -> Result<View, String> {
    let mut file = open_compound(path)?;
    let stream = read_stream(&mut file, "PowerPoint Document").ok_or("This is not a PowerPoint presentation")?;
    let mut found = PptText::default();
    ppt_records(&stream, 0, stream.len(), &mut found, PptPlace::Other, &mut 1);
    // PowerPoint keeps slide text in its slide list; other programs inside each slide's shapes.
    let has_text = |slides: &Vec<Slide>| slides.iter().any(|slide| slide.title.is_some() || !slide.blocks.is_empty());
    let mut slides = if has_text(&found.listed) { found.listed } else { found.drawn };
    slides.retain(|slide| slide.title.is_some() || !slide.blocks.is_empty());
    Ok(View::Slides { slides, note: Some("Text only: this older PowerPoint format is shown without its layout and pictures.".into()) })
}

#[derive(Default)]
struct PptText {
    /// Slides from the slide list.
    listed: Vec<Slide>,
    /// Slides from the slides' own drawings.
    drawn: Vec<Slide>,
}

#[derive(Clone, Copy, PartialEq)]
enum PptPlace {
    List,
    Slide,
    /// Masters, notes, and anything else: not shown.
    Other,
}

/// Walks PowerPoint records, gathering each slide's text. Each piece of text follows a header
/// saying what it is (types 0 and 6 are titles).
fn ppt_records(data: &[u8], start: usize, end: usize, found: &mut PptText, place: PptPlace, text_type: &mut u32) {
    let mut at = start;
    while at + 8 <= end {
        let (Some(ver_instance), Some(kind), Some(len)) = (u16_at(data, at), u16_at(data, at + 2), u32_at(data, at + 4)) else { return };
        let body = at + 8;
        let next = body.saturating_add(len as usize).min(end);
        let container = ver_instance & 0x000F == 0x000F;
        match kind {
            // The slide list: instance 0 lists the slides (1 the masters, 2 the notes).
            0x0FF0 if container => {
                let inner = if ver_instance >> 4 == 0 { PptPlace::List } else { PptPlace::Other };
                ppt_records(data, body, next, found, inner, text_type);
            }
            0x03EE if container => {
                found.drawn.push(Slide::default());
                ppt_records(data, body, next, found, PptPlace::Slide, text_type);
            }
            // Masters, notes pages, and the handout.
            0x03F8 | 0x03F0 | 0x0FC9 if container => {}
            0x03F3 if place == PptPlace::List => found.listed.push(Slide::default()),
            0x0F9F => *text_type = u32_at(data, body).unwrap_or(1),
            0x0FA0 | 0x0FA8 => {
                let bytes = data.get(body..next).unwrap_or(&[]);
                let text = if kind == 0x0FA0 {
                    String::from_utf16_lossy(&bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>())
                } else {
                    bytes.iter().map(|&b| crate::media_stream::windows_1252(b)).collect()
                };
                let text = text.replace('\r', "\n").replace('\u{0B}', "\n");
                let slide = match place {
                    PptPlace::List => found.listed.last_mut(),
                    PptPlace::Slide => found.drawn.last_mut(),
                    PptPlace::Other => None,
                };
                if let Some(slide) = slide.filter(|_| !text.trim().is_empty()) {
                    if matches!(*text_type, 0 | 6) && slide.title.is_none() {
                        slide.title = Some(text.trim().to_string());
                    } else {
                        slide.blocks.extend(paragraphs(&text));
                    }
                }
            }
            _ if container => ppt_records(data, body, next, found, place, text_type),
            _ => {}
        }
        if next <= at {
            return;
        }
        at = next;
    }
}

// ---------------------------------------------------------------------------
// Rich Text Format
// ---------------------------------------------------------------------------

/// Groups whose contents are not text to show.
const RTF_SKIP: [&str; 24] = [
    "fonttbl", "colortbl", "stylesheet", "info", "pict", "object", "header", "footer", "headerl", "headerr", "headerf", "footerl",
    "footerr", "footerf", "themedata", "colorschememapping", "datastore", "latentstyles", "listtable", "listoverridetable",
    "rsidtbl", "generator", "xmlnstbl", "fldinst",
];

/// The text of an RTF document, paragraphs on their own lines.
pub fn rtf_text(bytes: &[u8]) -> String {
    struct Group {
        skip: bool,
        unicode_skip: usize,
    }
    let mut out = String::new();
    let mut stack = vec![Group { skip: false, unicode_skip: 1 }];
    // Characters to drop after a \u escape (its plain-text stand-in).
    let mut pending_skip = 0usize;
    let mut at = 0;
    let mut group_start = false;
    while at < bytes.len() {
        let byte = bytes[at];
        let skip = stack.last().is_some_and(|g| g.skip);
        match byte {
            b'{' => {
                let inherited = stack.last().map_or((false, 1), |g| (g.skip, g.unicode_skip));
                stack.push(Group { skip: inherited.0, unicode_skip: inherited.1 });
                group_start = true;
                at += 1;
                continue;
            }
            b'}' => {
                if stack.len() > 1 {
                    stack.pop();
                }
                at += 1;
            }
            b'\\' => {
                let Some(&next) = bytes.get(at + 1) else { break };
                if next.is_ascii_alphabetic() {
                    let word_start = at + 1;
                    let mut end = word_start;
                    while end < bytes.len() && bytes[end].is_ascii_alphabetic() {
                        end += 1;
                    }
                    let word = std::str::from_utf8(&bytes[word_start..end]).unwrap_or("");
                    let number_start = end;
                    if end < bytes.len() && (bytes[end] == b'-' || bytes[end].is_ascii_digit()) {
                        end += 1;
                        while end < bytes.len() && bytes[end].is_ascii_digit() {
                            end += 1;
                        }
                    }
                    let number: Option<i64> = std::str::from_utf8(&bytes[number_start..end]).ok().and_then(|n| n.parse().ok());
                    if end < bytes.len() && bytes[end] == b' ' {
                        end += 1;
                    }
                    at = end;
                    if group_start && RTF_SKIP.contains(&word) {
                        if let Some(group) = stack.last_mut() {
                            group.skip = true;
                        }
                    }
                    group_start = false;
                    if stack.last().is_some_and(|g| g.skip) {
                        continue;
                    }
                    match word {
                        "par" | "line" | "sect" | "page" | "row" => out.push('\n'),
                        "tab" | "cell" => out.push('\t'),
                        "emdash" => out.push('—'),
                        "endash" => out.push('–'),
                        "bullet" => out.push('•'),
                        "lquote" => out.push('‘'),
                        "rquote" => out.push('’'),
                        "ldblquote" => out.push('“'),
                        "rdblquote" => out.push('”'),
                        "uc" => {
                            if let Some(group) = stack.last_mut() {
                                group.unicode_skip = number.unwrap_or(1).max(0) as usize;
                            }
                        }
                        "u" => {
                            if let Some(value) = number {
                                let value = if value < 0 { value + 65536 } else { value };
                                if let Some(c) = char::from_u32(value as u32) {
                                    out.push(c);
                                }
                                pending_skip = stack.last().map_or(1, |g| g.unicode_skip);
                            }
                        }
                        _ => {}
                    }
                    continue;
                }
                at += 2;
                match next {
                    b'*' => {
                        if let Some(group) = stack.last_mut() {
                            group.skip = true;
                        }
                    }
                    b'\'' => {
                        let hex = bytes.get(at..at + 2).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
                        at += 2;
                        if let Some(value) = hex {
                            if pending_skip > 0 {
                                pending_skip -= 1;
                            } else if !skip {
                                out.push(crate::media_stream::windows_1252(value));
                            }
                        }
                    }
                    b'\\' | b'{' | b'}' if !skip => {
                        if pending_skip > 0 {
                            pending_skip -= 1;
                        } else {
                            out.push(char::from(next));
                        }
                    }
                    b'~' if !skip => out.push(' '),
                    b'_' if !skip => out.push('-'),
                    b'\n' | b'\r' if !skip => out.push('\n'),
                    _ => {}
                }
                group_start = false;
                continue;
            }
            b'\r' | b'\n' => at += 1,
            _ => {
                at += 1;
                if skip {
                    group_start = false;
                    continue;
                }
                if pending_skip > 0 {
                    pending_skip -= 1;
                } else {
                    // Plain text is ASCII in RTF; anything else is read as Windows-1252.
                    out.push(crate::media_stream::windows_1252(byte));
                }
            }
        }
        group_start = false;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ownnas-docs-{}-{}", std::process::id(), rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    fn zip_with(path: &Path, parts: &[(&str, &str)]) {
        let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, body) in parts {
            zip.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn reads_rtf() {
        let rtf = concat!(r"{\rtf1\ansi{\fonttbl{\f0 Arial;}}{\*\generator Word;}\pard Hello \b bold \b0 world\par Caf\'e9 \u", r"8364?5 \{x\}\par{\info{\title Secret}}\tab end}").as_bytes();
        assert_eq!(rtf_text(rtf), "Hello bold world\nCafé €5 {x}\n\tend");
    }

    #[test]
    fn reads_pptx_slides_in_order() {
        let path = scratch("deck.pptx");
        let a = "http://schemas.openxmlformats.org/drawingml/2006/main";
        let p = "http://schemas.openxmlformats.org/presentationml/2006/main";
        let slide = |title: &str, body: &str, extra: &str| {
            format!(
                r#"<p:sld xmlns:a="{a}" xmlns:p="{p}" xmlns:r="{REL_NS}"><p:cSld><p:spTree>
                <p:sp><p:nvSpPr><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr><p:txBody><a:p><a:r><a:t>{title}</a:t></a:r></a:p></p:txBody></p:sp>
                <p:sp><p:txBody><a:p><a:r><a:t>{body}</a:t></a:r></a:p><a:p><a:pPr lvl="1"/><a:r><a:t>Sub point</a:t></a:r></a:p></p:txBody></p:sp>{extra}
                </p:spTree></p:cSld></p:sld>"#
            )
        };
        let pic = r#"<p:pic><p:blipFill><a:blip r:embed="rId2"/></p:blipFill></p:pic>"#;
        zip_with(&path, &[
            ("ppt/presentation.xml", &format!(r#"<p:presentation xmlns:p="{p}" xmlns:r="{REL_NS}"><p:sldIdLst><p:sldId id="256" r:id="rId8"/><p:sldId id="257" r:id="rId7"/></p:sldIdLst></p:presentation>"#)),
            ("ppt/_rels/presentation.xml.rels", r#"<Relationships><Relationship Id="rId7" Type="x/slide" Target="slides/slide1.xml"/><Relationship Id="rId8" Type="x/slide" Target="slides/slide2.xml"/></Relationships>"#),
            ("ppt/slides/slide1.xml", &slide("Second", "Body two", "")),
            ("ppt/slides/slide2.xml", &slide("First", "Body one", pic)),
            ("ppt/slides/_rels/slide2.xml.rels", r#"<Relationships><Relationship Id="rId2" Type="x/image" Target="../media/image1.png"/></Relationships>"#),
        ]);
        let View::Slides { slides, .. } = view(&path).unwrap() else { panic!() };
        assert_eq!(slides.len(), 2);
        assert_eq!(slides[0].title.as_deref(), Some("First"));
        assert_eq!(slides[0].blocks, [
            Block::Paragraph { text: "Body one".into(), level: 0 },
            Block::Paragraph { text: "Sub point".into(), level: 1 },
            Block::Image { entry: "ppt/media/image1.png".into() },
        ]);
        assert_eq!(slides[1].title.as_deref(), Some("Second"));
    }

    #[test]
    fn reads_odp_slides() {
        let path = scratch("deck.odp");
        let content = r#"<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:presentation="urn:oasis:names:tc:opendocument:xmlns:presentation:1.0" xmlns:xlink="http://www.w3.org/1999/xlink"><office:body><office:presentation>
            <draw:page><draw:frame presentation:class="title"><draw:text-box><text:p>Hello<text:s text:c="2"/>there</text:p></draw:text-box></draw:frame>
            <draw:frame><draw:text-box><text:list><text:list-item><text:p>Point</text:p></text:list-item></text:list></draw:text-box></draw:frame>
            <draw:frame><draw:image xlink:href="Pictures/a.png"/></draw:frame>
            <presentation:notes><draw:frame><draw:text-box><text:p>Say hi</text:p></draw:text-box></draw:frame></presentation:notes></draw:page>
            </office:presentation></office:body></office:document-content>"#;
        zip_with(&path, &[("content.xml", content)]);
        let View::Slides { slides, .. } = view(&path).unwrap() else { panic!() };
        assert_eq!(slides[0].title.as_deref(), Some("Hello  there"));
        assert_eq!(slides[0].blocks, [Block::Paragraph { text: "Point".into(), level: 1 }, Block::Image { entry: "Pictures/a.png".into() }]);
        assert_eq!(slides[0].notes.as_deref(), Some("Say hi"));
    }

    #[test]
    fn finds_an_iwork_preview() {
        let path = scratch("Budget.numbers");
        zip_with(&path, &[("Index/Document.iwa", "x"), ("preview.jpg", "jpeg")]);
        let View::Picture { entry, pdf, .. } = view(&path).unwrap() else { panic!() };
        assert_eq!((entry.as_str(), pdf), ("preview.jpg", false));
    }

    #[test]
    fn word_marks_and_fields() {
        assert_eq!(clean_word_text("Title\rPage \u{13} PAGE \u{14}3\u{15} of 9\ra\u{07}b\u{07}\u{07}c\u{07}d\u{07}\u{07}\r"), "Title\nPage 3 of 9\na\tb\nc\td\n\n");
        assert_eq!(excel_date(45488.0), "2024-07-15");
        assert_eq!(excel_date(45488.770833333336), "2024-07-15 18:30");
    }
}

