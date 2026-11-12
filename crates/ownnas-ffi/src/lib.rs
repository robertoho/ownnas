//! What the native apps can call in ownnas-core. Every call blocks, so apps run them off
//! the main thread (Kotlin: `Dispatchers.IO`).
use std::path::Path;
use std::sync::Arc;

use ownnas_core::api::{Api, ApiError};
use ownnas_core::trust::Trust;
use ownnas_core::{browse, manage, terminal};

uniffi::setup_scaffolding!();

mod gallery;
pub use gallery::*;

#[derive(Debug, uniffi::Error)]
pub enum ClientError {
    /// The token or password was rejected; sign in again.
    Unauthorized,
    NotFound,
    /// The file changed on the server since it was read.
    Conflict,
    /// [`Transfer::keep_going`] returned false.
    Cancelled,
    /// Terminals have their own password: ask for it and call [`Server::unlock_terminals`].
    Locked,
    /// The server's HTTPS certificate is not trusted: offer [`server_fingerprint`] to pin, or a CA file.
    Untrusted,
    Failed { reason: String },
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Unauthorized => write!(f, "Sign in again"),
            ClientError::NotFound => write!(f, "Not found on the server"),
            ClientError::Conflict => write!(f, "Changed on the server"),
            ClientError::Cancelled => write!(f, "Cancelled"),
            ClientError::Locked => write!(f, "Enter the terminal password"),
            ClientError::Untrusted => write!(f, "The server's certificate is not trusted"),
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
            ApiError::Locked => ClientError::Locked,
            ApiError::Untrusted => ClientError::Untrusted,
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
    /// Protected from deletion: "self", "contents", or "inside" a protected folder.
    #[uniffi(default = None)]
    pub protected: Option<String>,
}

/// How much a folder holds.
#[derive(uniffi::Record)]
pub struct FolderUsage {
    pub bytes: u64,
    pub files: u64,
    /// The count stopped early: at least this much.
    pub truncated: bool,
}

/// A video conversion running on the server.
#[derive(uniffi::Record)]
pub struct VideoJob {
    pub id: String,
    /// Where the new file goes.
    pub output: String,
    /// 0 to 1.
    pub progress: f64,
    pub done: bool,
    pub error: Option<String>,
}

impl From<ownnas_core::manage::VideoJob> for VideoJob {
    fn from(job: ownnas_core::manage::VideoJob) -> Self {
        VideoJob { id: job.id, output: job.output, progress: job.progress, done: job.done, error: job.error }
    }
}

/// An earlier version of a file, kept when it was replaced.
#[derive(uniffi::Record)]
pub struct FileVersion {
    pub id: i64,
    /// When it was replaced (milliseconds since 1970).
    pub saved_at: i64,
    /// When it had last been changed before that (milliseconds since 1970).
    pub modified: i64,
    pub size: u64,
    pub by: String,
    /// What replaced it: "edit", "upload", "sync", or "restore".
    pub reason: String,
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

/// A path that changed on the server, and what is there now.
#[derive(uniffi::Record, serde::Deserialize)]
pub struct Change {
    pub path: String,
    #[serde(default)]
    pub deleted: bool,
    #[serde(default)]
    pub dir: bool,
    #[serde(default)]
    pub size: u64,
    /// Milliseconds since the Unix epoch.
    #[serde(default, rename = "mtime")]
    pub modified: i64,
}

/// What changed since a cursor (see `Server::changes`).
#[derive(uniffi::Record, serde::Deserialize)]
pub struct Changes {
    /// Where to ask from next.
    pub cursor: String,
    /// The cursor no longer counts (the server restarted, or changes were missed): list again.
    pub expired: bool,
    /// False when the server cannot watch its folder: changes made on its disk are not seen.
    pub watching: bool,
    pub changes: Vec<Change>,
}

/// A file or folder that matches a search.
#[derive(uniffi::Record, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub name: String,
    pub path: String,
    pub dir: bool,
    pub size: u64,
    /// Seconds since the Unix epoch.
    pub modified: i64,
    pub kind: String,
    /// The tag that matched, when it was a tag.
    #[serde(default)]
    pub matched_tag: Option<String>,
    /// The words around the match inside the file, when it was its contents.
    #[serde(default)]
    pub matched_content: Option<String>,
}

