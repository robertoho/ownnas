//! Virtual photo albums: a tree of albums, shared by every user, that hold library paths.
//! Albums never touch the files; their entries follow files that OwnNAS moves or renames
//! (see `db::rewrite_path_meta`).

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::db::now_secs;

const MAX_NAME: usize = 120;
/// Photos added to or removed from an album in one request at most.
pub const MAX_ITEMS_PER_CHANGE: usize = 10_000;

#[derive(Debug, PartialEq)]
pub enum AlbumError {
    NotFound,
    Invalid(String),
    Conflict(String),
    Db(String),
}

impl std::fmt::Display for AlbumError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "That album no longer exists"),
            Self::Invalid(why) | Self::Conflict(why) | Self::Db(why) => write!(f, "{why}"),
        }
    }
}

impl From<rusqlite::Error> for AlbumError {
    fn from(err: rusqlite::Error) -> Self {
        Self::Db(format!("Could not update albums: {err}"))
    }
}

type Result<T> = std::result::Result<T, AlbumError>;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Album {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub name: String,
    /// The photo shown on the album's card: the chosen cover, or the latest photo added.
    pub cover: Option<String>,
    /// Photos added to this album itself (not to its sub-albums), outside the Trash.
    pub count: i64,
    pub albums: i64,
}

fn normalize_name(raw: &str) -> Result<String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(AlbumError::Invalid("Enter a name for the album".into()));
    }
    if name.chars().count() > MAX_NAME {
        return Err(AlbumError::Invalid(format!("Album names can be up to {MAX_NAME} characters")));
    }
    if name.chars().any(char::is_control) {
        return Err(AlbumError::Invalid("Album names cannot contain control characters".into()));
    }
    Ok(name.to_string())
}

fn exists(conn: &Connection, id: i64) -> Result<bool> {
    Ok(conn.query_row("SELECT 1 FROM albums WHERE id = ?1", [id], |_| Ok(())).optional()?.is_some())
}

fn check_sibling_name(conn: &Connection, parent: Option<i64>, name: &str, except: Option<i64>) -> Result<()> {
    let taken: Option<String> = conn
        .query_row(
            "SELECT name FROM albums WHERE parent_id IS ?1 AND lower(name) = lower(?2) AND id IS NOT ?3",
            params![parent, name, except],
            |row| row.get(0),
        )
        .optional()?;
    match taken {
        Some(existing) => Err(AlbumError::Conflict(format!("There is already an album called “{existing}” here"))),
        None => Ok(()),
    }
}

// Paths in the Trash, which album views leave out.
const NOT_IN_TRASH: &str = "substr(path, 1, 14) <> '.ownnas-trash/' AND substr(path, 1, 6) <> 'Trash/'";

