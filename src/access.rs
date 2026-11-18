//! Accounts limited to some library folders, each read-write or read-only (Settings → Users).
//!
//! A limited account must not see or reach anything else, so the check is made in one place,
//! before any handler runs (`gate`): every request a limited account may make is listed in
//! `ROUTES` with where its library paths are (query or JSON fields) and what it does with each.
//! A request that is not listed is refused, so one added later stays closed to limited accounts
//! until it is listed here. Handlers that return lists of paths (search, the Gallery, the sync
//! feed, …) keep only what the account may see: the Gallery through `workspaces::View`, which
//! every Gallery query needs, the others through `Access::sees`.
//!
//! Paths are compared after following links, like an agent key's folders (`agent_keys::check`),
//! so a link inside a folder does not reach outside it; and an item in the Trash counts as being
//! where it was deleted from.

use std::path::Path;

use axum::http::{Method, StatusCode};
use serde_json::Value;

use crate::agent_keys::within;
use crate::db::{User, UserFolder};
use crate::files;

/// What a request does with a library path.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Need {
    /// Looks at a folder on the way to the account's folders: the folders above them (shown
    /// with only what leads to them) count too.
    Browse,
    /// Looks at it, or at what is in it.
    Read,
    /// Makes or changes something at or inside it (a folder to make something in counts).
    Write,
    /// Moves, renames, or deletes it, so it must be inside a read-write folder, not one of the
    /// account's folders itself.
    Remove,
}

/// Why a path is refused.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Refusal {
    /// Outside the account's folders (or not there at all): the same answer, so it cannot tell.
    NotFound,
    /// In a folder the account may only look at.
    ReadOnly,
    /// One of the account's folders itself, which it may not move, rename, or delete.
    OwnFolder,
}

impl Refusal {
    pub fn status(self) -> StatusCode {
        match self {
            Refusal::NotFound => StatusCode::NOT_FOUND,
            Refusal::ReadOnly | Refusal::OwnFolder => StatusCode::FORBIDDEN,
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Refusal::NotFound => "Not found",
            Refusal::ReadOnly => "This account can only look at this folder",
            Refusal::OwnFolder => "This account cannot move, rename or delete a folder it was given",
        }
    }
}

/// Where an account may go: everywhere, or its folders.
#[derive(Clone, Debug, Default)]
pub struct Access {
    folders: Option<Vec<UserFolder>>,
}

impl Access {
    /// The whole library (administrators, and accounts that are not limited).
    pub fn whole() -> Self {
        Access { folders: None }
    }

    pub fn of(user: &User) -> Self {
        if user.limited && !user.is_admin {
            Access { folders: Some(user.folders.clone()) }
        } else {
            Access::whole()
        }
    }

    #[cfg(test)]
    pub fn limited_to(folders: &[(&str, bool)]) -> Self {
        Access { folders: Some(folders.iter().map(|(path, read_only)| UserFolder { path: path.to_string(), read_only: *read_only }).collect()) }
    }

    pub fn limited(&self) -> bool {
        self.folders.is_some()
    }

    /// Its folders as library paths, for filtering lists (`None` for the whole library).
    pub fn folder_paths(&self) -> Option<Vec<String>> {
        self.folders.as_ref().map(|folders| folders.iter().map(|folder| folder.path.clone()).collect())
    }

    /// Whether a library path, as an index holds it, is inside its folders (by name only:
    /// indexes hold where things really are).
    pub fn holds(&self, path: &str) -> bool {
        match &self.folders {
            None => true,
            Some(folders) => folders.iter().any(|folder| within(&folder.path, path)),
        }
    }

    /// Whether a path in a list (a folder's entries, search hits, the sync tree and feed) may be
    /// shown: inside its folders once links are followed, on the way to them, or in the Trash
    /// from inside them. (Walking a folder follows links, so a name alone is not enough.)
    pub fn sees(&self, root: &Path, path: &str) -> bool {
        !self.limited() || self.check(root, path, Need::Browse).is_ok()
    }

