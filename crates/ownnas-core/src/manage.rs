//! Changing files on a server: new folders, renames, moving to the Trash, and uploads.
//! Uploads use the same endpoint and conflict choices as the web app.
use std::io::{self, Cursor, Read};

use serde::Deserialize;
use serde_json::json;

use crate::api::{map_err, Api, ApiError, Fetch};

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

/// What the library already has of a photo.
#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PhotoCheck {
    /// A file with the same contents.
    pub copy: Option<String>,
    /// That copy is in the workspace asked about.
    #[serde(default)]
    pub in_workspace: bool,
    /// A smaller copy made from it.
    pub resized: Option<String>,
    /// It was deleted, and kept from being backed up again.
    #[serde(default)]
    pub blocked: bool,
}

#[derive(Deserialize, Debug, Clone)]
pub struct WorkspaceInfo {
    pub id: i64,
    pub name: String,
    pub folders: Vec<String>,
}

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
    #[serde(default)]
    paths: Vec<String>,
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

/// An earlier version of a file, kept when it was replaced.
#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FileVersion {
    pub id: i64,
    /// When it was replaced (seconds since 1970).
    pub saved_at: i64,
    /// When it had last been changed before that (seconds since 1970).
    pub modified: i64,
    pub size: u64,
    pub by: String,
    /// What replaced it: "edit", "upload", "sync", or "restore".
    pub reason: String,
}

fn paths_of(response: ureq::Response) -> Result<Vec<String>, ApiError> {
    #[derive(Deserialize)]
    struct Made {
        #[serde(default)]
        paths: Vec<String>,
    }
    let made: Made = response.into_json().map_err(|err| ApiError::Other(err.to_string()))?;
    Ok(made.paths)
}

