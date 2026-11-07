//! Gallery workspaces ("Family", "Work", "Customer 1"): each is a name, some library folders,
//! and photos and videos picked into it one by one from anywhere (a phone's camera roll can
//! feed both "Work" and "Private" without moving a file). A workspace shows the photos and videos
//! in its folders and the ones picked into it, with its own albums. Nothing in the library
//! changes. The filter is applied by the server, so access to a workspace can later be
//! granted to some users only.

use std::collections::HashSet;

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::albums::AlbumError;
use crate::db::now_secs;

const MAX_NAME: usize = 60;
const MAX_FOLDERS: usize = 200;
/// Photos picked into (or out of) workspaces in one go.
pub const MAX_ITEMS_PER_CHANGE: usize = 10_000;

/// Not a workspace: the Gallery's view of the photos in no workspace yet (outside every
/// workspace's folders, and picked into none), to sort them. Real ids start at 1.
pub const UNSORTED: i64 = 0;

type Result<T> = std::result::Result<T, AlbumError>;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub id: i64,
    pub name: String,
    pub folders: Vec<String>,
    /// Photos and videos in it (outside the Trash): in its folders, or picked into it.
    pub photos: i64,
    /// Of those, the ones picked into it one by one (outside its folders).
    pub picked: i64,
}

/// SQL that is true when `column` is in some workspace's folders, or one of them.
fn in_folders_sql(column: &str, workspace: Option<i64>) -> String {
    let which = workspace.map(|id| format!("wf.workspace_id = {id} AND ")).unwrap_or_default();
    format!(
        "EXISTS(SELECT 1 FROM workspace_folders wf WHERE {which}({column} = wf.folder OR substr({column}, 1, length(wf.folder) + 1) = wf.folder || '/'))"
    )
}

fn picked_sql(column: &str, workspace: Option<i64>) -> String {
    let which = workspace.map(|id| format!("wi.workspace_id = {id} AND ")).unwrap_or_default();
    format!("EXISTS(SELECT 1 FROM workspace_items wi WHERE {which}wi.path = {column})")
}

/// SQL that is true when `column` (a library path) is in the workspace: in one of its folders
/// (or one of them), or picked into it. Always true for `None` (every photo); for `UNSORTED`,
/// true when it is in no workspace at all. The id is a number, so it is safe to write into the
/// query.
pub fn filter(workspace: Option<i64>, column: &str) -> String {
    match workspace {
        None => "1".to_string(),
        Some(UNSORTED) => format!("(NOT {} AND NOT {})", in_folders_sql(column, None), picked_sql(column, None)),
        Some(id) => format!("({} OR {})", in_folders_sql(column, Some(id)), picked_sql(column, Some(id))),
    }
}

/// Who is in a workspace, for filtering in memory (smart search goes through every photo's
/// description rather than the database).
pub enum Members {
    Every,
    Inside { folders: Vec<String>, picked: HashSet<String> },
    /// The photos in no workspace (`UNSORTED`): outside all of these.
    Outside { folders: Vec<String>, picked: HashSet<String> },
}

impl Members {
    pub fn has(&self, path: &str) -> bool {
        match self {
            Members::Every => true,
            Members::Inside { folders, picked } => contains(folders, path) || picked.contains(path),
            Members::Outside { folders, picked } => !contains(folders, path) && !picked.contains(path),
        }
    }
}

pub fn members(conn: &Connection, workspace: Option<i64>) -> Result<Members> {
    let strings = |sql: &str, id: Option<i64>| -> Result<Vec<String>> {
        let mut stmt = conn.prepare(sql)?;
        let rows = match id {
            Some(id) => stmt.query_map([id], |row| row.get(0))?.collect::<std::result::Result<_, _>>()?,
            None => stmt.query_map([], |row| row.get(0))?.collect::<std::result::Result<_, _>>()?,
        };
        Ok(rows)
    };
    Ok(match workspace {
        None => Members::Every,
        Some(UNSORTED) => Members::Outside {
            folders: strings("SELECT DISTINCT folder FROM workspace_folders", None)?,
            picked: strings("SELECT DISTINCT path FROM workspace_items", None)?.into_iter().collect(),
        },
        Some(id) => Members::Inside {
            folders: folders(conn, id)?,
            picked: strings("SELECT path FROM workspace_items WHERE workspace_id = ?1", Some(id))?.into_iter().collect(),
        },
    })
}