    /// Whether the account may do `need` with `rel` (a library path as a request gives it).
    pub fn check(&self, root: &Path, rel: &str, need: Need) -> Result<(), Refusal> {
        let Some(folders) = &self.folders else { return Ok(()) };
        let at = located(root, rel).ok_or(Refusal::NotFound)?;
        let at = if at == files::TRASH_DIR {
            // The Trash itself: looked into, it shows only what came from the account's folders;
            // something moved into it is deleted (where it comes from is checked apart).
            return if need == Need::Browse || (need == Need::Write && folders.iter().any(|folder| !folder.read_only)) { Ok(()) } else { Err(Refusal::NotFound) };
        } else {
            match from_trash(root, &at) {
                Some(Some(original)) => original,
                Some(None) => return Err(Refusal::NotFound),
                None => at,
            }
        };
        if need == Need::Remove && folders.iter().any(|folder| folder.path == at) && !folders.iter().any(|folder| folder.path != at && within(&folder.path, &at) && !folder.read_only) {
            return Err(Refusal::OwnFolder);
        }
        // The closest of its folders decides: a read-only folder inside a read-write one stays
        // read-only, and the other way round.
        let closest = folders
            .iter()
            .filter(|folder| within(&folder.path, &at) && !(need == Need::Remove && folder.path == at))
            .max_by_key(|folder| folder.path.len());
        match closest {
            Some(folder) if folder.read_only && matches!(need, Need::Write | Need::Remove) => Err(Refusal::ReadOnly),
            Some(_) => Ok(()),
            None if need == Need::Browse && folders.iter().any(|folder| within(&at, &folder.path)) => Ok(()),
            None => Err(Refusal::NotFound),
        }
    }

}

/// SQL that is true when `column` (a library path) is one of `folders` or inside one.
pub fn folders_sql(folders: &[String], column: &str) -> String {
    if folders.is_empty() {
        return "0".to_string();
    }
    let each: Vec<String> = folders
        .iter()
        .map(|folder| {
            if folder.is_empty() {
                return "1".to_string();
            }
            let quoted = folder.replace('\'', "''");
            format!("({column} = '{quoted}' OR substr({column}, 1, length('{quoted}') + 1) = '{quoted}/')")
        })
        .collect();
    format!("({})", each.join(" OR "))
}

/// Where a path asked about really is, as a library path, once links are followed. A path that
/// does not exist yet (something about to be made) is where its closest existing folder is,
/// plus the rest. `None` for a path that is not valid or leads out of the library.
fn located(root: &Path, rel: &str) -> Option<String> {
    let parts = files::clean_parts(rel).ok()?;
    let root = root.canonicalize().ok()?;
    for known in (0..=parts.len()).rev() {
        let prefix = parts[..known].join("/");
        let lexical = root.join(&prefix);
        if std::fs::symlink_metadata(&lexical).is_err() {
            continue;
        }
        // There, but a link to nowhere: where it leads cannot be told, so it is refused.
        let found = files::resolve(&root, &prefix).ok()?;
        let inside = found.full.strip_prefix(&root).ok()?;
        let mut real: Vec<String> = inside.components().map(|part| part.as_os_str().to_string_lossy().into_owned()).collect();
        real.extend(parts[known..].iter().cloned());
        return Some(real.join("/"));
    }
    None
}

/// For a path in the Trash (`.ownnas-trash/<item>/…`), where it was before it was deleted:
/// `Some(None)` when that is not known, `None` for a path not in the Trash.
fn from_trash(root: &Path, path: &str) -> Option<Option<String>> {
    let rest = path.strip_prefix(files::TRASH_DIR)?.strip_prefix('/')?;
    let (item, inside) = rest.split_once('/').map_or((rest, None), |(item, inside)| (item, Some(inside)));
    Some(files::trash_original(root, item).map(|original| match inside {
        Some(inside) => format!("{original}/{inside}"),
        None => original,
    }))
}

/// Where a request's library paths are: `q:` a query parameter, `b:` a JSON body field;
/// `name[]` is each item of a list, and `.` goes into an object (`groups[].keep[]`).
type Fields = &'static [(&'static str, Need)];

