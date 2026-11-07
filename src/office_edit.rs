//! Small changes to Office files without an editor: paragraphs added at the end of a Word
//! (DOCX) or OpenDocument (ODT) text, and rows added after the last row of a spreadsheet (XLSX)
//! sheet. Only the one XML part that changes is rewritten; every other part of the file is
//! copied as it was, so styles, pictures and macros are left alone.

use std::io::{Read, Write};

/// The most a part read into memory may be (a huge sheet is better changed in an editor).
const MOST_PART: u64 = 64 * 1024 * 1024;

fn xml_text(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || *c == '\t')
        .collect::<String>()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The file with one part replaced by what `change` makes of it.
fn rewrite_part(bytes: &[u8], part: &str, change: impl FnOnce(&str) -> Result<String, String>) -> Result<Vec<u8>, String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|_| "That file is not a document OwnNAS can open".to_string())?;
    let original = {
        let mut entry = archive.by_name(part).map_err(|_| "That file is not a document OwnNAS can change".to_string())?;
        if entry.size() > MOST_PART {
            return Err("That document is too large to change here".into());
        }
        let mut text = String::new();
        entry.read_to_string(&mut text).map_err(|_| "Could not read the document".to_string())?;
        text
    };
    let changed = change(&original)?;
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for index in 0..archive.len() {
        let entry = archive.by_index_raw(index).map_err(|_| "Could not read the document".to_string())?;
        if entry.name() == part {
            let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            drop(entry);
            out.start_file(part, options).map_err(|_| "Could not write the document".to_string())?;
            out.write_all(changed.as_bytes()).map_err(|_| "Could not write the document".to_string())?;
        } else {
            // Copied as it is, compressed or not, so nothing else changes.
            out.raw_copy_file(entry).map_err(|_| "Could not write the document".to_string())?;
        }
    }
    Ok(out.finish().map_err(|_| "Could not write the document".to_string())?.into_inner())
}

/// Paragraphs added at the end of a Word document's text (before its section settings).
pub fn docx_add_paragraphs(bytes: &[u8], paragraphs: &[String]) -> Result<Vec<u8>, String> {
    rewrite_part(bytes, "word/document.xml", |xml| {
        let added: String = paragraphs
            .iter()
            .map(|text| format!("<w:p><w:r><w:t xml:space=\"preserve\">{}</w:t></w:r></w:p>", xml_text(text)))
            .collect();
        let body_end = xml.rfind("</w:body>").ok_or("That Word document has no body")?;
        // The last section's settings close the body and stay last.
        let at = xml[..body_end].rfind("<w:sectPr").filter(|at| !xml[*at..body_end].contains("</w:p>")).unwrap_or(body_end);
        Ok(format!("{}{added}{}", &xml[..at], &xml[at..]))
    })
}

/// Paragraphs added at the end of an OpenDocument text.
pub fn odt_add_paragraphs(bytes: &[u8], paragraphs: &[String]) -> Result<Vec<u8>, String> {
    rewrite_part(bytes, "content.xml", |xml| {
        let added: String = paragraphs.iter().map(|text| format!("<text:p>{}</text:p>", xml_text(text))).collect();
        let at = xml.rfind("</office:text>").ok_or("That document has no text")?;
        Ok(format!("{}{added}{}", &xml[..at], &xml[at..]))
    })
}

