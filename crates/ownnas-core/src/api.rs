//! HTTP client for the OwnNAS sync API.
use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use crate::model::{Node, Stamp};

pub struct Api {
    agent: ureq::Agent,
    pub(crate) server: String,
    pub(crate) token: Option<String>,
    /// Also used for terminal WebSockets, so they trust the same certificates.
    pub(crate) tls: Arc<rustls::ClientConfig>,
}

#[derive(Deserialize)]
struct RemoteEntry {
    path: String,
    dir: bool,
    size: u64,
    mtime: i64,
}

#[derive(Deserialize)]
struct TreeResponse {
    readonly: bool,
    entries: Vec<RemoteEntry>,
}

#[derive(Deserialize)]
struct EntryResponse {
    entry: RemoteEntry,
}

pub struct Tree {
    pub readonly: bool,
    /// Paths relative to the synced remote folder.
    pub nodes: Vec<(String, Node)>,
}

#[derive(Debug)]
pub enum ApiError {
    /// The server no longer has what the client based its change on.
    Conflict,
    Unauthorized,
    NotFound,
    /// Terminals have their own password; unlock them with it first.
    Locked,
    /// The caller stopped the transfer.
    Cancelled,
    Other(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Conflict => write!(f, "changed on the server"),
            ApiError::Unauthorized => write!(f, "the server rejected the sign-in token; run `ownnas-sync setup` again"),
            ApiError::NotFound => write!(f, "not found on the server"),
            ApiError::Locked => write!(f, "enter the terminal password"),
            ApiError::Cancelled => write!(f, "cancelled"),
            ApiError::Other(message) => write!(f, "{message}"),
        }
    }
}