/// Every request a limited account may make, by method and path, with its library paths.
/// Requests listed without paths either touch no library path, or the handler keeps only what
/// the account may see (each such handler takes the account's `Access`), as the notes say.
/// Anything else is refused: Settings, terminals, agent keys, remotes, drives, storage, backups
/// of the NAS and from phones, and changes to things every account shares (people, albums,
/// workspaces, the notes folder).
pub const ROUTES: &[(&str, &str, Fields)] = {
    use Need::*;
    &[
        // Signing in, and the account itself.
        ("GET", "/api/health", &[]),
        ("POST", "/api/login", &[]),
        ("POST", "/api/logout", &[]),
        ("GET", "/api/me", &[]),
        ("POST", "/api/password", &[]),
        ("GET", "/api/two-factor", &[]),
        ("POST", "/api/two-factor/setup", &[]),
        ("POST", "/api/two-factor/enable", &[]),
        ("POST", "/api/two-factor/disable", &[]),
        ("POST", "/api/two-factor/recovery", &[]),
        // What the server can do, and how far indexing is (no paths).
        ("GET", "/api/helpers", &[]),
        ("GET", "/api/index", &[]),
        ("GET", "/api/ml", &[]),
        ("GET", "/api/gallery/status", &[]),
        ("GET", "/api/gallery/settings", &[]),
        ("GET", "/api/map/tile/*", &[]),
        // Looking at files. The listing keeps what leads to the account's folders.
        ("GET", "/api/list", &[("q:path", Browse)]),
        ("GET", "/api/meta", &[("q:path", Read)]),
        ("GET", "/api/folder-appearance", &[("q:path", Browse)]),
        ("GET", "/api/raw", &[("q:path", Read)]),
        ("GET", "/api/docx", &[("q:path", Read)]),
        ("GET", "/api/odt", &[("q:path", Read)]),
        ("GET", "/api/xlsx", &[("q:path", Read)]),
        ("GET", "/api/sqlite", &[("q:path", Read)]),
        ("GET", "/api/sqlite/rows", &[("q:path", Read)]),
        ("GET", "/api/sqlite/value", &[("q:path", Read)]),
        ("POST", "/api/sqlite/query", &[("b:path", Read)]),
        ("GET", "/api/thumb", &[("q:path", Read)]),
        ("GET", "/api/display", &[("q:path", Read)]),
        ("GET", "/api/photo/motion", &[("q:path", Read)]),
        ("GET", "/api/photo/motion/video", &[("q:path", Read)]),
        ("GET", "/api/photo/motion/clip", &[("q:path", Read)]),
        ("POST", "/api/photo/motion/known", &[("b:paths[]", Read)]),
        ("GET", "/api/audio/info", &[("q:path", Read)]),
        ("GET", "/api/audio/cover", &[("q:path", Read)]),
        ("GET", "/api/music/folder", &[("q:path", Read)]),
        ("GET", "/api/video/probe", &[("q:path", Read)]),
        ("GET", "/api/video/stream", &[("q:path", Read)]),
        ("GET", "/api/subtitles", &[("q:path", Read)]),
        ("GET", "/api/document", &[("q:path", Read)]),
        ("GET", "/api/document/picture", &[("q:path", Read)]),
        ("GET", "/api/document/pdf", &[("q:path", Read)]),
        ("GET", "/api/djvu/info", &[("q:path", Read)]),
        ("GET", "/api/djvu/page", &[("q:path", Read)]),
        ("GET", "/api/archive/entry", &[("q:path", Read)]),
        ("GET", "/api/archive/list", &[("q:path", Read)]),
        ("GET", "/api/book/info", &[("q:path", Read)]),
        // `/api/book/<book>/<entry>`: the book is checked as a path (see `gate`).
        ("GET", "/api/book/*", &[]),
        ("GET", "/api/mail", &[("q:path", Read)]),
        ("GET", "/api/mbox", &[("q:path", Read)]),
        ("GET", "/api/mail/html", &[("q:path", Read)]),
        ("GET", "/api/mail/attachment", &[("q:path", Read)]),
        ("GET", "/api/font/info", &[("q:path", Read)]),
        ("GET", "/api/plist", &[("q:path", Read)]),
        ("GET", "/api/pdf/info", &[("q:path", Read)]),
        ("GET", "/api/usage", &[("q:path", Read)]),
        ("GET", "/api/hash", &[("q:path", Read)]),
        ("GET", "/api/annotations", &[("q:path", Read)]),
        ("POST", "/api/annotations", &[("b:path", Read)]),
        ("GET", "/api/gallery/details", &[("q:path", Read)]),
        ("GET", "/api/faces/in", &[("q:path", Read)]),
        // Downloads and ZIPs.
        ("GET", "/api/zip", &[("q:path", Read)]),
        ("POST", "/api/download", &[("b:paths[]", Read)]),
        // Making and changing files.
        ("POST", "/api/mkdir", &[("b:path", Write)]),
        ("POST", "/api/create", &[("b:path", Write)]),
        ("POST", "/api/write", &[("b:path", Write)]),
        ("POST", "/api/write-docx", &[("b:path", Write)]),
        ("POST", "/api/write-odt", &[("b:path", Write)]),
        ("POST", "/api/write-xlsx", &[("b:path", Write)]),
        ("POST", "/api/write-image", &[("b:path", Write)]),
        // These save their result next to the file, so its folder must be read-write.
        ("POST", "/api/convert-image", &[("b:path", Remove)]),
        ("POST", "/api/pdf/extract", &[("b:path", Remove)]),
        ("POST", "/api/pdf/split", &[("b:path", Remove)]),
        ("POST", "/api/pdf/merge", &[("b:paths[]", Remove)]),
        ("POST", "/api/pdf/rotate", &[("b:path", Write)]),
        ("POST", "/api/compress", &[("b:paths[]", Remove)]),
        ("POST", "/api/extract", &[("b:path", Remove)]),
        ("POST", "/api/duplicate", &[("b:path", Remove)]),
        ("POST", "/api/images/resize", &[("b:paths[]", Remove)]),
        ("POST", "/api/videos/convert", &[("b:path", Remove)]),
        ("GET", "/api/videos/job", &[]),
        ("DELETE", "/api/videos/job", &[]),
        ("POST", "/api/rename", &[("b:path", Remove)]),
        ("POST", "/api/move", &[("b:path", Remove), ("b:dest", Write)]),
        ("POST", "/api/undo-move", &[("b:path", Remove), ("b:dest", Write)]),
        ("POST", "/api/copy", &[("b:path", Read), ("b:dest", Write)]),
        ("POST", "/api/folder-appearance", &[("b:paths[]", Write)]),
        ("POST", "/api/protect", &[("b:paths[]", Write)]),
        ("POST", "/api/tags", &[("b:path", Write)]),
        ("DELETE", "/api/tags", &[("q:path", Write)]),
        ("POST", "/api/tags/bulk", &[("b:paths[]", Write)]),
        ("POST", "/api/comments", &[("b:path", Write)]),
        // The handler checks the comment's file.
        ("DELETE", "/api/comments", &[]),
        ("POST", "/api/sort-folder", &[("b:path", Write)]),
        ("POST", "/api/sort-folder/undo", &[("b:moves[].from", Remove), ("b:moves[].to", Write)]),
        ("POST", "/api/rename-many", &[("b:paths[]", Remove)]),
        ("POST", "/api/cleanup/dates", &[("b:paths[]", Write)]),
        ("POST", "/api/index/reindex", &[("b:path", Read)]),
        // Uploads (the handlers check each name, which may go into folders). The copies already
        // in the library they report are only the account's.
        ("POST", "/api/upload", &[("q:path", Write)]),
        ("POST", "/api/upload/conflicts", &[("b:path", Read)]),
        ("POST", "/api/upload/directories", &[("b:path", Write)]),
        ("POST", "/api/upload/same-size", &[]),
        ("POST", "/api/upload/duplicates", &[]),
        ("POST", "/api/gallery/upload/check", &[("b:folder", Read)]),
        // The Trash: an item counts as being where it was deleted from; the summary and
        // emptying are of the account's items only.
        ("DELETE", "/api/entry", &[("q:path", Remove)]),
        ("POST", "/api/restore", &[("b:path", Remove)]),
        ("GET", "/api/trash/summary", &[]),
        ("POST", "/api/trash/empty", &[]),
        // Bookmarks and recent files are the account's own, kept to its folders when listed.
        ("GET", "/api/bookmarks", &[]),
        ("POST", "/api/bookmarks", &[("b:path", Read)]),
        ("DELETE", "/api/bookmarks", &[]),
        ("GET", "/api/recent", &[]),
        ("POST", "/api/recent", &[("b:path", Read)]),
        // Searching, duplicates and tidying up: from a folder, with only the account's hits.
        ("GET", "/api/search", &[("q:path", Browse)]),
        ("GET", "/api/duplicates", &[("q:path", Read)]),
        ("GET", "/api/near-duplicates", &[("q:path", Read)]),
        ("POST", "/api/duplicates/trash", &[("b:groups[].keep[]", Read), ("b:groups[].remove[]", Remove)]),
        ("GET", "/api/cleanup/large", &[("q:path", Read)]),
        ("GET", "/api/cleanup/empty", &[("q:path", Read)]),
        ("GET", "/api/cleanup/dates", &[("q:path", Read)]),
        ("GET", "/api/cleanup/compare", &[("q:first", Read), ("q:second", Read)]),
        // Versions: listed for a file; by id, the handler checks the version's file.
        ("GET", "/api/versions", &[("q:path", Read)]),
        ("GET", "/api/versions/file", &[]),
        ("GET", "/api/versions/compare", &[]),
        ("POST", "/api/versions/restore", &[]),
        ("POST", "/api/versions/delete", &[]),
        // The activity log, of the account's folders.
        ("GET", "/api/activity", &[]),
        // The Gallery: every query goes through `workspaces::View`, which holds the folders.
        ("GET", "/api/gallery/timeline", &[]),
        ("GET", "/api/gallery/days", &[]),
        ("GET", "/api/gallery/places", &[]),
        ("GET", "/api/gallery/places/items", &[]),
        ("GET", "/api/gallery/memories", &[]),
        ("GET", "/api/gallery/search", &[]),
        ("GET", "/api/gallery/similar", &[("q:path", Read)]),
        ("POST", "/api/gallery/similar", &[("q:path", Read)]),
        ("GET", "/api/gallery/hidden", &[]),
        ("POST", "/api/gallery/hidden", &[("b:paths[]", Write)]),
        ("GET", "/api/gallery/tag", &[]),
        ("GET", "/api/tags/all", &[]),
        ("GET", "/api/people", &[]),
        ("GET", "/api/people/photos", &[]),
        // The handler checks the face's photo.
        ("GET", "/api/faces/crop", &[]),
        ("GET", "/api/albums", &[]),
        ("GET", "/api/albums/tree", &[]),
        ("GET", "/api/workspaces", &[]),
        ("POST", "/api/workspaces/membership", &[("b:paths[]", Read)]),
        ("POST", "/api/workspaces/folder-membership", &[("b:folders[]", Read)]),
        // Notes: in the notes folder, if the account may use it (the handlers check).
        ("GET", "/api/notes", &[]),
        ("POST", "/api/notes", &[]),
        ("POST", "/api/notes/daily", &[]),
        ("POST", "/api/notes/tick", &[("b:path", Write)]),
        ("POST", "/api/notes/meta", &[("b:path", Write)]),
        ("POST", "/api/notes/archive", &[("b:path", Remove)]),
        // Sync clients: the tree and the change feed keep what leads to the account's folders.
        ("GET", "/api/sync/tree", &[("q:path", Browse)]),
        ("GET", "/api/sync/file", &[("q:path", Read)]),
        ("PUT", "/api/sync/file", &[("q:path", Write)]),
        ("POST", "/api/sync/flush", &[]),
        ("POST", "/api/sync/mkdir", &[("b:path", Write)]),
        ("POST", "/api/sync/delete", &[("b:path", Remove)]),
        ("GET", "/api/sync/changes", &[("q:path", Browse)]),
    ]
};