fn xml_unescape(text: &str) -> String {
    text.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// The text pieces of a Word paragraph: where each `<w:t>`'s text is in the XML, and the text.
fn word_pieces(xml: &str, from: usize, to: usize) -> Vec<(usize, usize, String)> {
    let mut pieces = Vec::new();
    let mut at = from;
    while let Some(found) = xml[at..to].find("<w:t") {
        let open = at + found;
        // <w:t>, <w:t xml:space="preserve">, but not <w:tab/> or <w:tbl>.
        let after = xml.as_bytes().get(open + 4).copied();
        let Some(close_tag) = xml[open..to].find('>').map(|end| open + end) else { break };
        if !matches!(after, Some(b'>') | Some(b' ')) || xml.as_bytes()[close_tag - 1] == b'/' {
            at = close_tag + 1;
            continue;
        }
        let Some(end) = xml[close_tag..to].find("</w:t>").map(|end| close_tag + end) else { break };
        pieces.push((close_tag + 1, end, xml_unescape(&xml[close_tag + 1..end])));
        at = end + 6;
    }
    pieces
}

/// A passage of a Word document replaced, even when it runs across differently formatted
/// parts of a paragraph (the new text takes the formatting where the passage starts). It must
/// be in the document exactly once, and inside one paragraph.
pub fn docx_replace(bytes: &[u8], find: &str, replace: &str) -> Result<Vec<u8>, String> {
    if find.is_empty() {
        return Err("Give the passage to replace".into());
    }
    rewrite_part(bytes, "word/document.xml", |xml| {
        let mut found: Option<(Vec<(usize, usize, String)>, usize)> = None;
        let mut count = 0;
        let mut at = 0;
        while let Some(start) = xml[at..].find("<w:p").map(|start| at + start) {
            let after = xml.as_bytes().get(start + 4).copied();
            let Some(end) = xml[start..].find("</w:p>").map(|end| start + end) else { break };
            if !matches!(after, Some(b'>') | Some(b' ')) {
                at = start + 4;
                continue;
            }
            let pieces = word_pieces(xml, start, end);
            let text: String = pieces.iter().map(|(_, _, text)| text.as_str()).collect();
            let matches: Vec<usize> = text.match_indices(find).map(|(index, _)| index).collect();
            count += matches.len();
            if let (Some(&first), None) = (matches.first(), found.as_ref()) {
                found = Some((pieces, first));
            }
            at = end + 6;
        }
        match count {
            0 => return Err("That passage is not in the document (or it runs across two paragraphs)".into()),
            1 => {}
            n => return Err(format!("That passage is in the document {n} times: include more of the text around it")),
        }
        let (pieces, start) = found.ok_or("That passage is not in the document")?;
        let end = start + find.len();
        // Which pieces the passage covers, and the new text of each.
        let mut offset = 0;
        let mut edits: Vec<(usize, usize, String)> = Vec::new();
        let mut placed = false;
        for (from, to, text) in &pieces {
            let (piece_start, piece_end) = (offset, offset + text.len());
            offset = piece_end;
            if piece_end <= start || piece_start >= end {
                continue;
            }
            let keep_before = &text[..start.saturating_sub(piece_start).min(text.len())];
            let keep_after = &text[end.saturating_sub(piece_start).min(text.len())..];
            let middle = if placed { "" } else { replace };
            placed = true;
            edits.push((*from, *to, format!("{keep_before}{middle}{keep_after}")));
        }
        let mut out = xml.to_string();
        for (from, to, text) in edits.into_iter().rev() {
            out.replace_range(from..to, &xml_text(&text));
        }
        // Spaces at the edges of a run only stay with xml:space="preserve".
        Ok(out.replace("<w:t>", "<w:t xml:space=\"preserve\">"))
    })
}

/// A passage of an OpenDocument text replaced: it must be in the document exactly once, and
/// within one stretch of text (not across formatting).
pub fn odt_replace(bytes: &[u8], find: &str, replace: &str) -> Result<Vec<u8>, String> {
    if find.is_empty() {
        return Err("Give the passage to replace".into());
    }
    rewrite_part(bytes, "content.xml", |xml| {
        let escaped = xml_text(find);
        match xml.matches(&escaped).count() {
            0 => Err("That passage is not in the document as one stretch of text: give a shorter one, without formatting changes inside".into()),
            1 => Ok(xml.replacen(&escaped, &xml_text(replace), 1)),
            n => Err(format!("That passage is in the document {n} times: include more of the text around it")),
        }
    })
}

/// A column number (from 0) as letters: 0 is A, 26 is AA.
fn column(mut index: usize) -> String {
    let mut letters = Vec::new();
    loop {
        letters.push((b'A' + (index % 26) as u8) as char);
        if index < 26 {
            break;
        }
        index = index / 26 - 1;
    }
    letters.iter().rev().collect()
}

/// Letters as a count of columns: A is 1, AA is 27.
fn column_number(letters: &str) -> usize {
    letters.chars().filter(char::is_ascii_alphabetic).fold(0, |total, c| total * 26 + (c.to_ascii_uppercase() as usize - 'A' as usize + 1))
}

/// The sheets of a workbook: (name, part inside the file), in order.
fn xlsx_sheets(archive: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>) -> Result<Vec<(String, String)>, String> {
    let read = |archive: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>, name: &str| -> Option<String> {
        let mut text = String::new();
        archive.by_name(name).ok()?.read_to_string(&mut text).ok()?;
        Some(text)
    };
    let workbook = read(archive, "xl/workbook.xml").ok_or("That is not a spreadsheet OwnNAS can change")?;
    let rels = read(archive, "xl/_rels/workbook.xml.rels").unwrap_or_default();
    let attribute = |tag: &str, name: &str| -> Option<String> {
        let at = tag.find(&format!(" {name}=\""))? + name.len() + 3;
        Some(tag[at..].split('"').next()?.to_string())
    };
    let mut sheets = Vec::new();
    for tag in workbook.split("<sheet ").skip(1).map(|rest| rest.split('>').next().unwrap_or_default()) {
        let (Some(name), Some(id)) = (attribute(&format!(" {tag}"), "name"), attribute(&format!(" {tag}"), "r:id")) else { continue };
        let target = rels
            .split("<Relationship ")
            .skip(1)
            .map(|rest| format!(" {}", rest.split('>').next().unwrap_or_default()))
            .find(|rel| attribute(rel, "Id").as_deref() == Some(&id))
            .and_then(|rel| attribute(&rel, "Target"));
        let Some(target) = target else { continue };
        let part = if let Some(absolute) = target.strip_prefix('/') { absolute.to_string() } else { format!("xl/{target}") };
        sheets.push((name.replace("&amp;", "&"), part));
    }
    Ok(sheets)
}

/// Rows added after the last row of a sheet (the first, or the one named), as typed: numbers
/// stay numbers, everything else is text.
pub fn xlsx_add_rows(bytes: &[u8], sheet: &str, rows: &[Vec<serde_json::Value>]) -> Result<Vec<u8>, String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|_| "That is not a spreadsheet OwnNAS can open".to_string())?;
    let sheets = xlsx_sheets(&mut archive)?;
    let chosen = if sheet.trim().is_empty() { sheets.first() } else { sheets.iter().find(|(name, _)| name.eq_ignore_ascii_case(sheet.trim())) };
    let (_, part) = chosen.ok_or("There is no sheet by that name")?.clone();
    drop(archive);
    rewrite_part(bytes, &part, |xml| {
        // The last row number used: rows are numbered in their r attribute.
        let last = xml.match_indices("<row ").filter_map(|(at, _)| {
            let tag = &xml[at..at + xml[at..].find('>')?];
            let start = tag.find(" r=\"")? + 4;
            tag[start..].split('"').next()?.parse::<usize>().ok()
        }).max().unwrap_or(0);
        let mut added = String::new();
        for (offset, row) in rows.iter().enumerate() {
            let number = last + offset + 1;
            added.push_str(&format!("<row r=\"{number}\">"));
            for (index, cell) in row.iter().enumerate() {
                let reference = format!("{}{number}", column(index));
                match cell {
                    serde_json::Value::Null => {}
                    serde_json::Value::Number(value) => added.push_str(&format!("<c r=\"{reference}\"><v>{value}</v></c>")),
                    serde_json::Value::Bool(value) => added.push_str(&format!("<c r=\"{reference}\" t=\"b\"><v>{}</v></c>", u8::from(*value))),
                    other => {
                        let text = other.as_str().map(str::to_string).unwrap_or_else(|| other.to_string());
                        added.push_str(&format!("<c r=\"{reference}\" t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c>", xml_text(&text)));
                    }
                }
            }
            added.push_str("</row>");
        }
        let changed = if let Some(at) = xml.rfind("</sheetData>") {
            format!("{}{added}{}", &xml[..at], &xml[at..])
        } else if let Some(at) = xml.find("<sheetData/>") {
            format!("{}<sheetData>{added}</sheetData>{}", &xml[..at], &xml[at + "<sheetData/>".len()..])
        } else {
            return Err("That sheet has no cells to add to".into());
        };
        // The used range, when the sheet says it, grows to take in the new rows.
        let widest = rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
        Ok(match changed.find("<dimension ref=\"") {
            Some(at) => {
                let start = at + "<dimension ref=\"".len();
                let end = start + changed[start..].find('"').unwrap_or(0);
                let old = &changed[start..end];
                let first = old.split(':').next().unwrap_or("A1");
                let old_columns = old.split(':').nth(1).map_or(0, |cell| column_number(cell.trim_end_matches(|c: char| c.is_ascii_digit())));
                let columns = old_columns.max(widest);
                format!("{}{first}:{}{}{}", &changed[..start], column(columns - 1), last + rows.len(), &changed[end..])
            }
            None => changed,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zipped(parts: &[(&str, &str)]) -> Vec<u8> {
        let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, text) in parts {
            out.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            out.write_all(text.as_bytes()).unwrap();
        }
        out.finish().unwrap().into_inner()
    }

    fn part(bytes: &[u8], name: &str) -> String {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut text = String::new();
        archive.by_name(name).unwrap().read_to_string(&mut text).unwrap();
        text
    }

    #[test]
    fn word_paragraphs_go_before_the_section_settings() {
        let doc = zipped(&[
            ("[Content_Types].xml", "<Types/>"),
            ("word/document.xml", "<w:document><w:body><w:p><w:r><w:t>Hello</w:t></w:r></w:p><w:sectPr><w:pgSz/></w:sectPr></w:body></w:document>"),
            ("word/media/a.png", "picture"),
        ]);
        let changed = docx_add_paragraphs(&doc, &["Bob & Ann <3".into(), "Second".into()]).unwrap();
        assert_eq!(
            part(&changed, "word/document.xml"),
            "<w:document><w:body><w:p><w:r><w:t>Hello</w:t></w:r></w:p><w:p><w:r><w:t xml:space=\"preserve\">Bob &amp; Ann &lt;3</w:t></w:r></w:p><w:p><w:r><w:t xml:space=\"preserve\">Second</w:t></w:r></w:p><w:sectPr><w:pgSz/></w:sectPr></w:body></w:document>"
        );
        assert_eq!(part(&changed, "word/media/a.png"), "picture", "the rest is as it was");
        assert!(docx_add_paragraphs(b"not a zip", &["x".into()]).is_err());
    }

    #[test]
    fn word_passages_are_replaced_across_runs() {
        let doc = zipped(&[("word/document.xml", "<w:document><w:body><w:p><w:r><w:t>Pay </w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t xml:space=\"preserve\">100 EUR</w:t></w:r><w:r><w:t> by Friday &amp; on time.</w:t></w:r></w:p><w:p><w:r><w:t>Other</w:t></w:r></w:p></w:body></w:document>")]);
        let changed = docx_replace(&doc, "100 EUR by Friday", "120 EUR by Monday").unwrap();
        let xml = part(&changed, "word/document.xml");
        assert!(xml.contains("<w:t xml:space=\"preserve\">Pay </w:t>"), "{xml}");
        assert!(xml.contains("<w:b/></w:rPr><w:t xml:space=\"preserve\">120 EUR by Monday</w:t>"), "{xml}");
        assert!(xml.contains("<w:t xml:space=\"preserve\"> &amp; on time.</w:t>"), "{xml}");
        assert!(docx_replace(&doc, "Friday & on", "x").is_ok(), "entities are read as text");
        assert!(docx_replace(&doc, "nowhere", "x").is_err());
        assert!(docx_replace(&doc, "e", "x").unwrap_err().contains("times"));
        let odt = zipped(&[("content.xml", "<office:text><text:p>Hello Bob &amp; Ann</text:p></office:text>")]);
        assert!(part(&odt_replace(&odt, "Bob & Ann", "everyone").unwrap(), "content.xml").contains("<text:p>Hello everyone</text:p>"));
    }

    #[test]
    fn odt_paragraphs_go_at_the_end() {
        let doc = zipped(&[("content.xml", "<office:document-content><office:body><office:text><text:p>Hi</text:p></office:text></office:body></office:document-content>")]);
        let changed = odt_add_paragraphs(&doc, &["More".into()]).unwrap();
        assert!(part(&changed, "content.xml").contains("<text:p>Hi</text:p><text:p>More</text:p></office:text>"));
    }

    #[test]
    fn sheet_rows_go_after_the_last_one() {
        let book = zipped(&[
            ("xl/workbook.xml", "<workbook><sheets><sheet name=\"Data\" sheetId=\"1\" r:id=\"rId1\"/><sheet name=\"Other\" sheetId=\"2\" r:id=\"rId2\"/></sheets></workbook>"),
            ("xl/_rels/workbook.xml.rels", "<Relationships><Relationship Id=\"rId1\" Target=\"worksheets/sheet1.xml\"/><Relationship Id=\"rId2\" Target=\"worksheets/sheet2.xml\"/></Relationships>"),
            ("xl/worksheets/sheet1.xml", "<worksheet><dimension ref=\"A1:B2\"/><sheetData><row r=\"1\"><c r=\"A1\"/></row><row r=\"2\"><c r=\"A2\"/></row></sheetData></worksheet>"),
            ("xl/worksheets/sheet2.xml", "<worksheet><sheetData/></worksheet>"),
        ]);
        let rows = vec![vec![serde_json::json!("Ann & co"), serde_json::json!(12.5), serde_json::json!(null), serde_json::json!(true)]];
        let changed = xlsx_add_rows(&book, "", &rows).unwrap();
        assert_eq!(
            part(&changed, "xl/worksheets/sheet1.xml"),
            "<worksheet><dimension ref=\"A1:D3\"/><sheetData><row r=\"1\"><c r=\"A1\"/></row><row r=\"2\"><c r=\"A2\"/></row><row r=\"3\"><c r=\"A3\" t=\"inlineStr\"><is><t xml:space=\"preserve\">Ann &amp; co</t></is></c><c r=\"B3\"><v>12.5</v></c><c r=\"D3\" t=\"b\"><v>1</v></c></row></sheetData></worksheet>"
        );
        let other = xlsx_add_rows(&book, "other", &[vec![serde_json::json!("x")]]).unwrap();
        assert!(part(&other, "xl/worksheets/sheet2.xml").contains("<sheetData><row r=\"1\"><c r=\"A1\" t=\"inlineStr\">"));
        assert!(xlsx_add_rows(&book, "Missing", &rows).is_err());
        assert_eq!(column(0), "A");
        assert_eq!(column(25), "Z");
        assert_eq!(column(26), "AA");
        assert_eq!(column(701), "ZZ");
        assert_eq!(column_number("AA"), 27);
    }
}
