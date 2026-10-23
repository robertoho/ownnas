//! The Gallery for the apps: the photo timeline, albums, tags, people, smart search, places,
//! On this day, workspaces, hiding photos from the timeline, and phone backups to the server's
//! media upload folder.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::{failed, ClientError, Server, Transfer};

#[derive(uniffi::Record, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MediaItem {
    /// Path in the library.
    pub path: String,
    /// "image", "video", or "svg" (absent when the photo is not indexed yet).
    #[serde(default)]
    pub kind: Option<String>,
    /// When it was taken, in seconds. For `date_source` "exif" it is the camera's local time,
    /// stored as if it were UTC (show it in UTC); for "file" it is a real instant.
    #[serde(default)]
    pub taken_at: Option<i64>,
    #[serde(default)]
    pub date_source: Option<String>,
    #[serde(default)]
    pub width: Option<i64>,
    #[serde(default)]
    pub height: Option<i64>,
    #[serde(default)]
    pub size: Option<i64>,
    /// Left out of the Photos timeline.
    #[serde(default)]
    pub hidden: bool,
    /// The file is no longer where the album or person remembers it.
    #[serde(default)]
    pub missing: bool,
    /// How well it matches a search (higher is better).
    #[serde(default)]
    pub score: Option<f32>,
    /// In a person's photos: the face that is them (for "not this person").
    #[serde(default)]
    pub face_id: Option<i64>,
}

/// One line of a photo's details.
#[derive(uniffi::Record, Deserialize, Clone, Debug)]
pub struct DetailRow {
    pub label: String,
    pub value: String,
    /// A real moment (seconds since the Unix epoch), to show in the phone's time zone instead of `value`.
    #[serde(default)]
    pub time: Option<i64>,
}

#[derive(uniffi::Record, Deserialize, Clone, Debug)]
pub struct DetailSection {
    pub title: String,
    pub rows: Vec<DetailRow>,
}

#[derive(uniffi::Record, Deserialize, Clone, Debug)]
pub struct GeoPoint {
    pub lat: f64,
    pub lon: f64,
}

#[derive(uniffi::Record, Deserialize, Clone, Debug)]
pub struct AlbumRef {
    pub id: i64,
    pub name: String,
}

/// Everything the server knows about one photo or video.
#[derive(uniffi::Record, Deserialize, Clone, Debug)]
pub struct PhotoDetails {
    pub name: String,
    pub path: String,
    /// Where it was taken, from the camera's GPS.
    #[serde(default)]
    pub location: Option<GeoPoint>,
    #[serde(default)]
    pub albums: Vec<AlbumRef>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub sections: Vec<DetailSection>,
}

#[derive(uniffi::Record, Deserialize, Clone, Debug)]
pub struct DayCount {
    /// "2024-07-15" (UTC).
    pub day: String,
    pub count: i64,
}

#[derive(uniffi::Record, Deserialize, Clone, Debug)]
pub struct TimelineDays {
    /// Newest first.
    pub days: Vec<DayCount>,
    /// Photos hidden from the timeline.
    #[serde(default)]
    pub hidden: i64,
}

#[derive(uniffi::Record, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Album {
    pub id: i64,
    #[serde(default)]
    pub parent_id: Option<i64>,
    pub name: String,
    #[serde(default)]
    pub cover: Option<String>,
    /// Photos in this album itself.
    pub count: i64,
    /// Albums inside it.
    pub albums: i64,
}

#[derive(uniffi::Record, Deserialize, Clone, Debug)]
pub struct Crumb {
    pub id: i64,
    pub name: String,
}

#[derive(uniffi::Record, Deserialize, Clone, Debug)]
pub struct AlbumPage {
    /// None for the top level.
    pub album: Option<Album>,
    pub breadcrumb: Vec<Crumb>,
    /// The albums inside.
    pub albums: Vec<Album>,
    /// Its own photos.
    pub items: Vec<MediaItem>,
}

#[derive(uniffi::Record, Deserialize, Clone, Debug)]
pub struct TagCount {
    pub tag: String,
    pub count: i64,
}

#[derive(uniffi::Record, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Person {
    pub id: i64,
    pub name: Option<String>,
    pub faces: i64,
    pub photos: i64,
    pub hidden: bool,
    /// A face to show them by (see [`Server::face_picture`]).
    pub cover_face: Option<i64>,
}

