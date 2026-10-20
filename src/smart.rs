//! What the Gallery's machine learning finds, kept in OwnNAS's database: a description of each
//! photo (for search and "find similar"), and the faces in it, grouped into people. The ML
//! itself lives in `ml` (not built for Android); this module only stores and looks things up.

use std::sync::Arc;

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::db::now_secs;

/// A face joins a person when it is at least this close to the average of their faces.
pub const SAME_PERSON: f32 = 0.42;
/// People shown need at least this many faces (or a name): single faces are mostly strangers.
pub const MIN_FACES: i64 = 2;

const NOT_IN_TRASH: &str = "substr(path, 1, 14) <> '.ownnas-trash/' AND substr(path, 1, 6) <> 'Trash/'";

fn sql<T>(result: rusqlite::Result<T>) -> Result<T, String> {
    result.map_err(|err| format!("Could not read or update the photo index: {err}"))
}

pub fn similarity(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

pub fn normalize(vector: &mut [f32]) {
    let length = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
    if length > 0.0 {
        vector.iter_mut().for_each(|v| *v /= length);
    }
}

pub fn to_bytes(vector: &[f32]) -> Vec<u8> {
    vector.iter().flat_map(|value| value.to_le_bytes()).collect()
}

pub fn from_bytes(bytes: &[u8]) -> Vec<f32> {
    bytes.chunks_exact(4).map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])).collect()
}

/// A photo the index still has to look at.
#[derive(Clone, Debug)]
pub struct Pending {
    pub path: String,
    pub kind: String,
    pub size: i64,
    pub mtime: i64,
}

/// Photos and videos without an up-to-date description, newest first.
pub fn pending_descriptions(conn: &Connection, limit: usize) -> Result<Vec<Pending>, String> {
    pending(conn, "image_vectors", "", limit)
}

/// Photos not yet looked at for faces (videos are left out), newest first.
pub fn pending_faces(conn: &Connection, limit: usize) -> Result<Vec<Pending>, String> {
    pending(conn, "face_scans", "AND m.kind = 'image'", limit)
}

fn pending(conn: &Connection, table: &str, extra: &str, limit: usize) -> Result<Vec<Pending>, String> {
    let mut stmt = sql(conn.prepare(&format!(
        "SELECT m.path, m.kind, m.size, m.mtime FROM media_index m LEFT JOIN {table} d ON d.path = m.path
         WHERE (d.path IS NULL OR d.size <> m.size OR d.mtime <> m.mtime)
           AND substr(m.path, 1, 14) <> '.ownnas-trash/' AND substr(m.path, 1, 6) <> 'Trash/' {extra}
         ORDER BY m.taken_at DESC, m.path DESC LIMIT ?1"
    )))?;
    let rows = sql(stmt.query_map([limit as i64], |row| Ok(Pending { path: row.get(0)?, kind: row.get(1)?, size: row.get(2)?, mtime: row.get(3)? })))?;
    sql(rows.collect())
}

/// How many photos and videos there are, how many have a description, and how many of the
/// photos (videos are left out) have been looked at for faces, as (total, described, scanned, photos).
pub fn progress(conn: &Connection) -> Result<(i64, i64, i64), String> {
    let (total, described, scanned, _) = progress_with_photos(conn)?;
    Ok((total, described, scanned))
}

pub fn progress_with_photos(conn: &Connection) -> Result<(i64, i64, i64, i64), String> {
    sql(conn.query_row(
        &format!(
            "SELECT (SELECT count(*) FROM media_index WHERE {NOT_IN_TRASH}),
                    (SELECT count(*) FROM image_vectors v JOIN media_index m ON m.path = v.path AND m.size = v.size AND m.mtime = v.mtime),
                    (SELECT count(*) FROM face_scans f JOIN media_index m ON m.path = f.path AND m.size = f.size AND m.mtime = f.mtime),
                    (SELECT count(*) FROM media_index WHERE kind = 'image' AND {NOT_IN_TRASH})"
        ),
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    ))
}