/// What to do when an upload's name is taken; the web app's choices.
#[derive(uniffi::Enum, Clone, Copy)]
pub enum Conflict {
    Overwrite,
    KeepBoth,
    ArchiveOlder,
    ArchiveExisting,
    Ignore,
}

impl From<Conflict> for manage::Conflict {
    fn from(conflict: Conflict) -> Self {
        match conflict {
            Conflict::Overwrite => manage::Conflict::Overwrite,
            Conflict::KeepBoth => manage::Conflict::KeepBoth,
            Conflict::ArchiveOlder => manage::Conflict::ArchiveOlder,
            Conflict::ArchiveExisting => manage::Conflict::ArchiveExisting,
            Conflict::Ignore => manage::Conflict::Ignore,
        }
    }
}

/// What the server already has at an upload's name.
#[derive(uniffi::Record)]
pub struct Existing {
    pub dir: bool,
    /// Milliseconds since the Unix epoch.
    pub modified: i64,
    pub size: u64,
}

impl From<manage::Existing> for Existing {
    fn from(existing: manage::Existing) -> Self {
        Existing { dir: existing.dir, modified: existing.modified.saturating_mul(1000), size: existing.size }
    }
}

#[derive(uniffi::Enum)]
pub enum UploadResult {
    Saved,
    Skipped,
    /// The name is taken; ask, then upload again with a [`Conflict`] choice.
    Exists { existing: Existing },
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

fn trust(ca_pem: Option<Vec<u8>>, pinned: Vec<String>) -> Trust {
    Trust { ca_pem, pinned }
}

/// Signs in with a password and returns a token for [`Server::new`]. `ca_pem` is a private
/// CA to trust (the `ca.crt` from `ownnas tls generate`); `pinned` are certificate
/// fingerprints the user accepted.
#[uniffi::export]
pub fn sign_in(
    address: String,
    username: String,
    password: String,
    ca_pem: Option<Vec<u8>>,
    pinned: Vec<String>,
) -> Result<String, ClientError> {
    let api = Api::with_trust(&address, None, &trust(ca_pem, pinned)).map_err(failed)?;
    Ok(api.login(&username, &password)?)
}

/// The SHA-256 fingerprint (`AB:CD:…`) of the certificate an `https://` server presents, to
/// compare with what `ownnas serve` prints before pinning it.
#[uniffi::export]
pub fn server_fingerprint(address: String) -> Result<String, ClientError> {
    ownnas_core::trust::server_fingerprint(&address).map_err(failed)
}

/// One signed-in server.
#[derive(uniffi::Object)]
pub struct Server {
    api: Api,
    /// The Gallery workspace the gallery calls show (`None`: every photo).
    workspace: std::sync::Mutex<Option<i64>>,
}

#[uniffi::export]
impl Server {
    #[uniffi::constructor]
    pub fn new(address: String, token: String, ca_pem: Option<Vec<u8>>, pinned: Vec<String>) -> Result<Arc<Self>, ClientError> {
        let api = Api::with_trust(&address, Some(token), &trust(ca_pem, pinned)).map_err(failed)?;
        Ok(Arc::new(Server { api, workspace: std::sync::Mutex::new(None) }))
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
        self.list_with_hidden(path, false)
    }

