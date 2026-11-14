//! Gallery tools for `/mcp`: smart search, similar photos, albums, people, and photos by date.
//!
//! Only photos inside the key's folders are returned or counted. An album or a person whose
//! photos are all elsewhere is not listed, so a key learns nothing about the rest of the library.

use crate::agent_keys::{self, AgentKey};
use crate::albums::MediaItem;
use crate::http::AppState;
use crate::mcp::{allowed, as_text, blocking, failed, Args, Stop};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;

const PHOTOS_DEFAULT: u64 = 50;
const PHOTOS_MOST: u64 = 500;
/// How many photos a date range looks through before keeping the key's.
const TAKEN_SCAN: usize = 20_000;

pub fn tools() -> Vec<Value> {
    let read_only = json!({ "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false });
    let paging = |about: &str| {
        json!({
            "offset": { "type": "integer", "minimum": 0, "description": "How many to skip." },
            "limit": { "type": "integer", "minimum": 1, "maximum": PHOTOS_MOST, "description": about },
        })
    };
    let with = |mut base: Value, extra: Value| {
        if let (Some(base), Some(extra)) = (base.as_object_mut(), extra.as_object()) {
            base.extend(extra.clone());
        }
        base
    };
    let folder = json!({ "type": "string", "description": "Only photos in this folder. Leave out for every folder this key can use." });
    vec![
        json!({
            "name": "photo_search",
            "title": "Find photos by what they show",
            "description": "Finds photos and videos that show what the words describe, such as \"dog on a beach\", \"birthday cake\" or \"whiteboard\", best match first. Works when smart search is on in OwnNAS.",
            "inputSchema": { "type": "object", "required": ["query"], "properties": with(json!({
                "query": { "type": "string", "description": "What the photo shows." },
                "path": folder,
            }), paging("How many photos to return.")) },
            "annotations": read_only,
        }),
        json!({
            "name": "similar_photos",
            "title": "Find similar photos",
            "description": "Finds photos that look like the given one, most alike first.",
            "inputSchema": { "type": "object", "required": ["path"], "properties": with(json!({
                "path": { "type": "string", "description": "The photo to compare with." },
            }), paging("How many photos to return.")) },
            "annotations": read_only,
        }),
        json!({
            "name": "photos_taken",
            "title": "Photos taken between dates",
            "description": "Lists the photos and videos taken between two dates (inclusive), newest first.",
            "inputSchema": { "type": "object", "required": ["from", "to"], "properties": with(json!({
                "from": { "type": "string", "description": "First day, as YYYY-MM-DD." },
                "to": { "type": "string", "description": "Last day, as YYYY-MM-DD." },
                "path": folder,
            }), paging("How many photos to return.")) },
            "annotations": read_only,
        }),
        json!({
            "name": "list_albums",
            "title": "List photo albums",
            "description": "Lists the Gallery albums that have photos in this key's folders, with how many of those photos each has.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": read_only,
        }),
        json!({
            "name": "album_photos",
            "title": "Photos in an album",
            "description": "Lists the photos in an album, newest first.",
            "inputSchema": { "type": "object", "required": ["album"], "properties": with(json!({
                "album": { "type": "integer", "description": "The album's id, from list_albums." },
            }), paging("How many photos to return.")) },
            "annotations": read_only,
        }),
        json!({
            "name": "list_people",
            "title": "List people in photos",
            "description": "Lists the people OwnNAS recognised in photos in this key's folders: their name when someone gave one, and how many photos they are in.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": read_only,
        }),
        json!({
            "name": "person_photos",
            "title": "Photos of a person",
            "description": "Lists the photos a person is in, newest first.",
            "inputSchema": { "type": "object", "required": ["person"], "properties": with(json!({
                "person": { "type": "integer", "description": "The person's id, from list_people." },
            }), paging("How many photos to return.")) },
            "annotations": read_only,
        }),
    ]
}

pub(crate) async fn call(state: &Arc<AppState>, key: &AgentKey, name: &str, args: &Args<'_>) -> Option<Result<Vec<Value>, Stop>> {
    let page = (args.number("offset").unwrap_or(0) as usize, args.number("limit").unwrap_or(PHOTOS_DEFAULT).clamp(1, PHOTOS_MOST) as usize);
    Some(match name {
        "photo_search" => photo_search(state, key, &args.text("query"), &args.path(), page).await,
        "similar_photos" => similar_photos(state, key, &args.path(), page).await,
        "photos_taken" => photos_taken(state, key, &args.text("from"), &args.text("to"), &args.path(), page).await,
        "list_albums" => list_albums(state, key).await,
        "album_photos" => album_photos(state, key, args.integer("album"), page).await,
        "list_people" => list_people(state, key).await,
        "person_photos" => person_photos(state, key, args.integer("person"), page).await,
        _ => return None,
    })
}

/// Whether a library path is under one of `folders`, by name only. A quick first pass:
/// `agent_keys::check` has the last word on anything that is returned.
fn under(scope: &agent_keys::Scope, path: &str) -> bool {
    let shown = !path.split('/').any(|part| part.starts_with('.'));
    shown && scope.holds_name(path)
}

/// The items the key may see, in their order.
fn visible(root: &Path, scope: &agent_keys::Scope, items: Vec<MediaItem>) -> Vec<MediaItem> {
    items.into_iter().filter(|item| under(scope, &item.path) && agent_keys::check(root, scope, &item.path).is_some()).collect()
}

/// The folders to look in: the key's, or one of them when `path` is given.
async fn places(state: &Arc<AppState>, key: &AgentKey, path: &str) -> Result<agent_keys::Scope, Stop> {
    if path.is_empty() {
        return Ok(key.scope());
    }
    let found = allowed(state, key, path).await?;
    if !found.full.is_dir() {
        return failed("That is a file, not a folder.");
    }
    Ok(agent_keys::Scope { folders: vec![found.rel], ..key.scope() })
}

pub fn photo_json(item: &MediaItem) -> Value {
    let taken = item.taken_at.map(|at| match item.date_source.as_deref() {
        Some("exif") => crate::photo_details::wall_clock(at),
        _ => format!("{} UTC", crate::photo_details::wall_clock(at)),
    });
    json!({ "path": item.path, "kind": item.kind, "taken": taken, "width": item.width, "height": item.height, "size": item.size })
}

fn page_of(items: Vec<Value>, (offset, limit): (usize, usize), extra: Value) -> Vec<Value> {
    let more = items.len() > offset + limit;
    let shown: Vec<Value> = items.into_iter().skip(offset).take(limit).collect();
    let mut answer = json!({ "photos": shown, "more": more });
    if let (Some(answer), Some(extra)) = (answer.as_object_mut(), extra.as_object()) {
        answer.extend(extra.clone());
    }
    as_text(&answer)
}

fn media_items(state: &AppState, paths: &[String]) -> Vec<MediaItem> {
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    crate::albums::media_items(&conn, paths).unwrap_or_default()
}

/// Photos for (path, score) pairs, best first, with the score.
fn scored(state: &AppState, root: &Path, folders: &agent_keys::Scope, found: Vec<(String, f32)>) -> Vec<Value> {
    let paths: Vec<String> = found.iter().map(|(path, _)| path.clone()).collect();
    let scores: std::collections::HashMap<String, f32> = found.into_iter().collect();
    visible(root, folders, media_items(state, &paths))
        .iter()
        .map(|item| {
            let mut photo = photo_json(item);
            photo["score"] = json!((scores.get(&item.path).copied().unwrap_or(0.0) * 1000.0).round() / 1000.0);
            photo
        })
        .collect()
}

#[cfg(not(ownnas_ml))]
async fn photo_search(_: &Arc<AppState>, _: &AgentKey, _: &str, _: &str, _: (usize, usize)) -> Result<Vec<Value>, Stop> {
    failed("Finding photos by what they show is not available on this server.")
}

#[cfg(not(ownnas_ml))]
async fn similar_photos(_: &Arc<AppState>, _: &AgentKey, _: &str, _: (usize, usize)) -> Result<Vec<Value>, Stop> {
    failed("Finding similar photos is not available on this server.")
}

#[cfg(ownnas_ml)]
const SEARCH_OFF: &str = "Smart search is off in OwnNAS. An administrator can turn it on in Settings → Gallery; until then, use search to find photos by name or tag.";

#[cfg(ownnas_ml)]
async fn photo_search(state: &Arc<AppState>, key: &AgentKey, query: &str, path: &str, page: (usize, usize)) -> Result<Vec<Value>, Stop> {
    let text: String = query.trim().chars().take(300).collect();
    if text.is_empty() {
        return failed("Say what the photos show.");
    }
    if !crate::http::smart_search_on(state) {
        return failed(SEARCH_OFF);
    }
    let folders = places(state, key, path).await?;
    let (worker, root) = (state.clone(), state.root.clone());
    blocking(move || {
        let fail = |_| Stop::Failed("The search could not be finished.".to_string());
        let vector = worker.models.describe_text(&text).map_err(fail)?;
        let neutral = worker.models.neutral_text().map_err(fail)?;
        let descriptions = {
            let conn = worker.db.lock().unwrap_or_else(|err| err.into_inner());
            worker.media.descriptions.get(&conn).map_err(fail)?
        };
        let keep = |path: &str| under(&folders, path);
        let found = crate::smart::matching_text_where(&descriptions, &vector, &neutral, page.0 + page.1 + 1, &keep);
        Ok(page_of(scored(&worker, &root, &folders, found), page, json!({ "query": text })))
    })
    .await
}

#[cfg(ownnas_ml)]
async fn similar_photos(state: &Arc<AppState>, key: &AgentKey, path: &str, page: (usize, usize)) -> Result<Vec<Value>, Stop> {
    let sample = allowed(state, key, path).await?;
    if !crate::http::smart_search_on(state) {
        return failed(SEARCH_OFF);
    }
    let (worker, root, folders) = (state.clone(), state.root.clone(), key.scope());
    blocking(move || {
        let stored = {
            let conn = worker.db.lock().unwrap_or_else(|err| err.into_inner());
            crate::smart::description(&conn, &sample.rel).ok().flatten().filter(|vector| !vector.is_empty())
        };
        let vector = match stored {
            Some(vector) => vector,
            None => {
                let image = crate::thumbs::decode_for_analysis(&sample.full, 640).map_err(|_| Stop::Failed("That is not a photo OwnNAS can read.".to_string()))?;
                worker.models.describe_image(&image, 2).map_err(|_| Stop::Failed("Could not look at that photo.".to_string()))?
            }
        };
        let descriptions = {
            let conn = worker.db.lock().unwrap_or_else(|err| err.into_inner());
            worker.media.descriptions.get(&conn).map_err(|_| Stop::Failed("The search could not be finished.".to_string()))?
        };
        let keep = |candidate: &str| candidate != sample.rel && under(&folders, candidate);
        let found: Vec<_> = crate::smart::nearest_where(&descriptions, &vector, page.0 + page.1 + 1, &keep).into_iter().filter(|(_, score)| *score >= 0.6).collect();
        Ok(page_of(scored(&worker, &root, &folders, found), page, json!({ "like": sample.rel })))
    })
    .await
}

/// Seconds for the start of a YYYY-MM-DD day (photo dates are kept as wall-clock seconds).
fn day_start(text: &str) -> Option<i64> {
    let day = chrono::NaiveDate::parse_from_str(text.trim(), "%Y-%m-%d").ok()?;
    Some(day.and_hms_opt(0, 0, 0)?.and_utc().timestamp())
}

async fn photos_taken(state: &Arc<AppState>, key: &AgentKey, from: &str, to: &str, path: &str, page: (usize, usize)) -> Result<Vec<Value>, Stop> {
    let (Some(start), Some(end)) = (day_start(from), day_start(to)) else {
        return failed("Give the dates as YYYY-MM-DD.");
    };
    if end < start {
        return failed("The last day comes before the first.");
    }
    let folders = places(state, key, path).await?;
    let (worker, root) = (state.clone(), state.root.clone());
    blocking(move || {
        let items = {
            let conn = worker.db.lock().unwrap_or_else(|err| err.into_inner());
            crate::albums::taken_between(&conn, start, end + 86_399, TAKEN_SCAN, None).map_err(|_| Stop::Failed("Could not list the photos.".to_string()))?
        };
        let items: Vec<MediaItem> = items.into_iter().filter(|item| under(&folders, &item.path)).collect();
        // Checking every photo is slow for a large range: only the ones on this page.
        let wanted = page.0 + page.1 + 1;
        let shown = visible(&root, &folders, items.into_iter().take(wanted * 2).collect());
        Ok(page_of(shown.iter().map(photo_json).collect(), page, json!({ "from": from_text(start), "to": from_text(end) })))
    })
    .await
}

fn from_text(seconds: i64) -> String {
    crate::photo_details::wall_clock(seconds)[..10].to_string()
}

/// The albums' photos that the key may see, by album id, for the albums that have any.
fn albums_seen(state: &AppState, folders: &agent_keys::Scope) -> Result<Vec<(crate::albums::Album, Vec<MediaItem>)>, Stop> {
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    let fail = |_| Stop::Failed("Could not list the albums.".to_string());
    let mut seen = Vec::new();
    for album in crate::albums::all(&conn, None).map_err(fail)? {
        let items: Vec<MediaItem> = crate::albums::items(&conn, album.id).map_err(fail)?.into_iter().filter(|item| under(folders, &item.path)).collect();
        if !items.is_empty() {
            seen.push((album, items));
        }
    }
    Ok(seen)
}

/// "Trips / Italy" for an album inside another.
fn album_name(state: &AppState, id: i64) -> String {
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    crate::albums::breadcrumb(&conn, id).map(|parts| parts.into_iter().map(|(_, name)| name).collect::<Vec<_>>().join(" / ")).unwrap_or_default()
}

async fn list_albums(state: &Arc<AppState>, key: &AgentKey) -> Result<Vec<Value>, Stop> {
    let (worker, folders) = (state.clone(), key.scope());
    blocking(move || {
        let albums: Vec<Value> = albums_seen(&worker, &folders)?
            .into_iter()
            .map(|(album, items)| json!({ "id": album.id, "name": album_name(&worker, album.id), "photos": items.len() }))
            .collect();
        Ok(as_text(&json!({ "albums": albums })))
    })
    .await
}

/// An album the key may use: one with photos in its folders. `None` for any other, as for
/// one that does not exist.
pub(crate) fn album_seen(state: &AppState, folders: &agent_keys::Scope, id: i64) -> Result<Option<Vec<MediaItem>>, Stop> {
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    if crate::albums::get(&conn, id).is_err() {
        return Ok(None);
    }
    let items: Vec<MediaItem> = crate::albums::items(&conn, id).map_err(|_| Stop::Failed("Could not read the album.".to_string()))?.into_iter().filter(|item| under(folders, &item.path)).collect();
    Ok((!items.is_empty()).then_some(items))
}

async fn album_photos(state: &Arc<AppState>, key: &AgentKey, album: Option<i64>, page: (usize, usize)) -> Result<Vec<Value>, Stop> {
    let Some(id) = album else { return failed("Give the album's id, from list_albums.") };
    let (worker, root, folders) = (state.clone(), state.root.clone(), key.scope());
    blocking(move || {
        let items = album_seen(&worker, &folders, id)?.ok_or(Stop::Refused)?;
        let shown = visible(&root, &folders, items);
        Ok(page_of(shown.iter().map(photo_json).collect(), page, json!({ "album": id, "name": album_name(&worker, id) })))
    })
    .await
}

fn person_seen(state: &AppState, folders: &agent_keys::Scope, id: i64) -> Result<Vec<MediaItem>, Stop> {
    let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
    let photos = crate::smart::person_photos(&conn, id).map_err(|_| Stop::Failed("Could not read the people.".to_string()))?;
    Ok(photos.into_iter().map(|(item, _)| item).filter(|item| under(folders, &item.path)).collect())
}

async fn list_people(state: &Arc<AppState>, key: &AgentKey) -> Result<Vec<Value>, Stop> {
    let (worker, folders) = (state.clone(), key.scope());
    blocking(move || {
        let people = {
            let conn = worker.db.lock().unwrap_or_else(|err| err.into_inner());
            crate::smart::people(&conn, false).map_err(|_| Stop::Failed("Could not read the people.".to_string()))?
        };
        let mut shown = Vec::new();
        for person in people {
            let photos = person_seen(&worker, &folders, person.id)?.len();
            if photos > 0 {
                shown.push(json!({ "id": person.id, "name": person.name, "photos": photos }));
            }
        }
        Ok(as_text(&json!({ "people": shown, "note": "People without a name have not been named in OwnNAS yet." })))
    })
    .await
}

async fn person_photos(state: &Arc<AppState>, key: &AgentKey, person: Option<i64>, page: (usize, usize)) -> Result<Vec<Value>, Stop> {
    let Some(id) = person else { return failed("Give the person's id, from list_people.") };
    let (worker, root, folders) = (state.clone(), state.root.clone(), key.scope());
    blocking(move || {
        let items = person_seen(&worker, &folders, id)?;
        if items.is_empty() {
            return Err(Stop::Refused);
        }
        let name = {
            let conn = worker.db.lock().unwrap_or_else(|err| err.into_inner());
            crate::smart::person(&conn, id).ok().flatten().and_then(|person| person.name)
        };
        let shown = visible(&root, &folders, items);
        Ok(page_of(shown.iter().map(photo_json).collect(), page, json!({ "person": id, "name": name })))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn under_compares_whole_folder_names_and_skips_hidden_ones() {
        let scope = |folders: &[&str], excluded: &[&str]| agent_keys::Scope {
            folders: folders.iter().map(|s| s.to_string()).collect(),
            excluded: excluded.iter().map(|s| s.to_string()).collect(),
            extensions: None,
        };
        let folders = scope(&["Photos"], &[]);
        assert!(under(&folders, "Photos/a.jpg"));
        assert!(under(&folders, "Photos"));
        assert!(!under(&folders, "Photos2/a.jpg"));
        assert!(!under(&folders, "Other/a.jpg"));
        assert!(!under(&folders, "Photos/.private/a.jpg"));
        assert!(under(&scope(&[""], &[]), "Anything/a.jpg"));
        assert!(!under(&scope(&[""], &[]), ".ownnas-trash/a.jpg"));
        let some_out = scope(&["Photos"], &["Photos/Private", "Photos/me.jpg"]);
        assert!(!under(&some_out, "Photos/Private/a.jpg"), "excluded folders are skipped");
        assert!(!under(&some_out, "Photos/me.jpg"));
        assert!(under(&some_out, "Photos/Private2/a.jpg"));
    }

    #[test]
    fn days_read_as_dates() {
        assert_eq!(day_start("2024-02-29"), Some(1_709_164_800));
        assert_eq!(day_start("29/02/2024"), None);
        assert_eq!(from_text(1_709_164_800), "2024-02-29");
    }
}