pub fn store_description(conn: &Connection, item: &Pending, vector: &[f32]) -> Result<(), String> {
    sql(conn.execute(
        "INSERT OR REPLACE INTO image_vectors(path, size, mtime, vector, indexed_at) VALUES(?1, ?2, ?3, ?4, ?5)",
        params![item.path, item.size, item.mtime, to_bytes(vector), now_secs()],
    ))?;
    Ok(())
}

pub fn description(conn: &Connection, path: &str) -> Result<Option<Vec<f32>>, String> {
    let bytes: Option<Vec<u8>> = sql(conn.query_row("SELECT vector FROM image_vectors WHERE path = ?1", [path], |row| row.get(0)).optional())?;
    Ok(bytes.map(|bytes| from_bytes(&bytes)))
}

/// Every photo's description, for searching in memory.
pub struct Descriptions {
    pub paths: Vec<String>,
    /// All vectors one after another (512 numbers each).
    pub vectors: Vec<f32>,
    pub width: usize,
}

pub fn load_descriptions(conn: &Connection) -> Result<Descriptions, String> {
    let mut stmt = sql(conn.prepare(&format!("SELECT path, vector FROM image_vectors WHERE {NOT_IN_TRASH}")))?;
    let rows = sql(stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))))?;
    let mut out = Descriptions { paths: Vec::new(), vectors: Vec::new(), width: 0 };
    for row in rows {
        let (path, bytes) = sql(row)?;
        let vector = from_bytes(&bytes);
        // Photos that could not be read are kept with an empty description, so they are not
        // tried again and again.
        if vector.is_empty() {
            continue;
        }
        if out.width == 0 {
            out.width = vector.len();
        }
        if vector.len() != out.width {
            continue;
        }
        out.paths.push(path);
        out.vectors.extend(vector);
    }
    Ok(out)
}

/// The closest photos to `query`, best first, with their scores.
pub fn nearest(descriptions: &Descriptions, query: &[f32], limit: usize, skip: Option<&str>) -> Vec<(String, f32)> {
    nearest_where(descriptions, query, limit, &|path| skip != Some(path))
}

/// The closest photos to `query` among those `keep` accepts (a workspace's), best first.
pub fn nearest_where(descriptions: &Descriptions, query: &[f32], limit: usize, keep: &dyn Fn(&str) -> bool) -> Vec<(String, f32)> {
    if descriptions.width == 0 || query.len() != descriptions.width {
        return Vec::new();
    }
    let mut scored: Vec<(usize, f32)> = descriptions
        .vectors
        .chunks_exact(descriptions.width)
        .enumerate()
        .filter(|(index, _)| keep(&descriptions.paths[*index]))
        .map(|(index, vector)| (index, similarity(vector, query)))
        .collect();
    let keep = limit.min(scored.len());
    if keep == 0 {
        return Vec::new();
    }
    scored.select_nth_unstable_by(keep - 1, |a, b| b.1.total_cmp(&a.1));
    scored.truncate(keep);
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    scored.into_iter().map(|(index, score)| (descriptions.paths[index].clone(), score)).collect()
}

/// How far above a neutral phrase ("a photo") a photo must score to count as matching words.
/// Every picture scores about the same against any phrase; what stands out is the difference.
pub const TEXT_MARGIN: f32 = 0.012;

/// Photos that match words: those scoring clearly above `neutral`, best first.
pub fn matching_text(descriptions: &Descriptions, query: &[f32], neutral: &[f32], limit: usize) -> Vec<(String, f32)> {
    matching_text_where(descriptions, query, neutral, limit, &|_| true)
}

/// Photos that match words, among those `keep` accepts (a workspace's).
pub fn matching_text_where(descriptions: &Descriptions, query: &[f32], neutral: &[f32], limit: usize, keep: &dyn Fn(&str) -> bool) -> Vec<(String, f32)> {
    if descriptions.width == 0 || query.len() != descriptions.width || neutral.len() != descriptions.width {
        return Vec::new();
    }
    let mut found: Vec<(usize, f32)> = descriptions
        .vectors
        .chunks_exact(descriptions.width)
        .enumerate()
        .filter(|(index, _)| keep(&descriptions.paths[*index]))
        .filter_map(|(index, vector)| {
            let score = similarity(vector, query);
            (score - similarity(vector, neutral) >= TEXT_MARGIN).then_some((index, score))
        })
        .collect();
    found.sort_by(|a, b| b.1.total_cmp(&a.1));
    found.truncate(limit);
    found.into_iter().map(|(index, score)| (descriptions.paths[index].clone(), score)).collect()
}