pub fn create(conn: &Connection, parent: Option<i64>, name: &str) -> Result<i64> {
    let name = normalize_name(name)?;
    if let Some(parent) = parent {
        if !exists(conn, parent)? {
            return Err(AlbumError::NotFound);
        }
    }
    check_sibling_name(conn, parent, &name, None)?;
    let now = now_secs();
    conn.execute(
        "INSERT INTO albums(parent_id, name, created_at, updated_at) VALUES(?1, ?2, ?3, ?3)",
        params![parent, name, now],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn rename(conn: &Connection, id: i64, name: &str) -> Result<()> {
    let name = normalize_name(name)?;
    let parent = parent_of(conn, id)?;
    check_sibling_name(conn, parent, &name, Some(id))?;
    conn.execute("UPDATE albums SET name = ?1, updated_at = ?2 WHERE id = ?3", params![name, now_secs(), id])?;
    Ok(())
}

fn parent_of(conn: &Connection, id: i64) -> Result<Option<i64>> {
    conn.query_row("SELECT parent_id FROM albums WHERE id = ?1", [id], |row| row.get(0))
        .optional()?
        .ok_or(AlbumError::NotFound)
}

/// Moves an album (with everything in it) under another album, or to the top with `None`.
pub fn move_to(conn: &Connection, id: i64, parent: Option<i64>) -> Result<()> {
    parent_of(conn, id)?;
    if let Some(parent) = parent {
        if !exists(conn, parent)? {
            return Err(AlbumError::NotFound);
        }
        let inside: bool = conn.query_row(
            "WITH RECURSIVE up(id, parent_id) AS (
                SELECT id, parent_id FROM albums WHERE id = ?1
                UNION ALL SELECT a.id, a.parent_id FROM albums a JOIN up ON a.id = up.parent_id
             ) SELECT EXISTS(SELECT 1 FROM up WHERE id = ?2)",
            params![parent, id],
            |row| row.get(0),
        )?;
        if inside {
            return Err(AlbumError::Invalid("An album cannot go inside itself".into()));
        }
    }
    let name: String = conn.query_row("SELECT name FROM albums WHERE id = ?1", [id], |row| row.get(0))?;
    check_sibling_name(conn, parent, &name, Some(id))?;
    conn.execute("UPDATE albums SET parent_id = ?1, updated_at = ?2 WHERE id = ?3", params![parent, now_secs(), id])?;
    Ok(())
}

/// Deletes an album and the albums inside it. The photos themselves are not touched.
pub fn delete(conn: &Connection, id: i64) -> Result<()> {
    if conn.execute("DELETE FROM albums WHERE id = ?1", [id])? == 0 {
        return Err(AlbumError::NotFound);
    }
    Ok(())
}

pub fn set_cover(conn: &Connection, id: i64, path: Option<&str>) -> Result<()> {
    if conn.execute("UPDATE albums SET cover_path = ?1, updated_at = ?2 WHERE id = ?3", params![path, now_secs(), id])? == 0 {
        return Err(AlbumError::NotFound);
    }
    Ok(())
}

/// Adds photos to an album (or removes them); returns how many changed.
pub fn change_items(conn: &Connection, id: i64, paths: &[String], add: bool, by: &str) -> Result<usize> {
    if !exists(conn, id)? {
        return Err(AlbumError::NotFound);
    }
    if paths.len() > MAX_ITEMS_PER_CHANGE {
        return Err(AlbumError::Invalid(format!("Choose up to {MAX_ITEMS_PER_CHANGE} photos at a time")));
    }
    let now = now_secs();
    let mut changed = 0;
    let tx = conn.unchecked_transaction()?;
    for path in paths {
        changed += if add {
            tx.execute(
                "INSERT INTO album_items(album_id, path, added_at, added_by) VALUES(?1, ?2, ?3, ?4)
                 ON CONFLICT(album_id, path) DO NOTHING",
                params![id, path, now, by],
            )?
        } else {
            tx.execute("DELETE FROM album_items WHERE album_id = ?1 AND path = ?2", params![id, path])?
        };
    }
    tx.execute("UPDATE albums SET updated_at = ?1 WHERE id = ?2", params![now, id])?;
    if !add {
        tx.execute(
            "UPDATE albums SET cover_path = NULL WHERE id = ?1 AND cover_path IS NOT NULL
             AND NOT EXISTS(SELECT 1 FROM album_items WHERE album_id = ?1 AND path = albums.cover_path)",
            [id],
        )?;
    }
    tx.commit()?;
    Ok(changed)
}

fn album_rows(conn: &Connection, filter: &str, value: Option<i64>) -> Result<Vec<Album>> {
    let sql = format!(
        "SELECT a.id, a.parent_id, a.name,
            coalesce(
                a.cover_path,
                (SELECT path FROM album_items WHERE album_id = a.id AND {NOT_IN_TRASH} ORDER BY added_at DESC, path LIMIT 1),
                -- An album holding only other albums borrows a cover from inside them.
                (WITH RECURSIVE inside(id) AS (
                    SELECT c.id FROM albums c WHERE c.parent_id = a.id
                    UNION ALL SELECT c.id FROM albums c JOIN inside ON c.parent_id = inside.id
                 ) SELECT path FROM album_items WHERE album_id IN (SELECT id FROM inside) AND {NOT_IN_TRASH}
                   ORDER BY added_at DESC, path LIMIT 1)
            ),
            (SELECT count(*) FROM album_items WHERE album_id = a.id AND {NOT_IN_TRASH}),
            (SELECT count(*) FROM albums c WHERE c.parent_id = a.id)
         FROM albums a WHERE {filter} ORDER BY a.name COLLATE NOCASE, a.id"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([value], |row| {
        Ok(Album { id: row.get(0)?, parent_id: row.get(1)?, name: row.get(2)?, cover: row.get(3)?, count: row.get(4)?, albums: row.get(5)? })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub fn get(conn: &Connection, id: i64) -> Result<Album> {
    album_rows(conn, "a.id = ?1", Some(id))?.pop().ok_or(AlbumError::NotFound)
}

/// The albums directly inside `parent` (top-level albums for `None`).
pub fn children(conn: &Connection, parent: Option<i64>) -> Result<Vec<Album>> {
    album_rows(conn, "a.parent_id IS ?1", parent)
}

/// Every album, for pickers that show the whole tree.
pub fn all(conn: &Connection) -> Result<Vec<Album>> {
    album_rows(conn, "?1 IS NULL", None)
}

/// The album and the albums above it, from the top.
pub fn breadcrumb(conn: &Connection, id: i64) -> Result<Vec<(i64, String)>> {
    let mut stmt = conn.prepare(
        "WITH RECURSIVE up(id, parent_id, name, depth) AS (
            SELECT id, parent_id, name, 0 FROM albums WHERE id = ?1
            UNION ALL SELECT a.id, a.parent_id, a.name, up.depth + 1 FROM albums a JOIN up ON a.id = up.parent_id
         ) SELECT id, name FROM up ORDER BY depth DESC",
    )?;
    let rows = stmt.query_map([id], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// A photo in an album, a tag, or the timeline.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaItem {
    pub path: String,
    pub kind: Option<String>,
    pub taken_at: Option<i64>,
    /// "exif" (a local date and time, with no time zone) or "file" (a Unix time).
    pub date_source: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub size: Option<i64>,
}

fn media_item(row: &rusqlite::Row<'_>) -> rusqlite::Result<MediaItem> {
    Ok(MediaItem {
        path: row.get(0)?,
        kind: row.get(1)?,
        taken_at: row.get(2)?,
        date_source: row.get(3)?,
        width: row.get(4)?,
        height: row.get(5)?,
        size: row.get(6)?,
    })
}

const MEDIA_COLUMNS: &str = "m.kind, m.taken_at, m.date_source, m.width, m.height, m.size";

/// The photos added to an album itself, newest first by the date they were taken.
pub fn items(conn: &Connection, id: i64) -> Result<Vec<MediaItem>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT i.path, {MEDIA_COLUMNS} FROM album_items i LEFT JOIN media_index m ON m.path = i.path
         WHERE i.album_id = ?1 AND substr(i.path, 1, 14) <> '.ownnas-trash/' AND substr(i.path, 1, 6) <> 'Trash/'
         ORDER BY coalesce(m.taken_at, i.added_at) DESC, i.path DESC"
    ))?;
    let rows = stmt.query_map([id], media_item)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Albums a file is in, each as its full name ("Trips / Italy").
pub fn albums_for_path(conn: &Connection, path: &str) -> Result<Vec<(i64, String)>> {
    let ids: Vec<i64> = {
        let mut stmt = conn.prepare("SELECT album_id FROM album_items WHERE path = ?1")?;
        let rows = stmt.query_map([path], |row| row.get(0))?;
        rows.collect::<std::result::Result<_, _>>()?
    };
    let mut out = Vec::new();
    for id in ids {
        let names: Vec<String> = breadcrumb(conn, id)?.into_iter().map(|(_, name)| name).collect();
        out.push((id, names.join(" / ")));
    }
    out.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
    Ok(out)
}

/// One page of the timeline, newest first: items taken before `(taken_at, path)`.
pub fn timeline(conn: &Connection, before: Option<(i64, String)>, limit: usize) -> Result<Vec<MediaItem>> {
    let (at, path) = before.unwrap_or((i64::MAX, String::new()));
    let mut stmt = conn.prepare(&format!(
        "SELECT m.path, {MEDIA_COLUMNS} FROM media_index m
         WHERE (m.taken_at < ?1 OR (m.taken_at = ?1 AND m.path < ?2 AND ?2 <> ''))
           AND substr(m.path, 1, 14) <> '.ownnas-trash/' AND substr(m.path, 1, 6) <> 'Trash/'
         ORDER BY m.taken_at DESC, m.path DESC LIMIT ?3"
    ))?;
    let rows = stmt.query_map(params![at, path, limit as i64], media_item)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// How many photos and videos were taken each day (UTC dates), newest first: enough for the
/// Gallery to lay out the whole timeline before loading any of it.
pub fn days(conn: &Connection) -> Result<Vec<(String, i64)>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT strftime('%Y-%m-%d', taken_at, 'unixepoch') AS day, count(*) FROM media_index
         WHERE {NOT_IN_TRASH} GROUP BY day ORDER BY day DESC"
    ))?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Photos and videos taken in `[from, to)`, newest first.
