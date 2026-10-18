//! Saved e-mails: standard messages (.eml, read with `mail-parser`) and Outlook messages
//! (.msg, a compound file of MAPI properties). Who sent them, to whom, when, what they say,
//! and their attachments. An HTML body is shown in a sandbox, with pictures from the internet
//! left out (they would tell the sender the mail was opened).

use std::io::Read;
use std::path::Path;

use mail_parser::{MessageParser, MimeHeaders};
use serde::Serialize;

const MAX_MAIL: u64 = 100 * 1024 * 1024;

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Mail {
    pub subject: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub cc: Option<String>,
    /// Seconds since 1970.
    pub date: Option<i64>,
    pub text: Option<String>,
    pub has_html: bool,
    pub attachments: Vec<Attachment>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub index: usize,
    pub name: String,
    pub size: u64,
    pub mime: String,
    #[serde(skip)]
    pub content_id: Option<String>,
}

fn is_msg(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("msg"))
}

/// A mail file's bytes, or message `message` of a mailbox (.mbox).
fn read_bytes(path: &Path, message: Option<usize>) -> Result<Vec<u8>, String> {
    if let Some(index) = message {
        let spans = mbox_spans(path)?;
        let (start, end) = *spans.get(index).ok_or("This mailbox has no such message")?;
        if end - start > MAX_MAIL {
            return Err("This mail is too large to preview".into());
        }
        let mut file = std::fs::File::open(path).map_err(|_| "Could not read the mailbox".to_string())?;
        std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(start)).map_err(|_| "Could not read the mailbox".to_string())?;
        let mut bytes = vec![0; (end - start) as usize];
        file.read_exact(&mut bytes).map_err(|_| "Could not read the mailbox".to_string())?;
        // The mailbox's own "From " line is not part of the message.
        let body = bytes.iter().position(|&b| b == b'\n').map_or(0, |i| i + 1);
        return Ok(bytes[body..].to_vec());
    }
    let size = std::fs::metadata(path).map_err(|_| "Could not read the mail".to_string())?.len();
    if size > MAX_MAIL {
        return Err("This mail is too large to preview".into());
    }
    std::fs::read(path).map_err(|_| "Could not read the mail".to_string())
}

/// Where each message of a mailbox starts and ends: each begins with a "From " line at the
/// start of the file or after a blank line. Kept while the file is unchanged.
fn mbox_spans(path: &Path) -> Result<std::sync::Arc<Vec<(u64, u64)>>, String> {
    use std::collections::HashMap;
    use std::io::BufRead;
    use std::sync::{Arc, Mutex, OnceLock};
    type Cache = Mutex<HashMap<std::path::PathBuf, (std::time::SystemTime, u64, Arc<Vec<(u64, u64)>>)>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let meta = std::fs::metadata(path).map_err(|_| "Could not read the mailbox".to_string())?;
    let stamp = (meta.modified().unwrap_or(std::time::UNIX_EPOCH), meta.len());
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some((modified, len, spans)) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(path) {
        if (*modified, *len) == stamp {
            return Ok(spans.clone());
        }
    }
    let mut reader = std::io::BufReader::with_capacity(1 << 20, std::fs::File::open(path).map_err(|_| "Could not read the mailbox".to_string())?);
    let mut starts = Vec::new();
    let mut line = Vec::new();
    let mut at = 0u64;
    let mut blank_before = true;
    loop {
        line.clear();
        let read = reader.read_until(b'\n', &mut line).map_err(|_| "Could not read the mailbox".to_string())?;
        if read == 0 {
            break;
        }
        if blank_before && is_separator(&line) {
            starts.push(at);
        }
        blank_before = line.iter().all(|b| b.is_ascii_whitespace());
        at += read as u64;
    }
    let spans: Vec<(u64, u64)> = starts.iter().enumerate().map(|(i, &start)| (start, starts.get(i + 1).copied().unwrap_or(at))).collect();
    let spans = Arc::new(spans);
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    if cache.len() > 16 {
        cache.clear();
    }
    cache.insert(path.to_path_buf(), (stamp.0, stamp.1, spans.clone()));
    Ok(spans)
}

