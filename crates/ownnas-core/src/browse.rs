//! Browsing a server: who is signed in, folder listings, and thumbnails.
use std::fs;
use std::io::{Read, Write};
use std::path::Path;

use serde::Deserialize;

use crate::api::{Api, ApiError, Fetch};
use crate::manage::{Counted, Progress};

/// Thumbnails are small PNGs; anything bigger than this is not one.
const THUMBNAIL_LIMIT: u64 = 8 * 1024 * 1024;

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub username: String,
    pub admin: bool,
    pub readonly: bool,
    pub version: String,
    pub root_name: String,
    /// Terminals are turned on and this account may open them.
    #[serde(default)]
    pub terminal: bool,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub name: String,
    /// Path relative to the server's shared folder.
    pub path: String,
    pub dir: bool,
    pub size: u64,
    /// Seconds since the Unix epoch.
    pub modified: i64,
    /// The server's file kind, such as "image", "video", "audio", "text", or "pdf".
    pub kind: String,
    /// The server can make a thumbnail for it.
    pub thumb: bool,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
    pub path: String,
    pub parent: Option<String>,
    pub root_name: String,
    pub entries: Vec<Entry>,
    /// The server stopped listing at its limit; `entries` is not everything.
    pub truncated: bool,
}

fn io_err(err: std::io::Error) -> ApiError {
    ApiError::Other(err.to_string())
}

impl Api {
    pub fn account(&self) -> Result<Account, ApiError> {
        let response = self.request("GET", "/api/me").fetch()?;
        response.into_json().map_err(io_err)
    }

    /// Lists one folder (`""` for the shared folder itself).
    pub fn list(&self, path: &str) -> Result<Folder, ApiError> {
        let response = self
            .request("GET", "/api/list")
            .query("path", path)
            .fetch()?;
        response.into_json().map_err(io_err)
    }

    /// PNG bytes of a file's thumbnail.
    pub fn thumbnail(&self, path: &str) -> Result<Vec<u8>, ApiError> {
        let response = self
            .request("GET", "/api/thumb")
            .query("path", path)
            .fetch()?;
        let mut bytes = Vec::new();
        response
            .into_reader()
            .take(THUMBNAIL_LIMIT)
            .read_to_end(&mut bytes)
            .map_err(io_err)?;
        Ok(bytes)
    }

    /// Downloads a file to `destination`, replacing it only once every byte has arrived.
    /// Returns the number of bytes written.
    pub fn download_to(&self, path: &str, destination: &Path, progress: Progress) -> Result<u64, ApiError> {
        let (reader, stamp) = self.download(path)?;
        let temp = destination.with_extension("part");
        let cancelled = std::cell::Cell::new(false);
        let watch = |done: u64| {
            let go_on = progress(done);
            cancelled.set(!go_on);
            go_on
        };
        let written = (|| {
            let mut file = fs::File::create(&temp)?;
            let written = std::io::copy(&mut Counted::new(reader, &watch), &mut file)?;
            file.flush()?;
            Ok::<u64, std::io::Error>(written)
        })()
        .map_err(|err| {
            let _ = fs::remove_file(&temp);
            if cancelled.get() { ApiError::Cancelled } else { ApiError::Other(format!("Could not save the file: {err}")) }
        })?;
        if written != stamp.size {
            let _ = fs::remove_file(&temp);
            return Err(ApiError::Other("The download was cut short".into()));
        }
        fs::rename(&temp, destination).map_err(|err| ApiError::Other(format!("Could not save the file: {err}")))?;
        Ok(written)
    }
}

/// Turns what someone typed ("nas.local:8787", "https://nas.example/") into a server address.
/// Plain HTTP is assumed without a scheme, since that is how a fresh OwnNAS listens.
pub fn normalize_address(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Enter the server's address".into());
    }
    let (scheme, rest) = match trimmed.split_once("://") {
        Some((scheme, rest)) => (scheme.to_ascii_lowercase(), rest),
        None => ("http".to_string(), trimmed),
    };
    if scheme != "http" && scheme != "https" {
        return Err("The address must start with http:// or https://".into());
    }
    let rest = rest.trim_end_matches('/');
    if rest.is_empty() || rest.contains(char::is_whitespace) {
        return Err("That is not a server address".into());
    }
    Ok(format!("{scheme}://{rest}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_get_a_scheme_and_lose_trailing_slashes() {
        assert_eq!(normalize_address(" nas.local:8787/ ").unwrap(), "http://nas.local:8787");
        assert_eq!(normalize_address("https://nas.example/").unwrap(), "https://nas.example");
        assert!(normalize_address("").is_err());
        assert!(normalize_address("ftp://nas").is_err());
        assert!(normalize_address("http://").is_err());
        assert!(normalize_address("nas local").is_err());
    }

    #[test]
    fn listings_parse_and_ignore_extra_fields() {
        let folder: Folder = serde_json::from_str(
            r#"{"path":"Photos","parent":"","rootName":"media","truncated":false,"total":1,
                "entries":[{"name":"a.jpg","path":"Photos/a.jpg","dir":false,"size":3,"modified":5,
                "created":1,"kind":"image","thumb":true,"tags":["x"]}]}"#,
        )
        .unwrap();
        assert_eq!(folder.parent.as_deref(), Some(""));
        assert_eq!(folder.entries[0].kind, "image");
        assert!(folder.entries[0].thumb);
    }
}