    /// Lists a folder, with hidden files (names starting with a dot) when `hidden`.
    pub fn list_with_hidden(&self, path: String, hidden: bool) -> Result<Folder, ClientError> {
        let folder = self.api.list_with(&path, hidden)?;
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
                    // To the millisecond when the server says, so it matches the change feed and
                    // the version an upload checks.
                    modified: if entry.mtime_ms > 0 { entry.mtime_ms } else { entry.modified.saturating_mul(1000) },
                    kind: entry.kind,
                    thumb: entry.thumb,
                    protected: entry.protected,
                })
                .collect(),
        })
    }

    /// What changed on the server since `cursor` ("" starts from now), waiting up to
    /// `wait_seconds` for a change when there is none yet.
    pub fn changes(&self, cursor: String, wait_seconds: u32) -> Result<Changes, ClientError> {
        let wait = wait_seconds.min(55).to_string();
        Ok(self.api.get_json("/api/sync/changes", &[("cursor", &cursor), ("wait", &wait)])?)
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

    /// Moves into the folder `dest` (`""` is the top); returns the new path.
    pub fn move_to(&self, path: String, dest: String) -> Result<String, ClientError> {
        Ok(self.api.move_to(&path, &dest)?)
    }

    /// Copies into the folder `dest`, keeping both when the name is taken; returns the copy's path.
    pub fn copy_to(&self, path: String, dest: String) -> Result<String, ClientError> {
        Ok(self.api.copy_to(&path, &dest)?)
    }

    /// Copies next to itself ("name copy"); returns the copy's path.
    pub fn duplicate(&self, path: String) -> Result<String, ClientError> {
        Ok(self.api.duplicate(&path)?)
    }

    /// Puts an item in the Trash back where it was; returns its path.
    pub fn restore(&self, path: String) -> Result<String, ClientError> {
        Ok(self.api.restore(&path)?)
    }

    /// Packs items from one folder into an archive beside them ("zip", "tar", "tar.gz",
    /// "tar.xz"); returns its path.
    pub fn compress(&self, paths: Vec<String>, format: String) -> Result<String, ClientError> {
        Ok(self.api.compress(&paths, &format)?)
    }

    /// Unpacks an archive into a folder beside it, or loose beside it with `here`; returns
    /// the paths made.
    pub fn extract(&self, path: String, here: bool) -> Result<Vec<String>, ClientError> {
        Ok(self.api.extract(&path, here)?)
    }

    /// Tags a file or folder; returns the tag as stored.
    pub fn add_tag(&self, path: String, tag: String) -> Result<String, ClientError> {
        Ok(self.api.add_tag(&path, &tag)?)
    }

    /// Protects items from deletion, or allows it again; `contents` covers what folders hold.
    pub fn set_protection(&self, paths: Vec<String>, protect: bool, contents: bool) -> Result<(), ClientError> {
        Ok(self.api.set_protection(&paths, protect, contents)?)
    }

    /// A copy of a photo in another format beside it; returns its path.
    pub fn convert_image(&self, path: String, format: String) -> Result<String, ClientError> {
        Ok(self.api.convert_image(&path, &format)?)
    }

    /// Makes a photo smaller (to `percent`, or to fit `longest` pixels), as a copy or in place
    /// with `replace`. False when it was that small already.
    pub fn resize_image(&self, path: String, percent: Option<u32>, longest: Option<u32>, replace: bool) -> Result<bool, ClientError> {
        Ok(self.api.resize_image(&path, percent, longest, replace)?)
    }

    /// Pages ("1,3,5-8") of a PDF into a new one, or one per page with `separate`.
    pub fn pdf_extract(&self, path: String, pages: String, separate: bool) -> Result<Vec<String>, ClientError> {
        Ok(self.api.pdf_extract(&path, &pages, separate)?)
    }

    /// One PDF per page, in a folder beside it.
    pub fn pdf_split(&self, path: String) -> Result<Vec<String>, ClientError> {
        Ok(self.api.pdf_split(&path)?)
    }

    /// Joins PDFs in this order; returns the new one's path.
    pub fn pdf_merge(&self, paths: Vec<String>) -> Result<String, ClientError> {
        Ok(self.api.pdf_merge(&paths)?)
    }

    /// Turns pages (all when `pages` is empty) clockwise; returns the new PDF's path.
    pub fn pdf_rotate(&self, path: String, pages: String, degrees: i64) -> Result<String, ClientError> {
        Ok(self.api.pdf_rotate(&path, &pages, degrees)?)
    }

    /// Earlier versions of a file, newest first.
    pub fn versions(&self, path: String) -> Result<Vec<FileVersion>, ClientError> {
        Ok(self
            .api
            .versions(&path)?
            .into_iter()
            .map(|version| FileVersion {
                id: version.id,
                saved_at: version.saved_at * 1000,
                modified: version.modified * 1000,
                size: version.size,
                by: version.by,
                reason: version.reason,
            })
            .collect())
    }

    /// Puts an earlier version back.
    pub fn restore_version(&self, id: i64) -> Result<(), ClientError> {
        Ok(self.api.restore_version(id)?)
    }

    pub fn delete_version(&self, id: i64) -> Result<(), ClientError> {
        Ok(self.api.delete_version(id)?)
    }

    /// Deletes everything in the Trash for good.
    pub fn empty_trash(&self) -> Result<(), ClientError> {
        Ok(self.api.empty_trash()?)
    }

    /// How much a folder holds, everything inside counted.
    pub fn folder_usage(&self, path: String) -> Result<FolderUsage, ClientError> {
        let usage = self.api.folder_usage(&path)?;
        Ok(FolderUsage { bytes: usage.bytes, files: usage.files, truncated: usage.truncated })
    }

    /// Folders this user bookmarked.
    pub fn bookmarks(&self) -> Result<Vec<String>, ClientError> {
        Ok(self.api.bookmarks()?)
    }

    pub fn set_bookmark(&self, path: String, marked: bool) -> Result<(), ClientError> {
        if marked { Ok(self.api.add_bookmark(&path)?) } else { Ok(self.api.remove_bookmark(&path)?) }
    }

    /// A new file ("txt", "md", "docx", "xlsx", "odt", "csv", "json", "html", "m3u8") in
    /// `parent`; returns its path.
    pub fn create_file(&self, parent: String, name: String, kind: String) -> Result<String, ClientError> {
        Ok(self.api.create_file(&parent, &name, &kind)?)
    }

    /// Downloads a folder as a ZIP to the local path `destination`; returns its size.
    pub fn download_zip(&self, path: String, destination: String, transfer: Arc<dyn Transfer>) -> Result<u64, ClientError> {
        Ok(self.api.zip_to(&path, Path::new(&destination), &|done| transfer.keep_going(done))?)
    }

    /// Starts converting a video beside it; follow it with `video_job`.
    pub fn convert_video(&self, path: String, format: String, height: Option<u32>, speed: f64, audio: bool) -> Result<VideoJob, ClientError> {
        Ok(self.api.convert_video(&path, &format, height, speed, audio)?.into())
    }

    pub fn video_job(&self, id: String) -> Result<VideoJob, ClientError> {
        Ok(self.api.video_job(&id)?.into())
    }

    pub fn cancel_video_job(&self, id: String) -> Result<(), ClientError> {
        Ok(self.api.cancel_video_job(&id)?)
    }

    /// Files and folders in `path` and below whose name, tags, or contents (text, PDFs, and
    /// pictures read by OCR) match `words`. `tag:name` finds one tag exactly.
    pub fn search(&self, path: String, words: String) -> Result<Vec<SearchHit>, ClientError> {
        #[derive(serde::Deserialize)]
        struct Hits {
            hits: Vec<SearchHit>,
        }
        let hits: Hits = self.api.get_json("/api/search", &[("path", &path), ("q", &words)])?;
        Ok(hits.hits)
    }

    /// Moves to the server's Trash (or deletes, inside the Trash).
    pub fn trash(&self, path: String) -> Result<(), ClientError> {
        Ok(self.api.trash(&path)?)
    }

    /// What is already at `name` in `dir`, if anything. Check before [`Server::upload_fd`].
    pub fn check_upload(&self, dir: String, name: String) -> Result<Option<Existing>, ClientError> {
        Ok(self.api.check_upload(&dir, &name)?.map(Existing::from))
    }

    /// Uploads from an open file descriptor, which this call takes over and closes, through the
    /// web app's upload endpoint. Without `conflict`, a taken name returns
    /// [`UploadResult::Exists`] so the app can ask. `mtime_ms` becomes the saved file's
    /// modification time and is what [`Conflict::ArchiveOlder`] compares.
    #[cfg(unix)]
    #[allow(clippy::too_many_arguments)]
    pub fn upload_fd(
        &self,
        dir: String,
        name: String,
        fd: i32,
        size: Option<u64>,
        mtime_ms: Option<i64>,
        conflict: Option<Conflict>,
        transfer: Arc<dyn Transfer>,
    ) -> Result<UploadResult, ClientError> {
        use std::os::fd::{FromRawFd, OwnedFd};
        if fd < 0 {
            return Err(failed("The file could not be opened".into()));
        }
        // SAFETY: the caller hands over an open descriptor it no longer uses.
        let file = std::fs::File::from(unsafe { OwnedFd::from_raw_fd(fd) });
        let outcome = self.api.upload_file(
            &dir,
            &name,
            conflict.map(manage::Conflict::from),
            mtime_ms,
            size,
            file,
            &|done| transfer.keep_going(done),
        )?;
        Ok(match outcome {
            manage::Uploaded::Saved => UploadResult::Saved,
            manage::Uploaded::Skipped => UploadResult::Skipped,
            manage::Uploaded::Exists(existing) => UploadResult::Exists { existing: existing.into() },
        })
    }

    /// Saves a file at `path` only if the server still has the version the caller started from
    /// (`base_size` and `base_mtime_ms`; both `None`: nothing may be there yet), as sync clients
    /// do so that an edit on one side never silently replaces one on the other. Returns what
    /// is there now; [`ClientError::Conflict`] when it changed on the server meanwhile.
    #[allow(clippy::too_many_arguments)]
    pub fn upload_if_unchanged(
        &self,
        path: String,
        fd: i32,
        size: u64,
        mtime_ms: i64,
        base_size: Option<u64>,
        base_mtime_ms: Option<i64>,
        transfer: Arc<dyn Transfer>,
    ) -> Result<Change, ClientError> {
        use std::os::fd::{FromRawFd, OwnedFd};
        if fd < 0 {
            return Err(failed("The file could not be opened".into()));
        }
        // SAFETY: the caller hands over an open descriptor it no longer uses.
        let file = std::fs::File::from(unsafe { OwnedFd::from_raw_fd(fd) });
        let base = match (base_size, base_mtime_ms) {
            (Some(size), Some(mtime)) => Some(ownnas_core::model::Stamp { size, mtime }),
            _ => None,
        };
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let body = Counting { inner: file, done: 0, transfer, cancelled: cancelled.clone() };
        match self.api.upload(&path, base, mtime_ms, size, body) {
            Ok(stamp) => Ok(Change { path, deleted: false, dir: false, size: stamp.size, modified: stamp.mtime }),
            Err(_) if cancelled.load(std::sync::atomic::Ordering::Relaxed) => Err(ClientError::Cancelled),
            Err(err) => Err(err.into()),
        }
    }
}

