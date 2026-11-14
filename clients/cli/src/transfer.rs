//! Working out what to copy, and copying it over several connections at once.
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::ops::Bound;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, UNIX_EPOCH};

use ownnas_core::api::{Api, ApiError};
use ownnas_core::local;
use ownnas_core::model::{Node, Stamp};

/// Downloads at least this large keep their partial file when cut off, to go on from it later.
const RESUME_MIN: u64 = 8 * 1024 * 1024;
const ATTEMPTS: u32 = 4;

pub type Listing = BTreeMap<String, Node>;

/// What it takes to make the destination match the source.
#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    pub copy: Vec<String>,
    /// Empty folders only: copying a file makes the folders above it.
    pub mkdir: Vec<String>,
    /// Deepest first, so a folder comes after what is in it.
    pub remove: Vec<String>,
    pub unchanged: u64,
}

/// Compares the two listings once. A file with the same size and modification time is the same.
pub fn plan(source: &Listing, dest: &Listing, delete: bool) -> Plan {
    let mut out = Plan::default();
    let mut missing_dirs = Vec::new();
    for (path, node) in source {
        match (node.dir, dest.get(path)) {
            (true, Some(_)) => {}
            (true, None) => missing_dirs.push(path.clone()),
            (false, Some(other)) if other == node => out.unchanged += 1,
            (false, _) => out.copy.push(path.clone()),
        }
    }
    let made: BTreeSet<&str> = out.copy.iter().chain(&missing_dirs).map(String::as_str).collect();
    let mkdir = missing_dirs.iter().filter(|dir| {
        let inside = format!("{dir}/");
        !made.range::<str, _>((Bound::Included(inside.as_str()), Bound::Unbounded)).next().is_some_and(|path| path.starts_with(&inside))
    });
    out.mkdir = mkdir.cloned().collect();
    if delete {
        out.remove = dest.keys().rev().filter(|path| !source.contains_key(*path)).cloned().collect();
    }
    out
}

/// One side of a transfer between a local folder and a server folder.
pub enum Way {
    Up { local: PathBuf, remote: String },
    Down { remote: String, local: PathBuf },
}

pub struct Options {
    pub parallel: usize,
    pub verbose: bool,
}

#[derive(Default)]
pub struct Report {
    pub copied: u64,
    pub bytes: u64,
    pub removed: u64,
    /// The files copied, once they are safely on the disk: empty if that could not be made sure of.
    pub done: Vec<String>,
    pub failed: Vec<(String, String)>,
    pub took: Duration,
}

pub fn join(root: &str, rel: &str) -> String {
    if root.is_empty() {
        rel.to_string()
    } else {
        format!("{root}/{rel}")
    }
}

/// Worth trying again: the connection failed, or the server could not answer for now.
fn temporary(err: &ApiError) -> bool {
    let ApiError::Other(message) = err else { return false };
    let message = message.to_ascii_lowercase();
    ["network error", "connection failed", "dns failed", "timed out", "broken pipe", "reset", "interrupted", "cut short", "http 50", "does not match"]
        .iter()
        .any(|known| message.contains(known))
}

pub fn with_retries<T>(mut work: impl FnMut() -> Result<T, ApiError>) -> Result<T, ApiError> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        match work() {
            Err(err) if attempt < ATTEMPTS && temporary(&err) => std::thread::sleep(Duration::from_secs(1 << attempt)),
            other => return other,
        }
    }
}

pub fn io_failure(err: io::Error) -> ApiError {
    ApiError::Other(format!("Could not use a local file: {err}"))
}

/// Counts what passes through, for the progress line.
struct Counted<'a, R> {
    inner: R,
    moved: &'a AtomicU64,
    this_try: u64,
}

impl<'a, R> Counted<'a, R> {
    fn new(inner: R, moved: &'a AtomicU64) -> Self {
        Counted { inner, moved, this_try: 0 }
    }
}

impl<R: Read> Read for Counted<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let count = self.inner.read(buf)?;
        self.moved.fetch_add(count as u64, Ordering::Relaxed);
        self.this_try += count as u64;
        Ok(count)
    }
}

