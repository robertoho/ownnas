//! Uploading to the Gallery: each file is hashed here and looked for in the library first, so
//! what is already there is told and not sent.
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use ownnas_core::api::{Api, ApiError};
use ownnas_core::local;
use ownnas_core::manage::{Conflict, Uploaded};
use ownnas_core::model::Stamp;

use crate::delete::{After, Tally};
use crate::progress::{self, Slot};
use crate::transfer::{io_failure, with_retries};
use crate::{human_size, say, REMOTE_PREFIX};

pub struct Options {
    pub force: bool,
    pub parallel: usize,
    pub dry_run: bool,
    pub after: After,
}

/// What the Gallery shows, by file name.
fn is_media(name: &str) -> bool {
    const KINDS: [&str; 32] = [
        "jpg", "jpeg", "png", "gif", "webp", "avif", "heic", "heif", "jxl", "bmp", "tif", "tiff", "dng", "cr2", "cr3", "nef", "arw", "raf", "orf", "rw2", "pef", "srw", "mp4", "m4v", "mov", "webm",
        "mkv", "avi", "3gp", "mts", "m2ts", "mpg",
    ];
    let lower = name.to_ascii_lowercase();
    lower.rsplit_once('.').is_some_and(|(_, ext)| KINDS.contains(&ext))
}

struct Photo {
    full: PathBuf,
    name: String,
    size: u64,
    mtime: i64,
}

impl Photo {
    fn stamp(&self) -> Option<Stamp> {
        Some(Stamp { size: self.size, mtime: self.mtime })
    }
}

/// The photos and videos in these files and folders, and how many other files were left out.
fn collect(paths: &[PathBuf]) -> Result<(Vec<Photo>, u64), String> {
    let mut found = Vec::new();
    let mut others = 0;
    let mut add = |full: PathBuf, node: ownnas_core::model::Node| {
        let name = full.file_name().and_then(|name| name.to_str()).unwrap_or_default().to_string();
        if is_media(&name) {
            found.push(Photo { full, name, size: node.stamp.size, mtime: node.stamp.mtime });
        } else {
            others += 1;
        }
    };
    for path in paths {
        let meta = std::fs::metadata(path).map_err(|err| format!("Could not open {}: {err}", path.display()))?;
        if meta.is_dir() {
            let shown = path.display();
            progress::phase(format!("Looking through {shown}…"));
            let listing = local::scan_counting(path, &mut |seen| progress::status(format!("Looking through {shown}… {seen} found")))?;
            progress::clear();
            for (rel, node) in listing {
                if !node.dir {
                    add(local::full_path(path, &rel), node);
                }
            }
        } else {
            add(path.clone(), local::node_of(&meta));
        }
    }
    let size: u64 = found.iter().map(|photo| photo.size).sum();
    let left_out = if others > 0 { format!(", and {}, which are left out", crate::plural(others as usize, "other file")) } else { String::new() };
    progress::event(format!("Found {} ({}){left_out}", crate::plural(found.len(), "photo or video").replace("photo or videos", "photos and videos"), human_size(size)));
    Ok((found, others))
}

/// The folder to upload to, and the workspace if one was named.
fn destination(api: &Api, workspace: Option<&str>, to: Option<&str>) -> Result<(String, Option<i64>), String> {
    let Some(wanted) = workspace else {
        return match to {
            Some(folder) => Ok((folder.to_string(), None)),
            None => Err("Say where they go: --workspace <name>, or --to <folder>.".into()),
        };
    };
    let all = progress::waiting("Asking the server about its Gallery workspaces", || api.workspaces()).map_err(|err| say(&err))?;
    let Some(space) = all.iter().find(|space| space.name.eq_ignore_ascii_case(wanted)) else {
        let names: Vec<&str> = all.iter().map(|space| space.name.as_str()).collect();
        return Err(format!("There is no workspace called “{wanted}”. There are: {}", if names.is_empty() { "none yet".to_string() } else { names.join(", ") }));
    };
    let inside = |folder: &str| space.folders.iter().any(|mine| folder == mine || folder.starts_with(&format!("{mine}/")));
    match to {
        Some(folder) if inside(folder) => Ok((folder.to_string(), Some(space.id))),
        Some(folder) => Err(format!("{REMOTE_PREFIX}{folder} is not in “{}”, whose folders are: {}", space.name, space.folders.join(", "))),
        None => match space.folders.as_slice() {
            [] => Err(format!("“{}” has no folders yet, so there is nowhere to put photos.", space.name)),
            [only] => Ok((only.clone(), Some(space.id))),
            several => Err(format!("“{}” has several folders. Choose one with --to: {}", space.name, several.join(", "))),
        },
    }
}