/// A file being sent, telling `transfer` how far it got (and stopping when told to).
struct Counting {
    inner: std::fs::File,
    done: u64,
    transfer: Arc<dyn Transfer>,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
}

impl std::io::Read for Counting {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buf)?;
        self.done += read as u64;
        if !self.transfer.keep_going(self.done) {
            self.cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
            return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled"));
        }
        Ok(read)
    }
}

#[derive(uniffi::Record)]
pub struct Agent {
    /// Agent id such as "claude"; `None` when no coding agent runs in the terminal.
    pub name: Option<String>,
    pub label: Option<String>,
    /// "working", "waiting" (needs you), "idle", "done", or "error".
    pub status: Option<String>,
    pub detail: Option<String>,
    pub attention: Option<String>,
    pub last_message: Option<String>,
}

#[derive(uniffi::Record)]
pub struct TerminalInfo {
    pub id: String,
    pub path: String,
    pub owner: String,
    /// Milliseconds since the Unix epoch.
    pub created: i64,
    pub running: bool,
    pub exit_code: Option<u32>,
    pub title: Option<String>,
    pub name: Option<String>,
    pub agent: Agent,
}

impl From<terminal::TerminalInfo> for TerminalInfo {
    fn from(info: terminal::TerminalInfo) -> Self {
        TerminalInfo {
            id: info.id,
            path: info.path,
            owner: info.owner,
            created: info.created.saturating_mul(1000),
            running: info.running,
            exit_code: info.exit_code,
            title: info.title,
            name: info.name,
            agent: Agent {
                name: info.agent.name,
                label: info.agent.label,
                status: info.agent.status,
                detail: info.agent.detail,
                attention: info.agent.attention,
                last_message: info.agent.last_message,
            },
        }
    }
}