fn path_of(response: ureq::Response) -> Result<String, ApiError> {
    let body: PathResponse = response.into_json().map_err(|err| ApiError::Other(err.to_string()))?;
    body.path.ok_or_else(|| ApiError::Other("The server did not say where the item went".into()))
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

    /// Moves a file or folder into the folder `dest`; returns its new path.
    pub fn move_to(&self, path: &str, dest: &str) -> Result<String, ApiError> {
        let response = self.post_json("/api/move", json!({ "path": path, "dest": dest }))?;
        let body: PathResponse = response.into_json().map_err(|err| ApiError::Other(err.to_string()))?;
        body.path.ok_or_else(|| ApiError::Other("The server did not say where the item went".into()))
    }

    /// Copies a file or folder into the folder `dest`; returns the copy's path. When the name
    /// is taken there, the server keeps both ("name (2)").
    pub fn copy_to(&self, path: &str, dest: &str) -> Result<String, ApiError> {
        let response = self.post_json("/api/copy", json!({ "path": path, "dest": dest }))?;
        path_of(response)
    }

    /// Copies a file or folder next to itself ("name copy"); returns the copy's path.
    pub fn duplicate(&self, path: &str) -> Result<String, ApiError> {
        let response = self.post_json("/api/duplicate", json!({ "path": path }))?;
        path_of(response)
    }

    /// Puts an item in the Trash back where it was; returns its path.
    pub fn restore(&self, path: &str) -> Result<String, ApiError> {
        let response = self.post_json("/api/restore", json!({ "path": path }))?;
        path_of(response)
    }

    /// Packs `paths` (all in one folder) into an archive next to them; `format` is "zip",
    /// "tar", "tar.gz", or "tar.xz". Returns the archive's path.
    pub fn compress(&self, paths: &[String], format: &str) -> Result<String, ApiError> {
        let response = self.post_json("/api/compress", json!({ "paths": paths, "format": format }))?;
        path_of(response)
    }

    /// Unpacks an archive into a folder next to it, or straight beside it with `here`.
    /// Returns the paths it made.
    pub fn extract(&self, path: &str, here: bool) -> Result<Vec<String>, ApiError> {
        #[derive(serde::Deserialize)]
        struct Made {
            #[serde(default)]
            paths: Vec<String>,
        }
        let response = self.post_json("/api/extract", json!({ "path": path, "here": here }))?;
        let made: Made = response.into_json().map_err(|err| ApiError::Other(err.to_string()))?;
        Ok(made.paths)
    }

    /// Adds a tag to a file or folder; returns the tag as the server stored it.
    pub fn add_tag(&self, path: &str, tag: &str) -> Result<String, ApiError> {
        #[derive(serde::Deserialize)]
        struct Tagged {
            tag: String,
        }
        let response = self.post_json("/api/tags", json!({ "path": path, "tag": tag }))?;
        let tagged: Tagged = response.into_json().map_err(|err| ApiError::Other(err.to_string()))?;
        Ok(tagged.tag)
    }

    /// Protects items from deletion, or allows it again. `contents` also covers everything in
    /// folders.
    pub fn set_protection(&self, paths: &[String], protect: bool, contents: bool) -> Result<(), ApiError> {
        self.post_json("/api/protect", json!({ "paths": paths, "protect": protect, "contents": contents }))?;
        Ok(())
    }

    /// Saves a copy of a photo in another format ("jpeg", "png", "webp", "gif", "bmp",
    /// "tiff") beside it; returns the copy's path.
    pub fn convert_image(&self, path: &str, format: &str) -> Result<String, ApiError> {
        path_of(self.post_json("/api/convert-image", json!({ "path": path, "format": format }))?)
    }

    /// Makes a photo smaller: to `percent` of its size, or to fit `longest` pixels on its long
    /// side. With `replace` the original goes to the Trash; otherwise a copy is made. Returns
    /// whether it was resized (photos already that small are left alone).
    pub fn resize_image(&self, path: &str, percent: Option<u32>, longest: Option<u32>, replace: bool) -> Result<bool, ApiError> {
        #[derive(serde::Deserialize)]
        struct Problem {
            #[serde(default)]
            error: serde_json::Value,
        }
        #[derive(serde::Deserialize)]
        struct Resized {
            #[serde(default)]
            resized: Vec<serde_json::Value>,
            #[serde(default)]
            problems: Vec<Problem>,
        }
        let mut body = json!({ "paths": [path], "replace": replace });
        if let Some(percent) = percent {
            body["percent"] = json!(percent);
        }
        if let Some(longest) = longest {
            body["width"] = json!(longest);
            body["height"] = json!(longest);
        }
        let done: Resized = self.post_json("/api/images/resize", body)?.into_json().map_err(|err| ApiError::Other(err.to_string()))?;
        if let Some(problem) = done.problems.first() {
            let message = problem.error.as_str().map(String::from).unwrap_or_else(|| problem.error.to_string());
            return Err(ApiError::Other(message));
        }
        Ok(!done.resized.is_empty())
    }

    /// Copies pages ("1,3,5-8") of a PDF into a new PDF, or one PDF per page with
    /// `separate`; returns the paths made.
    pub fn pdf_extract(&self, path: &str, pages: &str, separate: bool) -> Result<Vec<String>, ApiError> {
        let mode = if separate { "separate" } else { "combined" };
        paths_of(self.post_json("/api/pdf/extract", json!({ "path": path, "pages": pages, "mode": mode }))?)
    }

    /// One PDF per page, in a folder beside it; returns the paths made.
    pub fn pdf_split(&self, path: &str) -> Result<Vec<String>, ApiError> {
        paths_of(self.post_json("/api/pdf/split", json!({ "path": path }))?)
    }

    /// Joins PDFs, in this order, into a new one beside the first; returns its path.
    pub fn pdf_merge(&self, paths: &[String]) -> Result<String, ApiError> {
        path_of(self.post_json("/api/pdf/merge", json!({ "paths": paths }))?)
    }

    /// Turns pages (all when `pages` is empty) clockwise by 90, 180, or 270 degrees, into a
    /// new PDF; returns its path.
    pub fn pdf_rotate(&self, path: &str, pages: &str, degrees: i64) -> Result<String, ApiError> {
        let pages = if pages.trim().is_empty() { serde_json::Value::Null } else { json!(pages.trim()) };
        path_of(self.post_json("/api/pdf/rotate", json!({ "path": path, "pages": pages, "degrees": degrees }))?)
    }

    /// Earlier versions of a file, newest first.
    pub fn versions(&self, path: &str) -> Result<Vec<FileVersion>, ApiError> {
        #[derive(serde::Deserialize)]
        struct Versions {
            versions: Vec<FileVersion>,
        }
        let found: Versions = self.get_json("/api/versions", &[("path", path)])?;
        Ok(found.versions)
    }

    /// Puts an earlier version back; what it replaces becomes a version too.
    pub fn restore_version(&self, id: i64) -> Result<(), ApiError> {
        self.post_json("/api/versions/restore", json!({ "id": id }))?;
        Ok(())
    }

    pub fn delete_version(&self, id: i64) -> Result<(), ApiError> {
        self.post_json("/api/versions/delete", json!({ "id": id }))?;
        Ok(())
    }

    /// Deletes everything in the Trash for good.
    pub fn empty_trash(&self) -> Result<(), ApiError> {
        self.post_json("/api/trash/empty", json!({}))?;
        Ok(())
    }

    /// Whether the library already has a photo with these contents.
    pub fn check_photo(&self, name: &str, size: u64, sha256: &str, folder: &str, workspace: Option<i64>) -> Result<PhotoCheck, ApiError> {
        let response = self.post_json(
            "/api/gallery/upload/check",
            json!({ "name": name, "size": size, "sha256": sha256, "folder": folder, "workspace": workspace }),
        )?;
        response.into_json().map_err(|err| ApiError::Other(err.to_string()))
    }

    /// The Gallery's workspaces.
    pub fn workspaces(&self) -> Result<Vec<WorkspaceInfo>, ApiError> {
        #[derive(Deserialize)]
        struct Answer {
            workspaces: Vec<WorkspaceInfo>,
        }
        let response = self.request("GET", "/api/workspaces").fetch()?;
        let answer: Answer = response.into_json().map_err(|err| ApiError::Other(err.to_string()))?;
        Ok(answer.workspaces)
    }

    /// Has the Gallery look through the library again, to show what was added.
    pub fn rescan_gallery(&self) -> Result<(), ApiError> {
        self.post_json("/api/gallery/rescan", json!({}))?;
        Ok(())
    }

    /// Moves a file or folder to the server's Trash, or deletes it if it is already there.
    pub fn trash(&self, path: &str) -> Result<(), ApiError> {
        self.request("DELETE", "/api/entry")
            .query("path", path)
            .fetch()?;
        Ok(())
    }

    /// What is already at `name` in `dir`, if anything. Ask before uploading, as the web app does.
    pub fn check_upload(&self, dir: &str, name: &str) -> Result<Option<Existing>, ApiError> {
        let response = self.post_json("/api/upload/conflicts", json!({ "path": dir, "names": [name] }))?;
        let body: CheckResponse = response.into_json().map_err(|err| ApiError::Other(err.to_string()))?;
        Ok(body.items.into_iter().next().filter(|item| item.exists).map(|item| item.existing))
    }

    /// Uploads one file into `dir` through the web app's upload endpoint. Without `conflict`,
    /// a taken name comes back as [`Uploaded::Exists`]. `size` is sent when known. `mtime`
    /// (milliseconds) becomes the saved file's modification time, as the web app sends it, and
    /// is what [`Conflict::ArchiveOlder`] compares.
    #[allow(clippy::too_many_arguments)]
    pub fn upload_file(
        &self,
        dir: &str,
        name: &str,
        conflict: Option<Conflict>,
        mtime: Option<i64>,
        size: Option<u64>,
        body: impl Read,
        progress: Progress,
    ) -> Result<Uploaded, ApiError> {
        self.upload_file_at(dir, name, conflict, mtime, size, body, progress).map(|(uploaded, _)| uploaded)
    }

    /// [`Api::upload_file`], also saying where the file was saved (a server from before this
    /// does not say).
    #[allow(clippy::too_many_arguments)]
    pub fn upload_file_at(
        &self,
        dir: &str,
        name: &str,
        conflict: Option<Conflict>,
        mtime: Option<i64>,
        size: Option<u64>,
        body: impl Read,
        progress: Progress,
    ) -> Result<(Uploaded, Option<String>), ApiError> {
        let boundary = format!("----OwnNAS{}", hex::encode(rand::random::<[u8; 12]>()));
        let (head, tail) = form_parts(&boundary, name);
        let mut request = self
            .request("POST", "/api/upload")
            .query("path", dir)
            .set("Content-Type", &format!("multipart/form-data; boundary={boundary}"));
        if let Some(conflict) = conflict {
            request = request.query("conflict", conflict.param());
        }
        if let Some(mtime) = mtime.filter(|mtime| *mtime > 0) {
            request = request.query("mtime", &mtime.to_string());
            if conflict == Some(Conflict::ArchiveOlder) {
                request = request.query("modified", &(mtime / 1000).to_string());
            }
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
                let path = body.paths.into_iter().next();
                Ok((if body.saved > 0 { Uploaded::Saved } else { Uploaded::Skipped }, path))
            }
            Err(_) if cancelled.get() => Err(ApiError::Cancelled),
            Err(ureq::Error::Status(409, response)) => {
                let body: ConflictResponse = response.into_json().unwrap_or(ConflictResponse { exists: false, existing: None });
                match body.existing.filter(|_| body.exists) {
                    Some(existing) => Ok((Uploaded::Exists(existing), None)),
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
