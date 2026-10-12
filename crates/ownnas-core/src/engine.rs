//! Runs sync passes: scan both sides, plan, and apply the plan.
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::api::{Api, ApiError};
use crate::local;
use crate::model::{self, Base, Config, Node, State};
use crate::plan::{plan, Action};

#[derive(Default, Debug)]
pub struct Summary {
    pub uploaded: usize,
    pub downloaded: usize,
    pub deleted_remote: usize,
    pub deleted_local: usize,
    pub folders: usize,
    pub conflicts: usize,
    pub skipped: usize,
    pub failed: usize,
    /// Local changes not sent because the server is read-only.
    pub held: usize,
}

impl Summary {
    pub fn changes(&self) -> usize {
        self.uploaded + self.downloaded + self.deleted_remote + self.deleted_local + self.folders + self.conflicts
    }

    fn add(&mut self, other: &Summary) {
        self.uploaded += other.uploaded;
        self.downloaded += other.downloaded;
        self.deleted_remote += other.deleted_remote;
        self.deleted_local += other.deleted_local;
        self.folders += other.folders;
        self.conflicts += other.conflicts;
        self.skipped += other.skipped;
        self.failed += other.failed;
        self.held = other.held;
    }
}

impl std::fmt::Display for Summary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} uploaded, {} downloaded, {} deleted on server, {} deleted here, {} folders, {} conflicts",
            self.uploaded, self.downloaded, self.deleted_remote, self.deleted_local, self.folders, self.conflicts
        )?;
        if self.skipped > 0 {
            write!(f, ", {} skipped", self.skipped)?;
        }
        if self.failed > 0 {
            write!(f, ", {} failed", self.failed)?;
        }
        if self.held > 0 {
            write!(f, ", {} local changes held (server is read-only)", self.held)?;
        }
        Ok(())
    }
}

pub struct Syncer<'a> {
    pub root: &'a Path,
    pub config: &'a Config,
    pub api: &'a Api,
    pub host: String,
    pub dry_run: bool,
    pub verbose: bool,
    /// Where notices (conflicts, skips, errors) go. `None` prints them, as the CLI wants.
    pub notices: Option<&'a (dyn Fn(String) + Sync)>,
}