/// Whether `path` is in one of `folders` (or is one of them).
pub fn contains(folders: &[String], path: &str) -> bool {
    folders.iter().any(|folder| path == folder || (path.starts_with(folder.as_str()) && path.as_bytes().get(folder.len()) == Some(&b'/')))
}

/// The Gallery view asked for: a workspace checked to exist, `UNSORTED`, or `None` (every photo).
pub fn check_view(conn: &Connection, workspace: Option<i64>) -> Result<Option<i64>> {
    match workspace {
        Some(UNSORTED) => Ok(Some(UNSORTED)),
        other => check(conn, other),
    }
}

/// The workspace asked for, checked to exist (`None` stays `None`: every photo).
pub fn check(conn: &Connection, workspace: Option<i64>) -> Result<Option<i64>> {
    match workspace {
        None => Ok(None),
        Some(id) => {
            let found = conn.query_row("SELECT 1 FROM workspaces WHERE id = ?1", [id], |_| Ok(())).optional()?;
            found.map(|_| Some(id)).ok_or_else(|| AlbumError::Missing("That workspace no longer exists".into()))
        }
    }
}

pub fn folders(conn: &Connection, id: i64) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT folder FROM workspace_folders WHERE workspace_id = ?1 ORDER BY folder COLLATE NOCASE")?;
    let rows = stmt.query_map([id], |row| row.get(0))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub fn list(conn: &Connection) -> Result<Vec<Workspace>> {
    let found: Vec<(i64, String)> = {
        let mut stmt = conn.prepare("SELECT id, name FROM workspaces ORDER BY name COLLATE NOCASE, id")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<std::result::Result<_, _>>()?
    };
    let mut out = Vec::with_capacity(found.len());
    for (id, name) in found {
        let (photos, picked) = conn.query_row(
            &format!(
                "SELECT count(*), coalesce(sum(NOT {}), 0) FROM media_index m
                 WHERE substr(m.path, 1, 14) <> '.ownnas-trash/' AND substr(m.path, 1, 6) <> 'Trash/' AND {}",
                in_folders_sql("m.path", Some(id)),
                filter(Some(id), "m.path")
            ),
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        out.push(Workspace { id, name, folders: folders(conn, id)?, photos, picked });
    }
    Ok(out)
}

/// The photos and videos in no workspace yet (outside the Trash).
pub fn unsorted_count(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row(
        &format!(
            "SELECT count(*) FROM media_index m WHERE substr(m.path, 1, 14) <> '.ownnas-trash/' AND substr(m.path, 1, 6) <> 'Trash/' AND {}",
            filter(Some(UNSORTED), "m.path")
        ),
        [],
        |row| row.get(0),
    )?)
}

/// Picks photos into workspaces (or takes them out); returns how many changed. A photo already
/// in a workspace's folders is in it anyway, so it is not picked, and can't be taken out
/// (only by taking the folder out of the workspace).
pub fn change_items(conn: &Connection, workspaces: &[i64], paths: &[String], add: bool, by: &str) -> Result<usize> {
    if paths.len() > MAX_ITEMS_PER_CHANGE {
        return Err(AlbumError::Invalid(format!("Choose up to {MAX_ITEMS_PER_CHANGE} photos at a time")));
    }
    let tx = conn.unchecked_transaction()?;
    let now = now_secs();
    let mut changed = 0;
    for &id in workspaces {
        check(&tx, Some(id))?;
        let folders = folders(&tx, id)?;
        for path in paths {
            changed += if add {
                if contains(&folders, path) {
                    continue;
                }
                tx.execute(
                    "INSERT INTO workspace_items(workspace_id, path, added_at, added_by) VALUES(?1, ?2, ?3, ?4)
                     ON CONFLICT(workspace_id, path) DO NOTHING",
                    params![id, path, now, by],
                )?
            } else {
                tx.execute("DELETE FROM workspace_items WHERE workspace_id = ?1 AND path = ?2", params![id, path])?
            };
        }
        tx.execute("UPDATE workspaces SET updated_at = ?1 WHERE id = ?2", params![now, id])?;
    }
    tx.commit()?;
    Ok(changed)
}

/// Where some photos stand in each workspace: how many of them are in it (through its folders
/// or picked), and how many of those were picked (and so can be taken out).
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Membership {
    pub id: i64,
    pub inside: usize,
    pub picked: usize,
}

