//! Deleting local files once the server has them. A file goes only after the server, asked just
//! before, gives the SHA-256 of a file of its own that matches this one's, taken again then too:
//! never because of a name, a size or a date alone.
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ownnas_core::api::Api;
use ownnas_core::local;
use ownnas_core::model::Stamp;

use crate::progress::{self, Meter};
use crate::transfer::with_retries;

/// What to delete here, after an upload.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum After {
    #[default]
    Keep,
    /// Files this run uploaded.
    Uploaded,
    /// Those, and the files that were not sent because the server already has them.
    UploadedOrThere,
}

impl After {
    pub fn deletes(self) -> bool {
        self != After::Keep
    }

    pub fn deletes_there(self) -> bool {
        self == After::UploadedOrThere
    }
}

#[derive(clap::Args, Default)]
pub struct Flags {
    /// Delete each file here, for good (not to a Trash), once it is uploaded and the server's
    /// SHA-256 of it matches this file's. Files that fail, are cut off, or are not sent stay.
    /// Each deleted file is named, with a count at the end; --dry-run lists them first.
    #[arg(long, conflicts_with = "delete_after_upload_or_duplicate")]
    pub delete_after_upload: bool,
    /// The same, and also delete the files here that were not sent because the server already
    /// has them, once the server's SHA-256 of its copy matches. A file whose server copy only
    /// shares its name, size or date, or is a smaller copy, stays. Folders stay, even if emptied.
    #[arg(long)]
    pub delete_after_upload_or_duplicate: bool,
}

impl Flags {
    pub fn after(&self) -> After {
        if self.delete_after_upload_or_duplicate {
            After::UploadedOrThere
        } else if self.delete_after_upload {
            After::Uploaded
        } else {
            After::Keep
        }
    }
}

/// Why a file was not deleted, when that is not a fault.
pub enum Outcome {
    Deleted,
    Kept(String),
}

/// Deletes `file` if it is still `stamp` (when given) and its SHA-256, taken now, is
/// `server_sha256`, the server's for its copy. An error means it could not be checked or deleted.
pub fn delete_if_same(file: &Path, stamp: Option<Stamp>, server_sha256: &str) -> Result<Outcome, String> {
    let meta = std::fs::symlink_metadata(file).map_err(|err| format!("Could not look at it: {err}"))?;
    if !meta.is_file() {
        return Ok(Outcome::Kept("it is not a plain file".into()));
    }
    if stamp.is_some_and(|stamp| local::node_of(&meta).stamp != stamp) {
        return Ok(Outcome::Kept("it changed here while this ran".into()));
    }
    let here = local::sha256(file).map_err(|err| format!("Could not read it: {err}"))?;
    if !here.eq_ignore_ascii_case(server_sha256) {
        return Ok(Outcome::Kept("its contents differ from the server's copy".into()));
    }
    std::fs::remove_file(file).map_err(|err| format!("Could not delete it: {err}"))?;
    Ok(Outcome::Deleted)
}

/// What was deleted, kept and could not be checked, told as it happens and counted for the end.
#[derive(Default)]
pub struct Tally {
    pub dry_run: bool,
    deleted: Mutex<Vec<PathBuf>>,
    kept: Mutex<u64>,
    failed: Mutex<u64>,
}

impl Tally {
    pub fn new(dry_run: bool) -> Self {
        Tally { dry_run, ..Tally::default() }
    }

    fn deleted(&self, file: &Path) {
        if self.dry_run {
            outln!("would delete here: {}", file.display());
        } else {
            outln!("deleted here: {}", file.display());
        }
        self.deleted.lock().unwrap_or_else(|poison| poison.into_inner()).push(file.to_path_buf());
    }

    pub fn keep(&self, file: &Path, why: &str) {
        outln!("kept here: {}: {why}", file.display());
        *self.kept.lock().unwrap_or_else(|poison| poison.into_inner()) += 1;
    }

    /// In a dry run: one that would be deleted once uploaded, which cannot be checked before.
    pub fn planned(&self, file: &Path) {
        outln!("would delete here once uploaded and checked: {}", file.display());
        self.deleted.lock().unwrap_or_else(|poison| poison.into_inner()).push(file.to_path_buf());
    }

    pub fn fail(&self, file: &Path, why: &str) {
        errln!("not deleted: {}: {why}", file.display());
        *self.failed.lock().unwrap_or_else(|poison| poison.into_inner()) += 1;
    }