// ---- Faces and people ----

/// A face found in a photo: its box as fractions of the picture, how sure the finder was, and
/// its 128 numbers.
pub struct FoundFace {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub score: f32,
    pub vector: Vec<f32>,
}

/// The people's average faces, kept in memory while faces are being sorted.
#[derive(Default)]
pub struct PeopleIndex {
    people: Vec<(i64, Vec<f32>, i64)>,
}

impl PeopleIndex {
    pub fn load(conn: &Connection) -> Result<Self, String> {
        let mut stmt = sql(conn.prepare("SELECT id, centroid, faces FROM people WHERE centroid IS NOT NULL"))?;
        let rows = sql(stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?, row.get::<_, i64>(2)?))))?;
        let mut people = Vec::new();
        for row in rows {
            let (id, centroid, count) = sql(row)?;
            people.push((id, from_bytes(&centroid), count));
        }
        Ok(Self { people })
    }

    fn closest(&self, vector: &[f32]) -> Option<(usize, f32)> {
        self.people
            .iter()
            .enumerate()
            .map(|(index, (_, centroid, _))| (index, similarity(centroid, vector)))
            .max_by(|a, b| a.1.total_cmp(&b.1))
    }
}

/// Saves the faces found in a photo (replacing any found before) and puts each one with the
/// person it looks like, or starts a new person.
pub fn store_faces(conn: &Connection, item: &Pending, found: &[FoundFace], people: &mut PeopleIndex) -> Result<(), String> {
    let tx = sql(conn.unchecked_transaction())?;
    sql(tx.execute("DELETE FROM faces WHERE path = ?1", [&item.path]))?;
    let now = now_secs();
    for face in found {
        let person = match people.closest(&face.vector) {
            Some((index, score)) if score >= SAME_PERSON => {
                let (id, centroid, count) = &mut people.people[index];
                // The person's average moves a little toward the new face.
                for (value, new) in centroid.iter_mut().zip(&face.vector) {
                    *value = *value * *count as f32 + new;
                }
                normalize(centroid);
                *count += 1;
                sql(tx.execute(
                    "UPDATE people SET centroid = ?1, faces = faces + 1, updated_at = ?2 WHERE id = ?3",
                    params![to_bytes(centroid), now, *id],
                ))?;
                *id
            }
            _ => {
                sql(tx.execute(
                    "INSERT INTO people(name, centroid, faces, created_at, updated_at) VALUES(NULL, ?1, 1, ?2, ?2)",
                    params![to_bytes(&face.vector), now],
                ))?;
                let id = tx.last_insert_rowid();
                people.people.push((id, face.vector.clone(), 1));
                id
            }
        };
        sql(tx.execute(
            "INSERT INTO faces(path, x, y, w, h, score, vector, person_id, created_at) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![item.path, face.x, face.y, face.w, face.h, face.score, to_bytes(&face.vector), person, now],
        ))?;
    }
    sql(tx.execute(
        "INSERT OR REPLACE INTO face_scans(path, size, mtime, faces, scanned_at) VALUES(?1, ?2, ?3, ?4, ?5)",
        params![item.path, item.size, item.mtime, found.len() as i64, now],
    ))?;
    sql(tx.commit())
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Person {
    pub id: i64,
    pub name: Option<String>,
    pub faces: i64,
    pub photos: i64,
    pub hidden: bool,
    /// The clearest face, to show the person by.
    pub cover_face: Option<i64>,
}

/// The people worth showing: named, or seen at least twice. Hidden ones only when asked.
pub fn people(conn: &Connection, hidden: bool) -> Result<Vec<Person>, String> {
    people_in(conn, hidden, None)
}

/// The people worth showing who are in a workspace's photos, with their photos there.
pub fn people_in(conn: &Connection, hidden: bool, workspace: Option<i64>) -> Result<Vec<Person>, String> {
    let in_workspace = crate::workspaces::filter(workspace, "f.path");
    let mut stmt = sql(conn.prepare(&format!(
        "SELECT * FROM (SELECT p.id, p.name, p.faces,
            (SELECT count(DISTINCT path) FROM faces f WHERE f.person_id = p.id AND {NOT_IN_TRASH} AND {in_workspace}) AS photos,
            p.hidden,
            (SELECT id FROM faces f WHERE f.person_id = p.id AND {NOT_IN_TRASH} AND {in_workspace} ORDER BY f.score * f.w DESC LIMIT 1)
         FROM people p
         WHERE p.hidden = ?1 AND (p.name IS NOT NULL OR p.faces >= ?2)
         ORDER BY p.name IS NULL, p.faces DESC, p.id)
         WHERE photos > 0 OR ?3"
    )))?;
    let rows = sql(stmt.query_map(params![hidden as i64, MIN_FACES, workspace.is_none()], |row| {
        Ok(Person { id: row.get(0)?, name: row.get(1)?, faces: row.get(2)?, photos: row.get(3)?, hidden: row.get::<_, i64>(4)? != 0, cover_face: row.get(5)? })
    }))?;
    sql(rows.collect())
}