pub fn membership(conn: &Connection, paths: &[String]) -> Result<Vec<Membership>> {
    let ids: Vec<i64> = {
        let mut stmt = conn.prepare("SELECT id FROM workspaces ORDER BY id")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect::<std::result::Result<_, _>>()?
    };
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let folders = folders(conn, id)?;
        let mut stmt = conn.prepare("SELECT 1 FROM workspace_items WHERE workspace_id = ?1 AND path = ?2")?;
        let (mut inside, mut picked) = (0, 0);
        for path in paths {
            if contains(&folders, path) {
                inside += 1;
            } else if stmt.query_row(params![id, path], |_| Ok(())).optional()?.is_some() {
                inside += 1;
                picked += 1;
            }
        }
        out.push(Membership { id, inside, picked });
    }
    Ok(out)
}

/// Where some folders stand in a workspace: how many are among its folders, how many are in it
/// through a folder above them (and the first such folder), and how many hold a folder of it.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FolderStanding {
    pub id: i64,
    pub direct: usize,
    pub through: usize,
    pub via: Option<String>,
    pub inner: usize,
}

pub fn folder_membership(conn: &Connection, chosen: &[String]) -> Result<Vec<FolderStanding>> {
    let ids: Vec<i64> = {
        let mut stmt = conn.prepare("SELECT id FROM workspaces ORDER BY id")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect::<std::result::Result<_, _>>()?
    };
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let folders = folders(conn, id)?;
        let mut standing = FolderStanding { id, direct: 0, through: 0, via: None, inner: 0 };
        for folder in chosen {
            if folders.contains(folder) {
                standing.direct += 1;
            } else if let Some(above) = folders.iter().find(|above| contains(std::slice::from_ref(above), folder)) {
                standing.through += 1;
                standing.via.get_or_insert_with(|| above.clone());
            } else if folders.iter().any(|below| contains(std::slice::from_ref(folder), below)) {
                standing.inner += 1;
            }
        }
        out.push(standing);
    }
    Ok(out)
}

/// Shows everything in `chosen` (checked folders, see `normalize_folders`) in workspaces, now
/// and later, or stops. Folders inside a chosen one that were listed on their own are taken in
/// by it. A folder in a workspace only through a folder above it can't be taken out here: the
/// error names that folder. Picked photos stay picked, so nothing is lost if the folder goes
/// again. Returns how many workspaces changed.
pub fn change_folders(conn: &Connection, workspaces: &[i64], chosen: &[String], add: bool) -> Result<usize> {
    let tx = conn.unchecked_transaction()?;
    let mut changed = 0;
    for &id in workspaces {
        check(&tx, Some(id))?;
        let name: String = tx.query_row("SELECT name FROM workspaces WHERE id = ?1", [id], |row| row.get(0))?;
        let mut had = folders(&tx, id)?;
        had.sort();
        let mut next = had.clone();
        if add {
            next.extend(chosen.iter().cloned());
        } else {
            for folder in chosen {
                if !had.contains(folder) {
                    if let Some(above) = had.iter().find(|above| contains(std::slice::from_ref(above), folder)) {
                        return Err(AlbumError::Invalid(format!("Everything in “{above}” shows in {name}, so “{folder}” does too. Take “{above}” out of {name} instead.")));
                    }
                }
            }
            next.retain(|folder| !chosen.contains(folder));
        }
        let next = normalize_folders(&next)?;
        if next != had {
            set_folders(&tx, id, &next)?;
            tx.execute("UPDATE workspaces SET updated_at = ?1 WHERE id = ?2", params![now_secs(), id])?;
            changed += 1;
        }
    }
    tx.commit()?;
    Ok(changed)
}

pub fn get(conn: &Connection, id: i64) -> Result<Workspace> {
    list(conn)?.into_iter().find(|workspace| workspace.id == id).ok_or_else(|| AlbumError::Missing("That workspace no longer exists".into()))
}

