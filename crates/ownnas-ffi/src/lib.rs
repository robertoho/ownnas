//! What the native apps can call in ownnas-core. Every call blocks, so apps run them off
//! the main thread (Kotlin: `Dispatchers.IO`).
use std::path::Path;
use std::sync::Arc;

use ownnas_core::api::{Api, ApiError};
use ownnas_core::browse;

uniffi::setup_scaffolding!();

#[derive(Debug, uniffi::Error)]
pub enum ClientError {
    /// The token or password was rejected; sign in again.
    Unauthorized,
    NotFound,
    /// The file changed on the server since it was read.
    Conflict,
    /// [`Transfer::keep_going`] returned false.
    Cancelled,
    Failed { reason: String },
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Unauthorized => write!(f, "Sign in again"),
            ClientError::NotFound => write!(f, "Not found on the server"),
            ClientError::Conflict => write!(f, "Changed on the server"),
            ClientError::Cancelled => write!(f, "Cancelled"),
            ClientError::Failed { reason } => write!(f, "{reason}"),
        }
    }
}

impl From<ApiError> for ClientError {
    fn from(err: ApiError) -> Self {
        match err {
            ApiError::Unauthorized => ClientError::Unauthorized,
            ApiError::NotFound => ClientError::NotFound,
            ApiError::Conflict => ClientError::Conflict,
            ApiError::Cancelled => ClientError::Cancelled,
            ApiError::Other(reason) => ClientError::Failed { reason },
        }
    }
}

fn failed(reason: String) -> ClientError {
    ClientError::Failed { reason }
}

#[derive(uniffi::Record)]
pub struct Account {
    pub username: String,
    pub admin: bool,
    pub readonly: bool,
    pub version: String,
    pub root_name: String,
    pub terminal: bool,
}

#[derive(uniffi::Record)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub dir: bool,
    pub size: u64,
    /// Milliseconds since the Unix epoch.
    pub modified: i64,
    pub kind: String,
    pub thumb: bool,
}

#[derive(uniffi::Record)]
pub struct Folder {
    pub path: String,
    /// `None` at the top of the share.
    pub parent: Option<String>,
    pub root_name: String,
    pub entries: Vec<Entry>,
    pub truncated: bool,
}

/// Follows an upload or download. Called often from the transferring thread.
#[uniffi::export(with_foreign)]
pub trait Transfer: Send + Sync {
    /// `done` bytes have moved so far. Return false to cancel.
    fn keep_going(&self, done: u64) -> bool;
}

/// Checks and normalizes a typed server address, adding `http://` when there is no scheme.
#[uniffi::export]
pub fn normalize_address(input: String) -> Result<String, ClientError> {
    browse::normalize_address(&input).map_err(failed)
}

/// Signs in with a password and returns a token for [`Server::new`].
#[uniffi::export]
pub fn sign_in(address: String, username: String, password: String, ca_pem: Option<Vec<u8>>) -> Result<String, ClientError> {
    let api = Api::new(&address, None, ca_pem.as_deref()).map_err(failed)?;
    Ok(api.login(&username, &password)?)
}

/// One signed-in server.
#[derive(uniffi::Object)]
pub struct Server {
    api: Api,
}

#[uniffi::export]
impl Server {
    #[uniffi::constructor]
    pub fn new(address: String, token: String, ca_pem: Option<Vec<u8>>) -> Result<Arc<Self>, ClientError> {
        let api = Api::new(&address, Some(token), ca_pem.as_deref()).map_err(failed)?;
        Ok(Arc::new(Server { api }))
    }

    /// Ends this token's session on the server.
    pub fn sign_out(&self) -> Result<(), ClientError> {
        Ok(self.api.logout()?)
    }

    pub fn account(&self) -> Result<Account, ClientError> {
        let account = self.api.account()?;
        Ok(Account {
            username: account.username,
            admin: account.admin,
            readonly: account.readonly,
            version: account.version,
            root_name: account.root_name,
            terminal: account.terminal,
        })
    }

    pub fn list(&self, path: String) -> Result<Folder, ClientError> {
        let folder = self.api.list(&path)?;
        Ok(Folder {
            // The server reports "" as the parent of a top-level folder and none for the top.
            parent: folder.parent,
            path: folder.path,
            root_name: folder.root_name,
            truncated: folder.truncated,
            entries: folder
                .entries
                .into_iter()
                .map(|entry| Entry {
                    name: entry.name,
                    path: entry.path,
                    dir: entry.dir,
                    size: entry.size,
                    modified: entry.modified.saturating_mul(1000),
                    kind: entry.kind,
                    thumb: entry.thumb,
                })
                .collect(),
        })
    }

    /// PNG bytes of a file's thumbnail.
    pub fn thumbnail(&self, path: String) -> Result<Vec<u8>, ClientError> {
        Ok(self.api.thumbnail(&path)?)
    }

    /// Downloads a file to the local path `destination`; returns its size.
    pub fn download(&self, path: String, destination: String, transfer: Arc<dyn Transfer>) -> Result<u64, ClientError> {
        Ok(self.api.download_to(&path, Path::new(&destination), &|done| transfer.keep_going(done))?)
    }

    /// Creates the folder `name` inside `parent`; returns its path.
    pub fn create_folder(&self, parent: String, name: String) -> Result<String, ClientError> {
        Ok(self.api.create_folder(&parent, &name)?)
    }

    /// Renames in place; returns the new path.
    pub fn rename(&self, path: String, name: String) -> Result<String, ClientError> {
        Ok(self.api.rename(&path, &name)?)
    }

    /// Moves to the server's Trash (or deletes, inside the Trash).
    pub fn trash(&self, path: String) -> Result<(), ClientError> {
        Ok(self.api.trash(&path)?)
    }

    /// Uploads from an open file descriptor, which this call takes over and closes, as a new
    /// file in `dir`. A taken name gets a number. `size` and `mtime_ms` may be unknown.
    /// Returns the file's path on the server.
    #[cfg(unix)]
    pub fn upload_fd(
        &self,
        dir: String,
        name: String,
        fd: i32,
        size: Option<u64>,
        mtime_ms: Option<i64>,
        transfer: Arc<dyn Transfer>,
    ) -> Result<String, ClientError> {
        use std::os::fd::{FromRawFd, OwnedFd};
        if fd < 0 {
            return Err(failed("The file could not be opened".into()));
        }
        // SAFETY: the caller hands over an open descriptor it no longer uses.
        let file = std::fs::File::from(unsafe { OwnedFd::from_raw_fd(fd) });
        Ok(self.api.upload_new(&dir, &name, size, mtime_ms, file, &|done| transfer.keep_going(done))?)
    }
}
