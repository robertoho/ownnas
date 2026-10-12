//! Changing files on a server: new folders, renames, moving to the Trash, and uploads.
use std::collections::HashSet;
use std::io::{self, Read};

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

#[derive(Deserialize)]
struct PathResponse {
    path: Option<String>,
}

#[derive(Deserialize)]
struct UploadedEntry {
    path: String,
}

#[derive(Deserialize)]
struct UploadResponse {
    entry: UploadedEntry,
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() { name.to_string() } else { format!("{dir}/{name}") }
}

/// `name`, or `name (2).ext`, `name (3).ext`, … whichever is not in `taken`. Compares
/// without case, since the server's disk may not tell `A.jpg` and `a.jpg` apart.
pub fn free_name(taken: &HashSet<String>, name: &str) -> String {
    let lower: HashSet<String> = taken.iter().map(|name| name.to_lowercase()).collect();
    if !lower.contains(&name.to_lowercase()) {
        return name.to_string();
    }
    let (stem, ext) = match name.rfind('.') {
        Some(dot) if dot > 0 => (&name[..dot], &name[dot..]),
        _ => (name, ""),
    };
    (2..)
        .map(|n| format!("{stem} ({n}){ext}"))
        .find(|candidate| !lower.contains(&candidate.to_lowercase()))
        .expect("an unused name")
}

impl Api {
    /// Creates `name` inside `parent` and returns its path.
    pub fn create_folder(&self, parent: &str, name: &str) -> Result<String, ApiError> {
        self.request("POST", "/api/mkdir")
            .send_json(json!({ "path": parent, "name": name }))
            .map_err(map_err)?;
        Ok(join(parent, name))
    }

    /// Renames a file or folder in place and returns its new path.
    pub fn rename(&self, path: &str, name: &str) -> Result<String, ApiError> {
        let response = self
            .request("POST", "/api/rename")
            .send_json(json!({ "path": path, "name": name }))
            .map_err(map_err)?;
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

    /// Uploads a new file into `dir` without replacing anything: a taken name gets a number,
    /// as in `photo (2).jpg`. `size` is sent when known so the server can check the length;
    /// `mtime` is milliseconds since the Unix epoch. Returns the file's path on the server.
    pub fn upload_new(
        &self,
        dir: &str,
        name: &str,
        size: Option<u64>,
        mtime: Option<i64>,
        body: impl Read,
        progress: Progress,
    ) -> Result<String, ApiError> {
        let taken: HashSet<String> = self.list(dir)?.entries.into_iter().map(|entry| entry.name).collect();
        let path = join(dir, &free_name(&taken, name));
        let mut request = self
            .request("PUT", "/api/sync/file")
            .query("path", &path)
            // `none`: the server refuses if something appeared at this name meanwhile.
            .query("base", "none")
            .set("Content-Type", "application/octet-stream");
        if let Some(mtime) = mtime {
            request = request.query("mtime", &mtime.to_string());
        }
        if let Some(size) = size {
            request = request.set("Content-Length", &size.to_string());
        }
        let cancelled = std::cell::Cell::new(false);
        let watch = |done: u64| {
            let go_on = progress(done);
            cancelled.set(!go_on);
            go_on
        };
        let response = request
            .send(Counted::new(body, &watch))
            .map_err(|err| if cancelled.get() { ApiError::Cancelled } else { map_err(err) })?;
        let body: UploadResponse = response.into_json().map_err(|err| ApiError::Other(err.to_string()))?;
        Ok(body.entry.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_names_count_up_before_the_extension() {
        let taken: HashSet<String> = ["photo.jpg", "Photo (2).JPG", "notes", ".env"].map(String::from).into();
        assert_eq!(free_name(&taken, "other.jpg"), "other.jpg");
        assert_eq!(free_name(&taken, "PHOTO.jpg"), "PHOTO (3).jpg");
        assert_eq!(free_name(&taken, "notes"), "notes (2)");
        assert_eq!(free_name(&taken, ".env"), ".env (2)");
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
