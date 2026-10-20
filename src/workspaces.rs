//! Gallery workspaces ("Family", "Work", "Customer 1"): each is a name and some library folders,
//! and shows only the photos and videos stored in them, with its own albums. Nothing in the
//! library changes. The filter is applied by the server, so access to a workspace can later be
//! granted to some users only.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::albums::AlbumError;
use crate::db::now_secs;

const MAX_NAME: usize = 60;
const MAX_FOLDERS: usize = 200;

type Result<T> = std::result::Result<T, AlbumError>;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub id: i64,
    pub name: String,
    pub folders: Vec<String>,
    /// Photos and videos in its folders (outside the Trash).
    pub photos: i64,
}

/// SQL that is true when `column` (a library path) is in the workspace: in one of its folders,
/// or one of them. Always true for `None` (every photo). The id is a number, so it is safe to
/// write into the query.
pub fn filter(workspace: Option<i64>, column: &str) -> String {
    match workspace {
        None => "1".to_string(),
        Some(id) => format!(
            "EXISTS(SELECT 1 FROM workspace_folders wf WHERE wf.workspace_id = {id}
                AND ({column} = wf.folder OR substr({column}, 1, length(wf.folder) + 1) = wf.folder || '/'))"
        ),
    }
}

/// Whether `path` is in one of `folders` (or is one of them).
pub fn contains(folders: &[String], path: &str) -> bool {
    folders.iter().any(|folder| path == folder || (path.starts_with(folder.as_str()) && path.as_bytes().get(folder.len()) == Some(&b'/')))
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
        let photos = conn.query_row(
            &format!(
                "SELECT count(*) FROM media_index m WHERE substr(m.path, 1, 14) <> '.ownnas-trash/' AND substr(m.path, 1, 6) <> 'Trash/' AND {}",
                filter(Some(id), "m.path")
            ),
            [],
            |row| row.get(0),
        )?;
        out.push(Workspace { id, name, folders: folders(conn, id)?, photos });
    }
    Ok(out)
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
}