#[derive(uniffi::Record, Deserialize, Clone, Debug)]
pub struct PersonPage {
    pub person: Person,
    pub items: Vec<MediaItem>,
}

/// Which smart features the server has switched on (and ready).
#[derive(uniffi::Record, Clone, Debug)]
pub struct SmartFeatures {
    pub search: bool,
    pub faces: bool,
}

#[derive(uniffi::Record, Deserialize, Clone, Debug)]
pub struct BackupResult {
    /// Where the photo is (or already was) on the server.
    pub path: String,
    /// It was already there; nothing was sent.
    pub skipped: bool,
    /// It was deleted on the server and is never backed up again (`path` is empty).
    #[serde(default)]
    pub blocked: bool,
}

/// A photo deleted on the server that phones don't back up again.
#[derive(uniffi::Record, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BlockedPhoto {
    pub sha256: String,
    pub size: i64,
    pub name: String,
    /// Where it was on the server.
    pub path: String,
    #[serde(default)]
    pub taken_at: Option<i64>,
    pub blocked_at: i64,
    #[serde(default)]
    pub blocked_by: String,
}

/// A Gallery workspace ("Family", "Work"…): some library folders, shown on their own.
#[derive(uniffi::Record, Deserialize, Clone, Debug)]
pub struct Workspace {
    pub id: i64,
    pub name: String,
    pub folders: Vec<String>,
    /// Photos and videos in its folders.
    pub photos: i64,
}

/// Photos taken on this day in an earlier year.
#[derive(uniffi::Record, Deserialize, Clone, Debug)]
pub struct MemoryYear {
    pub year: i32,
    pub count: i64,
    /// Some of them, best first.
    pub items: Vec<MediaItem>,
}

/// A box on the map, in degrees.
#[derive(uniffi::Record, Deserialize, Clone, Copy, Debug)]
pub struct MapBounds {
    pub west: f64,
    pub south: f64,
    pub east: f64,
    pub north: f64,
}

/// Photos close together on the map: where they are on average, how many, and the newest.
#[derive(uniffi::Record, Clone, Debug)]
pub struct Place {
    pub lat: f64,
    pub lon: f64,
    pub count: u64,
    pub newest: MediaItem,
    /// The box they were gathered from: [`Server::place_photos`] with it lists them all.
    pub area: MapBounds,
}

#[derive(Deserialize)]
struct RawPlace {
    lat: f64,
    lon: f64,
    count: u64,
    newest: MediaItem,
}

/// How many map pixels the server gathers photos over (`cell` in the server's `albums::places`).
const PLACE_CELL: f64 = 72.0;

fn mercator_y(lat: f64) -> f64 {
    let lat = lat.clamp(-85.05, 85.05).to_radians();
    (1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / std::f64::consts::PI) / 2.0
}

fn mercator_lat(y: f64) -> f64 {
    (std::f64::consts::PI * (1.0 - 2.0 * y)).sinh().atan().to_degrees()
}

/// Joins groups less than a grid square apart, so a town whose photos straddle a line of the
/// grid is one place. The joined area is the box around both (it may take in a few more photos).
fn merge_neighbours(mut places: Vec<Place>, zoom: u8) -> Vec<Place> {
    let square = 360.0 / (256.0 * 2f64.powi(i32::from(zoom.min(22))) / PLACE_CELL);
    places.sort_by(|a, b| b.count.cmp(&a.count));
    let mut out: Vec<Place> = Vec::new();
    for place in places {
        let near = out.iter_mut().find(|group| {
            // A square is narrower north to south away from the equator (Mercator).
            (group.lon - place.lon).abs() < square && (group.lat - place.lat).abs() < square * group.lat.to_radians().cos()
        });
        match near {
            Some(group) => {
                let total = (group.count + place.count) as f64;
                group.lat = (group.lat * group.count as f64 + place.lat * place.count as f64) / total;
                group.lon = (group.lon * group.count as f64 + place.lon * place.count as f64) / total;
                group.count += place.count;
                group.area = MapBounds {
                    west: group.area.west.min(place.area.west),
                    south: group.area.south.min(place.area.south),
                    east: group.area.east.max(place.area.east),
                    north: group.area.north.max(place.area.north),
                };
                if place.newest.taken_at > group.newest.taken_at {
                    group.newest = place.newest;
                }
            }
            None => out.push(place),
        }
    }
    out.sort_by(|a, b| b.count.cmp(&a.count));
    out
}