pub fn person(conn: &Connection, id: i64) -> Result<Option<Person>, String> {
    let all = people(conn, false)?.into_iter().chain(people(conn, true)?);
    let found = all.into_iter().find(|person| person.id == id);
    if found.is_some() {
        return Ok(found);
    }
    // A person below the showing threshold still exists (after a correction, say).
    sql(conn
        .query_row("SELECT id, name, faces, hidden FROM people WHERE id = ?1", [id], |row| {
            Ok(Person { id: row.get(0)?, name: row.get(1)?, faces: row.get(2)?, photos: row.get(2)?, hidden: row.get::<_, i64>(3)? != 0, cover_face: None })
        })
        .optional())
}

/// The photos a person is in, newest first, with the face that is them in each.
pub fn person_photos(conn: &Connection, id: i64) -> Result<Vec<(crate::albums::MediaItem, i64)>, String> {
    person_photos_in(conn, id, None)
}

/// The photos a person is in within a workspace.
pub fn person_photos_in(conn: &Connection, id: i64, workspace: Option<i64>) -> Result<Vec<(crate::albums::MediaItem, i64)>, String> {
    let mut stmt = sql(conn.prepare(&format!(
        "SELECT f.path, m.kind, m.taken_at, m.date_source, m.width, m.height, m.size,
                EXISTS(SELECT 1 FROM timeline_hidden h WHERE h.path = f.path), min(f.id)
         FROM faces f LEFT JOIN media_index m ON m.path = f.path
         WHERE f.person_id = ?1 AND {} AND {}
         GROUP BY f.path ORDER BY m.taken_at DESC, f.path DESC",
        NOT_IN_TRASH.replace("path", "f.path"),
        crate::workspaces::filter(workspace, "f.path")
    )))?;
    let rows = sql(stmt.query_map([id], |row| {
        Ok((
            crate::albums::MediaItem {
                path: row.get(0)?,
                kind: row.get(1)?,
                taken_at: row.get(2)?,
                date_source: row.get(3)?,
                width: row.get(4)?,
                height: row.get(5)?,
                size: row.get(6)?,
                hidden: row.get(7)?,
            },
            row.get(8)?,
        ))
    }))?;
    sql(rows.collect())
}

/// The people (if any) in a photo, for the preview.
pub fn faces_in(conn: &Connection, path: &str) -> Result<Vec<(i64, Option<i64>, Option<String>)>, String> {
    let mut stmt = sql(conn.prepare(
        "SELECT f.id, f.person_id, p.name FROM faces f LEFT JOIN people p ON p.id = f.person_id WHERE f.path = ?1 ORDER BY f.x",
    ))?;
    let rows = sql(stmt.query_map([path], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))))?;
    sql(rows.collect())
}

