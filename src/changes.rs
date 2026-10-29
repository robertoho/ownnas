//! The change feed sync clients follow (the Finder and Files locations, the Windows sync app):
//! the paths in the shared folder that changed, in order, so a client asks "what changed since
//! my cursor?" instead of listing everything again.
//!
//! A file system watcher on the shared folder feeds it, so changes made through OwnNAS and made
//! on the server's disk directly both show up. The journal is kept in memory. A cursor is
//! `epoch-seq`. When the server restarts, the watcher misses events, or a client is too far
//! behind, the epoch is new and the client is told its cursor expired: it lists again.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How many changes are kept; a client further behind lists again.
const KEEP: usize = 200_000;

#[derive(Default)]
struct Journal {
    epoch: u64,
    /// The sequence number of the next change.
    next: u64,
    /// Changed paths with their sequence numbers, oldest first.
    events: VecDeque<(u64, String)>,
}

pub struct Changes {
    journal: Mutex<Journal>,
    wake: tokio::sync::Notify,
    /// False when the folder could not be watched (clients then list folders when they look).
    watching: std::sync::atomic::AtomicBool,
    /// Kept alive for as long as the feed is.
    watcher: Mutex<Option<notify::RecommendedWatcher>>,
}

/// What a client asking for changes gets.
#[derive(Debug, PartialEq, Eq)]
pub enum Since {
    /// The cursor is from another epoch, or too old: list everything again, from `cursor` on.
    Expired { cursor: String },
    /// The paths changed after the cursor (each once, in order), and the cursor to ask from next.
    Changed { cursor: String, paths: Vec<String> },
}

impl Changes {
    /// A feed with nothing watched (for tests, and until `watch`).
    pub fn new() -> Arc<Self> {
        // A new epoch on every start: cursors handed out before do not count.
        let epoch = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(1);
        Arc::new(Self {
            journal: Mutex::new(Journal { epoch, next: 1, events: VecDeque::new() }),
            wake: tokio::sync::Notify::new(),
            watching: std::sync::atomic::AtomicBool::new(false),
            watcher: Mutex::new(None),
        })
    }

