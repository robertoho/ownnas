//! More e-book formats: Kindle (MOBI, AZW, AZW3, without DRM), FictionBook (.fb2), and DjVu
//! (scanned books, pages drawn by `djvu-rs`). EPUB is in `ebook`.

use std::path::Path;

use crate::documents::{Block, View};

const MAX_BOOK: u64 = 200 * 1024 * 1024;

pub fn is_kindle(extension: &str) -> bool {
    matches!(extension, "mobi" | "azw" | "azw3" | "prc")
}

fn extension_of(path: &Path) -> String {
    path.extension().map(|ext| ext.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

fn check_size(path: &Path) -> Result<(), String> {
    let size = std::fs::metadata(path).map_err(|_| "Could not read the book".to_string())?.len();
    if size > MAX_BOOK {
        return Err("This book is too large to preview".into());
    }
    Ok(())
}

pub fn view(path: &Path) -> Result<View, String> {
    check_size(path)?;
    match extension_of(path).as_str() {
        ext if is_kindle(ext) => kindle(path),
        "fb2" => fiction_book(path),
        _ => Err("This book format is not read here".into()),
    }
}

/// A Kindle book (a Palm database): its records, and what its first one says about them.
struct Kindle {
    data: Vec<u8>,
    offsets: Vec<usize>,
}

fn be16(data: &[u8], at: usize) -> Option<usize> {
    Some(usize::from(u16::from_be_bytes(data.get(at..at + 2)?.try_into().ok()?)))
}

fn be32(data: &[u8], at: usize) -> Option<usize> {
    Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?) as usize)
}

impl Kindle {
    fn open(path: &Path) -> Result<Self, String> {
        let data = std::fs::read(path).map_err(|_| "Could not read the book".to_string())?;
        let count = be16(&data, 76).ok_or("This is not a Kindle book")?;
        let offsets: Vec<usize> = (0..count).map(|i| be32(&data, 78 + i * 8)).collect::<Option<_>>().ok_or("This Kindle book is damaged")?;
        if data.get(60..68) != Some(b"BOOKMOBI") || offsets.is_empty() {
            return Err("This is not a Kindle book".into());
        }
        Ok(Self { data, offsets })
    }

    fn record(&self, index: usize) -> Option<&[u8]> {
        let start = *self.offsets.get(index)?;
        let end = self.offsets.get(index + 1).copied().unwrap_or(self.data.len());
        self.data.get(start..end)
    }

    /// The book's text (HTML), from its text records.
    fn html(&self) -> Result<String, String> {
        let head = self.record(0).ok_or("This Kindle book is damaged")?;
        let compression = be16(head, 0).unwrap_or(1);
        let text_records = be16(head, 8).unwrap_or(0);
        if be16(head, 12).unwrap_or(0) != 0 {
            return Err("This Kindle book is protected (DRM) and cannot be shown".into());
        }
        if compression == 17480 {
            return Err("This Kindle book uses a compression (HUFF/CDIC) that is not read here".into());
        }
        let header_len = be32(head, 20).unwrap_or(0);
        // Which kinds of data each text record carries after its text.
        let extra_flags = if header_len >= 0xE4 { be16(head, 0xF2).unwrap_or(0) } else { 0 };
        let mut text = Vec::new();
        for index in 1..=text_records {
            let Some(record) = self.record(index) else { break };
            let record = strip_trailing(record, extra_flags);
            match compression {
                2 => palmdoc_decompress(record, &mut text),
                _ => text.extend_from_slice(record),
            }
        }
        let utf8 = be32(head, 28) == Some(65001);
        Ok(if utf8 { String::from_utf8_lossy(&text).into_owned() } else { text.iter().map(|&b| crate::media_stream::windows_1252(b)).collect() })
    }