/// A mailbox's line between messages: "From sender Mon Jul 15 18:30:00 2024". A line of a
/// message that merely starts with "From " has no time in it.
fn is_separator(line: &[u8]) -> bool {
    line.starts_with(b"From ") && line.windows(5).any(|w| w[0].is_ascii_digit() && w[1].is_ascii_digit() && w[2] == b':' && w[3].is_ascii_digit() && w[4].is_ascii_digit())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub index: usize,
    pub from: Option<String>,
    pub subject: Option<String>,
    pub date: Option<i64>,
    pub attachments: bool,
}

/// How many messages a mailbox holds, and a page of them (sender, subject, date).
pub fn mailbox(path: &Path, from: usize, count: usize) -> Result<(usize, Vec<Summary>), String> {
    let total = mbox_spans(path)?.len();
    let mut page = Vec::new();
    for index in from..(from + count).min(total) {
        let Ok(mail) = read(path, Some(index)) else { continue };
        page.push(Summary { index, from: mail.from, subject: mail.subject, date: mail.date, attachments: !mail.attachments.is_empty() });
    }
    Ok((total, page))
}

pub fn read(path: &Path, message: Option<usize>) -> Result<Mail, String> {
    if is_msg(path) && message.is_none() {
        return Outlook::open(path)?.mail();
    }
    let bytes = read_bytes(path, message)?;
    let message = MessageParser::default().parse(&bytes).ok_or("This file is not an e-mail")?;
    let people = |address: Option<&mail_parser::Address>| {
        address.map(|a| a.iter().map(|addr| match (addr.name(), addr.address()) {
            (Some(name), Some(email)) if name != email => format!("{name} <{email}>"),
            (_, Some(email)) => email.to_string(),
            (Some(name), None) => name.to_string(),
            (None, None) => String::new(),
        }).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", ")).filter(|s| !s.is_empty())
    };
    let attachments = eml_attachments(&message);
    Ok(Mail {
        subject: message.subject().map(str::to_string),
        from: people(message.from()),
        to: people(message.to()),
        cc: people(message.cc()),
        date: message.date().map(|d| d.to_timestamp()),
        text: message.body_text(0).map(|t| t.into_owned()),
        has_html: message.html_body_count() > 0 && message.body_html(0).is_some(),
        attachments,
    })
}

fn eml_attachments(message: &mail_parser::Message) -> Vec<Attachment> {
    message
        .attachments()
        .enumerate()
        .map(|(index, part)| {
            let mime = part.content_type().map(|t| format!("{}/{}", t.ctype(), t.subtype().unwrap_or("octet-stream"))).unwrap_or_else(|| "application/octet-stream".into());
            Attachment {
                index,
                name: part.attachment_name().map(str::to_string).unwrap_or_else(|| format!("attachment-{}", index + 1)),
                size: part.contents().len() as u64,
                mime,
                content_id: part.content_id().map(|id| id.trim_matches(|c| c == '<' || c == '>').to_string()),
            }
        })
        .collect()
}

/// The HTML body, ready for the sandbox: links open in a new tab, and pictures sent with the
/// mail ("cid:") point at the attachment address `attachment_url(index)`.
pub fn html(path: &Path, message: Option<usize>, attachment_url: impl Fn(usize) -> String) -> Result<String, String> {
    let (html, attachments) = if is_msg(path) && message.is_none() {
        let mut outlook = Outlook::open(path)?;
        let mail = outlook.mail()?;
        (outlook.html().ok_or("This mail has no HTML body")?, mail.attachments)
    } else {
        let bytes = read_bytes(path, message)?;
        let message = MessageParser::default().parse(&bytes).ok_or("This file is not an e-mail")?;
        let html = message.body_html(0).ok_or("This mail has no HTML body")?.into_owned();
        (html, eml_attachments(&message))
    };
    let mut html = html;
    for attachment in &attachments {
        if let Some(id) = &attachment.content_id {
            html = html.replace(&format!("cid:{id}"), &attachment_url(attachment.index));
        }
    }
    Ok(format!("<!doctype html><meta charset=\"utf-8\"><base target=\"_blank\">{html}"))
}