#[uniffi::export]
impl Server {
    /// Every open terminal; [`ClientError::Locked`] means ask for the terminal password.
    pub fn terminals(&self) -> Result<Vec<TerminalInfo>, ClientError> {
        Ok(self.api.terminals()?.into_iter().map(TerminalInfo::from).collect())
    }

    /// Starts a shell in the folder `path`.
    pub fn open_terminal(&self, path: String, rows: u16, cols: u16) -> Result<TerminalInfo, ClientError> {
        Ok(self.api.open_terminal(&path, cols, rows)?.into())
    }

    /// Ends a terminal and everything running in it.
    pub fn close_terminal(&self, id: String) -> Result<(), ClientError> {
        Ok(self.api.close_terminal(&id)?)
    }

    /// Names a terminal, as shown in every app and in the web app's tabs.
    pub fn rename_terminal(&self, id: String, name: String) -> Result<TerminalInfo, ClientError> {
        Ok(self.api.rename_terminal(&id, &name)?.into())
    }

    pub fn unlock_terminals(&self, password: String) -> Result<(), ClientError> {
        Ok(self.api.unlock_terminals(&password)?)
    }

    /// Uploads a pasted or attached file (an open descriptor this call takes over) into the
    /// terminal's temporary folder on the server; returns its path there for
    /// [`TerminalSession::paste_paths`].
    #[cfg(unix)]
    pub fn upload_to_terminal(
        &self,
        id: String,
        name: String,
        fd: i32,
        size: Option<u64>,
        transfer: Arc<dyn Transfer>,
    ) -> Result<String, ClientError> {
        use std::os::fd::{FromRawFd, OwnedFd};
        if fd < 0 {
            return Err(failed("The file could not be opened".into()));
        }
        // SAFETY: the caller hands over an open descriptor it no longer uses.
        let file = std::fs::File::from(unsafe { OwnedFd::from_raw_fd(fd) });
        Ok(self.api.upload_to_terminal(&id, &name, size, file, &|done| transfer.keep_going(done))?)
    }