/// Where a face is: its photo and box (fractions of the picture).
pub fn face_box(conn: &Connection, id: i64) -> Result<Option<(String, f32, f32, f32, f32)>, String> {
    sql(conn
        .query_row("SELECT path, x, y, w, h FROM faces WHERE id = ?1", [id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)))
        .optional())
}

pub fn rename_person(conn: &Connection, id: i64, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.chars().count() > 80 || name.chars().any(char::is_control) {
        return Err("Names can be up to 80 characters".into());
    }
    let name = (!name.is_empty()).then_some(name);
    if sql(conn.execute("UPDATE people SET name = ?1, updated_at = ?2 WHERE id = ?3", params![name, now_secs(), id]))? == 0 {
        return Err("That person is no longer there".into());
    }
    Ok(())
}

pub fn hide_person(conn: &Connection, id: i64, hidden: bool) -> Result<(), String> {
    sql(conn.execute("UPDATE people SET hidden = ?1, updated_at = ?2 WHERE id = ?3", params![hidden as i64, now_secs(), id]))?;
    Ok(())
}

/// Two groups that are the same person become one (keeping `into`'s name, or `from`'s).
pub fn merge_people(conn: &Connection, from: i64, into: i64) -> Result<(), String> {
    if from == into {
        return Ok(());
    }
    let tx = sql(conn.unchecked_transaction())?;
    sql(tx.execute("UPDATE faces SET person_id = ?1, manual = 1 WHERE person_id = ?2", params![into, from]))?;
    sql(tx.execute(
        "UPDATE people SET name = coalesce(name, (SELECT name FROM people WHERE id = ?2)), updated_at = ?3 WHERE id = ?1",
        params![into, from, now_secs()],
    ))?;
    sql(tx.execute("DELETE FROM people WHERE id = ?1", [from]))?;
    recount(&tx, into)?;
    sql(tx.commit())
}

/// Moves faces to another person, or (with `None`) out to a new person of their own: for
/// "this is not them" corrections, which later sorting leaves alone.
pub fn move_faces(conn: &Connection, faces: &[i64], to: Option<i64>) -> Result<i64, String> {
    let tx = sql(conn.unchecked_transaction())?;
    let target = match to {
        Some(id) => id,
        None => {
            sql(tx.execute("INSERT INTO people(name, faces, created_at, updated_at) VALUES(NULL, 0, ?1, ?1)", [now_secs()]))?;
            tx.last_insert_rowid()
        }
    };
    let mut touched = vec![target];
    for face in faces {
        let before: Option<Option<i64>> = sql(tx.query_row("SELECT person_id FROM faces WHERE id = ?1", [face], |row| row.get(0)).optional())?;
        if let Some(Some(person)) = before {
            touched.push(person);
        }
        sql(tx.execute("UPDATE faces SET person_id = ?1, manual = 1 WHERE id = ?2", params![target, face]))?;
    }
    touched.sort_unstable();
    touched.dedup();
    for person in touched {
        recount(&tx, person)?;
    }
    sql(tx.execute("DELETE FROM people WHERE faces = 0 AND name IS NULL", []))?;
    sql(tx.commit())?;
    Ok(target)
}

/// A person's face count and average face, worked out again from their faces.
fn recount(conn: &Connection, id: i64) -> Result<(), String> {
    let mut stmt = sql(conn.prepare("SELECT vector FROM faces WHERE person_id = ?1"))?;
    let rows = sql(stmt.query_map([id], |row| row.get::<_, Vec<u8>>(0)))?;
    let mut sum: Vec<f32> = Vec::new();
    let mut count = 0i64;
    for row in rows {
        let vector = from_bytes(&sql(row)?);
        if sum.is_empty() {
            sum = vec![0.0; vector.len()];
        }
        for (total, value) in sum.iter_mut().zip(&vector) {
            *total += value;
        }
        count += 1;
    }
    normalize(&mut sum);
    let centroid = (count > 0).then(|| to_bytes(&sum));
    sql(conn.execute("UPDATE people SET faces = ?1, centroid = ?2, updated_at = ?3 WHERE id = ?4", params![count, centroid, now_secs(), id]))?;
    Ok(())
}