fn normalize_name(conn: &Connection, raw: &str, except: Option<i64>) -> Result<String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(AlbumError::Invalid("Enter a name for the workspace".into()));
    }
    if name.chars().count() > MAX_NAME {
        return Err(AlbumError::Invalid(format!("Workspace names can be up to {MAX_NAME} characters")));
    }
    if name.chars().any(char::is_control) {
        return Err(AlbumError::Invalid("Workspace names cannot contain control characters".into()));
    }
    let taken: Option<String> = conn
        .query_row("SELECT name FROM workspaces WHERE lower(name) = lower(?1) AND id IS NOT ?2", params![name, except], |row| row.get(0))
        .optional()?;
    if let Some(existing) = taken {
        return Err(AlbumError::Conflict(format!("There is already a workspace called “{existing}”")));
    }
    Ok(name.to_string())
}

/// Folders as library paths without slashes around them, each once, and none inside another
/// one of them (it would add nothing).
pub fn normalize_folders(raw: &[String]) -> Result<Vec<String>> {
    if raw.len() > MAX_FOLDERS {
        return Err(AlbumError::Invalid(format!("A workspace can have up to {MAX_FOLDERS} folders")));
    }
    let mut folders: Vec<String> = raw.iter().map(|folder| folder.trim().trim_matches('/').to_string()).collect();
    if folders.iter().any(String::is_empty) {
        return Err(AlbumError::Invalid("Choose folders inside the library, not the whole library".into()));
    }
    folders.sort();
    folders.dedup();
    let all = folders.clone();
    folders.retain(|folder| !all.iter().any(|other| other != folder && contains(std::slice::from_ref(other), folder)));
    Ok(folders)
}

fn set_folders(conn: &Connection, id: i64, folders: &[String]) -> Result<()> {
    conn.execute("DELETE FROM workspace_folders WHERE workspace_id = ?1", [id])?;
    for folder in folders {
        conn.execute("INSERT INTO workspace_folders(workspace_id, folder) VALUES(?1, ?2)", params![id, folder])?;
    }
    Ok(())
}

/// A new workspace; `folders` must already be checked (see `normalize_folders`).
pub fn create(conn: &Connection, name: &str, folders: &[String]) -> Result<i64> {
    let name = normalize_name(conn, name, None)?;
    let tx = conn.unchecked_transaction()?;
    let now = now_secs();
    tx.execute("INSERT INTO workspaces(name, created_at, updated_at) VALUES(?1, ?2, ?2)", params![name, now])?;
    let id = tx.last_insert_rowid();
    set_folders(&tx, id, folders)?;
    tx.commit()?;
    Ok(id)
}

pub fn update(conn: &Connection, id: i64, name: Option<&str>, folders: Option<&[String]>) -> Result<()> {
    check(conn, Some(id))?;
    let tx = conn.unchecked_transaction()?;
    if let Some(name) = name {
        let name = normalize_name(&tx, name, Some(id))?;
        tx.execute("UPDATE workspaces SET name = ?1 WHERE id = ?2", params![name, id])?;
    }
    if let Some(folders) = folders {
        set_folders(&tx, id, folders)?;
    }
    tx.execute("UPDATE workspaces SET updated_at = ?1 WHERE id = ?2", params![now_secs(), id])?;
    tx.commit()?;
    Ok(())
}