pub(crate) fn map_err(err: ureq::Error) -> ApiError {
    match err {
        ureq::Error::Status(401, _) => ApiError::Unauthorized,
        ureq::Error::Status(404, _) => ApiError::NotFound,
        ureq::Error::Status(409, _) => ApiError::Conflict,
        ureq::Error::Status(code, response) => {
            let body = response.into_json::<serde_json::Value>().unwrap_or_default();
            if body["locked"] == true {
                return ApiError::Locked;
            }
            let message = body["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("HTTP {code}"));
            ApiError::Other(message)
        }
        ureq::Error::Transport(transport) => ApiError::Other(transport.to_string()),
    }
}

fn base_param(base: Option<Stamp>) -> String {
    match base {
        Some(stamp) => format!("{}:{}", stamp.size, stamp.mtime),
        None => "none".to_string(),
    }
}

impl Api {
    /// `ca_pem` adds a private CA (such as the one `ownnas tls` generates) to the public roots.
    pub fn new(server: &str, token: Option<String>, ca_pem: Option<&[u8]>) -> Result<Self, String> {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        if let Some(pem) = ca_pem {
            use rustls::pki_types::{pem::PemObject, CertificateDer};
            let mut added = 0;
            for cert in CertificateDer::pem_slice_iter(pem) {
                let cert = cert.map_err(|err| format!("Could not read the CA certificate: {err}"))?;
                roots
                    .add(cert)
                    .map_err(|err| format!("Could not use the CA certificate: {err}"))?;
                added += 1;
            }
            if added == 0 {
                return Err("The CA certificate file has no PEM certificates".into());
            }
        }
        let tls = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|err| err.to_string())?
            .with_root_certificates(roots)
            .with_no_client_auth();
        let tls = Arc::new(tls);
        let agent = ureq::AgentBuilder::new()
            .tls_config(tls.clone())
            .timeout_connect(Duration::from_secs(15))
            .timeout_read(Duration::from_secs(120))
            .build();
        Ok(Self {
            agent,
            server: server.trim_end_matches('/').to_string(),
            token,
            tls,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.server, path)
    }

    /// Sends a JSON POST. ureq retries GET and DELETE when a pooled connection turns out to be
    /// dead (phones and NATs drop idle ones), but not POST, so this does it here: the request
    /// never reached the server in that case, so sending it again is safe.
    pub(crate) fn post_json(&self, path: &str, body: serde_json::Value) -> Result<ureq::Response, ApiError> {
        match self.request("POST", path).send_json(&body) {
            Err(ureq::Error::Transport(transport)) if transport.kind() == ureq::ErrorKind::Io => {
                self.request("POST", path).send_json(&body).map_err(map_err)
            }
            other => other.map_err(map_err),
        }
    }

    pub(crate) fn request(&self, method: &str, path: &str) -> ureq::Request {
        let request = self.agent.request(method, &self.url(path));
        match &self.token {
            Some(token) => request.set("Authorization", &format!("Bearer {token}")),
            None => request.set("x-ownnas", "1"),
        }
    }

    pub fn login(&self, username: &str, password: &str) -> Result<String, ApiError> {
        let response = self.post_json("/api/login", json!({ "username": username, "password": password, "token": true }))?;
        let body: serde_json::Value = response.into_json().map_err(|err| ApiError::Other(err.to_string()))?;
        body["token"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| ApiError::Other("This OwnNAS server is too old for sync; update it first".into()))
    }

    pub fn logout(&self) -> Result<(), ApiError> {
        self.post_json("/api/logout", json!({}))?;
        Ok(())
    }

    /// Lists everything under `remote_root`, with paths made relative to it.
    pub fn tree(&self, remote_root: &str) -> Result<Tree, ApiError> {
        let response = self
            .request("GET", "/api/sync/tree")
            .query("path", remote_root)
            .call()
            .map_err(map_err)?;
        let body: TreeResponse = response.into_json().map_err(|err| ApiError::Other(err.to_string()))?;
        let prefix = if remote_root.is_empty() { String::new() } else { format!("{remote_root}/") };
        let nodes = body
            .entries
            .into_iter()
            .filter_map(|entry| {
                let rel = entry.path.strip_prefix(&prefix)?.to_string();
                Some((rel, node_of(&entry)))
            })
            .collect();
        Ok(Tree { readonly: body.readonly, nodes })
    }

    /// Streams a file. Returns the reader and the server's stamp for those bytes.
    pub fn download(&self, path: &str) -> Result<(Box<dyn Read + Send + Sync>, Stamp), ApiError> {
        let response = self
            .request("GET", "/api/sync/file")
            .query("path", path)
            .call()
            .map_err(map_err)?;
        let header = |name: &str| response.header(name).and_then(|value| value.parse::<i64>().ok());
        let stamp = match (header("x-ownnas-size"), header("x-ownnas-mtime")) {
            (Some(size), Some(mtime)) => Stamp { size: size as u64, mtime },
            _ => return Err(ApiError::Other("The server did not send a file version".into())),
        };
        Ok((response.into_reader(), stamp))
    }

    /// Uploads a file if the server still has `base` (`None` = nothing) at `path`.
    pub fn upload(
        &self,
        path: &str,
        base: Option<Stamp>,
        mtime: i64,
        size: u64,
        body: impl Read,
    ) -> Result<Stamp, ApiError> {
        let response = self
            .request("PUT", "/api/sync/file")
            .query("path", path)
            .query("base", &base_param(base))
            .query("mtime", &mtime.to_string())
            .set("Content-Type", "application/octet-stream")
            .set("Content-Length", &size.to_string())
            .send(body)
            .map_err(map_err)?;
        let body: EntryResponse = response.into_json().map_err(|err| ApiError::Other(err.to_string()))?;
        Ok(node_of(&body.entry).stamp)
    }

    pub fn mkdir(&self, path: &str) -> Result<(), ApiError> {
        self.post_json("/api/sync/mkdir", json!({ "path": path }))?;
        Ok(())
    }

    /// Deletes a file (`Some(stamp)`) or an empty folder (`None`). Files go to the server's Trash.
    pub fn delete(&self, path: &str, base: Option<Stamp>) -> Result<(), ApiError> {
        let base = match base {
            Some(stamp) => base_param(Some(stamp)),
            None => "dir".to_string(),
        };
        self.post_json("/api/sync/delete", json!({ "path": path, "base": base }))?;
        Ok(())
    }

    pub fn sha256(&self, path: &str) -> Result<String, ApiError> {
        let response = self
            .request("GET", "/api/hash")
            .query("path", path)
            .call()
            .map_err(map_err)?;
        let body: serde_json::Value = response.into_json().map_err(|err| ApiError::Other(err.to_string()))?;
        body["sha256"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| ApiError::Other("The server did not return a hash".into()))
    }
}

fn node_of(entry: &RemoteEntry) -> Node {
    Node {
        dir: entry.dir,
        stamp: Stamp {
            size: if entry.dir { 0 } else { entry.size },
            mtime: if entry.dir { 0 } else { entry.mtime },
        },
    }
}