/// The fields of a request a limited account may make, if it may make it.
pub fn route(method: &Method, path: &str) -> Option<Fields> {
    let method = if method == Method::HEAD { "GET" } else { method.as_str() };
    ROUTES.iter().find_map(|(allowed_method, pattern, fields)| {
        let matches = match pattern.strip_suffix('*') {
            Some(prefix) => path.starts_with(prefix),
            None => path == *pattern,
        };
        (matches && *allowed_method == method).then_some(*fields)
    })
}

/// The values at a field path (`paths[]`, `groups[].keep[]`) in a JSON body; `Err` when a
/// value there is not text (the request is then refused rather than left unchecked). A single
/// path left out counts as "" (the top of the library), as the handlers take it.
pub fn body_values(body: &Value, field: &str) -> Result<Vec<String>, ()> {
    let mut at: Vec<&Value> = vec![body];
    let steps: Vec<&str> = field.split('.').collect();
    let mut missing = 0;
    for step in &steps {
        let (name, each) = step.strip_suffix("[]").map_or((*step, false), |name| (name, true));
        let mut next = Vec::new();
        for value in at {
            match value.get(name) {
                None | Some(Value::Null) if each => {}
                None | Some(Value::Null) => missing += 1,
                Some(Value::Array(items)) if each => next.extend(items.iter()),
                Some(_) if each => return Err(()),
                Some(found) => next.push(found),
            }
        }
        at = next;
    }
    let mut values = at.into_iter().map(|value| value.as_str().map(str::to_string).ok_or(())).collect::<Result<Vec<_>, _>>()?;
    values.extend(std::iter::repeat_n(String::new(), missing));
    Ok(values)
}