/// Deletes a workspace. Its albums stay, with no workspace (they show under All); photos and
/// folders are not touched.
pub fn delete(conn: &Connection, id: i64) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    // Top-level names must stay unique where the albums land.
    let mut stmt = tx.prepare("SELECT id, name FROM albums WHERE workspace_id = ?1 AND parent_id IS NULL")?;
    let moving: Vec<(i64, String)> = stmt.query_map([id], |row| Ok((row.get(0)?, row.get(1)?)))?.collect::<std::result::Result<_, _>>()?;
    drop(stmt);
    for (album, name) in moving {
        let mut free = name.clone();
        let mut n = 2;
        while tx
            .query_row("SELECT 1 FROM albums WHERE parent_id IS NULL AND workspace_id IS NULL AND lower(name) = lower(?1)", [&free], |_| Ok(()))
            .optional()?
            .is_some()
        {
            free = format!("{name} ({n})");
            n += 1;
        }
        tx.execute("UPDATE albums SET name = ?1 WHERE id = ?2", params![free, album])?;
    }
    tx.execute("UPDATE albums SET workspace_id = NULL WHERE workspace_id = ?1", [id])?;
    tx.execute("DELETE FROM workspace_items WHERE workspace_id = ?1", [id])?;
    if tx.execute("DELETE FROM workspaces WHERE id = ?1", [id])? == 0 {
        return Err(AlbumError::Missing("That workspace no longer exists".into()));
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn conn() -> Connection {
        let path = std::env::temp_dir().join(format!("ownnas-workspaces-{}.sqlite", rand::random::<u64>()));
        let conn = db::open(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        conn
    }

    #[test]
    fn folders_are_tidied_and_matched_whole() {
        let folders = normalize_folders(&["/Work/".into(), "Work/2024".into(), "Family".into(), "Work".into()]).unwrap();
        assert_eq!(folders, ["Family", "Work"], "inside another one, or twice, adds nothing");
        assert!(contains(&folders, "Work/a.jpg"));
        assert!(contains(&folders, "Work"));
        assert!(!contains(&folders, "Workshop/a.jpg"), "a folder whose name only starts the same");
        assert!(normalize_folders(&["/".into()]).is_err());
    }

    #[test]
    fn workspaces_filter_photos_and_keep_albums_when_deleted() {
        let conn = conn();
        for path in ["Work/a.jpg", "Work/b.jpg", "Workshop/c.jpg", "Family/d.jpg", ".ownnas-trash/1/Work/e.jpg"] {
            conn.execute("INSERT INTO media_index(path, kind, size, mtime, taken_at, date_source, indexed_at) VALUES(?1, 'image', 1, 1, 1, 'exif', 1)", [path]).unwrap();
        }
        let work = create(&conn, "Work", &["Work".into()]).unwrap();
        assert!(matches!(create(&conn, " work ", &[]), Err(AlbumError::Conflict(_))));
        let found = list(&conn).unwrap();
        assert_eq!((found[0].name.as_str(), found[0].photos), ("Work", 2));
        let count = |ws| -> i64 { conn.query_row(&format!("SELECT count(*) FROM media_index m WHERE {}", filter(ws, "m.path")), [], |row| row.get(0)).unwrap() };
        assert_eq!(count(Some(work)), 2);
        assert_eq!(count(None), 5);

        // Renaming the folder through OwnNAS keeps it in the workspace.
        db::rewrite_path_meta(&conn, "Work", "Jobs").unwrap();
        assert_eq!(folders(&conn, work).unwrap(), ["Jobs"]);

        // Deleting the workspace leaves its albums, under All, with names that stay unique.
        crate::albums::create(&conn, None, "Trips", None).unwrap();
        let album = crate::albums::create(&conn, None, "Trips", Some(work)).unwrap();
        delete(&conn, work).unwrap();
        let moved = crate::albums::get(&conn, album).unwrap();
        assert_eq!((moved.name.as_str(), moved.workspace_id), ("Trips (2)", None));
        assert!(check(&conn, Some(work)).is_err());
    }

    #[test]
    fn a_folder_can_show_everything_in_it_in_several_workspaces() {
        let conn = conn();
        for path in ["Phone/1.jpg", "Phone/2024/2.jpg", "Other/3.jpg"] {
            conn.execute("INSERT INTO media_index(path, kind, size, mtime, taken_at, date_source, indexed_at) VALUES(?1, 'image', 1, 1, 1, 'exif', 1)", [path]).unwrap();
        }
        let work = create(&conn, "Work", &["Phone/2024".into()]).unwrap();
        let home = create(&conn, "Home", &[]).unwrap();
        change_items(&conn, &[home], &["Phone/1.jpg".into()], true, "me").unwrap();
        let standing = |folder: &str| folder_membership(&conn, &[folder.to_string()]).unwrap();
        assert_eq!(standing("Phone"), [
            FolderStanding { id: work, direct: 0, through: 0, via: None, inner: 1 },
            FolderStanding { id: home, direct: 0, through: 0, via: None, inner: 0 },
        ]);

        assert_eq!(change_folders(&conn, &[work, home], &["Phone".into()], true).unwrap(), 2);
        assert_eq!(folders(&conn, work).unwrap(), ["Phone"], "the sub-folder is taken in by it");
        assert_eq!(folders(&conn, home).unwrap(), ["Phone"]);
        assert_eq!(change_folders(&conn, &[work], &["Phone".into()], true).unwrap(), 0, "already there");
        let photos = |ws| -> i64 { conn.query_row(&format!("SELECT count(*) FROM media_index m WHERE {}", filter(Some(ws), "m.path")), [], |row| row.get(0)).unwrap() };
        assert_eq!((photos(work), photos(home)), (2, 2));
        assert_eq!(standing("Phone/2024")[0], FolderStanding { id: work, direct: 0, through: 1, via: Some("Phone".into()), inner: 0 });

        let refused = change_folders(&conn, &[work], &["Phone/2024".into()], false).unwrap_err().to_string();
        assert!(refused.contains("“Phone”"), "{refused}");
        assert_eq!(change_folders(&conn, &[home], &["Phone".into()], false).unwrap(), 1);
        assert_eq!(photos(home), 1, "what was picked before stays picked");
        assert_eq!(photos(work), 2);
    }

    #[test]
    fn photos_picked_one_by_one_show_in_each_workspace_they_are_picked_into() {
        let conn = conn();
        for path in ["Phone/1.jpg", "Phone/2.jpg", "Phone/3.jpg", "Work/a.jpg", ".ownnas-trash/1/Phone/4.jpg"] {
            conn.execute("INSERT INTO media_index(path, kind, size, mtime, taken_at, date_source, indexed_at) VALUES(?1, 'image', 1, 1, 1, 'exif', 1)", [path]).unwrap();
        }
        let work = create(&conn, "Work", &["Work".into()]).unwrap();
        let private = create(&conn, "Private", &[]).unwrap();
        let count = |ws| -> i64 {
            conn.query_row(&format!("SELECT count(*) FROM media_index m WHERE substr(m.path, 1, 14) <> '.ownnas-trash/' AND {}", filter(ws, "m.path")), [], |row| row.get(0)).unwrap()
        };
        assert_eq!((count(Some(work)), count(Some(private)), count(Some(UNSORTED))), (1, 0, 3));

        // The same photo in both, another in one; one in the workspace's folders is not picked.
        let both = "Phone/1.jpg".to_string();
        assert_eq!(change_items(&conn, &[work, private], &[both.clone()], true, "ana").unwrap(), 2);
        assert_eq!(change_items(&conn, &[work], &["Work/a.jpg".into()], true, "ana").unwrap(), 0, "in its folders already");
        assert_eq!(change_items(&conn, &[private], &["Phone/2.jpg".into()], true, "ana").unwrap(), 1);
        assert_eq!(change_items(&conn, &[private], &["Phone/2.jpg".into()], true, "ana").unwrap(), 0, "picked already");
        assert_eq!((count(Some(work)), count(Some(private)), count(Some(UNSORTED))), (2, 2, 1));
        let found = list(&conn).unwrap();
        let private_row = found.iter().find(|space| space.id == private).unwrap();
        assert_eq!((private_row.photos, private_row.picked), (2, 2));
        assert_eq!(unsorted_count(&conn).unwrap(), 1);

        let members = members(&conn, Some(work)).unwrap();
        assert!(members.has("Phone/1.jpg") && members.has("Work/a.jpg") && !members.has("Phone/2.jpg"));
        assert!(super::members(&conn, Some(UNSORTED)).unwrap().has("Phone/3.jpg"));
        assert!(!super::members(&conn, Some(UNSORTED)).unwrap().has("Phone/1.jpg"));

        let seen = membership(&conn, &[both.clone(), "Work/a.jpg".into()]).unwrap();
        assert_eq!(seen, [Membership { id: work, inside: 2, picked: 1 }, Membership { id: private, inside: 1, picked: 1 }]);

        // A picked photo follows its file when it moves, and goes when it is deleted.
        db::rewrite_path_meta(&conn, "Phone", "Camera").unwrap();
        assert!(super::members(&conn, Some(private)).unwrap().has("Camera/2.jpg"));
        db::delete_path_meta(&conn, "Camera/2.jpg").unwrap();
        assert!(!super::members(&conn, Some(private)).unwrap().has("Camera/2.jpg"));

        // Taking one out of a workspace leaves it in the other.
        assert_eq!(change_items(&conn, &[work], &["Camera/1.jpg".into()], false, "ana").unwrap(), 1);
        assert!(super::members(&conn, Some(private)).unwrap().has("Camera/1.jpg"));
        assert!(change_items(&conn, &[UNSORTED], &["Camera/1.jpg".into()], true, "ana").is_err(), "not a workspace");

        // Deleting a workspace forgets what was picked into it.
        delete(&conn, private).unwrap();
        let left: i64 = conn.query_row("SELECT count(*) FROM workspace_items WHERE workspace_id = ?1", [private], |row| row.get(0)).unwrap();
        assert_eq!(left, 0);
    }
}
