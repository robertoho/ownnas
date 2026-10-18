//! EPUB books: their title, author, chapters in reading order, and table of contents, read
//! from the package inside (an EPUB is a zip of web pages). The chapters themselves are served
//! as they are, in a sandbox, by `/api/book/…`.

use std::io::Read;
use std::path::Path;

use serde::Serialize;

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Book {
    pub title: Option<String>,
    pub author: Option<String>,
    /// The pages in reading order, as paths inside the book.
    pub chapters: Vec<String>,
    pub toc: Vec<TocEntry>,
    pub cover: Option<String>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TocEntry {
    pub title: String,
    /// A path inside the book, maybe with "#part".
    pub entry: String,
    pub level: u8,
}

fn read_part(zip: &mut zip::ZipArchive<std::io::BufReader<std::fs::File>>, name: &str) -> Option<String> {
    let mut part = zip.by_name(name).ok()?;
    if part.size() > 16 * 1024 * 1024 {
        return None;
    }
    let mut text = String::new();
    part.read_to_string(&mut text).ok()?;
    Some(text)
}

/// `href` (relative to the file in `dir`, maybe "%20"-escaped) as a path inside the book.
fn resolve(dir: &str, href: &str) -> String {
    let (path, fragment) = href.split_once('#').map_or((href, None), |(p, f)| (p, Some(f)));
    let path = percent_decode(path);
    let mut parts: Vec<String> = if path.starts_with('/') { Vec::new() } else { dir.split('/').filter(|p| !p.is_empty()).map(str::to_string).collect() };
    for piece in path.split('/') {
        match piece {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            other => parts.push(other.to_string()),
        }
    }
    let joined = parts.join("/");
    match fragment {
        Some(fragment) => format!("{joined}#{fragment}"),
        None => joined,
    }
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' && at + 3 <= bytes.len() {
            if let Some(value) = std::str::from_utf8(&bytes[at + 1..at + 3]).ok().and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                out.push(value);
                at += 3;
                continue;
            }
        }
        out.push(bytes[at]);
        at += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn dir_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