/// One attachment: its name, media type, and contents.
pub fn attachment(path: &Path, message: Option<usize>, index: usize) -> Result<(String, String, Vec<u8>), String> {
    if is_msg(path) && message.is_none() {
        return Outlook::open(path)?.attachment(index);
    }
    let bytes = read_bytes(path, message)?;
    let message = MessageParser::default().parse(&bytes).ok_or("This file is not an e-mail")?;
    let part = message.attachments().nth(index).ok_or("This mail has no such attachment")?;
    let info = &eml_attachments(&message)[index];
    Ok((info.name.clone(), info.mime.clone(), part.contents().to_vec()))
}

// ---------------------------------------------------------------------------
// Outlook .msg
// ---------------------------------------------------------------------------

struct Outlook {
    file: cfb::CompoundFile<std::fs::File>,
}

impl Outlook {
    fn open(path: &Path) -> Result<Self, String> {
        let file = cfb::open(path).map_err(|_| "This is not an Outlook message".to_string())?;
        Ok(Self { file })
    }

    fn stream(&mut self, path: &str) -> Option<Vec<u8>> {
        let stream = self.file.open_stream(path).ok()?;
        let mut bytes = Vec::new();
        stream.take(MAX_MAIL).read_to_end(&mut bytes).ok()?;
        Some(bytes)
    }

    /// A text property (stored as UTF-16, or as 8-bit text in older messages).
    fn text(&mut self, storage: &str, id: &str) -> Option<String> {
        if let Some(bytes) = self.stream(&format!("{storage}__substg1.0_{id}001F")) {
            let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            return Some(String::from_utf16_lossy(&units).trim_end_matches('\0').to_string()).filter(|s| !s.trim().is_empty());
        }
        let bytes = self.stream(&format!("{storage}__substg1.0_{id}001E"))?;
        Some(bytes.iter().take_while(|&&b| b != 0).map(|&b| crate::media_stream::windows_1252(b)).collect::<String>()).filter(|s| !s.trim().is_empty())
    }

    fn attachment_storages(&self) -> Vec<String> {
        let mut found: Vec<String> = self
            .file
            .read_root_storage()
            .filter(|entry| entry.is_storage() && entry.name().starts_with("__attach_version1.0_#"))
            .map(|entry| format!("/{}/", entry.name()))
            .collect();
        found.sort();
        found
    }

    fn mail(&mut self) -> Result<Mail, String> {
        let root = "/";
        let mut mail = Mail { subject: self.text(root, "0037"), ..Mail::default() };
        let sender_name = self.text(root, "0C1A");
        let sender_email = self.text(root, "5D01").or_else(|| self.text(root, "0C1F"));
        mail.from = match (sender_name, sender_email) {
            (Some(name), Some(email)) if name != email => Some(format!("{name} <{email}>")),
            (name, email) => email.or(name),
        };
        mail.to = self.text(root, "0E04");
        mail.cc = self.text(root, "0E03");
        mail.text = self.text(root, "1000");
        mail.has_html = self.html().is_some();
        // The date from the message's own headers, when it was received.
        if let Some(headers) = self.text(root, "007D") {
            mail.date = MessageParser::default().parse(headers.as_bytes()).and_then(|m| m.date().map(|d| d.to_timestamp()));
        }
        for (index, storage) in self.attachment_storages().into_iter().enumerate() {
            let name = self.text(&storage, "3707").or_else(|| self.text(&storage, "3704")).unwrap_or_else(|| format!("attachment-{}", index + 1));
            let size = self.file.entry(format!("{storage}__substg1.0_37010102")).map(|e| e.len()).unwrap_or(0);
            let mime = self.text(&storage, "370E").unwrap_or_else(|| mime_guess::from_path(&name).first_raw().unwrap_or("application/octet-stream").to_string());
            let content_id = self.text(&storage, "3712");
            mail.attachments.push(Attachment { index, name, size, mime, content_id });
        }
        Ok(mail)
    }