/// Uploads a file, as long as the server still has `base` there.
pub fn upload_one(api: &Api, file: &Path, remote: &str, stamp: Stamp, base: Option<Stamp>, later: bool, moved: &AtomicU64) -> Result<(), ApiError> {
    with_retries(|| {
        let opened = fs::File::open(file).map_err(io_failure)?;
        let mut body = Counted::new(opened.take(stamp.size), moved);
        let sent = if later {
            api.upload_unflushed(remote, base, stamp.mtime, stamp.size, &mut body)
        } else {
            api.upload(remote, base, stamp.mtime, stamp.size, &mut body)
        };
        let this_try = body.this_try;
        if sent.is_err() {
            moved.fetch_sub(this_try, Ordering::Relaxed);
        }
        match sent? {
            stored if stored.size == stamp.size => Ok(()),
            _ => Err(ApiError::Other("The file changed while it was being sent".into())),
        }
    })
}

fn name_hash(name: &str) -> u64 {
    name.bytes().fold(0xcbf29ce484222325, |hash, byte| (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3))
}

/// Downloads a file beside `dest` and moves it into place once complete. A large one cut off
/// goes on from where it stopped, even in a later run, as long as the file on the server is the
/// same version; the result is then checked against the server's checksum.
/// Returns the byte it went on from, if it did. With `later`, the caller makes sure of the file
/// on the disk afterwards (`local::flush_files`).
pub fn download_one(api: &Api, remote: &str, dest: &Path, stamp: Stamp, later: bool, moved: &AtomicU64) -> Result<Option<u64>, ApiError> {
    let parent = dest.parent().ok_or_else(|| ApiError::Other("Nowhere to save the file".into()))?;
    fs::create_dir_all(parent).map_err(io_failure)?;
    let name = dest.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let family = format!(".ownnas-dl-resume-{:016x}-", name_hash(&name));
    let part_name = format!("{family}{}-{}", stamp.size, stamp.mtime);
    let part = parent.join(&part_name);
    if stamp.size >= RESUME_MIN {
        // Left by an earlier version of the file on the server.
        for item in fs::read_dir(parent).map_err(io_failure)?.flatten() {
            let other = item.file_name().to_string_lossy().into_owned();
            if other.starts_with(&family) && other != part_name {
                let _ = fs::remove_file(item.path());
            }
        }
    }
    let result = with_retries(|| fetch_part(api, remote, &part, stamp, later, moved));
    if result.is_err() && stamp.size < RESUME_MIN {
        let _ = fs::remove_file(&part);
    }
    let resumed = result?;
    fs::rename(&part, dest).map_err(io_failure)?;
    Ok(resumed)
}

fn fetch_part(api: &Api, remote: &str, part: &Path, stamp: Stamp, later: bool, moved: &AtomicU64) -> Result<Option<u64>, ApiError> {
    let have = fs::metadata(part).map_or(0, |meta| meta.len());
    let from = if stamp.size >= RESUME_MIN && have <= stamp.size { have } else { 0 };
    let mut file = fs::OpenOptions::new().create(true).truncate(false).write(true).open(part).map_err(io_failure)?;
    let mut started = from;
    if from < stamp.size || stamp.size == 0 {
        let (reader, current, start) = api.download_from(remote, from)?;
        if current != stamp {
            let _ = fs::remove_file(part);
            return Err(ApiError::Other(format!("{remote} changed on the server after it was listed; run this again")));
        }
        started = start;
        file.set_len(start).map_err(io_failure)?;
        file.seek(SeekFrom::Start(start)).map_err(io_failure)?;
        let mut body = Counted::new(reader, moved);
        let copied = io::copy(&mut body, &mut file);
        let this_try = body.this_try;
        if let Err(err) = copied {
            moved.fetch_sub(this_try, Ordering::Relaxed);
            return Err(ApiError::Other(format!("The download was interrupted: {err}")));
        }
        if start + this_try != stamp.size {
            moved.fetch_sub(this_try, Ordering::Relaxed);
            return Err(ApiError::Other("The download was cut short".into()));
        }
    }
    file.flush().map_err(io_failure)?;
    file.set_modified(UNIX_EPOCH + Duration::from_millis(stamp.mtime.max(0) as u64)).map_err(io_failure)?;
    if !later {
        file.sync_all().map_err(io_failure)?;
    }
    drop(file);
    if started == 0 {
        return Ok(None);
    }
    let expected = api.sha256(remote)?;
    let actual = local::sha256(part).map_err(io_failure)?;
    if !expected.eq_ignore_ascii_case(&actual) {
        let _ = fs::remove_file(part);
        return Err(ApiError::Other("The resumed file does not match the server's checksum".into()));
    }
    Ok(Some(started))
}