    pub fn mark_terminal_seen(&self, id: String) -> Result<(), ClientError> {
        Ok(self.api.mark_terminal_seen(&id)?)
    }

    /// Connects to a terminal's screen. It reconnects by itself until closed.
    pub fn connect_terminal(
        &self,
        id: String,
        rows: u16,
        cols: u16,
        listener: Arc<dyn TerminalListener>,
    ) -> Result<Arc<TerminalSession>, ClientError> {
        let session = terminal::Session::connect(&self.api, &id, rows, cols, Arc::new(Relay(listener)))?;
        Ok(Arc::new(TerminalSession { session }))
    }
}

#[derive(uniffi::Enum, Clone, Copy)]
pub enum TerminalConnection {
    Connecting,
    Live,
    Reconnecting,
    Exited { code: Option<u32> },
    Closed,
}

/// Hears about a terminal session, on the session's own thread.
#[uniffi::export(with_foreign)]
pub trait TerminalListener: Send + Sync {
    /// The screen changed; draw a new [`TerminalSession::snapshot`] when convenient.
    fn screen_changed(&self);
    fn connection_changed(&self, connection: TerminalConnection, message: Option<String>);
}

struct Relay(Arc<dyn TerminalListener>);

impl terminal::Listener for Relay {
    fn screen_changed(&self) {
        self.0.screen_changed();
    }

    fn connection_changed(&self, connection: terminal::Connection, message: Option<String>) {
        let connection = match connection {
            terminal::Connection::Connecting => TerminalConnection::Connecting,
            terminal::Connection::Live => TerminalConnection::Live,
            terminal::Connection::Reconnecting => TerminalConnection::Reconnecting,
            terminal::Connection::Exited(code) => TerminalConnection::Exited { code },
            terminal::Connection::Closed => TerminalConnection::Closed,
        };
        self.0.connection_changed(connection, message);
    }
}

/// A run of cells that look the same. Colors are 0xAARRGGBB; 0 is the theme's default color,
/// 1 the default background drawn as text (inverse), and 2 the default text color drawn as
/// background.
#[derive(uniffi::Record)]
pub struct TerminalRun {
    pub col: u16,
    pub width: u16,
    pub text: String,
    pub fg: u32,
    pub bg: u32,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
}

#[derive(uniffi::Record)]
pub struct TerminalScreen {
    pub rows: u16,
    pub cols: u16,
    pub lines: Vec<Vec<TerminalRun>>,
    pub cursor_row: u16,
    pub cursor_col: u16,
    pub cursor_visible: bool,
    pub scrolled_back: u32,
    pub application_cursor: bool,
    pub title: Option<String>,
}

#[derive(uniffi::Object)]
pub struct TerminalSession {
    session: terminal::Session,
}