#[derive(Default)]
struct Counts {
    uploaded: AtomicU64,
    bytes: AtomicU64,
    there: AtomicU64,
    skipped: AtomicU64,
    failed: AtomicU64,
}

/// Sends a photo and has the server read it back.
fn send(api: &Api, photo: &Photo, folder: &str, sha256: &str, slot: &Slot) -> Result<String, ApiError> {
    with_retries(|| {
        let file = std::fs::File::open(&photo.full).map_err(io_failure)?;
        slot.doing("sending");
        let sending = |done: u64| {
            slot.moved.store(done, Ordering::Relaxed);
            true
        };
        let (uploaded, path) = api.upload_file_at(folder, &photo.name, Some(Conflict::KeepBoth), Some(photo.mtime), Some(photo.size), file, &sending)?;
        let (Uploaded::Saved, Some(path)) = (uploaded, path) else {
            return Err(ApiError::Other("The server did not say where it saved the file".into()));
        };
        slot.doing("having the server read back");
        if !api.sha256(&path)?.eq_ignore_ascii_case(sha256) {
            return Err(ApiError::Other(format!("{path} does not match what was sent")));
        }
        Ok(path)
    })
}

pub fn run(api: &Api, paths: &[PathBuf], workspace: Option<&str>, to: Option<&str>, options: &Options) -> Result<(), String> {
    let (folder, workspace_id) = destination(api, workspace, to)?;
    let (found, others) = collect(paths)?;
    if found.is_empty() {
        return Err(format!("No photos or videos there{}.", if others > 0 { format!(" ({others} other files)") } else { String::new() }));
    }
    let started = std::time::Instant::now();
    let total = found.len();
    let queue = Mutex::new(found.into_iter().collect::<VecDeque<_>>());
    // Which file of this run has which contents; one that failed gives its place up.
    let claimed: Mutex<HashMap<String, String>> = Mutex::new(HashMap::new());
    let counts = Counts::default();
    let tally = Tally::new(options.dry_run);
    // The contents the server is known to have, by SHA-256: uploaded and read back, or a copy
    // whose checksum the server gave.
    let confirmed: Mutex<HashSet<String>> = Mutex::new(HashSet::new());
    let confirm = |sha256: &str| confirmed.lock().unwrap_or_else(|poison| poison.into_inner()).insert(sha256.to_string());
    // Files with the same contents as another of this run, settled once that one is.
    let twins: Mutex<Vec<(PathBuf, Option<Stamp>, String)>> = Mutex::new(Vec::new());

    let handle = |photo: &Photo, slot: &Slot| -> Result<(), ApiError> {
        progress::event(format!("looking at {} ({})", photo.full.display(), human_size(photo.size)));
        let sha256 = local::sha256_counting(&photo.full, &|read| slot.moved.store(read, Ordering::Relaxed)).map_err(io_failure)?;
        if !options.force {
            {
                let mut claims = claimed.lock().unwrap_or_else(|poison| poison.into_inner());
                if let Some(first) = claims.get(&sha256) {
                    counts.there.fetch_add(1, Ordering::Relaxed);
                    outln!("already there: {} is the same as {first}, in this upload", photo.full.display());
                    if options.after.deletes_there() {
                        twins.lock().unwrap_or_else(|poison| poison.into_inner()).push((photo.full.clone(), photo.stamp(), sha256));
                    }
                    return Ok(());
                }
                claims.insert(sha256.clone(), photo.full.display().to_string());
            }
            slot.doing("asking the server about");
            let check = with_retries(|| api.check_photo(&photo.name, photo.size, &sha256, &folder, workspace_id))?;
            if let Some(copy) = check.copy {
                counts.there.fetch_add(1, Ordering::Relaxed);
                let outside = if workspace_id.is_some() && !check.in_workspace { " (outside this workspace)" } else { "" };
                outln!("already there: {} is {REMOTE_PREFIX}{copy}{outside}", photo.full.display());
                if options.after.deletes_there() {
                    // The server found it by its checksum; its own, taken now, has the last word.
                    slot.doing("asking the server for the checksum of");
                    let theirs = with_retries(|| api.sha256(&copy)).map_err(|err| say(&err));
                    if theirs.as_deref().is_ok_and(|theirs| theirs.eq_ignore_ascii_case(&sha256)) {
                        confirm(&sha256);
                    }
                    tally.settle(&photo.full, photo.stamp(), theirs);
                }
                return Ok(());
            }
            if let Some(smaller) = check.resized {
                counts.there.fetch_add(1, Ordering::Relaxed);
                outln!("already there: {} has a smaller copy, {REMOTE_PREFIX}{smaller}", photo.full.display());
                if options.after.deletes_there() {
                    tally.keep(&photo.full, "the server has only a smaller copy of it");
                }
                return Ok(());
            }
            if check.blocked {
                counts.skipped.fetch_add(1, Ordering::Relaxed);
                outln!("skipped: {} was deleted, and kept from being backed up again", photo.full.display());
                if options.after.deletes_there() {
                    tally.keep(&photo.full, "the server does not have it");
                }
                return Ok(());
            }
        }
        if options.dry_run {
            counts.uploaded.fetch_add(1, Ordering::Relaxed);
            outln!("would upload: {}", photo.full.display());
            if options.after.deletes() {
                confirm(&sha256);
                tally.planned(&photo.full);
            }
            return Ok(());
        }
        progress::event(format!("sending {} ({})", photo.full.display(), human_size(photo.size)));
        match send(api, photo, &folder, &sha256, slot) {
            Ok(path) => {
                counts.uploaded.fetch_add(1, Ordering::Relaxed);
                counts.bytes.fetch_add(photo.size, Ordering::Relaxed);
                outln!("uploaded: {} to {REMOTE_PREFIX}{path}", photo.full.display());
                // `send` had the server read it back, and its SHA-256 matched this one's.
                confirm(&sha256);
                if options.after.deletes() {
                    slot.doing("checking before deleting");
                    tally.settle(&photo.full, photo.stamp(), Ok(sha256));
                }
                Ok(())
            }
            Err(err) => {
                claimed.lock().unwrap_or_else(|poison| poison.into_inner()).remove(&sha256);
                Err(err)
            }
        }
    };

    let meter = progress::Meter::new("photos and videos", total, None);
    meter.run(|| {
        std::thread::scope(|scope| {
            for _ in 0..options.parallel.clamp(1, 32) {
                scope.spawn(|| loop {
                    let Some(photo) = queue.lock().unwrap_or_else(|poison| poison.into_inner()).pop_front() else { break };
                    let slot = meter.begin(&photo.name, photo.size, "hashing");
                    let handled = handle(&photo, &slot);
                    meter.end(&slot, handled.is_ok());
                    if let Err(err) = handled {
                        counts.failed.fetch_add(1, Ordering::Relaxed);
                        errln!("failed: {}: {}", photo.full.display(), say(&err));
                    }
                });
            }
        })
    });

    // A twin goes only if the server has its contents, now that every first one is settled.
    let confirmed = confirmed.into_inner().unwrap_or_else(|poison| poison.into_inner());
    for (file, stamp, sha256) in twins.into_inner().unwrap_or_else(|poison| poison.into_inner()) {
        if confirmed.contains(&sha256) {
            tally.settle(&file, stamp, Ok(sha256));
        } else {
            tally.keep(&file, "the file it is the same as did not reach the server");
        }
    }

    let get = |count: &AtomicU64| count.load(Ordering::Relaxed);
    let uploaded = get(&counts.uploaded);
    let verb = if options.dry_run { "to upload" } else { "uploaded" };
    let mut line = format!("{uploaded} {verb}");
    if !options.dry_run {
        line.push_str(&format!(" ({})", human_size(get(&counts.bytes))));
    }
    line.push_str(&format!(", {} already there", get(&counts.there)));
    if get(&counts.skipped) > 0 {
        line.push_str(&format!(", {} skipped", get(&counts.skipped)));
    }
    if others > 0 {
        line.push_str(&format!(", {others} not photos or videos"));
    }
    if options.after.deletes() {
        line.push_str(&tally.summary());
    }
    outln!("{line}, of {total}, in {:.1} s", started.elapsed().as_secs_f64());
    // Their dates and thumbnails come from the scan, which then shows them in the Gallery.
    if uploaded > 0 && !options.dry_run {
        if let Err(err) = progress::waiting("Asking the Gallery to look for them", || api.rescan_gallery()) {
            errln!("Could not ask the Gallery to look for them: {}. They show after its next scan.", say(&err));
        }
    }
    match get(&counts.failed) {
        0 => tally.result(),
        failed => Err(format!("{failed} failed.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_photos_and_videos_by_name() {
        for name in ["a.jpg", "B.HEIC", "c.MOV", "d.mp4", "e.dng"] {
            assert!(is_media(name), "{name}");
        }
        for name in ["notes.txt", "jpg", "archive.zip"] {
            assert!(!is_media(name), "{name}");
        }
    }
}