impl Syncer<'_> {
    /// What to call this machine in messages.
    fn device(&self) -> &'static str {
        if cfg!(any(target_os = "android", target_os = "ios")) { "phone" } else { "computer" }
    }

    fn remote_path(&self, rel: &str) -> String {
        if self.config.remote.is_empty() {
            rel.to_string()
        } else {
            format!("{}/{rel}", self.config.remote)
        }
    }

    /// Runs passes until nothing is left to do (conflict copies need a second pass to upload).
    pub fn run(&self) -> Result<Summary, String> {
        let mut total = Summary::default();
        for _ in 0..3 {
            let pass = self.pass()?;
            let done = pass.changes() == 0 || self.dry_run;
            total.add(&pass);
            if done {
                break;
            }
        }
        Ok(total)
    }

    fn pass(&self) -> Result<Summary, String> {
        let mut state = model::load_state(self.root)?;
        let tree = self.api.tree(&self.config.remote).map_err(|err| err.to_string())?;
        let remote: BTreeMap<String, Node> = tree
            .nodes
            .into_iter()
            .filter(|(path, _)| !local::ignored_path(path))
            .collect();
        let local_tree = local::scan(self.root)?;
        let actions = plan(&local_tree, &remote, &state.entries);
        let session = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
            .to_string();
        let mut summary = Summary::default();
        let mut since_save = 0;
        for action in actions {
            if tree.readonly && action.writes_remote() {
                summary.held += 1;
                continue;
            }
            if self.dry_run {
                println!("would {}", describe(&action));
                continue;
            }
            match self.apply(&action, &mut state, &session, &mut summary) {
                Ok(()) => {}
                Err(ApiError::Unauthorized) => {
                    model::save_state(self.root, &state)?;
                    return Err(ApiError::Unauthorized.to_string());
                }
                Err(err) => {
                    summary.failed += 1;
                    self.notice(format!("error: {}: {err}", action_path(&action)), true);
                }
            }
            since_save += 1;
            if since_save >= 50 {
                model::save_state(self.root, &state)?;
                since_save = 0;
            }
        }
        if !self.dry_run {
            model::save_state(self.root, &state)?;
        }
        Ok(summary)
    }

    fn log(&self, message: impl std::fmt::Display) {
        if self.verbose {
            println!("{message}");
        }
    }

    /// Something the user should hear about even without `verbose`.
    fn notice(&self, message: String, error: bool) {
        match self.notices {
            Some(notices) => notices(message),
            None if error => eprintln!("{message}"),
            None => println!("{message}"),
        }
    }

    fn apply(&self, action: &Action, state: &mut State, session: &str, summary: &mut Summary) -> Result<(), ApiError> {
        let io = |err: std::io::Error| ApiError::Other(err.to_string());
        let entries = &mut state.entries;
        match action {
            Action::Record { path, base } => {
                entries.insert(path.clone(), *base);
            }
            Action::Forget(path) => {
                entries.remove(path);
            }
            Action::Skip { path, reason } => {
                summary.skipped += 1;
                self.notice(format!("skip: {path} {reason}"), true);
            }
            Action::MkdirRemote(path) => {
                self.api.mkdir(&self.remote_path(path))?;
                entries.insert(path.clone(), Base::dir());
                summary.folders += 1;
                self.log(format!("+ server folder {path}"));
            }
            Action::MkdirLocal(path) => {
                fs::create_dir_all(local::full_path(self.root, path)).map_err(io)?;
                entries.insert(path.clone(), Base::dir());
                summary.folders += 1;
                self.log(format!("+ local folder {path}"));
            }
            Action::Upload { path, base, local: planned } => {
                let full = local::full_path(self.root, path);
                let file = fs::File::open(&full).map_err(io)?;
                let now = local::node_of(&file.metadata().map_err(io)?);
                if now.dir || now.stamp != *planned {
                    // Still being written; the next pass will pick it up.
                    return Ok(());
                }
                match self.api.upload(&self.remote_path(path), *base, planned.mtime, planned.size, file) {
                    Ok(remote) => {
                        entries.insert(path.clone(), Base::file(*planned, remote));
                        summary.uploaded += 1;
                        self.log(format!("↑ {path}"));
                    }
                    Err(ApiError::Conflict) => self.log(format!("~ {path} changed on the server; retrying")),
                    Err(err) => return Err(err),
                }
            }
            Action::Download { path, expect } => {
                if self.download(path, *expect, entries)? {
                    summary.downloaded += 1;
                    self.log(format!("↓ {path}"));
                }
            }
            Action::DeleteRemote { path, base } => match self.api.delete(&self.remote_path(path), *base) {
                Ok(()) => {
                    entries.remove(path);
                    summary.deleted_remote += 1;
                    self.log(format!("- server {path}"));
                }
                Err(ApiError::Conflict) if base.is_none() => {
                    // The server folder has items this computer has not seen: keep the folder.
                    fs::create_dir_all(local::full_path(self.root, path)).map_err(io)?;
                    entries.insert(path.clone(), Base::dir());
                }
                Err(ApiError::Conflict) => {
                    // Edited on the server after this computer deleted it; download it again.
                    entries.remove(path);
                }
                Err(err) => return Err(err),
            },
            Action::DeleteLocal { path, expect } => {
                if local::stat(self.root, path) != Some(*expect) {
                    return Ok(());
                }
                let full = local::full_path(self.root, path);
                if expect.dir && fs::read_dir(&full).map_err(io)?.next().is_some() {
                    // New local items inside a folder deleted on the server: recreate it there.
                    self.api.mkdir(&self.remote_path(path))?;
                    entries.insert(path.clone(), Base::dir());
                    return Ok(());
                }
                local::remove(self.root, path, session).map_err(io)?;
                entries.remove(path);
                summary.deleted_local += 1;
                self.log(format!("- local {path}"));
            }
            Action::Conflict { path, local: local_stamp, remote } => {
                let full = local::full_path(self.root, path);
                let same = match (local::sha256(&full), self.api.sha256(&self.remote_path(path))) {
                    (Ok(a), Ok(b)) => a == b,
                    _ => false,
                };
                if same {
                    entries.insert(path.clone(), Base::file(*local_stamp, *remote));
                    return Ok(());
                }
                let copy = local::conflict_name(path, &self.host, SystemTime::now());
                fs::rename(&full, local::full_path(self.root, &copy)).map_err(io)?;
                entries.remove(path);
                summary.conflicts += 1;
                self.notice(format!("conflict: {path} changed on both sides; kept this {}'s version as {copy}", self.device()), false);
                self.download(path, None, entries)?;
            }
        }
        Ok(())
    }

    fn download(&self, path: &str, expect: Option<Node>, entries: &mut BTreeMap<String, Base>) -> Result<bool, ApiError> {
        let (reader, remote) = self.api.download(&self.remote_path(path))?;
        let written = local::write_download(self.root, path, reader, remote.mtime, expect)
            .map_err(|err| ApiError::Other(err.to_string()))?;
        match written {
            Some(local_stamp) => {
                entries.insert(path.to_string(), Base::file(local_stamp, remote));
                Ok(true)
            }
            None => {
                self.log(format!("~ {path} changed here during download; retrying"));
                Ok(false)
            }
        }
    }
}

fn action_path(action: &Action) -> &str {
    match action {
        Action::MkdirRemote(path) | Action::MkdirLocal(path) | Action::Forget(path) => path,
        Action::Upload { path, .. }
        | Action::Download { path, .. }
        | Action::DeleteRemote { path, .. }
        | Action::DeleteLocal { path, .. }
        | Action::Conflict { path, .. }
        | Action::Record { path, .. }
        | Action::Skip { path, .. } => path,
    }
}

fn describe(action: &Action) -> String {
    let path = action_path(action);
    match action {
        Action::MkdirRemote(_) => format!("create server folder {path}"),
        Action::MkdirLocal(_) => format!("create local folder {path}"),
        Action::Upload { .. } => format!("upload {path}"),
        Action::Download { .. } => format!("download {path}"),
        Action::DeleteRemote { .. } => format!("delete {path} on the server (to its Trash)"),
        Action::DeleteLocal { .. } => format!("delete local {path} (kept in {}/deleted)", model::META_DIR),
        Action::Conflict { .. } => format!("resolve conflict on {path}"),
        Action::Record { .. } => format!("record {path} as in sync"),
        Action::Forget(_) => format!("forget {path}"),
        Action::Skip { reason, .. } => format!("skip {path}: {reason}"),
    }
}

pub fn host_name() -> String {
    let from_env = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).ok();
    let name = from_env.or_else(|| {
        std::process::Command::new("hostname")
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok())
    });
    let name = name.map(|n| n.trim().split('.').next().unwrap_or("").to_string()).unwrap_or_default();
    let clean: String = name.chars().filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_').collect();
    if clean.is_empty() { "this computer".to_string() } else { clean }
}