pub fn taken_between(conn: &Connection, from: i64, to: i64, limit: usize) -> Result<Vec<MediaItem>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT m.path, {MEDIA_COLUMNS} FROM media_index m
         WHERE m.taken_at >= ?1 AND m.taken_at < ?2
           AND substr(m.path, 1, 14) <> '.ownnas-trash/' AND substr(m.path, 1, 6) <> 'Trash/'
         ORDER BY m.taken_at DESC, m.path DESC LIMIT ?3"
    ))?;
    let rows = stmt.query_map(params![from, to, limit as i64], media_item)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Every tag with how many files carry it.
pub fn tags(conn: &Connection) -> Result<Vec<(String, i64)>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT min(tag), count(*) FROM file_tags WHERE {NOT_IN_TRASH} GROUP BY tag COLLATE NOCASE ORDER BY min(tag) COLLATE NOCASE"
    ))?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Files with a tag, newest first where the date is known.
pub fn tagged(conn: &Connection, tag: &str) -> Result<Vec<MediaItem>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT t.path, {MEDIA_COLUMNS} FROM file_tags t LEFT JOIN media_index m ON m.path = t.path
         WHERE t.tag = ?1 COLLATE NOCASE AND substr(t.path, 1, 14) <> '.ownnas-trash/' AND substr(t.path, 1, 6) <> 'Trash/'
         ORDER BY coalesce(m.taken_at, t.created_at) DESC, t.path DESC"
    ))?;
    let rows = stmt.query_map([tag], media_item)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn conn() -> Connection {
        let path = std::env::temp_dir().join(format!("ownnas-albums-{}.sqlite", rand::random::<u64>()));
        let conn = db::open(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        conn
    }

    fn index(conn: &Connection, path: &str, taken_at: i64) {
        conn.execute(
            "INSERT INTO media_index(path, kind, size, mtime, taken_at, date_source, indexed_at) VALUES(?1, 'image', 1, 1, ?2, 'exif', 1)",
            params![path, taken_at],
        )
        .unwrap();
    }

    #[test]
    fn albums_nest_like_folders() {
        let conn = conn();
        let trips = create(&conn, None, "Trips").unwrap();
        let italy = create(&conn, Some(trips), "Italy").unwrap();
        let rome = create(&conn, Some(italy), "Rome").unwrap();
        assert_eq!(create(&conn, None, " trips ").unwrap_err(), AlbumError::Conflict("There is already an album called “Trips” here".into()));
        assert!(create(&conn, Some(trips), "Rome").is_ok(), "the same name can be used in another album");
        assert!(matches!(create(&conn, None, "  "), Err(AlbumError::Invalid(_))));
        assert_eq!(create(&conn, Some(9999), "Lost"), Err(AlbumError::NotFound));

        assert_eq!(breadcrumb(&conn, rome).unwrap(), vec![(trips, "Trips".into()), (italy, "Italy".into()), (rome, "Rome".into())]);
        change_items(&conn, rome, &["rome.jpg".into()], true, "ana").unwrap();
        assert_eq!(get(&conn, trips).unwrap().cover.as_deref(), Some("rome.jpg"), "an album of albums borrows a cover");
        change_items(&conn, rome, &["rome.jpg".into()], false, "ana").unwrap();
        assert_eq!(children(&conn, None).unwrap().iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["Trips"]);
        assert_eq!(get(&conn, trips).unwrap().albums, 2);

        // An album cannot move inside itself or below its own sub-albums.
        assert!(matches!(move_to(&conn, trips, Some(rome)), Err(AlbumError::Invalid(_))));
        assert!(matches!(move_to(&conn, trips, Some(trips)), Err(AlbumError::Invalid(_))));
        // Moving next to an album with the same name is refused.
        assert!(matches!(move_to(&conn, rome, Some(trips)), Err(AlbumError::Conflict(_))));
        move_to(&conn, rome, None).unwrap();
        assert_eq!(breadcrumb(&conn, rome).unwrap().len(), 1);
        rename(&conn, rome, "Roma").unwrap();
        assert_eq!(get(&conn, rome).unwrap().name, "Roma");

        // Deleting an album deletes the albums inside it, and nothing else.
        change_items(&conn, italy, &["a.jpg".into()], true, "ana").unwrap();
        delete(&conn, trips).unwrap();
        assert_eq!(get(&conn, italy).unwrap_err(), AlbumError::NotFound);
        assert_eq!(all(&conn).unwrap().len(), 1);
        let left: i64 = conn.query_row("SELECT count(*) FROM album_items", [], |row| row.get(0)).unwrap();
        assert_eq!(left, 0);
    }

    #[test]
    fn items_follow_their_files() {
        let conn = conn();
        let album = create(&conn, None, "Family").unwrap();
        index(&conn, "photos/a.jpg", 100);
        index(&conn, "photos/b.jpg", 300);
        index(&conn, "photos/c.jpg", 200);
        assert_eq!(change_items(&conn, album, &["photos/a.jpg".into(), "photos/b.jpg".into(), "photos/a.jpg".into()], true, "ana").unwrap(), 2);
        assert_eq!(items(&conn, album).unwrap().iter().map(|i| i.path.as_str()).collect::<Vec<_>>(), ["photos/b.jpg", "photos/a.jpg"]);

        // Renaming a folder, and moving to the Trash and back, keep the album entries.
        db::rewrite_path_meta(&conn, "photos", "pictures").unwrap();
        assert_eq!(items(&conn, album).unwrap()[0].path, "pictures/b.jpg");
        db::rewrite_path_meta(&conn, "pictures/b.jpg", ".ownnas-trash/x1/b.jpg").unwrap();
        assert_eq!(items(&conn, album).unwrap().len(), 1, "photos in the Trash are hidden");
        assert_eq!(get(&conn, album).unwrap().count, 1);
        db::rewrite_path_meta(&conn, ".ownnas-trash/x1/b.jpg", "pictures/b.jpg").unwrap();
        assert_eq!(items(&conn, album).unwrap().len(), 2, "restoring brings it back");
        // A copy is not added to albums; deleting forever removes the entry.
        db::copy_path_meta(&conn, "pictures/a.jpg", "pictures/a copy.jpg").unwrap();
        assert_eq!(albums_for_path(&conn, "pictures/a copy.jpg").unwrap(), vec![]);
        assert_eq!(timeline(&conn, None, 10).unwrap().len(), 4);
        db::delete_path_meta(&conn, "pictures/a.jpg").unwrap();
        assert_eq!(items(&conn, album).unwrap().len(), 1);
        assert_eq!(albums_for_path(&conn, "pictures/b.jpg").unwrap(), vec![(album, "Family".into())]);

        set_cover(&conn, album, Some("pictures/b.jpg")).unwrap();
        db::rewrite_path_meta(&conn, "pictures/b.jpg", "best.jpg").unwrap();
        assert_eq!(get(&conn, album).unwrap().cover.as_deref(), Some("best.jpg"));
        change_items(&conn, album, &["best.jpg".into()], false, "ana").unwrap();
        assert_eq!(get(&conn, album).unwrap().cover, None, "a removed photo stops being the cover");
    }

    #[test]
    fn timeline_pages_newest_first() {
        let conn = conn();
        for (path, at) in [("a.jpg", 10), ("b.jpg", 30), ("c.jpg", 30), ("d.jpg", 20), (".ownnas-trash/1/e.jpg", 40)] {
            index(&conn, path, at);
        }
        let first = timeline(&conn, None, 2).unwrap();
        assert_eq!(first.iter().map(|i| i.path.as_str()).collect::<Vec<_>>(), ["c.jpg", "b.jpg"]);
        let last = first.last().unwrap();
        let next = timeline(&conn, Some((last.taken_at.unwrap(), last.path.clone())), 10).unwrap();
        assert_eq!(next.iter().map(|i| i.path.as_str()).collect::<Vec<_>>(), ["d.jpg", "a.jpg"]);

        assert_eq!(days(&conn).unwrap(), vec![("1970-01-01".into(), 4)]);
        assert_eq!(taken_between(&conn, 20, 31, 10).unwrap().iter().map(|i| i.path.as_str()).collect::<Vec<_>>(), ["c.jpg", "b.jpg", "d.jpg"]);
        db::add_tag(&conn, "a.jpg", "Beach").unwrap();
        db::add_tag(&conn, "b.jpg", "beach").unwrap();
        assert_eq!(tags(&conn).unwrap(), vec![("Beach".into(), 2)]);
        assert_eq!(tagged(&conn, "BEACH").unwrap().iter().map(|i| i.path.as_str()).collect::<Vec<_>>(), ["b.jpg", "a.jpg"]);
    }
}