/// Forgets the descriptions and faces of photos no longer in the index (deleted or moved outside
/// OwnNAS), so "find similar" and People do not show them. True when something was forgotten.
pub fn forget_unindexed(conn: &Connection) -> Result<bool, String> {
    let gone = "NOT IN (SELECT path FROM media_index)";
    let mut forgotten = sql(conn.execute(&format!("DELETE FROM image_vectors WHERE path {gone}"), []))?;
    forgotten += sql(conn.execute(&format!("DELETE FROM face_scans WHERE path {gone}"), []))?;
    let faces = sql(conn.execute(&format!("DELETE FROM faces WHERE path {gone}"), []))?;
    if faces > 0 {
        sql(conn.execute_batch(
            "UPDATE people SET faces = (SELECT count(*) FROM faces WHERE person_id = people.id);
             DELETE FROM people WHERE faces = 0 AND name IS NULL;",
        ))?;
    }
    Ok(forgotten + faces > 0)
}

/// Forgets every face and person (the photos are not touched).
pub fn forget_faces(conn: &Connection) -> Result<(), String> {
    sql(conn.execute_batch("DELETE FROM faces; DELETE FROM people; DELETE FROM face_scans;"))
}

/// The in-memory copy of all descriptions, reloaded when they change.
#[derive(Default)]
pub struct DescriptionCache {
    cached: std::sync::Mutex<Option<(u64, Arc<Descriptions>)>>,
    version: std::sync::atomic::AtomicU64,
}