    /// The book's title, author, and cover picture's number, from its EXTH metadata (or the
    /// title in its header).
    fn title_author(&self) -> (Option<String>, Option<String>, Option<usize>) {
        let Some(head) = self.record(0) else { return (None, None, None) };
        let header_len = be32(head, 20).unwrap_or(0);
        let mut title = None;
        let mut author = None;
        let mut cover = None;
        let exth = 16 + header_len;
        if head.get(exth..exth + 4) == Some(b"EXTH") {
            let count = be32(head, exth + 8).unwrap_or(0);
            let mut at = exth + 12;
            for _ in 0..count {
                let (Some(kind), Some(len)) = (be32(head, at), be32(head, at + 4)) else { break };
                let value = head.get(at + 8..at + len).map(|v| String::from_utf8_lossy(v).trim().to_string());
                match kind {
                    100 if author.is_none() => author = value,
                    503 => title = value,
                    // The cover, counted from the first picture.
                    201 => cover = be32(head, at + 8).map(|n| n + 1),
                    _ => {}
                }
                at += len.max(8);
            }
        }
        if title.is_none() {
            if let (Some(at), Some(len)) = (be32(head, 0x54), be32(head, 0x58)) {
                title = head.get(at..at + len).map(|v| String::from_utf8_lossy(v).to_string());
            }
        }
        (title.filter(|t| !t.is_empty()), author.filter(|a| !a.is_empty()), cover.filter(|&n| n < 0xFFFF_FFFF))
    }

    /// Picture number `n` (numbered from 1, as the book's pages name them).
    fn image(&self, n: usize) -> Option<&[u8]> {
        let head = self.record(0)?;
        let first = be32(head, 0x6C)?;
        self.record(first + n.checked_sub(1)?)
    }
}

/// A text record without what follows its text: the sizes are written backwards at its end.
fn strip_trailing(record: &[u8], flags: usize) -> &[u8] {
    let mut end = record.len();
    for bit in 1..16 {
        if flags & (1 << bit) == 0 {
            continue;
        }
        // The size is in the last four bytes, 7 bits each; a byte with its top bit set starts it.
        let mut size = 0usize;
        for &byte in &record[end.saturating_sub(4)..end] {
            if byte & 0x80 != 0 {
                size = 0;
            }
            size = (size << 7) | usize::from(byte & 0x7F);
        }
        end = end.saturating_sub(size);
    }
    if flags & 1 != 0 && end > 0 {
        let extra = usize::from(record[end - 1] & 3) + 1;
        end = end.saturating_sub(extra);
    }
    &record[..end]
}

/// PalmDoc's LZ77 compression.
fn palmdoc_decompress(input: &[u8], out: &mut Vec<u8>) {
    let start = out.len();
    let mut at = 0;
    while at < input.len() {
        let c = input[at];
        at += 1;
        match c {
            0x01..=0x08 => {
                let end = (at + usize::from(c)).min(input.len());
                out.extend_from_slice(&input[at..end]);
                at = end;
            }
            0x00 | 0x09..=0x7F => out.push(c),
            0x80..=0xBF => {
                let Some(&next) = input.get(at) else { break };
                at += 1;
                let pair = (usize::from(c) << 8 | usize::from(next)) & 0x3FFF;
                let distance = pair >> 3;
                let length = (pair & 7) + 3;
                if distance == 0 || distance > out.len() - start {
                    continue;
                }
                for _ in 0..length {
                    let byte = out[out.len() - distance];
                    out.push(byte);
                }
            }
            _ => {
                out.push(b' ');
                out.push(c ^ 0x80);
            }
        }
    }
}

fn kindle(path: &Path) -> Result<View, String> {
    let book = Kindle::open(path)?;
    let html = book.html()?;
    let (title, author, cover) = book.title_author();
    let mut blocks = Vec::new();
    if let Some(cover) = cover.filter(|&n| book.image(n).is_some()) {
        blocks.push(Block::Image { entry: format!("#picture/{cover}") });
    }
    if let Some(title) = title {
        blocks.push(Block::Heading { level: 1, text: title });
    }
    if let Some(author) = author {
        blocks.push(Block::Paragraph { text: author, level: 0 });
    }
    blocks.extend(html_blocks(&html));
    Ok(View::Document { blocks, note: None })
}

