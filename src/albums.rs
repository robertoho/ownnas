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
    /// Something else that is gone (a workspace), with what to say.
    Missing(String),
    Invalid(String),
    Conflict(String),
    Db(String),
}

impl std::fmt::Display for AlbumError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "That album no longer exists"),
            Self::Missing(why) | Self::Invalid(why) | Self::Conflict(why) | Self::Db(why) => write!(f, "{why}"),
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
    /// The Gallery workspace it belongs to (with everything inside it); none shows under All.
    pub workspace_id: Option<i64>,
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

/// Names are unique among an album's sub-albums, and among a workspace's top-level albums.
fn check_sibling_name(conn: &Connection, parent: Option<i64>, workspace: Option<i64>, name: &str, except: Option<i64>) -> Result<()> {
    let taken: Option<String> = conn
        .query_row(
            "SELECT name FROM albums WHERE parent_id IS ?1 AND (?1 IS NOT NULL OR workspace_id IS ?4)
             AND lower(name) = lower(?2) AND id IS NOT ?3",
            params![parent, name, except, workspace],
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

/// A new album inside `parent` (in its workspace), or at the top of `workspace`.
pub fn create(conn: &Connection, parent: Option<i64>, name: &str, workspace: Option<i64>) -> Result<i64> {
    let name = normalize_name(name)?;
    let workspace = match parent {
        Some(parent) => {
            if !exists(conn, parent)? {
                return Err(AlbumError::NotFound);
            }
            workspace_of(conn, parent)?
        }
        None => workspace,
    };
    check_sibling_name(conn, parent, workspace, &name, None)?;
    let now = now_secs();
    conn.execute(
        "INSERT INTO albums(parent_id, workspace_id, name, created_at, updated_at) VALUES(?1, ?2, ?3, ?4, ?4)",
        params![parent, workspace, name, now],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn rename(conn: &Connection, id: i64, name: &str) -> Result<()> {
    let name = normalize_name(name)?;
    let parent = parent_of(conn, id)?;
    check_sibling_name(conn, parent, workspace_of(conn, id)?, &name, Some(id))?;
    conn.execute("UPDATE albums SET name = ?1, updated_at = ?2 WHERE id = ?3", params![name, now_secs(), id])?;
    Ok(())
}

fn parent_of(conn: &Connection, id: i64) -> Result<Option<i64>> {
    conn.query_row("SELECT parent_id FROM albums WHERE id = ?1", [id], |row| row.get(0))
        .optional()?
        .ok_or(AlbumError::NotFound)
}

fn workspace_of(conn: &Connection, id: i64) -> Result<Option<i64>> {
    conn.query_row("SELECT workspace_id FROM albums WHERE id = ?1", [id], |row| row.get(0))
        .optional()?
        .ok_or(AlbumError::NotFound)
}

/// Puts an album and every album inside it in a workspace.
fn set_subtree_workspace(conn: &Connection, id: i64, workspace: Option<i64>) -> Result<()> {
    conn.execute(
        "WITH RECURSIVE inside(id) AS (
            SELECT ?1 UNION ALL SELECT a.id FROM albums a JOIN inside ON a.parent_id = inside.id
         ) UPDATE albums SET workspace_id = ?2 WHERE id IN (SELECT id FROM inside)",
        params![id, workspace],
    )?;
    Ok(())
}

/// Moves an album (with everything in it) to another workspace, at its top level.
pub fn set_workspace(conn: &Connection, id: i64, workspace: Option<i64>) -> Result<()> {
    let parent = parent_of(conn, id)?;
    if parent.is_none() && workspace_of(conn, id)? == workspace {
        return Ok(());
    }
    let name: String = conn.query_row("SELECT name FROM albums WHERE id = ?1", [id], |row| row.get(0))?;
    check_sibling_name(conn, None, workspace, &name, Some(id))?;
    let tx = conn.unchecked_transaction()?;
    tx.execute("UPDATE albums SET parent_id = NULL, updated_at = ?1 WHERE id = ?2", params![now_secs(), id])?;
    set_subtree_workspace(&tx, id, workspace)?;
    tx.commit()?;
    Ok(())
}

/// Moves an album (with everything in it) under another album (into its workspace), or to the
/// top of its own workspace with `None`.
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
    let workspace = match parent {
        Some(parent) => workspace_of(conn, parent)?,
        None => workspace_of(conn, id)?,
    };
    check_sibling_name(conn, parent, workspace, &name, Some(id))?;
    let tx = conn.unchecked_transaction()?;
    tx.execute("UPDATE albums SET parent_id = ?1, updated_at = ?2 WHERE id = ?3", params![parent, now_secs(), id])?;
    set_subtree_workspace(&tx, id, workspace)?;
    tx.commit()?;
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
        "SELECT a.id, a.parent_id, a.workspace_id, a.name,
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
        Ok(Album { id: row.get(0)?, parent_id: row.get(1)?, workspace_id: row.get(2)?, name: row.get(3)?, cover: row.get(4)?, count: row.get(5)?, albums: row.get(6)? })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub fn get(conn: &Connection, id: i64) -> Result<Album> {
    album_rows(conn, "a.id = ?1", Some(id))?.pop().ok_or(AlbumError::NotFound)
}

/// The albums directly inside `parent`, or the top-level albums of `workspace` (of every
/// workspace for `None`).
pub fn children(conn: &Connection, parent: Option<i64>, workspace: Option<i64>) -> Result<Vec<Album>> {
    match (parent, workspace) {
        (Some(parent), _) => album_rows(conn, "a.parent_id = ?1", Some(parent)),
        (None, Some(workspace)) => album_rows(conn, "a.parent_id IS NULL AND a.workspace_id = ?1", Some(workspace)),
        (None, None) => album_rows(conn, "a.parent_id IS NULL AND ?1 IS NULL", None),
    }
}

/// Every album of a workspace (of every one for `None`), for pickers that show the whole tree.
pub fn all(conn: &Connection, workspace: Option<i64>) -> Result<Vec<Album>> {
    match workspace {
        Some(workspace) => album_rows(conn, "a.workspace_id = ?1", Some(workspace)),
        None => album_rows(conn, "?1 IS NULL", None),
    }
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
    /// Left out of the Photos timeline (it still shows in folders, albums, and tags).
    pub hidden: bool,
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
        hidden: row.get(7)?,
    })
}

const MEDIA_COLUMNS: &str = "m.kind, m.taken_at, m.date_source, m.width, m.height, m.size";
/// The "hidden from the timeline" flag of the row's path (`p`).
fn hidden_column(alias: &str) -> String {
    format!("EXISTS(SELECT 1 FROM timeline_hidden h WHERE h.path = {alias}.path)")
}
const NOT_HIDDEN: &str = "NOT EXISTS(SELECT 1 FROM timeline_hidden h WHERE h.path = m.path)";

/// The photos added to an album itself, newest first by the date they were taken.
pub fn items(conn: &Connection, id: i64) -> Result<Vec<MediaItem>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT i.path, {MEDIA_COLUMNS}, {} FROM album_items i LEFT JOIN media_index m ON m.path = i.path
         WHERE i.album_id = ?1 AND substr(i.path, 1, 14) <> '.ownnas-trash/' AND substr(i.path, 1, 6) <> 'Trash/'
         ORDER BY coalesce(m.taken_at, i.added_at) DESC, i.path DESC",
        hidden_column("i")
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
pub fn timeline(conn: &Connection, before: Option<(i64, String)>, limit: usize, workspace: Option<i64>) -> Result<Vec<MediaItem>> {
    let in_workspace = crate::workspaces::filter(workspace, "m.path");
    let (at, path) = before.unwrap_or((i64::MAX, String::new()));
    let mut stmt = conn.prepare(&format!(
        "SELECT m.path, {MEDIA_COLUMNS}, 0 FROM media_index m
         WHERE (m.taken_at < ?1 OR (m.taken_at = ?1 AND m.path < ?2 AND ?2 <> ''))
           AND substr(m.path, 1, 14) <> '.ownnas-trash/' AND substr(m.path, 1, 6) <> 'Trash/' AND {NOT_HIDDEN} AND {in_workspace}
         ORDER BY m.taken_at DESC, m.path DESC LIMIT ?3"
    ))?;
    let rows = stmt.query_map(params![at, path, limit as i64], media_item)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// How many photos and videos were taken each day (UTC dates), newest first: enough for the
/// Gallery to lay out the whole timeline before loading any of it.
pub fn days(conn: &Connection, workspace: Option<i64>) -> Result<Vec<(String, i64)>> {
    let in_workspace = crate::workspaces::filter(workspace, "m.path");
    let mut stmt = conn.prepare(&format!(
        "SELECT strftime('%Y-%m-%d', taken_at, 'unixepoch') AS day, count(*) FROM media_index m
         WHERE {NOT_IN_TRASH} AND {NOT_HIDDEN} AND {in_workspace} GROUP BY day ORDER BY day DESC"
    ))?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Photos and videos taken in `[from, to)`, newest first.
pub fn taken_between(conn: &Connection, from: i64, to: i64, limit: usize, workspace: Option<i64>) -> Result<Vec<MediaItem>> {
    let in_workspace = crate::workspaces::filter(workspace, "m.path");
    let mut stmt = conn.prepare(&format!(
        "SELECT m.path, {MEDIA_COLUMNS}, 0 FROM media_index m
         WHERE m.taken_at >= ?1 AND m.taken_at < ?2
           AND substr(m.path, 1, 14) <> '.ownnas-trash/' AND substr(m.path, 1, 6) <> 'Trash/' AND {NOT_HIDDEN} AND {in_workspace}
         ORDER BY m.taken_at DESC, m.path DESC LIMIT ?3"
    ))?;
    let rows = stmt.query_map(params![from, to, limit as i64], media_item)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// A group of photos close together on the map at some zoom: where they are on average, how
/// many, and the newest of them (for its picture).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Place {
    pub lat: f64,
    pub lon: f64,
    pub count: u64,
    pub newest: MediaItem,
}

/// The box (west, south, east, north) a map shows.
#[derive(Clone, Copy, Debug)]
pub struct Bounds {
    pub west: f64,
    pub south: f64,
    pub east: f64,
    pub north: f64,
}

/// Web map position (0..1) of a latitude, from the top.
fn mercator_y(lat: f64) -> f64 {
    let lat = lat.clamp(-85.05, 85.05).to_radians();
    (1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / std::f64::consts::PI) / 2.0
}

fn geotagged(conn: &Connection, bounds: Option<Bounds>, workspace: Option<i64>) -> Result<Vec<(f64, f64, MediaItem)>> {
    let in_workspace = crate::workspaces::filter(workspace, "m.path");
    let b = bounds.unwrap_or(Bounds { west: -180.0, south: -90.0, east: 180.0, north: 90.0 });
    // A box over the date line has its west east of its east.
    let mut stmt = conn.prepare(&format!(
        "SELECT m.lat, m.lon, m.path, {MEDIA_COLUMNS}, 0 FROM media_index m
         WHERE m.lat IS NOT NULL AND m.lon IS NOT NULL AND m.lat BETWEEN ?2 AND ?4
           AND (CASE WHEN ?1 <= ?3 THEN m.lon BETWEEN ?1 AND ?3 ELSE m.lon >= ?1 OR m.lon <= ?3 END)
           AND substr(m.path, 1, 14) <> '.ownnas-trash/' AND substr(m.path, 1, 6) <> 'Trash/' AND {NOT_HIDDEN} AND {in_workspace}
         ORDER BY m.taken_at DESC, m.path DESC"
    ))?;
    let rows = stmt.query_map(params![b.west, b.south, b.east, b.north], |row| {
        let item = MediaItem {
            path: row.get(2)?,
            kind: row.get(3)?,
            taken_at: row.get(4)?,
            date_source: row.get(5)?,
            width: row.get(6)?,
            height: row.get(7)?,
            size: row.get(8)?,
            hidden: row.get(9)?,
        };
        Ok((row.get(0)?, row.get(1)?, item))
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Photos with a place, grouped into cells about `cell` pixels wide at `zoom`, within `bounds`.
pub fn places(conn: &Connection, zoom: u8, bounds: Option<Bounds>, cell: f64, workspace: Option<i64>) -> Result<Vec<Place>> {
    let world = 256.0 * 2f64.powi(i32::from(zoom.min(22)));
    let mut cells: std::collections::HashMap<(i64, i64), (f64, f64, u64, Option<MediaItem>)> = std::collections::HashMap::new();
    // Newest first, so a cell's first photo is its newest.
    for (lat, lon, item) in geotagged(conn, bounds, workspace)? {
        let x = ((lon + 180.0) / 360.0 * world / cell).floor() as i64;
        let y = (mercator_y(lat) * world / cell).floor() as i64;
        let entry = cells.entry((x, y)).or_insert((0.0, 0.0, 0, None));
        entry.0 += lat;
        entry.1 += lon;
        entry.2 += 1;
        entry.3.get_or_insert(item);
    }
    let mut out: Vec<Place> = cells
        .into_values()
        .filter_map(|(lat, lon, count, newest)| Some(Place { lat: lat / count as f64, lon: lon / count as f64, count, newest: newest? }))
        .collect();
    out.sort_by(|a, b| b.count.cmp(&a.count));
    Ok(out)
}

/// The photos with a place within `bounds`, newest first, and how many there are.
pub fn placed_items(conn: &Connection, bounds: Bounds, limit: usize, workspace: Option<i64>) -> Result<(Vec<MediaItem>, usize)> {
    let all = geotagged(conn, Some(bounds), workspace)?;
    let total = all.len();
    Ok((all.into_iter().take(limit).map(|(_, _, item)| item).collect(), total))
}

/// The box around every photo with a place, and how many there are.
pub fn place_extent(conn: &Connection, workspace: Option<i64>) -> Result<Option<(Bounds, u64)>> {
    let in_workspace = crate::workspaces::filter(workspace, "m.path");
    let row: (Option<f64>, Option<f64>, Option<f64>, Option<f64>, i64) = conn.query_row(
        &format!(
            "SELECT min(lon), min(lat), max(lon), max(lat), count(*) FROM media_index m
             WHERE m.lat IS NOT NULL AND m.lon IS NOT NULL
               AND substr(m.path, 1, 14) <> '.ownnas-trash/' AND substr(m.path, 1, 6) <> 'Trash/' AND {NOT_HIDDEN} AND {in_workspace}"
        ),
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
    )?;
    Ok(match row {
        (Some(west), Some(south), Some(east), Some(north), count) if count > 0 => Some((Bounds { west, south, east, north }, count as u64)),
        _ => None,
    })
}

/// Photos and videos taken on this day of the year (`month`, `day`) in earlier years than
/// `year`, newest year first. Only dates from the camera count: a file's date is often just
/// when it was copied.
pub fn on_this_day(conn: &Connection, month: u32, day: u32, year: i32, per_year: usize, workspace: Option<i64>) -> Result<Vec<(i32, usize, Vec<MediaItem>)>> {
    let in_workspace = crate::workspaces::filter(workspace, "m.path");
    let mut stmt = conn.prepare(&format!(
        "SELECT m.path, {MEDIA_COLUMNS}, 0, CAST(strftime('%Y', m.taken_at, 'unixepoch') AS INTEGER) AS year FROM media_index m
         WHERE strftime('%m-%d', m.taken_at, 'unixepoch') = ?1 AND m.date_source <> 'file'
           AND CAST(strftime('%Y', m.taken_at, 'unixepoch') AS INTEGER) < ?2
           AND substr(m.path, 1, 14) <> '.ownnas-trash/' AND substr(m.path, 1, 6) <> 'Trash/' AND {NOT_HIDDEN} AND {in_workspace}
         ORDER BY year DESC, m.taken_at, m.path"
    ))?;
    let rows = stmt.query_map(params![format!("{month:02}-{day:02}"), year], |row| Ok((row.get::<_, i32>(8)?, media_item(row)?)))?;
    let mut years: Vec<(i32, usize, Vec<MediaItem>)> = Vec::new();
    for row in rows {
        let (year, item) = row?;
        match years.last_mut() {
            Some((last, count, items)) if *last == year => {
                *count += 1;
                if items.len() < per_year {
                    items.push(item);
                }
            }
            _ => years.push((year, 1, vec![item])),
        }
    }
    Ok(years)
}

/// The media items for some paths, in the same order (paths not indexed get no details).
pub fn media_items(conn: &Connection, paths: &[String]) -> Result<Vec<MediaItem>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT ?1, m.kind, m.taken_at, m.date_source, m.width, m.height, m.size,
                EXISTS(SELECT 1 FROM timeline_hidden h WHERE h.path = ?1)
         FROM (SELECT 1) LEFT JOIN media_index m ON m.path = ?1"
    ))?;
    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        out.push(stmt.query_row([path], media_item)?);
    }
    Ok(out)
}

/// Hides photos from the timeline (or shows them again); returns how many changed.
pub fn set_hidden(conn: &Connection, paths: &[String], hidden: bool, by: &str) -> Result<usize> {
    if paths.len() > MAX_ITEMS_PER_CHANGE {
        return Err(AlbumError::Invalid(format!("Choose up to {MAX_ITEMS_PER_CHANGE} photos at a time")));
    }
    let now = now_secs();
    let tx = conn.unchecked_transaction()?;
    let mut changed = 0;
    for path in paths {
        changed += if hidden {
            tx.execute(
                "INSERT INTO timeline_hidden(path, hidden_at, hidden_by) VALUES(?1, ?2, ?3) ON CONFLICT(path) DO NOTHING",
                params![path, now, by],
            )?
        } else {
            tx.execute("DELETE FROM timeline_hidden WHERE path = ?1", [path])?
        };
    }
    tx.commit()?;
    Ok(changed)
}

/// The photos hidden from the timeline, newest first.
pub fn hidden(conn: &Connection, workspace: Option<i64>) -> Result<Vec<MediaItem>> {
    let in_workspace = crate::workspaces::filter(workspace, "h.path");
    let mut stmt = conn.prepare(&format!(
        "SELECT h.path, {MEDIA_COLUMNS}, 1 FROM timeline_hidden h LEFT JOIN media_index m ON m.path = h.path
         WHERE substr(h.path, 1, 14) <> '.ownnas-trash/' AND substr(h.path, 1, 6) <> 'Trash/' AND {in_workspace}
         ORDER BY coalesce(m.taken_at, h.hidden_at) DESC, h.path DESC"
    ))?;
    let rows = stmt.query_map([], media_item)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub fn hidden_count(conn: &Connection, workspace: Option<i64>) -> Result<i64> {
    Ok(conn.query_row(
        &format!(
            "SELECT count(*) FROM timeline_hidden h WHERE substr(h.path, 1, 14) <> '.ownnas-trash/' AND substr(h.path, 1, 6) <> 'Trash/' AND {}",
            crate::workspaces::filter(workspace, "h.path")
        ),
        [],
        |row| row.get(0),
    )?)
}

/// Every tag with how many files carry it (in a workspace's folders only, with one).
pub fn tags(conn: &Connection, workspace: Option<i64>) -> Result<Vec<(String, i64)>> {
    let in_workspace = crate::workspaces::filter(workspace, "path");
    let mut stmt = conn.prepare(&format!(
        "SELECT min(tag), count(*) FROM file_tags WHERE {NOT_IN_TRASH} AND {in_workspace} GROUP BY tag COLLATE NOCASE ORDER BY min(tag) COLLATE NOCASE"
    ))?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Files with a tag, newest first where the date is known.
pub fn tagged(conn: &Connection, tag: &str, workspace: Option<i64>) -> Result<Vec<MediaItem>> {
    let in_workspace = crate::workspaces::filter(workspace, "t.path");
    let mut stmt = conn.prepare(&format!(
        "SELECT t.path, {MEDIA_COLUMNS}, {} FROM file_tags t LEFT JOIN media_index m ON m.path = t.path
         WHERE t.tag = ?1 COLLATE NOCASE AND substr(t.path, 1, 14) <> '.ownnas-trash/' AND substr(t.path, 1, 6) <> 'Trash/' AND {in_workspace}
         ORDER BY coalesce(m.taken_at, t.created_at) DESC, t.path DESC",
        hidden_column("t")
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
    fn places_group_on_the_map_and_memories_come_by_day() {
        let conn = conn();
        // 2023-10-19 12:00 and 2021-10-19 08:00 (camera times), a file date, and today.
        index(&conn, "paris/a.jpg", 1_697_716_800);
        index(&conn, "paris/b.jpg", 1_634_630_400);
        index(&conn, "sydney.jpg", 1_697_716_800 + 60);
        index(&conn, "copied.jpg", 1_697_716_800 + 120);
        conn.execute("UPDATE media_index SET date_source = 'file' WHERE path = 'copied.jpg'", []).unwrap();
        index(&conn, "today.jpg", 1_792_400_000);
        for (path, lat, lon) in [("paris/a.jpg", 48.857, 2.295), ("paris/b.jpg", 48.86, 2.34), ("sydney.jpg", -33.86, 151.21)] {
            conn.execute("UPDATE media_index SET lat = ?2, lon = ?3 WHERE path = ?1", params![path, lat, lon]).unwrap();
        }
        let world = places(&conn, 2, None, 72.0, None).unwrap();
        assert_eq!(world.iter().map(|place| place.count).collect::<Vec<_>>(), [2, 1], "Paris is one place from afar");
        assert_eq!(world[0].newest.path, "paris/a.jpg");
        assert_eq!(places(&conn, 16, None, 72.0, None).unwrap().len(), 3, "and two places close up");
        let europe = Bounds { west: -10.0, south: 35.0, east: 30.0, north: 60.0 };
        assert_eq!(placed_items(&conn, europe, 10, None).unwrap().1, 2);
        // Across the date line.
        let pacific = Bounds { west: 150.0, south: -40.0, east: -170.0, north: 0.0 };
        assert_eq!(placed_items(&conn, pacific, 10, None).unwrap().0[0].path, "sydney.jpg");
        assert_eq!(place_extent(&conn, None).unwrap().unwrap().1, 3);

        let years = on_this_day(&conn, 10, 19, 2026, 10, None).unwrap();
        let summary: Vec<(i32, usize)> = years.iter().map(|(year, count, _)| (*year, *count)).collect();
        assert_eq!(summary, [(2023, 2), (2021, 1)], "not this year, and not file dates");
    }

    #[test]
    fn albums_nest_like_folders() {
        let conn = conn();
        let trips = create(&conn, None, "Trips", None).unwrap();
        let italy = create(&conn, Some(trips), "Italy", None).unwrap();
        let rome = create(&conn, Some(italy), "Rome", None).unwrap();
        assert_eq!(create(&conn, None, " trips ", None).unwrap_err(), AlbumError::Conflict("There is already an album called “Trips” here".into()));
        assert!(create(&conn, Some(trips), "Rome", None).is_ok(), "the same name can be used in another album");
        assert!(matches!(create(&conn, None, "  ", None), Err(AlbumError::Invalid(_))));
        assert_eq!(create(&conn, Some(9999), "Lost", None), Err(AlbumError::NotFound));

        assert_eq!(breadcrumb(&conn, rome).unwrap(), vec![(trips, "Trips".into()), (italy, "Italy".into()), (rome, "Rome".into())]);
        change_items(&conn, rome, &["rome.jpg".into()], true, "ana").unwrap();
        assert_eq!(get(&conn, trips).unwrap().cover.as_deref(), Some("rome.jpg"), "an album of albums borrows a cover");
        change_items(&conn, rome, &["rome.jpg".into()], false, "ana").unwrap();
        assert_eq!(children(&conn, None, None).unwrap().iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["Trips"]);
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
        assert_eq!(all(&conn, None).unwrap().len(), 1);
        let left: i64 = conn.query_row("SELECT count(*) FROM album_items", [], |row| row.get(0)).unwrap();
        assert_eq!(left, 0);
    }

    #[test]
    fn items_follow_their_files() {
        let conn = conn();
        let album = create(&conn, None, "Family", None).unwrap();
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
        assert_eq!(timeline(&conn, None, 10, None).unwrap().len(), 4);
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
        let first = timeline(&conn, None, 2, None).unwrap();
        assert_eq!(first.iter().map(|i| i.path.as_str()).collect::<Vec<_>>(), ["c.jpg", "b.jpg"]);
        let last = first.last().unwrap();
        let next = timeline(&conn, Some((last.taken_at.unwrap(), last.path.clone())), 10, None).unwrap();
        assert_eq!(next.iter().map(|i| i.path.as_str()).collect::<Vec<_>>(), ["d.jpg", "a.jpg"]);

        assert_eq!(days(&conn, None).unwrap(), vec![("1970-01-01".into(), 4)]);
        assert_eq!(taken_between(&conn, 20, 31, 10, None).unwrap().iter().map(|i| i.path.as_str()).collect::<Vec<_>>(), ["c.jpg", "b.jpg", "d.jpg"]);
        // Hidden photos leave the timeline (and its day counts) but keep their albums and tags.
        assert_eq!(set_hidden(&conn, &["b.jpg".into()], true, "ana").unwrap(), 1);
        assert_eq!(timeline(&conn, None, 10, None).unwrap().iter().map(|i| i.path.as_str()).collect::<Vec<_>>(), ["c.jpg", "d.jpg", "a.jpg"]);
        assert_eq!(days(&conn, None).unwrap(), vec![("1970-01-01".into(), 3)]);
        assert_eq!(hidden(&conn, None).unwrap().iter().map(|i| (i.path.as_str(), i.hidden)).collect::<Vec<_>>(), [("b.jpg", true)]);
        db::rewrite_path_meta(&conn, "b.jpg", "moved/b.jpg").unwrap();
        assert_eq!(hidden(&conn, None).unwrap()[0].path, "moved/b.jpg", "the flag follows the file");
        db::rewrite_path_meta(&conn, "moved/b.jpg", "b.jpg").unwrap();
        db::add_tag(&conn, "a.jpg", "Beach").unwrap();
        db::add_tag(&conn, "b.jpg", "beach").unwrap();
        assert_eq!(tags(&conn, None).unwrap(), vec![("Beach".into(), 2)]);
        let beach = tagged(&conn, "BEACH", None).unwrap();
        assert_eq!(beach.iter().map(|i| (i.path.as_str(), i.hidden)).collect::<Vec<_>>(), [("b.jpg", true), ("a.jpg", false)]);
        assert_eq!(set_hidden(&conn, &["b.jpg".into()], false, "ana").unwrap(), 1);
        assert_eq!(hidden_count(&conn, None).unwrap(), 0);
    }
}