#[uniffi::export]
impl TerminalSession {
    pub fn snapshot(&self) -> TerminalScreen {
        let snapshot = self.session.snapshot();
        TerminalScreen {
            rows: snapshot.rows,
            cols: snapshot.cols,
            lines: snapshot
                .lines
                .into_iter()
                .map(|line| {
                    line.into_iter()
                        .map(|run| TerminalRun {
                            col: run.col,
                            width: run.width,
                            text: run.text,
                            fg: run.fg,
                            bg: run.bg,
                            bold: run.bold,
                            dim: run.dim,
                            italic: run.italic,
                            underline: run.underline,
                        })
                        .collect()
                })
                .collect(),
            cursor_row: snapshot.cursor_row,
            cursor_col: snapshot.cursor_col,
            cursor_visible: snapshot.cursor_visible,
            scrolled_back: snapshot.scrolled_back,
            application_cursor: snapshot.application_cursor,
            title: snapshot.title,
        }
    }

    /// Text typed on the keyboard; returns the view to the live screen.
    pub fn type_text(&self, text: String) {
        self.session.type_text(&text);
    }

    /// Raw bytes, such as an escape sequence for a special key.
    pub fn send_bytes(&self, bytes: Vec<u8>) {
        self.session.scroll_to(0);
        self.session.send(bytes);
    }

    pub fn paste(&self, text: String) {
        self.session.paste(&text);
    }

    /// Pastes uploaded files' paths, shell-quoted and separated by spaces, as the web terminal does.
    pub fn paste_paths(&self, paths: Vec<String>) {
        self.session.paste_paths(&paths);
    }

    pub fn resize(&self, rows: u16, cols: u16) {
        self.session.resize(rows, cols);
    }

    /// Positive scrolls back into history. Returns how far back the view now is.
    pub fn scroll_by(&self, lines: i32) -> u32 {
        self.session.scroll_by(lines)
    }

    /// Disconnects; the shell keeps running on the server.
    pub fn disconnect(&self) {
        self.session.close();
    }
}

#[derive(uniffi::Record)]
pub struct SyncSummary {
    pub uploaded: u32,
    pub downloaded: u32,
    pub deleted_on_server: u32,
    pub deleted_here: u32,
    pub folders: u32,
    pub conflicts: u32,
    pub skipped: u32,
    pub failed: u32,
    /// Local changes not sent because the server is read-only.
    pub held: u32,
    /// Conflicts, skips, and errors worth showing, one per line.
    pub notices: Vec<String>,
}

#[uniffi::export]
impl Server {
    /// Two-way sync of the local folder `local_root` with the server folder `remote`, as
    /// `ownnas-sync run` does: deletions go to the server's Trash or to `.ownnas-sync/deleted`
    /// here (kept 30 days), and edits on both sides keep both copies. `device` names this phone
    /// in conflict copies. State lives in `local_root/.ownnas-sync`, so syncs resume.
    pub fn sync_folder(&self, local_root: String, remote: String, device: String) -> Result<SyncSummary, ClientError> {
        let root = Path::new(&local_root);
        std::fs::create_dir_all(root).map_err(|err| failed(format!("Could not open {local_root}: {err}")))?;
        ownnas_core::local::prune_deleted(root, 30);
        let config = ownnas_core::model::Config {
            server: String::new(),
            remote: remote.trim_matches('/').to_string(),
            username: String::new(),
            token: String::new(),
            ca_cert: None,
        };
        let notices = std::sync::Mutex::new(Vec::new());
        let collect = |message: String| notices.lock().unwrap_or_else(|err| err.into_inner()).push(message);
        let syncer = ownnas_core::engine::Syncer {
            root,
            config: &config,
            api: &self.api,
            host: device,
            dry_run: false,
            verbose: false,
            notices: Some(&collect),
        };
        let summary = syncer.run().map_err(|err| {
            if err == ApiError::Unauthorized.to_string() { ClientError::Unauthorized } else { failed(err) }
        })?;
        let count = |n: usize| n.min(u32::MAX as usize) as u32;
        Ok(SyncSummary {
            uploaded: count(summary.uploaded),
            downloaded: count(summary.downloaded),
            deleted_on_server: count(summary.deleted_remote),
            deleted_here: count(summary.deleted_local),
            folders: count(summary.folders),
            conflicts: count(summary.conflicts),
            skipped: count(summary.skipped),
            failed: count(summary.failed),
            held: count(summary.held),
            notices: notices.into_inner().unwrap_or_default(),
        })
    }
}