/// `%`-decoding of one part of a URL path.
pub fn decode_segment(segment: &str) -> Option<String> {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = std::str::from_utf8(bytes.get(index + 1..index + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library() -> std::path::PathBuf {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!("ownnas-access-{}-{}", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["Family/Photos/2024", "Family/Private", "Work/Archive", "Workshop", "Other"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::write(root.join("Other/secret.txt"), "x").unwrap();
        root.canonicalize().unwrap()
    }

    #[test]
    fn folders_and_what_leads_to_them() {
        let root = library();
        let access = Access::limited_to(&[("Family/Photos", false), ("Work", false), ("Work/Archive", true)]);
        let ok = |rel: &str, need| access.check(&root, rel, need);
        assert_eq!(ok("Family/Photos/2024", Need::Write), Ok(()));
        assert_eq!(ok("Family/Photos/new/deeper.txt", Need::Write), Ok(()), "not made yet");
        assert_eq!(ok("Family", Need::Browse), Ok(()));
        assert_eq!(ok("", Need::Browse), Ok(()));
        assert_eq!(ok("Family", Need::Read), Err(Refusal::NotFound));
        assert_eq!(ok("Family/Private", Need::Browse), Err(Refusal::NotFound));
        assert_eq!(ok("Workshop", Need::Read), Err(Refusal::NotFound), "not Work");
        assert_eq!(ok("Other/secret.txt", Need::Read), Err(Refusal::NotFound));
        assert_eq!(ok("Family/Photos/../Private", Need::Read), Err(Refusal::NotFound));
        assert_eq!(ok("../etc", Need::Read), Err(Refusal::NotFound));
        // The closest folder decides.
        assert_eq!(ok("Work/Archive/old.txt", Need::Write), Err(Refusal::ReadOnly));
        assert_eq!(ok("Work/Archive/old.txt", Need::Read), Ok(()));
        // Its own folders stay where they are; Work/Archive is inside Work, which may change it.
        assert_eq!(ok("Family/Photos", Need::Remove), Err(Refusal::OwnFolder));
        assert_eq!(ok("Work/Archive", Need::Remove), Ok(()));
        assert_eq!(ok("Family/Photos/2024", Need::Remove), Ok(()));
        // Read-only everywhere: nothing changes.
        let reader = Access::limited_to(&[("Family", true)]);
        assert_eq!(reader.check(&root, "Family/Photos", Need::Write), Err(Refusal::ReadOnly));
        assert_eq!(reader.check(&root, "Family/Photos", Need::Read), Ok(()));
        // No folders left: nothing at all.
        let none = Access::limited_to(&[]);
        assert_eq!(none.check(&root, "", Need::Browse), Err(Refusal::NotFound));
        assert!(Access::whole().check(&root, "Other/secret.txt", Need::Remove).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn links_count_where_they_lead() {
        let root = library();
        std::os::unix::fs::symlink(root.join("Other"), root.join("Work/other")).unwrap();
        std::os::unix::fs::symlink(root.join("Nowhere"), root.join("Work/dangling")).unwrap();
        let access = Access::limited_to(&[("Work", false)]);
        assert_eq!(access.check(&root, "Work/other/secret.txt", Need::Read), Err(Refusal::NotFound));
        assert_eq!(access.check(&root, "Work/other/new.txt", Need::Write), Err(Refusal::NotFound));
        assert_eq!(access.check(&root, "Work/dangling/new.txt", Need::Write), Err(Refusal::NotFound));
        // And a link into its folder from outside is followed there.
        std::os::unix::fs::symlink(root.join("Work"), root.join("Other/to-work")).unwrap();
        assert_eq!(access.check(&root, "Other/to-work/Archive", Need::Read), Ok(()));
    }

    #[test]
    fn trash_items_count_where_they_were() {
        let root = library();
        std::fs::write(root.join("Family/Photos/a.jpg"), "a").unwrap();
        std::fs::write(root.join("Other/b.jpg"), "b").unwrap();
        let a = files::trash_entry(&root, "Family/Photos/a.jpg").unwrap();
        let b = files::trash_entry(&root, "Other/b.jpg").unwrap();
        let access = Access::limited_to(&[("Family/Photos", false)]);
        assert_eq!(access.check(&root, &a, Need::Remove), Ok(()));
        assert_eq!(access.check(&root, &b, Need::Read), Err(Refusal::NotFound));
        assert_eq!(access.check(&root, files::TRASH_DIR, Need::Browse), Ok(()));
        assert!(access.sees(&root, &a) && !access.sees(&root, &b));
        assert!(access.sees(&root, "Family") && !access.sees(&root, "Other"));
        let reader = Access::limited_to(&[("Family/Photos", true)]);
        assert_eq!(reader.check(&root, &a, Need::Remove), Err(Refusal::ReadOnly));
    }

    #[test]
    fn sql_and_fields() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let access = Access::limited_to(&[("Bob's", false), ("A/B", true)]);
        for (path, inside) in [("Bob's/x", true), ("Bob's", true), ("Bob'sx", false), ("A/B/c", true), ("A/Bc", false), ("A", false)] {
            let found: bool = conn.query_row(&format!("SELECT {} FROM (SELECT ?1 AS p)", folders_sql(&access.folder_paths().unwrap(), "p")), [path], |row| row.get(0)).unwrap();
            assert_eq!(found, inside, "{path}");
        }
        assert_eq!(folders_sql(&[], "p"), "0");
        let body = serde_json::json!({ "groups": [{ "keep": ["a"], "remove": ["b", "c"] }, { "keep": ["d"] }], "path": "e", "n": 1 });
        assert_eq!(body_values(&body, "groups[].remove[]").unwrap(), vec!["b", "c"]);
        assert_eq!(body_values(&body, "groups[].keep[]").unwrap(), vec!["a", "d"]);
        assert_eq!(body_values(&body, "path").unwrap(), vec!["e"]);
        assert_eq!(body_values(&body, "missing").unwrap(), vec![""], "a path left out is the top");
        assert!(body_values(&body, "list[]").unwrap().is_empty());
        assert!(body_values(&body, "n").is_err());
        assert_eq!(decode_segment("Family%2FPhotos%20x.epub").as_deref(), Some("Family/Photos x.epub"));
    }
}