    fn html(&mut self) -> Option<String> {
        if let Some(bytes) = self.stream("/__substg1.0_10130102") {
            return Some(String::from_utf8_lossy(&bytes).trim_end_matches('\0').to_string());
        }
        self.text("/", "1013")
    }

    fn attachment(&mut self, index: usize) -> Result<(String, String, Vec<u8>), String> {
        let mail = self.mail()?;
        let info = mail.attachments.get(index).ok_or("This mail has no such attachment")?.clone();
        let storage = self.attachment_storages().into_iter().nth(index).ok_or("This mail has no such attachment")?;
        let bytes = self.stream(&format!("{storage}__substg1.0_37010102")).ok_or("This attachment is not a file (it may be another message)")?;
        Ok((info.name, info.mime, bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_an_eml_with_html_and_an_attachment() {
        let dir = std::env::temp_dir().join(format!("ownnas-mail-{}-{}", std::process::id(), rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("hello.eml");
        let eml = "From: Ana <ana@example.com>\r\nTo: bo@example.com, Cy <cy@example.com>\r\nSubject: =?UTF-8?Q?Caf=C3=A9_plans?=\r\nDate: Mon, 15 Jul 2024 18:30:00 +0000\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"b1\"\r\n\r\n--b1\r\nContent-Type: multipart/alternative; boundary=\"b2\"\r\n\r\n--b2\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nSee you at 7.\r\n--b2\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>See you at <b>7</b>. <img src=\"cid:logo@x\"></p>\r\n--b2--\r\n--b1\r\nContent-Type: image/png\r\nContent-ID: <logo@x>\r\nContent-Disposition: attachment; filename=\"logo.png\"\r\nContent-Transfer-Encoding: base64\r\n\r\naGVsbG8=\r\n--b1--\r\n";
        std::fs::write(&path, eml).unwrap();
        let mail = read(&path, None).unwrap();
        assert_eq!(mail.subject.as_deref(), Some("Café plans"));
        assert_eq!(mail.from.as_deref(), Some("Ana <ana@example.com>"));
        assert_eq!(mail.to.as_deref(), Some("bo@example.com, Cy <cy@example.com>"));
        assert_eq!(mail.date, Some(1_721_068_200));
        assert_eq!(mail.text.as_deref().map(str::trim), Some("See you at 7."));
        assert!(mail.has_html);
        assert_eq!((mail.attachments[0].name.as_str(), mail.attachments[0].mime.as_str(), mail.attachments[0].size), ("logo.png", "image/png", 5));
        let html = html(&path, None, |index| format!("/a?i={index}")).unwrap();
        assert!(html.contains("<img src=\"/a?i=0\">") && html.contains("<base target=\"_blank\">"));
        assert_eq!(attachment(&path, None, 0).unwrap().2, b"hello");
        // The same mail twice in a mailbox, after another.
        let mbox = dir.join("box.mbox");
        std::fs::write(&mbox, format!("From ana@example.com Mon Jul 15 18:30:00 2024\r\nFrom: Bo <bo@example.com>\r\nSubject: First\r\n\r\nFrom the start.\r\n\r\nFrom ana@example.com Mon Jul 15 18:30:00 2024\r\n{eml}")).unwrap();
        let (total, page) = mailbox(&mbox, 0, 10).unwrap();
        assert_eq!(total, 2);
        assert_eq!(page[0].subject.as_deref(), Some("First"));
        assert_eq!(page[1].subject.as_deref(), Some("Café plans"));
        assert!(page[1].attachments);
        assert_eq!(read(&mbox, Some(0)).unwrap().text.as_deref().map(str::trim), Some("From the start."));
        assert_eq!(attachment(&mbox, Some(1), 0).unwrap().2, b"hello");
        let _ = std::fs::remove_dir_all(dir);
    }
}
