//! Changing files on a server: new folders, renames, moving to the Trash, and uploads.
//! Uploads use the same endpoint and conflict choices as the web app.
use std::io::{self, Cursor, Read};

use serde::Deserialize;
use serde_json::json;

use crate::api::{map_err, Api, ApiError};

/// Called with the bytes moved so far. Returning `false` cancels the transfer.
pub type Progress<'a> = &'a dyn Fn(u64) -> bool;

/// Counts bytes as they are read and stops with an error once `progress` says to.
pub(crate) struct Counted<'a, R> {
    inner: R,
    done: u64,
    progress: Progress<'a>,
}

impl<'a, R> Counted<'a, R> {
    pub(crate) fn new(inner: R, progress: Progress<'a>) -> Self {
        Counted { inner, done: 0, progress }
    }
}

impl<R: Read> Read for Counted<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buf)?;
        self.done += read as u64;
        if !(self.progress)(self.done) {
            // Not `Interrupted`: `io::copy` and friends retry that kind, which would spin forever.
            return Err(io::Error::other(CANCELLED));
        }
        Ok(read)
    }
}

const CANCELLED: &str = "Cancelled";

/// What to do when an upload's name is taken: the web app's choices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conflict {
    Overwrite,
    /// Save under a numbered name, such as `photo (2).jpg`.
    KeepBoth,
    /// Keep whichever of the two is newer here; move the older one to the archive.
    ArchiveOlder,
    /// Move the file that is here to the archive and save the upload.
    ArchiveExisting,
    /// Skip this upload.
    Ignore,
}

impl Conflict {
    fn param(self) -> &'static str {
        match self {
            Conflict::Overwrite => "overwrite",
            Conflict::KeepBoth => "keep",
            Conflict::ArchiveOlder => "archive-older",
            Conflict::ArchiveExisting => "archive-existing",
            Conflict::Ignore => "ignore",
        }
    }
}

/// What the server has at an upload's name.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Existing {
    /// It is a folder; only keeping both or ignoring makes sense then.
    pub dir: bool,
    /// Seconds since the Unix epoch.
    pub modified: i64,
    pub size: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Uploaded {
    Saved,
    /// The server skipped it, as asked with [`Conflict::Ignore`].
    Skipped,
    /// The name is taken and no choice was given; ask, then upload again with one.
    Exists(Existing),
}

#[derive(Deserialize)]
struct PathResponse {
    path: Option<String>,
}

#[derive(Deserialize)]
struct CheckItem {
    exists: bool,
    #[serde(flatten)]
    existing: Existing,
}

#[derive(Deserialize)]
struct CheckResponse {
    items: Vec<CheckItem>,
}

#[derive(Deserialize)]
struct UploadResponse {
    saved: u32,
}

#[derive(Deserialize)]
struct ConflictResponse {
    #[serde(default)]
    exists: bool,
    #[serde(flatten)]
    existing: Option<Existing>,
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() { name.to_string() } else { format!("{dir}/{name}") }
}

/// The multipart form a browser sends for one file, around the file's bytes. Quotes and line
/// breaks in the name are percent-encoded, as browsers do.
fn form_parts(boundary: &str, name: &str) -> (Vec<u8>, Vec<u8>) {
    let safe: String = name
        .chars()
        .map(|c| match c {
            '"' => "%22".to_string(),
            '\r' => "%0D".to_string(),
            '\n' => "%0A".to_string(),
            c => c.to_string(),
        })
        .collect();
    let head = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{safe}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    );
    let tail = format!("\r\n--{boundary}--\r\n");
    (head.into_bytes(), tail.into_bytes())
}

impl Api {
    /// Creates `name` inside `parent` and returns its path.
    pub fn create_folder(&self, parent: &str, name: &str) -> Result<String, ApiError> {
        self.post_json("/api/mkdir", json!({ "path": parent, "name": name }))?;
        Ok(join(parent, name))
    }

    /// Renames a file or folder in place and returns its new path.
    pub fn rename(&self, path: &str, name: &str) -> Result<String, ApiError> {
        let response = self.post_json("/api/rename", json!({ "path": path, "name": name }))?;
        let body: PathResponse = response.into_json().map_err(|err| ApiError::Other(err.to_string()))?;
        body.path.ok_or_else(|| ApiError::Other("The server did not say where the item went".into()))
    }

    /// Moves a file or folder to the server's Trash, or deletes it if it is already there.
    pub fn trash(&self, path: &str) -> Result<(), ApiError> {
        self.request("DELETE", "/api/entry")
            .query("path", path)
            .call()
            .map_err(map_err)?;
        Ok(())
    }