/// The square of the server's grid at `zoom` that holds a group's average point (an average
/// stays inside the square its photos are in).
fn place_area(lat: f64, lon: f64, zoom: u8) -> MapBounds {
    let squares = 256.0 * 2f64.powi(i32::from(zoom.min(22))) / PLACE_CELL;
    let x = ((lon + 180.0) / 360.0 * squares).floor();
    let y = (mercator_y(lat) * squares).floor();
    MapBounds {
        west: x / squares * 360.0 - 180.0,
        east: (x + 1.0) / squares * 360.0 - 180.0,
        north: mercator_lat(y / squares),
        south: mercator_lat((y + 1.0) / squares),
    }
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct PlaceMap {
    pub places: Vec<Place>,
    /// The box around every photo with a place (to fit the map to them at first).
    pub extent: Option<MapBounds>,
    /// Photos with a place.
    pub total: u64,
}

#[derive(uniffi::Record, Deserialize, Clone, Debug)]
pub struct PlacePhotos {
    pub items: Vec<MediaItem>,
    /// How many there are (more than `items` when there are many).
    pub total: u64,
}

#[derive(Deserialize)]
struct Items {
    items: Vec<MediaItem>,
}

fn id_param(id: i64) -> String {
    id.to_string()
}

impl Server {
    /// Runs a gallery request with the chosen workspace added to its query.
    fn scoped<T>(&self, query: &[(&str, &str)], call: impl FnOnce(&[(&str, &str)]) -> T) -> T {
        let workspace = self.workspace.lock().unwrap_or_else(|err| err.into_inner()).map(|id| id.to_string());
        let mut query = query.to_vec();
        if let Some(id) = &workspace {
            query.push(("workspace", id));
        }
        call(&query)
    }
}

#[uniffi::export]
impl Server {
    /// The Gallery's workspaces, by name.
    pub fn workspaces(&self) -> Result<Vec<Workspace>, ClientError> {
        #[derive(Deserialize)]
        struct List {
            workspaces: Vec<Workspace>,
        }
        let list: List = self.api.get_json("/api/workspaces", &[])?;
        Ok(list.workspaces)
    }

    /// Shows one workspace in the gallery calls from now on (`None`: every photo). A workspace
    /// that no longer exists makes them fail with `NotFound`.
    pub fn set_gallery_workspace(&self, id: Option<i64>) {
        *self.workspace.lock().unwrap_or_else(|err| err.into_inner()) = id;
    }

    pub fn gallery_workspace(&self) -> Option<i64> {
        *self.workspace.lock().unwrap_or_else(|err| err.into_inner())
    }

    /// How many photos were taken each day, newest first, to lay the timeline out.
    pub fn gallery_days(&self) -> Result<TimelineDays, ClientError> {
        Ok(self.scoped(&[], |query| self.api.get_json("/api/gallery/days", query))?)
    }

    /// The photos taken in [from, to) (seconds), newest first.
    pub fn gallery_between(&self, from: i64, to: i64) -> Result<Vec<MediaItem>, ClientError> {
        let (from, to) = (from.to_string(), to.to_string());
        let page: Items = self.scoped(&[("from", &from), ("to", &to), ("limit", "50000")], |query| self.api.get_json("/api/gallery/timeline", query))?;
        Ok(page.items)
    }

    /// Photos taken on `month`/`day` in the years before `year`, newest year first.
    pub fn on_this_day(&self, month: u32, day: u32, year: i32) -> Result<Vec<MemoryYear>, ClientError> {
        #[derive(Deserialize)]
        struct Years {
            years: Vec<MemoryYear>,
        }
        let (month, day, year) = (month.to_string(), day.to_string(), year.to_string());
        let years: Years = self.scoped(&[("month", &month), ("day", &day), ("year", &year)], |query| self.api.get_json("/api/gallery/memories", query))?;
        Ok(years.years)
    }

    /// Photos with a place, grouped for a map at `zoom` (0–20) inside `bounds` (everywhere for
    /// `None`).
    pub fn places(&self, zoom: u8, bounds: Option<MapBounds>) -> Result<PlaceMap, ClientError> {
        #[derive(Deserialize)]
        struct Extent {
            west: f64,
            south: f64,
            east: f64,
            north: f64,
            count: u64,
        }
        #[derive(Deserialize)]
        struct Answer {
            places: Vec<RawPlace>,
            extent: Option<Extent>,
        }
        let level = zoom.min(20);
        let zoom = level.to_string();
        let edges = bounds.map(|b| [b.west.to_string(), b.south.to_string(), b.east.to_string(), b.north.to_string()]);
        let mut query = vec![("zoom", zoom.as_str())];
        if let Some([west, south, east, north]) = &edges {
            query.extend([("west", west.as_str()), ("south", south.as_str()), ("east", east.as_str()), ("north", north.as_str())]);
        }
        let answer: Answer = self.scoped(&query, |query| self.api.get_json("/api/gallery/places", query))?;
        let total = answer.extent.as_ref().map_or(0, |extent| extent.count);
        let places = answer.places.into_iter().map(|p| Place { area: place_area(p.lat, p.lon, level), lat: p.lat, lon: p.lon, count: p.count, newest: p.newest });
        Ok(PlaceMap {
            places: merge_neighbours(places.collect(), level),
            extent: answer.extent.map(|e| MapBounds { west: e.west, south: e.south, east: e.east, north: e.north }),
            total,
        })
    }

    /// The photos taken inside `bounds`, newest first (up to `limit`).
    pub fn place_photos(&self, bounds: MapBounds, limit: u32) -> Result<PlacePhotos, ClientError> {
        let [west, south, east, north, limit] = [bounds.west, bounds.south, bounds.east, bounds.north, limit as f64].map(|n| n.to_string());
        let query = [("west", west.as_str()), ("south", south.as_str()), ("east", east.as_str()), ("north", north.as_str()), ("limit", limit.as_str())];
        Ok(self.scoped(&query, |query| self.api.get_json("/api/gallery/places/items", query))?)
    }

    /// An album with what is inside it, or the top-level albums for `None`.
    pub fn album(&self, id: Option<i64>) -> Result<AlbumPage, ClientError> {
        let id = id.map(id_param);
        let query: Vec<(&str, &str)> = id.iter().map(|id| ("id", id.as_str())).collect();
        Ok(self.scoped(&query, |query| self.api.get_json("/api/albums", query))?)
    }

    /// Every album, for choosing one.
    pub fn all_albums(&self) -> Result<Vec<Album>, ClientError> {
        #[derive(Deserialize)]
        struct Tree {
            albums: Vec<Album>,
        }
        let tree: Tree = self.scoped(&[], |query| self.api.get_json("/api/albums/tree", query))?;
        Ok(tree.albums)
    }

    /// A new album, in the workspace shown (inside `parent_id`, it goes where that is).
    pub fn create_album(&self, name: String, parent_id: Option<i64>) -> Result<Album, ClientError> {
        #[derive(Deserialize)]
        struct Created {
            album: Album,
        }
        let created: Created = self.scoped(&[], |query| self.api.send_json("POST", "/api/albums", query, json!({ "name": name, "parentId": parent_id })))?;
        Ok(created.album)
    }

    pub fn rename_album(&self, id: i64, name: String) -> Result<(), ClientError> {
        let _: Value = self.api.send_json("PATCH", "/api/albums", &[], json!({ "id": id, "name": name }))?;
        Ok(())
    }

    /// Deletes the album and the albums inside it; the photos are not touched.
    pub fn delete_album(&self, id: i64) -> Result<(), ClientError> {
        let id = id.to_string();
        let _: Value = self.api.send_json("DELETE", "/api/albums", &[("id", &id)], json!({}))?;
        Ok(())
    }

    /// Adds photos to an album (or removes them); returns how many changed.
    pub fn change_album(&self, album_id: i64, paths: Vec<String>, add: bool) -> Result<i64, ClientError> {
        let body = json!({ "albumId": album_id, "paths": paths, "action": if add { "add" } else { "remove" } });
        let answer: Value = self.api.send_json("POST", "/api/albums/items", &[], body)?;
        Ok(answer["changed"].as_i64().unwrap_or(0))
    }

    pub fn tags(&self) -> Result<Vec<TagCount>, ClientError> {
        #[derive(Deserialize)]
        struct Tags {
            tags: Vec<TagCount>,
        }
        let tags: Tags = self.scoped(&[], |query| self.api.get_json("/api/tags/all", query))?;
        Ok(tags.tags)
    }

    pub fn tagged(&self, tag: String) -> Result<Vec<MediaItem>, ClientError> {
        let page: Items = self.scoped(&[("tag", &tag)], |query| self.api.get_json("/api/gallery/tag", query))?;
        Ok(page.items)
    }

    /// Tags photos (or takes the tag off).
    pub fn tag_photos(&self, paths: Vec<String>, tag: String, add: bool) -> Result<(), ClientError> {
        let body = json!({ "paths": paths, "tag": tag, "action": if add { "add" } else { "remove" } });
        let _: Value = self.api.send_json("POST", "/api/tags/bulk", &[], body)?;
        Ok(())
    }

    /// Leaves photos out of the timeline (or puts them back). Returns how many changed.
    pub fn hide_from_timeline(&self, paths: Vec<String>, hidden: bool) -> Result<i64, ClientError> {
        let answer: Value = self.api.send_json("POST", "/api/gallery/hidden", &[], json!({ "paths": paths, "hidden": hidden }))?;
        Ok(answer["changed"].as_i64().unwrap_or(0))
    }

    pub fn hidden_photos(&self) -> Result<Vec<MediaItem>, ClientError> {
        let page: Items = self.scoped(&[], |query| self.api.get_json("/api/gallery/hidden", query))?;
        Ok(page.items)
    }

    pub fn smart_features(&self) -> Result<SmartFeatures, ClientError> {
        let status: Value = self.api.get_json("/api/ml", &[])?;
        let on = |name: &str| status["available"] == true && status[name]["enabled"] == true && status[name]["ready"] == true;
        Ok(SmartFeatures { search: on("search"), faces: on("faces") })
    }

    /// Photos showing what the words describe, best first.
    pub fn search_photos(&self, words: String) -> Result<Vec<MediaItem>, ClientError> {
        let page: Items = self.scoped(&[("q", &words)], |query| self.api.get_json("/api/gallery/search", query))?;
        Ok(page.items)
    }

    /// Photos that look like one in the library.
    pub fn similar_photos(&self, path: String) -> Result<Vec<MediaItem>, ClientError> {
        let page: Items = self.scoped(&[("path", &path)], |query| self.api.get_json("/api/gallery/similar", query))?;
        Ok(page.items)
    }

    pub fn people(&self, hidden: bool) -> Result<Vec<Person>, ClientError> {
        #[derive(Deserialize)]
        struct People {
            people: Vec<Person>,
        }
        let query: &[(&str, &str)] = if hidden { &[("hidden", "1")] } else { &[] };
        let people: People = self.scoped(query, |query| self.api.get_json("/api/people", query))?;
        Ok(people.people)
    }

    pub fn person(&self, id: i64) -> Result<PersonPage, ClientError> {
        let id = id.to_string();
        Ok(self.scoped(&[("id", &id)], |query| self.api.get_json("/api/people/photos", query))?)
    }

    /// Renames, hides or shows, or merges (`into`) a person.
    pub fn change_person(&self, id: i64, name: Option<String>, hidden: Option<bool>, into: Option<i64>) -> Result<(), ClientError> {
        let _: Value = self.api.send_json("POST", "/api/people", &[], json!({ "id": id, "name": name, "hidden": hidden, "into": into }))?;
        Ok(())
    }

    /// Moves faces to another person, or (`None`) out to one of their own: "not this person".
    pub fn move_faces(&self, faces: Vec<i64>, to: Option<i64>) -> Result<(), ClientError> {
        let _: Value = self.api.send_json("POST", "/api/faces/move", &[], json!({ "faces": faces, "to": to }))?;
        Ok(())
    }

    /// Everything known about a photo or video: file, camera, exposure, place, streams, albums…
    pub fn photo_details(&self, path: String) -> Result<PhotoDetails, ClientError> {
        Ok(self.api.get_json("/api/gallery/details", &[("path", &path)])?)
    }

    /// JPEG of one face, cut out of its photo.
    pub fn face_picture(&self, face_id: i64) -> Result<Vec<u8>, ClientError> {
        let id = face_id.to_string();
        Ok(self.api.get_bytes("/api/faces/crop", &[("id", &id)], 4 * 1024 * 1024)?)
    }

    /// Deletes photos from the server (into the Trash) and remembers their contents, so no phone
    /// backs them up again. Returns how many were done.
    pub fn delete_and_block(&self, paths: Vec<String>) -> Result<u32, ClientError> {
        #[derive(Deserialize)]
        struct Done {
            blocked: u32,
            #[serde(default)]
            problems: Vec<Value>,
        }
        let done: Done = self.api.send_json("POST", "/api/backup/blocked", &[], json!({ "paths": paths, "delete": true }))?;
        if done.blocked == 0 {
            if let Some(problem) = done.problems.first() {
                return Err(failed(problem["error"].as_str().unwrap_or("Could not delete the photo").to_string()));
            }
        }
        Ok(done.blocked)
    }

    /// Photos deleted on the server that phones don't back up again, newest first.
    pub fn blocked_photos(&self) -> Result<Vec<BlockedPhoto>, ClientError> {
        #[derive(Deserialize)]
        struct Page {
            photos: Vec<BlockedPhoto>,
        }
        let page: Page = self.api.get_json("/api/backup/blocked", &[])?;
        Ok(page.photos)
    }

    /// The small picture kept with a blocked photo (JPEG).
    pub fn blocked_preview(&self, sha256: String) -> Result<Vec<u8>, ClientError> {
        Ok(self.api.get_bytes("/api/backup/blocked/preview", &[("sha256", &sha256)], 1024 * 1024)?)
    }

    /// Lets phones back a deleted photo up again.
    pub fn unblock_photo(&self, sha256: String) -> Result<(), ClientError> {
        let _: Value = self.api.send_json("DELETE", "/api/backup/blocked", &[("sha256", &sha256)], json!({}))?;
        Ok(())
    }

    /// Where phones back up to, or `None` when phone backups are off on the server.
    pub fn backup_folder(&self) -> Result<Option<String>, ClientError> {
        let config: Value = self.api.get_json("/api/backup", &[])?;
        Ok(config["folder"].as_str().map(str::to_string))
    }

    /// Backs one photo or video up, read from the open file descriptor `fd` (handed over).
    /// It goes to `<folder>/<device>/<year>/<month>/<name>`. The file is hashed first and not sent
    /// when the server has the same contents already (under its name or a numbered copy); the
    /// server checks the hash of what arrives too. `mtime_ms` is when it was taken.
    #[cfg(unix)]
    pub fn backup_fd(&self, device: String, name: String, fd: i32, size: u64, mtime_ms: i64, transfer: Arc<dyn Transfer>) -> Result<BackupResult, ClientError> {
        use std::io::{Read, Seek};
        use std::os::fd::{FromRawFd, OwnedFd};
        use sha2::Digest;
        if fd < 0 {
            return Err(failed("The file could not be opened".into()));
        }
        // SAFETY: the caller hands over an open descriptor it no longer uses.
        let mut file = std::fs::File::from(unsafe { OwnedFd::from_raw_fd(fd) });
        let mut hasher = sha2::Sha256::new();
        let mut buffer = vec![0u8; 1 << 20];
        loop {
            if !transfer.keep_going(0) {
                return Err(ClientError::Cancelled);
            }
            let read = file.read(&mut buffer).map_err(|err| failed(format!("The file could not be read: {err}")))?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        let sha256: String = hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect();
        file.rewind().map_err(|err| failed(format!("The file could not be read: {err}")))?;
        let (size_text, mtime_text) = (size.to_string(), mtime_ms.to_string());
        let query = [
            ("device", device.as_str()),
            ("name", name.as_str()),
            ("size", size_text.as_str()),
            ("mtime", mtime_text.as_str()),
            ("sha256", sha256.as_str()),
        ];
        // Ask first, so a photo the server has is never sent again. Older servers don't know
        // this question; the upload itself still skips copies there.
        #[derive(Deserialize)]
        struct Found {
            path: Option<String>,
            #[serde(default)]
            blocked: bool,
        }
        match self.api.get_json::<Found>("/api/backup/has", &query) {
            Ok(Found { blocked: true, .. }) => return Ok(BackupResult { path: String::new(), skipped: true, blocked: true }),
            Ok(Found { path: Some(path), .. }) => return Ok(BackupResult { path, skipped: true, blocked: false }),
            _ => {}
        }
        Ok(self.api.post_stream("/api/backup/upload", &query, file, Some(size), &|done| transfer.keep_going(done))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_answers_parse() {
        let item: MediaItem = serde_json::from_str(
            r#"{"path":"a/b.jpg","kind":"image","takenAt":5,"dateSource":"exif","width":4,"height":3,"size":9,"hidden":true,"score":0.3,"faceId":7}"#,
        )
        .unwrap();
        assert_eq!((item.taken_at, item.hidden, item.face_id, item.missing), (Some(5), true, Some(7), false));
        let bare: MediaItem = serde_json::from_str(r#"{"path":"x.jpg"}"#).unwrap();
        assert_eq!(bare.kind, None);
        let page: AlbumPage = serde_json::from_str(
            r#"{"album":{"id":2,"parentId":1,"name":"Italy","cover":null,"count":4,"albums":0},"breadcrumb":[{"id":1,"name":"Trips"},{"id":2,"name":"Italy"}],"albums":[],"items":[{"path":"p.jpg","missing":true}]}"#,
        )
        .unwrap();
        assert_eq!(page.album.unwrap().parent_id, Some(1));
        assert!(page.items[0].missing);
        let person: PersonPage = serde_json::from_str(
            r#"{"person":{"id":1,"name":null,"faces":3,"photos":3,"hidden":false,"coverFace":3},"items":[{"path":"l.jpg","faceId":1}]}"#,
        )
        .unwrap();
        assert_eq!(person.person.cover_face, Some(3));
        let details: PhotoDetails = serde_json::from_str(
            r#"{"name":"a.jpg","path":"p/a.jpg","kind":"image","location":{"lat":41.3,"lon":2.1},"albums":[{"id":1,"name":"Trips"}],"tags":["sea"],"sections":[{"title":"File","rows":[{"label":"Modified","value":"x","time":5},{"label":"Size","value":"1 KB"}]}]}"#,
        )
        .unwrap();
        assert_eq!((details.sections[0].rows[0].time, details.location.unwrap().lat), (Some(5), 41.3));
        let workspace: Workspace = serde_json::from_str(r#"{"id":3,"name":"Work","folders":["Jobs"],"photos":12}"#).unwrap();
        assert_eq!((workspace.id, workspace.photos), (3, 12));
        let year: MemoryYear = serde_json::from_str(r#"{"year":2019,"count":4,"items":[{"path":"a.jpg","takenAt":1}]}"#).unwrap();
        assert_eq!((year.year, year.items.len()), (2019, 1));
        let place: RawPlace = serde_json::from_str(r#"{"lat":41.3,"lon":2.1,"count":7,"newest":{"path":"b.jpg"}}"#).unwrap();
        assert_eq!((place.count, place.newest.path.as_str()), (7, "b.jpg"));
    }

    #[test]
    fn place_areas_hold_their_point() {
        for (lat, lon, zoom) in [(41.38, 2.17, 9), (-33.86, 151.2, 4), (64.1, -21.9, 12), (0.0, 0.0, 0)] {
            let area = place_area(lat, lon, zoom);
            assert!(area.west <= lon && lon <= area.east && area.south <= lat && lat <= area.north, "{lat},{lon} at {zoom}: {area:?}");
        }
        // Squares meet: the next one along starts where this one ends.
        let a = place_area(41.38, 2.17, 9);
        let b = place_area(41.38, a.east + 1e-9, 9);
        assert!((b.west - a.east).abs() < 1e-9);
    }

    #[test]
    fn neighbouring_places_are_one() {
        let item = |path: &str, taken_at| MediaItem { path: path.into(), kind: None, taken_at: Some(taken_at), date_source: None, width: None, height: None, size: None, hidden: false, missing: false, score: None, face_id: None };
        let at = |lat, lon, count, newest| Place { area: place_area(lat, lon, 7), lat, lon, count, newest };
        // Barcelona either side of a grid line, and Paris.
        let places = vec![at(41.39, 2.10, 3, item("a", 1)), at(41.37, 2.25, 2, item("b", 9)), at(48.86, 2.35, 4, item("c", 5))];
        let merged = merge_neighbours(places, 7);
        assert_eq!(merged.len(), 2);
        let barcelona = merged.iter().find(|p| p.lat < 45.0).unwrap();
        assert_eq!((barcelona.count, barcelona.newest.path.as_str()), (5, "b"));
        assert!(barcelona.area.west <= 2.10 && barcelona.area.east >= 2.25);
    }
}