/// Of the files that look the same on both sides, those whose contents are not. One that cannot
/// be compared counts as different, which only means copying it again.
pub fn differing(api: &Api, way: &Way, paths: Vec<String>, parallel: usize) -> Vec<String> {
    let (root, remote) = match way {
        Way::Up { local, remote } | Way::Down { remote, local } => (local, remote),
    };
    let queue = Mutex::new(paths.into_iter().collect::<VecDeque<_>>());
    let found = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..parallel.clamp(1, 64) {
            scope.spawn(|| loop {
                let Some(path) = queue.lock().unwrap_or_else(|poison| poison.into_inner()).pop_front() else { break };
                let same = match (local::sha256(&local::full_path(root, &path)), api.sha256(&join(remote, &path))) {
                    (Ok(here), Ok(there)) => here.eq_ignore_ascii_case(&there),
                    _ => false,
                };
                if !same {
                    found.lock().unwrap_or_else(|poison| poison.into_inner()).push(path);
                }
            });
        }
    });
    found.into_inner().unwrap_or_else(|poison| poison.into_inner())
}

fn human_rate(bytes: u64, took: Duration) -> String {
    format!("{}/s", crate::human_size((bytes as f64 / took.as_secs_f64().max(0.001)) as u64))
}

/// Carries out a plan. Nothing is removed if anything failed to copy.
pub fn run(api: &Api, way: &Way, plan: &Plan, source: &Listing, dest: &Listing, options: &Options) -> Report {
    let started = Instant::now();
    let moved = AtomicU64::new(0);
    let finished = AtomicU64::new(0);
    let done = AtomicBool::new(false);
    let failed = Mutex::new(Vec::new());
    let queue = Mutex::new(plan.copy.iter().collect::<VecDeque<_>>());
    let downloaded = Mutex::new(Vec::new());
    let done_paths = Mutex::new(Vec::new());
    let total_bytes: u64 = plan.copy.iter().filter_map(|path| source.get(path)).map(|node| node.stamp.size).sum();
    let fail = |path: &str, err: ApiError| failed.lock().unwrap_or_else(|poison| poison.into_inner()).push((path.to_string(), crate::say(&err)));

    for dir in &plan.mkdir {
        let made = match way {
            Way::Up { remote, .. } => with_retries(|| api.mkdir(&join(remote, dir))),
            Way::Down { local: root, .. } => fs::create_dir_all(local::full_path(root, dir)).map_err(io_failure),
        };
        if let Err(err) = made {
            fail(dir, err);
        }
    }

    std::thread::scope(|scope| {
        if std::io::IsTerminal::is_terminal(&io::stderr()) {
            scope.spawn(|| {
                while !done.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(250));
                    let bytes = moved.load(Ordering::Relaxed);
                    eprint!(
                        "\r\x1b[K{}/{} files, {} of {}, {}",
                        finished.load(Ordering::Relaxed),
                        plan.copy.len(),
                        crate::human_size(bytes),
                        crate::human_size(total_bytes),
                        human_rate(bytes, started.elapsed())
                    );
                    let _ = io::stderr().flush();
                }
                eprint!("\r\x1b[K");
            });
        }
        let workers: Vec<_> = (0..options.parallel.clamp(1, 64))
            .map(|_| {
                scope.spawn(|| loop {
                    let Some(path) = queue.lock().unwrap_or_else(|poison| poison.into_inner()).pop_front() else { break };
                    let stamp = source[path].stamp;
                    let base = dest.get(path).filter(|node| !node.dir).map(|node| node.stamp);
                    let copied = match way {
                        Way::Up { local: root, remote } => {
                            upload_one(api, &local::full_path(root, path), &join(remote, path), stamp, base, true, &moved).map(|()| None)
                        }
                        Way::Down { remote, local: root } => {
                            let target = local::full_path(root, path);
                            let got = download_one(api, &join(remote, path), &target, stamp, true, &moved);
                            if got.is_ok() {
                                downloaded.lock().unwrap_or_else(|poison| poison.into_inner()).push(target);
                            }
                            got
                        }
                    };
                    match copied {
                        Ok(resumed) => {
                            finished.fetch_add(1, Ordering::Relaxed);
                            done_paths.lock().unwrap_or_else(|poison| poison.into_inner()).push(path.clone());
                            if options.verbose {
                                match resumed {
                                    Some(from) => println!("copied {path} (went on from {})", crate::human_size(from)),
                                    None => println!("copied {path}"),
                                }
                            }
                        }
                        Err(err) => fail(path, err),
                    }
                })
            })
            .collect();
        for worker in workers {
            let _ = worker.join();
        }
        done.store(true, Ordering::Relaxed);
    });

    let mut failed = failed.into_inner().unwrap_or_else(|poison| poison.into_inner());
    let mut done_paths = done_paths.into_inner().unwrap_or_else(|poison| poison.into_inner());
    // Only now is it safe to call them copied, and to remove anything because of them.
    if finished.load(Ordering::Relaxed) > 0 {
        let flushed = match way {
            Way::Up { .. } => with_retries(|| api.flush()),
            Way::Down { .. } => local::flush_files(&downloaded.into_inner().unwrap_or_else(|poison| poison.into_inner())).map_err(io_failure),
        };
        if let Err(err) = flushed {
            done_paths.clear();
            failed.push(("(all files)".to_string(), format!("Could not make sure they are safely on the disk: {}", crate::say(&err))));
        }
    }
    let failed_paths: BTreeSet<&String> = failed.iter().map(|(path, _)| path).collect();
    let bytes = plan.copy.iter().filter(|path| !failed_paths.contains(path)).filter_map(|path| source.get(path)).map(|node| node.stamp.size).sum();
    done_paths.sort();
    let mut report = Report { copied: finished.load(Ordering::Relaxed), bytes, done: done_paths, ..Report::default() };

    if failed.is_empty() {
        let session = format!("ownnascli-{}", std::time::SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |time| time.as_secs()));
        for path in &plan.remove {
            let node = dest[path];
            let removed = match way {
                Way::Up { remote, .. } => with_retries(|| api.delete(&join(remote, path), (!node.dir).then_some(node.stamp))),
                Way::Down { local: root, .. } => local::remove(root, path, &session).map_err(io_failure),
            };
            match removed {
                Ok(()) => {
                    report.removed += 1;
                    if options.verbose {
                        println!("removed {path}");
                    }
                }
                Err(err) => failed.push((path.clone(), crate::say(&err))),
            }
        }
    } else if !plan.remove.is_empty() {
        eprintln!("Removed nothing, because some files failed to copy.");
    }
    report.failed = failed;
    report.took = started.elapsed();
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing(items: &[(&str, Node)]) -> Listing {
        items.iter().map(|(path, node)| (path.to_string(), *node)).collect()
    }

    #[test]
    fn copies_only_what_differs() {
        let source = listing(&[("same", Node::file(1, 1)), ("new", Node::file(1, 1)), ("size", Node::file(2, 1)), ("time", Node::file(1, 2))]);
        let dest = listing(&[("same", Node::file(1, 1)), ("size", Node::file(1, 1)), ("time", Node::file(1, 1))]);
        let plan = plan(&source, &dest, false);
        assert_eq!(plan.copy, ["new", "size", "time"]);
        assert_eq!(plan.unchanged, 1);
        assert!(plan.remove.is_empty(), "copying never removes");
    }

    #[test]
    fn makes_only_folders_nothing_lands_in() {
        let source = listing(&[("a", Node::dir()), ("a/f", Node::file(1, 1)), ("b", Node::dir()), ("b/c", Node::dir()), ("d-e", Node::dir()), ("d", Node::dir())]);
        let plan = plan(&source, &Listing::new(), false);
        assert_eq!(plan.mkdir, ["b/c", "d", "d-e"]);
    }

    #[test]
    fn removes_contents_before_their_folder() {
        let dest = listing(&[("x", Node::dir()), ("x/y", Node::file(1, 1)), ("x-z", Node::file(1, 1)), ("keep", Node::file(1, 1))]);
        let source = listing(&[("keep", Node::file(1, 1))]);
        let plan = plan(&source, &dest, true);
        assert_eq!(plan.remove, ["x/y", "x-z", "x"]);
        assert_eq!(plan.unchanged, 1);
    }
}