impl DescriptionCache {
    /// Marks the copy as stale (descriptions were added, or files moved).
    pub fn invalidate(&self) {
        self.version.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn get(&self, conn: &Connection) -> Result<Arc<Descriptions>, String> {
        let version = self.version.load(std::sync::atomic::Ordering::Relaxed);
        let mut cached = self.cached.lock().unwrap_or_else(|err| err.into_inner());
        if let Some((at, descriptions)) = cached.as_ref() {
            if *at == version {
                return Ok(descriptions.clone());
            }
        }
        let descriptions = Arc::new(load_descriptions(conn)?);
        *cached = Some((version, descriptions.clone()));
        Ok(descriptions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn conn() -> Connection {
        let path = std::env::temp_dir().join(format!("ownnas-smart-{}.sqlite", rand::random::<u64>()));
        let conn = db::open(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        conn
    }

    fn unit(values: &[f32]) -> Vec<f32> {
        let mut vector = values.to_vec();
        normalize(&mut vector);
        vector
    }

    fn index(conn: &Connection, path: &str) -> Pending {
        conn.execute(
            "INSERT OR REPLACE INTO media_index(path, kind, size, mtime, taken_at, date_source, indexed_at) VALUES(?1, 'image', 10, 20, 30, 'file', 1)",
            [path],
        )
        .unwrap();
        Pending { path: path.into(), kind: "image".into(), size: 10, mtime: 20 }
    }

    fn face(vector: Vec<f32>) -> FoundFace {
        FoundFace { x: 0.1, y: 0.1, w: 0.2, h: 0.2, score: 0.9, vector }
    }

    #[test]
    fn descriptions_are_searched_and_follow_files() {
        let conn = conn();
        let beach = index(&conn, "trip/beach.jpg");
        let snow = index(&conn, "trip/snow.jpg");
        assert_eq!(pending_descriptions(&conn, 10).unwrap().len(), 2);
        store_description(&conn, &beach, &unit(&[1.0, 0.1, 0.0])).unwrap();
        store_description(&conn, &snow, &unit(&[0.0, 0.2, 1.0])).unwrap();
        assert!(pending_descriptions(&conn, 10).unwrap().is_empty());
        let cache = DescriptionCache::default();
        let neutral = unit(&[0.5, 0.5, 0.5]);
        let words = matching_text(&cache.get(&conn).unwrap(), &unit(&[1.0, 0.0, 0.0]), &neutral, 10);
        assert_eq!(words.iter().map(|(path, _)| path.as_str()).collect::<Vec<_>>(), ["trip/beach.jpg"], "only what stands out");
        let found = nearest(&cache.get(&conn).unwrap(), &unit(&[0.9, 0.0, 0.1]), 10, None);
        assert_eq!(found[0].0, "trip/beach.jpg");
        assert!(found[0].1 > found[1].1);
        assert_eq!(nearest(&cache.get(&conn).unwrap(), &unit(&[1.0, 0.1, 0.0]), 10, Some("trip/beach.jpg"))[0].0, "trip/snow.jpg");
        // Moves keep the description; a changed file is looked at again.
        db::rewrite_path_meta(&conn, "trip", "travel").unwrap();
        cache.invalidate();
        assert_eq!(nearest(&cache.get(&conn).unwrap(), &unit(&[1.0, 0.0, 0.0]), 1, None)[0].0, "travel/beach.jpg");
        conn.execute("UPDATE media_index SET size = 99 WHERE path = 'travel/snow.jpg'", []).unwrap();
        assert_eq!(pending_descriptions(&conn, 10).unwrap()[0].path, "travel/snow.jpg");
        assert_eq!(progress(&conn).unwrap(), (2, 1, 0));
    }

    #[test]
    fn faces_group_into_people_and_corrections_stick() {
        let conn = conn();
        let ana = unit(&[1.0, 0.05, 0.0, 0.0]);
        let ana_again = unit(&[0.95, 0.1, 0.05, 0.0]);
        let bruno = unit(&[0.0, 0.0, 1.0, 0.1]);
        let mut people_index = PeopleIndex::load(&conn).unwrap();
        let first = index(&conn, "a.jpg");
        store_faces(&conn, &first, &[face(ana.clone()), face(bruno.clone())], &mut people_index).unwrap();
        let second = index(&conn, "b.jpg");
        store_faces(&conn, &second, &[face(ana_again)], &mut people_index).unwrap();
        // Ana is in two photos (shown); Bruno only once (not shown until named).
        let shown = people(&conn, false).unwrap();
        assert_eq!(shown.len(), 1);
        assert_eq!((shown[0].faces, shown[0].photos), (2, 2));
        let ana_id = shown[0].id;
        rename_person(&conn, ana_id, " Ana ").unwrap();
        assert_eq!(people(&conn, false).unwrap()[0].name.as_deref(), Some("Ana"));
        assert_eq!(person_photos(&conn, ana_id).unwrap().len(), 2);
        let in_first = faces_in(&conn, "a.jpg").unwrap();
        assert_eq!(in_first.len(), 2);

        // "Not Ana": the face moves out to a person of its own.
        let (_, face_id) = person_photos(&conn, ana_id).unwrap().into_iter().find(|(item, _)| item.path == "b.jpg").unwrap();
        let moved_to = move_faces(&conn, &[face_id], None).unwrap();
        assert_ne!(moved_to, ana_id);
        assert_eq!(person(&conn, ana_id).unwrap().unwrap().faces, 1);
        // Merging puts them back together.
        merge_people(&conn, moved_to, ana_id).unwrap();
        assert_eq!(person(&conn, ana_id).unwrap().unwrap().faces, 2);
        assert!(person(&conn, moved_to).unwrap().is_none());

        // Faces follow their photos, and go with them.
        db::rewrite_path_meta(&conn, "b.jpg", "kept/b.jpg").unwrap();
        assert!(person_photos(&conn, ana_id).unwrap().iter().any(|(item, _)| item.path == "kept/b.jpg"));
        db::delete_path_meta(&conn, "kept/b.jpg").unwrap();
        assert_eq!(person(&conn, ana_id).unwrap().unwrap().faces, 1);
        hide_person(&conn, ana_id, true).unwrap();
        assert!(people(&conn, false).unwrap().is_empty());
        assert_eq!(people(&conn, true).unwrap()[0].id, ana_id);
        forget_faces(&conn).unwrap();
        assert!(people(&conn, true).unwrap().is_empty());
        assert_eq!(pending_faces(&conn, 10).unwrap().len(), 1);
    }
}