/// One of a Kindle book's pictures (they are numbered from 1 in its pages), or a FictionBook's.
pub fn picture(path: &Path, index: usize) -> Result<(Vec<u8>, &'static str), String> {
    check_size(path)?;
    let bytes = if extension_of(path) == "fb2" {
        let text = std::fs::read_to_string(path).map_err(|_| "Could not read the book".to_string())?;
        let doc = roxmltree::Document::parse(&text).map_err(|_| "This book could not be read".to_string())?;
        let binary = doc.descendants().filter(|n| n.has_tag_name("binary")).nth(index).ok_or("This book has no such picture")?;
        let encoded: String = binary.text().unwrap_or("").split_whitespace().collect();
        base64_decode(&encoded).ok_or("This picture could not be read")?
    } else {
        let book = Kindle::open(path)?;
        book.image(index).ok_or("This book has no such picture")?.to_vec()
    };
    let media = if bytes.starts_with(b"\x89PNG") {
        "image/png"
    } else if bytes.starts_with(b"GIF8") {
        "image/gif"
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        "image/jpeg"
    } else {
        return Err("This picture is in a format browsers do not show".into());
    };
    Ok((bytes, media))
}

fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let value = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => u32::from(c - b'A'),
            b'a'..=b'z' => u32::from(c - b'a') + 26,
            b'0'..=b'9' => u32::from(c - b'0') + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        })
    };
    let clean: Vec<u8> = text.bytes().filter(|&c| c != b'=').collect();
    let mut out = Vec::with_capacity(clean.len() * 3 / 4);
    for chunk in clean.chunks(4) {
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= value(c)? << (18 - 6 * i);
        }
        out.push((n >> 16) as u8);
        if chunk.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(n as u8);
        }
    }
    Some(out)
}

/// A book's HTML as headings, paragraphs, and pictures: tags that start a block end the one
/// before; everything else is text.
pub fn html_blocks(html: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut text = String::new();
    let mut heading: Option<u8> = None;
    let mut skip = 0usize;
    let flush = |text: &mut String, heading: Option<u8>, blocks: &mut Vec<Block>| {
        let clean = decode_entities(&text.split_whitespace().collect::<Vec<_>>().join(" "));
        if !clean.is_empty() {
            blocks.push(match heading {
                Some(level) => Block::Heading { level, text: clean },
                None => Block::Paragraph { text: clean, level: 0 },
            });
        }
        text.clear();
    };
    let mut rest = html;
    while let Some(open) = rest.find('<') {
        if skip == 0 {
            text.push_str(&rest[..open]);
        }
        let Some(close) = rest[open..].find('>') else { break };
        let tag = &rest[open + 1..open + close];
        rest = &rest[open + close + 1..];
        let closing = tag.starts_with('/');
        let name: String = tag.trim_start_matches('/').chars().take_while(|c| c.is_ascii_alphanumeric() || *c == ':').collect::<String>().to_ascii_lowercase();
        match name.as_str() {
            "script" | "style" | "head" => {
                if closing {
                    skip = skip.saturating_sub(1);
                } else if !tag.ends_with('/') {
                    skip += 1;
                }
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                flush(&mut text, heading, &mut blocks);
                heading = if closing { None } else { name[1..].parse().ok() };
            }
            "p" | "div" | "br" | "li" | "blockquote" | "tr" | "mbp:pagebreak" | "section" | "table" => flush(&mut text, heading, &mut blocks),
            "img" | "image" => {
                flush(&mut text, heading, &mut blocks);
                // Kindle pictures are numbered: "recindex" in older books, "kindle:embed:000A"
                // (base 32) in newer ones.
                let number = attribute(tag, "recindex").and_then(|v| v.trim_start_matches('0').parse::<usize>().ok()).or_else(|| {
                    let src = attribute(tag, "src")?;
                    let code = src.strip_prefix("kindle:embed:")?.split('?').next()?;
                    usize::from_str_radix(code, 32).ok()
                });
                if let Some(index) = number {
                    blocks.push(Block::Image { entry: format!("#picture/{index}") });
                }
            }
            _ => {}
        }
    }
    if skip == 0 {
        text.push_str(rest);
    }
    flush(&mut text, heading, &mut blocks);
    blocks
}

fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let at = tag.to_ascii_lowercase().find(&format!("{name}="))? + name.len() + 1;
    let value = &tag[at..];
    let quote = value.chars().next()?;
    if quote == '"' || quote == '\'' {
        value[1..].split(quote).next()
    } else {
        value.split(|c: char| c.is_whitespace() || c == '/').next()
    }
}

fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let Some(semi) = rest[..rest.len().min(12)].find(';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            "mdash" => Some('—'),
            "ndash" => Some('–'),
            "hellip" => Some('…'),
            "laquo" => Some('«'),
            "raquo" => Some('»'),
            "rsquo" => Some('’'),
            "lsquo" => Some('‘'),
            "rdquo" => Some('”'),
            "ldquo" => Some('“'),
            _ if entity.starts_with("#x") || entity.starts_with("#X") => u32::from_str_radix(&entity[2..], 16).ok().and_then(char::from_u32),
            _ if entity.starts_with('#') => entity[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// FictionBook: an XML book with its title, author, sections, and pictures inside.
fn fiction_book(path: &Path) -> Result<View, String> {
    let text = std::fs::read_to_string(path).map_err(|_| "This book could not be read (it should be UTF-8 XML)".to_string())?;
    let doc = roxmltree::Document::parse_with_options(&text, roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() }).map_err(|_| "This book could not be read".to_string())?;
    let mut blocks = Vec::new();
    let info = doc.descendants().find(|n| n.has_tag_name("title-info"));
    let text_of = |node: roxmltree::Node| node.descendants().filter(|n| n.is_text()).map(|n| n.text().unwrap_or("")).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ");
    if let Some(info) = info {
        if let Some(title) = info.children().find(|n| n.has_tag_name("book-title")) {
            blocks.push(Block::Heading { level: 1, text: text_of(title) });
        }
        let authors: Vec<String> = info.children().filter(|n| n.has_tag_name("author")).map(|a| ["first-name", "middle-name", "last-name"].iter().filter_map(|t| a.children().find(|n| n.has_tag_name(*t)).map(text_of)).collect::<Vec<_>>().join(" ")).filter(|a| !a.is_empty()).collect();
        if !authors.is_empty() {
            blocks.push(Block::Paragraph { text: authors.join(", "), level: 0 });
        }
    }
    let binaries: Vec<String> = doc.descendants().filter(|n| n.has_tag_name("binary")).filter_map(|n| n.attribute("id").map(str::to_string)).collect();
    fn walk(node: roxmltree::Node, depth: u8, blocks: &mut Vec<Block>, binaries: &[String], text_of: &dyn Fn(roxmltree::Node) -> String) {
        for child in node.children().filter(|n| n.is_element()) {
            match child.tag_name().name() {
                "section" => walk(child, depth + 1, blocks, binaries, text_of),
                "title" => blocks.push(Block::Heading { level: (depth + 1).min(6), text: text_of(child) }),
                "p" | "v" | "subtitle" | "text-author" => {
                    let text = text_of(child);
                    if !text.is_empty() {
                        blocks.push(Block::Paragraph { text, level: 0 });
                    }
                }
                "image" => {
                    let href = child.attributes().find(|a| a.name() == "href").map(|a| a.value().trim_start_matches('#')).unwrap_or("");
                    if let Some(index) = binaries.iter().position(|b| b == href) {
                        blocks.push(Block::Image { entry: format!("#picture/{index}") });
                    }
                }
                "empty-line" => {}
                _ => walk(child, depth, blocks, binaries, text_of),
            }
        }
    }
    for body in doc.descendants().filter(|n| n.has_tag_name("body")) {
        walk(body, 0, &mut blocks, &binaries, &text_of);
    }
    Ok(View::Document { blocks, note: None })
}

// ---------------------------------------------------------------------------
// DjVu
// ---------------------------------------------------------------------------

/// How many pages a DjVu book has.
pub fn djvu_pages(path: &Path) -> Result<usize, String> {
    check_size(path)?;
    let doc = djvu_rs::Document::open(path).map_err(|_| "This DjVu book could not be read".to_string())?;
    Ok(doc.page_count())
}

/// Page `index` of a DjVu book as a PNG at most `width` pixels wide.
pub fn djvu_page(path: &Path, index: usize, width: u32) -> Result<Vec<u8>, String> {
    check_size(path)?;
    let doc = djvu_rs::Document::open(path).map_err(|_| "This DjVu book could not be read".to_string())?;
    let page = doc.page(index).map_err(|_| "This book has no such page".to_string())?;
    let (full_w, full_h) = (page.width().max(1), page.height().max(1));
    let width = width.clamp(200, 2400).min(full_w);
    let height = ((u64::from(full_h) * u64::from(width)) / u64::from(full_w)).max(1) as u32;
    let pixmap = page.render_to_size(width, height).map_err(|_| "This page could not be drawn".to_string())?;
    let picture = image::RgbaImage::from_raw(pixmap.width, pixmap.height, pixmap.data.to_vec()).ok_or("This page could not be drawn")?;
    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(picture).to_rgb8().write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).map_err(|_| "This page could not be drawn".to_string())?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_becomes_blocks() {
        let blocks = html_blocks("<html><head><style>p{}</style></head><body><h2>Chapter&nbsp;1</h2><p>It was a <i>dark</i> &amp; stormy night.</p><mbp:pagebreak/><p><img recindex=\"00003\"/></p>Last words</body></html>");
        assert_eq!(blocks, [
            Block::Heading { level: 2, text: "Chapter 1".into() },
            Block::Paragraph { text: "It was a dark & stormy night.".into(), level: 0 },
            Block::Image { entry: "#picture/3".into() },
            Block::Paragraph { text: "Last words".into(), level: 0 },
        ]);
        assert_eq!(base64_decode("aGVsbG8="), Some(b"hello".to_vec()));
    }

    #[test]
    fn reads_fiction_book() {
        let dir = std::env::temp_dir().join(format!("ownnas-fb2-{}-{}", std::process::id(), rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tale.fb2");
        std::fs::write(&path, r##"<?xml version="1.0" encoding="utf-8"?><FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2.0" xmlns:l="http://www.w3.org/1999/xlink"><description><title-info><author><first-name>Ana</first-name><last-name>Nowak</last-name></author><book-title>A Tale</book-title></title-info></description><body><section><title><p>One</p></title><p>First words.</p><image l:href="#pic"/></section></body><binary id="pic" content-type="image/png">iVBORw0KGgo=</binary></FictionBook>"##).unwrap();
        let View::Document { blocks, .. } = view(&path).unwrap() else { panic!() };
        assert_eq!(blocks, [
            Block::Heading { level: 1, text: "A Tale".into() },
            Block::Paragraph { text: "Ana Nowak".into(), level: 0 },
            Block::Heading { level: 2, text: "One".into() },
            Block::Paragraph { text: "First words.".into(), level: 0 },
            Block::Image { entry: "#picture/0".into() },
        ]);
        assert_eq!(picture(&path, 0).unwrap().1, "image/png");
        let _ = std::fs::remove_dir_all(dir);
    }
}