    /// Checks one file against the server's SHA-256, or the server's answer's failure, and
    /// deletes it if they match (or, in a dry run, says it would).
    pub fn settle(&self, file: &Path, stamp: Option<Stamp>, server_sha256: Result<String, String>) {
        let server_sha256 = match server_sha256 {
            Ok(sha) => sha,
            Err(why) => return self.fail(file, &format!("Could not get the server's checksum: {why}")),
        };
        if self.dry_run {
            return match local::sha256(file) {
                Ok(here) if here.eq_ignore_ascii_case(&server_sha256) => self.deleted(file),
                Ok(_) => self.keep(file, "its contents differ from the server's copy"),
                Err(err) => self.fail(file, &format!("Could not read it: {err}")),
            };
        }
        match delete_if_same(file, stamp, &server_sha256) {
            Ok(Outcome::Deleted) => self.deleted(file),
            Ok(Outcome::Kept(why)) => self.keep(file, &why),
            Err(why) => self.fail(file, &why),
        }
    }

    /// Settles each (file here, its version when listed, its path on the server), several at once.
    pub fn settle_all(&self, api: &Api, items: Vec<(PathBuf, Option<Stamp>, String)>, parallel: usize) {
        if items.is_empty() {
            return;
        }
        progress::phase(format!("Checking {} files against the server's checksums before deleting them here…", items.len()));
        let meter = Meter::new("files checked", items.len(), None);
        let queue = Mutex::new(items.into_iter().collect::<VecDeque<_>>());
        meter.run(|| {
            std::thread::scope(|scope| {
                for _ in 0..parallel.clamp(1, 64) {
                    scope.spawn(|| loop {
                        let Some((file, stamp, remote)) = queue.lock().unwrap_or_else(|poison| poison.into_inner()).pop_front() else { break };
                        let slot = meter.begin(&remote, stamp.map_or(0, |stamp| stamp.size), "asking the server for the checksum of");
                        let theirs = with_retries(|| api.sha256(&remote)).map_err(|err| crate::say(&err));
                        slot.doing("checking");
                        self.settle(&file, stamp, theirs);
                        meter.end(&slot, false);
                    });
                }
            })
        });
    }

    /// The end of a run: an error if some could not be checked or deleted.
    pub fn result(&self) -> Result<(), String> {
        match self.failed_count() {
            0 => Ok(()),
            failed => Err(format!("{failed} could not be deleted here.")),
        }
    }

    pub fn deleted_count(&self) -> usize {
        self.deleted.lock().unwrap_or_else(|poison| poison.into_inner()).len()
    }

    pub fn failed_count(&self) -> u64 {
        *self.failed.lock().unwrap_or_else(|poison| poison.into_inner())
    }

    /// For the summary line: ", 3 deleted here, 1 kept here".
    pub fn summary(&self) -> String {
        let mut line = format!(", {} {}", self.deleted_count(), if self.dry_run { "to delete here" } else { "deleted here" });
        let kept = *self.kept.lock().unwrap_or_else(|poison| poison.into_inner());
        if kept > 0 {
            line.push_str(&format!(", {kept} kept here"));
        }
        let failed = self.failed_count();
        if failed > 0 {
            line.push_str(&format!(", {failed} could not be deleted"));
        }
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ownnascli-delete-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn deletes_only_a_file_that_matches_the_servers_checksum() {
        let dir = scratch("match");
        let file = dir.join("a.jpg");
        std::fs::write(&file, b"photo").unwrap();
        let sha = local::sha256(&file).unwrap();
        let stamp = local::node_of(&std::fs::metadata(&file).unwrap()).stamp;

        let other = "0".repeat(64);
        assert!(matches!(delete_if_same(&file, Some(stamp), &other), Ok(Outcome::Kept(_))), "other contents stay");
        assert!(file.exists());
        let moved = Stamp { size: stamp.size, mtime: stamp.mtime + 1000 };
        assert!(matches!(delete_if_same(&file, Some(moved), &sha), Ok(Outcome::Kept(_))), "a file that changed stays");
        assert!(file.exists());
        assert!(matches!(delete_if_same(&file, Some(stamp), &sha.to_ascii_uppercase()), Ok(Outcome::Deleted)));
        assert!(!file.exists());
        assert!(delete_if_same(&file, None, &sha).is_err(), "a missing file is a failure, not a deletion");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_dry_run_deletes_nothing() {
        let dir = scratch("dry");
        let file = dir.join("b.txt");
        std::fs::write(&file, b"text").unwrap();
        let sha = local::sha256(&file).unwrap();
        let tally = Tally::new(true);
        tally.settle(&file, None, Ok(sha));
        assert!(file.exists());
        assert_eq!(tally.deleted_count(), 1);
        tally.settle(&file, None, Err("offline".into()));
        assert_eq!(tally.failed_count(), 1, "no checksum from the server, no deletion");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_two_options_mean_what_they_say() {
        let flags = |upload, duplicate| Flags { delete_after_upload: upload, delete_after_upload_or_duplicate: duplicate }.after();
        assert_eq!(flags(false, false), After::Keep);
        assert_eq!(flags(true, false), After::Uploaded);
        assert_eq!(flags(false, true), After::UploadedOrThere);
        assert!(!After::Uploaded.deletes_there() && After::Uploaded.deletes());
    }
}