    /// Starts watching `root`; changes under it (as paths relative to it) go into the feed.
    pub fn watch(self: &Arc<Self>, root: &Path) {
        use notify::Watcher;
        let feed = Arc::downgrade(self);
        // Watchers may report paths spelled another way (macOS's /private/var for /var).
        let bases = [Some(root.to_path_buf()), std::fs::canonicalize(root).ok()];
        let handler = move |result: notify::Result<notify::Event>| {
            let Some(feed) = feed.upgrade() else { return };
            match result {
                Ok(event) if event.need_rescan() => feed.expire(),
                Ok(event) => {
                    for path in &event.paths {
                        if let Some(rel) = bases.iter().flatten().find_map(|base| relative(base, path)) {
                            feed.record(rel);
                        }
                    }
                }
                // Events were lost: what clients know may be wrong.
                Err(_) => feed.expire(),
            }
        };
        match notify::recommended_watcher(handler).and_then(|mut watcher| watcher.watch(root, notify::RecursiveMode::Recursive).map(|_| watcher)) {
            Ok(watcher) => {
                *self.watcher.lock().unwrap_or_else(|err| err.into_inner()) = Some(watcher);
                self.watching.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            Err(err) => {
                crate::db_watch::log(&format!("sync clients will not hear of changes at once: the shared folder could not be watched ({err})"));
            }
        }
    }

    pub fn watching(&self) -> bool {
        self.watching.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn journal(&self) -> std::sync::MutexGuard<'_, Journal> {
        self.journal.lock().unwrap_or_else(|err| err.into_inner())
    }

    /// A path (relative to the shared folder) changed.
    pub fn record(&self, rel: String) {
        {
            let mut journal = self.journal();
            // The same path again right away (a write in several parts) is one change.
            if journal.events.back().is_some_and(|(_, last)| *last == rel) {
                return;
            }
            let seq = journal.next;
            journal.next += 1;
            journal.events.push_back((seq, rel));
            if journal.events.len() > KEEP {
                journal.events.pop_front();
            }
        }
        self.wake.notify_waiters();
    }

    /// Changes were missed: every client lists again.
    pub fn expire(&self) {
        {
            let mut journal = self.journal();
            journal.epoch += 1;
            journal.events.clear();
        }
        self.wake.notify_waiters();
    }

    /// What changed after `cursor` (none: start from now).
    pub fn since(&self, cursor: Option<&str>) -> Since {
        let journal = self.journal();
        let now = format!("{}-{}", journal.epoch, journal.next - 1);
        let Some(cursor) = cursor.filter(|cursor| !cursor.is_empty()) else {
            return Since::Changed { cursor: now, paths: Vec::new() };
        };
        let parsed = cursor.split_once('-').and_then(|(epoch, seq)| Some((epoch.parse::<u64>().ok()?, seq.parse::<u64>().ok()?)));
        let Some((epoch, seq)) = parsed else { return Since::Expired { cursor: now } };
        let oldest = journal.events.front().map_or(journal.next, |(first, _)| *first);
        // From another epoch, from the future, or older than what is kept.
        if epoch != journal.epoch || seq >= journal.next || seq + 1 < oldest {
            return Since::Expired { cursor: now };
        }
        let mut paths: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        // Newest first, so a path changed twice counts once, at its last change.
        for (number, path) in journal.events.iter().rev() {
            if *number <= seq {
                break;
            }
            if seen.insert(path.clone()) {
                paths.push(path.clone());
            }
        }
        paths.reverse();
        Since::Changed { cursor: now, paths }
    }

    /// Like `since`, but waits up to `wait` for a change when there is none yet.
    pub async fn wait(&self, cursor: Option<&str>, wait: Duration) -> Since {
        let notified = self.wake.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let first = self.since(cursor);
        if !matches!(&first, Since::Changed { paths, .. } if paths.is_empty()) || cursor.is_none_or(str::is_empty) || wait.is_zero() {
            return first;
        }
        let _ = tokio::time::timeout(wait, notified).await;
        // Let a burst of changes (a folder copied) arrive together.
        tokio::time::sleep(Duration::from_millis(150)).await;
        self.since(cursor)
    }
}

/// `path` relative to `root` with `/` between parts, or None when it is not under it.
fn relative(root: &Path, path: &Path) -> Option<String> {
    let rest = path.strip_prefix(root).ok()?;
    let parts: Vec<String> = rest.components().map(|part| part.as_os_str().to_string_lossy().into_owned()).collect();
    (!parts.is_empty()).then(|| parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursors_give_what_changed_after_them() {
        let feed = Changes::new();
        let Since::Changed { cursor: start, paths } = feed.since(None) else { panic!("a fresh start") };
        assert!(paths.is_empty());
        feed.record("a.txt".into());
        feed.record("a.txt".into());
        feed.record("Photos/b.jpg".into());
        feed.record("a.txt".into());
        let Since::Changed { cursor, paths } = feed.since(Some(&start)) else { panic!("changes") };
        assert_eq!(paths, ["Photos/b.jpg", "a.txt"], "each path once, at its last change");
        assert_eq!(feed.since(Some(&cursor)), Since::Changed { cursor: cursor.clone(), paths: vec![] });
        assert!(matches!(feed.since(Some("1-0")), Since::Expired { .. }), "another epoch");
        assert!(matches!(feed.since(Some("nonsense")), Since::Expired { .. }));
        feed.expire();
        assert!(matches!(feed.since(Some(&cursor)), Since::Expired { .. }), "missed changes expire every cursor");
    }

    #[tokio::test]
    async fn the_watcher_feeds_changes_on_disk() {
        let root = std::env::temp_dir().join(format!("ownnas-changes-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(root.join("Docs")).unwrap();
        let feed = Changes::new();
        feed.watch(&root);
        assert!(feed.watching());
        let Since::Changed { cursor, .. } = feed.since(None) else { panic!() };
        // Some watchers start a moment after they are set up.
        tokio::time::sleep(Duration::from_millis(300)).await;
        std::fs::write(root.join("Docs/note.txt"), b"hi").unwrap();
        let mut found = Vec::new();
        for _ in 0..40 {
            if let Since::Changed { paths, .. } = feed.wait(Some(&cursor), Duration::from_millis(500)).await {
                found = paths;
            }
            if found.iter().any(|path| path == "Docs/note.txt") {
                break;
            }
        }
        assert!(found.iter().any(|path| path == "Docs/note.txt"), "{found:?}");
        std::fs::remove_dir_all(&root).ok();
    }
}