    /// What is already at `name` in `dir`, if anything. Ask before uploading, as the web app does.
    pub fn check_upload(&self, dir: &str, name: &str) -> Result<Option<Existing>, ApiError> {
        let response = self.post_json("/api/upload/conflicts", json!({ "path": dir, "names": [name] }))?;
        let body: CheckResponse = response.into_json().map_err(|err| ApiError::Other(err.to_string()))?;
        Ok(body.items.into_iter().next().filter(|item| item.exists).map(|item| item.existing))
    }

    /// Uploads one file into `dir` through the web app's upload endpoint. Without `conflict`,
    /// a taken name comes back as [`Uploaded::Exists`]. `size` is sent when known;
    /// `modified` (seconds) is needed for [`Conflict::ArchiveOlder`].
    #[allow(clippy::too_many_arguments)]
    pub fn upload_file(
        &self,
        dir: &str,
        name: &str,
        conflict: Option<Conflict>,
        modified: Option<i64>,
        size: Option<u64>,
        body: impl Read,
        progress: Progress,
    ) -> Result<Uploaded, ApiError> {
        let boundary = format!("----OwnNAS{}", hex::encode(rand::random::<[u8; 12]>()));
        let (head, tail) = form_parts(&boundary, name);
        let mut request = self
            .request("POST", "/api/upload")
            .query("path", dir)
            .set("Content-Type", &format!("multipart/form-data; boundary={boundary}"));
        if let Some(conflict) = conflict {
            request = request.query("conflict", conflict.param());
        }
        if let Some(modified) = modified {
            request = request.query("modified", &modified.to_string());
        }
        if let Some(size) = size {
            request = request.set("Content-Length", &(head.len() as u64 + size + tail.len() as u64).to_string());
        }
        let cancelled = std::cell::Cell::new(false);
        let watch = |done: u64| {
            let go_on = progress(done);
            cancelled.set(!go_on);
            go_on
        };
        let form = Cursor::new(head).chain(Counted::new(body, &watch)).chain(Cursor::new(tail));
        match request.send(form) {
            Ok(response) => {
                let body: UploadResponse = response.into_json().map_err(|err| ApiError::Other(err.to_string()))?;
                Ok(if body.saved > 0 { Uploaded::Saved } else { Uploaded::Skipped })
            }
            Err(_) if cancelled.get() => Err(ApiError::Cancelled),
            Err(ureq::Error::Status(409, response)) => {
                let body: ConflictResponse = response.into_json().unwrap_or(ConflictResponse { exists: false, existing: None });
                match body.existing.filter(|_| body.exists) {
                    Some(existing) => Ok(Uploaded::Exists(existing)),
                    None => Err(ApiError::Conflict),
                }
            }
            Err(err) => Err(map_err(err)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_parts_wrap_the_file_like_a_browser() {
        let (head, tail) = form_parts("B", "my \"best\".jpg");
        assert_eq!(
            String::from_utf8(head).unwrap(),
            "--B\r\nContent-Disposition: form-data; name=\"file\"; filename=\"my %22best%22.jpg\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        );
        assert_eq!(String::from_utf8(tail).unwrap(), "\r\n--B--\r\n");
    }

    #[test]
    fn conflict_answers_parse() {
        let body: ConflictResponse =
            serde_json::from_str(r#"{"error":"x","exists":true,"dir":false,"modified":5,"size":9}"#).unwrap();
        assert!(body.exists);
        assert_eq!(body.existing, Some(Existing { dir: false, modified: 5, size: 9 }));
        let check: CheckResponse =
            serde_json::from_str(r#"{"items":[{"name":"a","exists":false,"dir":false,"modified":0,"size":0}]}"#).unwrap();
        assert!(!check.items[0].exists);
    }

    #[test]
    fn counted_reports_progress_and_can_cancel() {
        let seen = std::cell::Cell::new(0);
        let keep_going = |done: u64| {
            seen.set(done);
            true
        };
        let mut out = Vec::new();
        Counted::new(&b"hello world"[..], &keep_going).read_to_end(&mut out).unwrap();
        assert_eq!(seen.get(), 11);

        let stop = |done: u64| done <= 4;
        let mut reader = Counted::new(&b"hello world"[..], &stop);
        let mut buf = [0u8; 4];
        assert_eq!(reader.read(&mut buf).unwrap(), 4);
        assert_eq!(reader.read(&mut buf).unwrap_err().to_string(), CANCELLED);

        // A cancel must end `io::copy` rather than be retried.
        let mut reader = Counted::new(&b"hello world"[..], &stop);
        assert!(io::copy(&mut reader, &mut io::sink()).is_err());
    }
}