fn text_of(node: roxmltree::Node) -> String {
    node.descendants().filter(|n| n.is_text()).map(|n| n.text().unwrap_or("")).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn read(path: &Path) -> Result<Book, String> {
    let file = std::fs::File::open(path).map_err(|_| "Could not open the book".to_string())?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file)).map_err(|_| "This book is damaged or not an EPUB".to_string())?;
    let container = read_part(&mut zip, "META-INF/container.xml").ok_or("This book has no contents list (META-INF/container.xml)")?;
    let container = roxmltree::Document::parse(&container).map_err(|_| "This book's contents list could not be read".to_string())?;
    let opf_path = container
        .descendants()
        .find(|n| n.has_tag_name("rootfile"))
        .and_then(|n| n.attribute("full-path"))
        .ok_or("This book does not say where its contents are")?
        .to_string();
    let opf = read_part(&mut zip, &opf_path).ok_or("This book's package could not be read")?;
    let opf = roxmltree::Document::parse(&opf).map_err(|_| "This book's package could not be read".to_string())?;
    let opf_dir = dir_of(&opf_path).to_string();

    let mut book = Book::default();
    let metadata = opf.descendants().find(|n| n.has_tag_name("metadata"));
    if let Some(metadata) = metadata {
        book.title = metadata.children().find(|n| n.has_tag_name("title")).map(text_of).filter(|t| !t.is_empty());
        book.author = metadata.children().find(|n| n.has_tag_name("creator")).map(text_of).filter(|t| !t.is_empty());
    }
    // id → (path, media type, properties)
    let manifest: Vec<(String, String, String, String)> = opf
        .descendants()
        .filter(|n| n.has_tag_name("item"))
        .filter_map(|n| {
            Some((
                n.attribute("id")?.to_string(),
                resolve(&opf_dir, n.attribute("href")?),
                n.attribute("media-type").unwrap_or("").to_string(),
                n.attribute("properties").unwrap_or("").to_string(),
            ))
        })
        .collect();
    let by_id = |id: &str| manifest.iter().find(|(item, ..)| item == id);
    let spine = opf.descendants().find(|n| n.has_tag_name("spine"));
    if let Some(spine) = spine {
        book.chapters = spine
            .children()
            .filter(|n| n.has_tag_name("itemref") && n.attribute("linear") != Some("no"))
            .filter_map(|n| by_id(n.attribute("idref")?).map(|(_, path, ..)| path.clone()))
            .collect();
    }
    // The cover: EPUB 3 marks it; EPUB 2 names it in a <meta name="cover">.
    book.cover = manifest
        .iter()
        .find(|(_, _, media, properties)| properties.split_whitespace().any(|p| p == "cover-image") && media.starts_with("image/"))
        .map(|(_, path, ..)| path.clone())
        .or_else(|| {
            let id = opf.descendants().find(|n| n.has_tag_name("meta") && n.attribute("name") == Some("cover"))?.attribute("content")?;
            by_id(id).map(|(_, path, ..)| path.clone())
        });

    // The table of contents: EPUB 3's navigation page, or EPUB 2's NCX.
    if let Some((_, nav_path, ..)) = manifest.iter().find(|(_, _, _, properties)| properties.split_whitespace().any(|p| p == "nav")) {
        if let Some(nav) = read_part(&mut zip, nav_path) {
            if let Ok(doc) = roxmltree::Document::parse(&nav) {
                let toc_nav = doc.descendants().find(|n| n.has_tag_name("nav") && n.attributes().any(|a| a.name() == "type" && a.value() == "toc")).or_else(|| doc.descendants().find(|n| n.has_tag_name("nav")));
                if let Some(toc_nav) = toc_nav {
                    let nav_dir = dir_of(nav_path);
                    for link in toc_nav.descendants().filter(|n| n.has_tag_name("a")) {
                        let Some(href) = link.attribute("href") else { continue };
                        let level = link.ancestors().filter(|n| n.has_tag_name("ol") || n.has_tag_name("ul")).count().saturating_sub(1) as u8;
                        let title = text_of(link);
                        if !title.is_empty() {
                            book.toc.push(TocEntry { title, entry: resolve(nav_dir, href), level });
                        }
                    }
                }
            }
        }
    }
    if book.toc.is_empty() {
        let ncx = spine.and_then(|s| s.attribute("toc")).and_then(|id| by_id(id)).map(|(_, path, ..)| path.clone()).or_else(|| manifest.iter().find(|(_, _, media, _)| media == "application/x-dtbncx+xml").map(|(_, path, ..)| path.clone()));
        if let Some(ncx_path) = ncx {
            if let Some(ncx) = read_part(&mut zip, &ncx_path) {
                if let Ok(doc) = roxmltree::Document::parse(&ncx) {
                    let ncx_dir = dir_of(&ncx_path);
                    for point in doc.descendants().filter(|n| n.has_tag_name("navPoint")) {
                        let title = point.children().find(|n| n.has_tag_name("navLabel")).map(text_of).unwrap_or_default();
                        let src = point.children().find(|n| n.has_tag_name("content")).and_then(|n| n.attribute("src"));
                        let level = point.ancestors().filter(|n| n.has_tag_name("navPoint")).count().saturating_sub(1) as u8;
                        if let (false, Some(src)) = (title.is_empty(), src) {
                            book.toc.push(TocEntry { title, entry: resolve(ncx_dir, src), level });
                        }
                    }
                }
            }
        }
    }
    if book.chapters.is_empty() {
        return Err("This book has no pages to read".into());
    }
    Ok(book)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn reads_an_epub3_book() {
        let dir = std::env::temp_dir().join(format!("ownnas-epub-{}-{}", std::process::id(), rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("book.epub");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        let mut add = |name: &str, body: &str| {
            zip.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        };
        add("mimetype", "application/epub+zip");
        add("META-INF/container.xml", r#"<container><rootfiles><rootfile full-path="OEBPS/content.opf"/></rootfiles></container>"#);
        add("OEBPS/content.opf", r#"<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/"><metadata><dc:title>A  Tale</dc:title><dc:creator>Someone</dc:creator></metadata>
            <manifest><item id="nav" href="nav.xhtml" properties="nav" media-type="application/xhtml+xml"/><item id="c1" href="text/one.xhtml" media-type="application/xhtml+xml"/><item id="c2" href="text/two%20b.xhtml" media-type="application/xhtml+xml"/><item id="img" href="images/cover.jpg" properties="cover-image" media-type="image/jpeg"/></manifest>
            <spine><itemref idref="c1"/><itemref idref="c2"/><itemref idref="nav" linear="no"/></spine></package>"#);
        add("OEBPS/nav.xhtml", r#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><body><nav epub:type="toc"><ol><li><a href="text/one.xhtml">One</a><ol><li><a href="text/one.xhtml#s2">One, part two</a></li></ol></li><li><a href="text/two%20b.xhtml">Two</a></li></ol></nav></body></html>"#);
        drop(add);
        zip.finish().unwrap();
        let book = read(&path).unwrap();
        assert_eq!((book.title.as_deref(), book.author.as_deref()), (Some("A Tale"), Some("Someone")));
        assert_eq!(book.chapters, ["OEBPS/text/one.xhtml", "OEBPS/text/two b.xhtml"]);
        assert_eq!(book.cover.as_deref(), Some("OEBPS/images/cover.jpg"));
        assert_eq!(book.toc[1], TocEntry { title: "One, part two".into(), entry: "OEBPS/text/one.xhtml#s2".into(), level: 1 });
        assert_eq!(book.toc[2].entry, "OEBPS/text/two b.xhtml");
        let _ = std::fs::remove_dir_all(dir);
    }
}
