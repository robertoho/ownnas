//! The Gallery for the apps: the photo timeline, albums, tags, people, smart search, hiding
//! photos from the timeline, and phone backups to the server's media upload folder.

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
}

#[derive(Deserialize)]
struct Items {
    items: Vec<MediaItem>,
}

fn id_param(id: i64) -> String {
    id.to_string()
}

#[uniffi::export]
impl Server {
    /// How many photos were taken each day, newest first, to lay the timeline out.
    pub fn gallery_days(&self) -> Result<TimelineDays, ClientError> {
        Ok(self.api.get_json("/api/gallery/days", &[])?)
    }

    /// The photos taken in [from, to) (seconds), newest first.
    pub fn gallery_between(&self, from: i64, to: i64) -> Result<Vec<MediaItem>, ClientError> {
        let (from, to) = (from.to_string(), to.to_string());
        let page: Items = self.api.get_json("/api/gallery/timeline", &[("from", &from), ("to", &to), ("limit", "50000")])?;
        Ok(page.items)
    }

    /// An album with what is inside it, or the top-level albums for `None`.
    pub fn album(&self, id: Option<i64>) -> Result<AlbumPage, ClientError> {
        let id = id.map(id_param);
        let query: Vec<(&str, &str)> = id.iter().map(|id| ("id", id.as_str())).collect();
        Ok(self.api.get_json("/api/albums", &query)?)
    }

    /// Every album, for choosing one.
    pub fn all_albums(&self) -> Result<Vec<Album>, ClientError> {
        #[derive(Deserialize)]
        struct Tree {
            albums: Vec<Album>,
        }
        let tree: Tree = self.api.get_json("/api/albums/tree", &[])?;
        Ok(tree.albums)
    }

    pub fn create_album(&self, name: String, parent_id: Option<i64>) -> Result<Album, ClientError> {
        #[derive(Deserialize)]
        struct Created {
            album: Album,
        }
        let created: Created = self.api.send_json("POST", "/api/albums", &[], json!({ "name": name, "parentId": parent_id }))?;
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
        let tags: Tags = self.api.get_json("/api/tags/all", &[])?;
        Ok(tags.tags)
    }

    pub fn tagged(&self, tag: String) -> Result<Vec<MediaItem>, ClientError> {
        let page: Items = self.api.get_json("/api/gallery/tag", &[("tag", &tag)])?;
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
        let page: Items = self.api.get_json("/api/gallery/hidden", &[])?;
        Ok(page.items)
    }

    pub fn smart_features(&self) -> Result<SmartFeatures, ClientError> {
        let status: Value = self.api.get_json("/api/ml", &[])?;
        let on = |name: &str| status["available"] == true && status[name]["enabled"] == true && status[name]["ready"] == true;
        Ok(SmartFeatures { search: on("search"), faces: on("faces") })
    }

    /// Photos showing what the words describe, best first.
    pub fn search_photos(&self, words: String) -> Result<Vec<MediaItem>, ClientError> {
        let page: Items = self.api.get_json("/api/gallery/search", &[("q", &words)])?;
        Ok(page.items)
    }

    /// Photos that look like one in the library.
    pub fn similar_photos(&self, path: String) -> Result<Vec<MediaItem>, ClientError> {
        let page: Items = self.api.get_json("/api/gallery/similar", &[("path", &path)])?;
        Ok(page.items)
    }

    pub fn people(&self, hidden: bool) -> Result<Vec<Person>, ClientError> {
        #[derive(Deserialize)]
        struct People {
            people: Vec<Person>,
        }
        let query: &[(&str, &str)] = if hidden { &[("hidden", "1")] } else { &[] };
        let people: People = self.api.get_json("/api/people", query)?;
        Ok(people.people)
    }

    pub fn person(&self, id: i64) -> Result<PersonPage, ClientError> {
        let id = id.to_string();
        Ok(self.api.get_json("/api/people/photos", &[("id", &id)])?)
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

    /// JPEG of one face, cut out of its photo.
    pub fn face_picture(&self, face_id: i64) -> Result<Vec<u8>, ClientError> {
        let id = face_id.to_string();
        Ok(self.api.get_bytes("/api/faces/crop", &[("id", &id)], 4 * 1024 * 1024)?)
    }

    /// Where phones back up to, or `None` when phone backups are off on the server.
    pub fn backup_folder(&self) -> Result<Option<String>, ClientError> {
        let config: Value = self.api.get_json("/api/backup", &[])?;
        Ok(config["folder"].as_str().map(str::to_string))
    }

    /// Backs one photo or video up, read from the open file descriptor `fd` (handed over).
    /// It goes to `<folder>/<device>/<year>/<month>/<name>`; the server skips it when it is
    /// already there. `mtime_ms` is when it was taken.
    #[cfg(unix)]
    pub fn backup_fd(&self, device: String, name: String, fd: i32, size: u64, mtime_ms: i64, transfer: Arc<dyn Transfer>) -> Result<BackupResult, ClientError> {
        use std::os::fd::{FromRawFd, OwnedFd};
        if fd < 0 {
            return Err(failed("The file could not be opened".into()));
        }
        // SAFETY: the caller hands over an open descriptor it no longer uses.
        let file = std::fs::File::from(unsafe { OwnedFd::from_raw_fd(fd) });
        let (size_text, mtime_text) = (size.to_string(), mtime_ms.to_string());
        let query = [("device", device.as_str()), ("name", name.as_str()), ("size", size_text.as_str()), ("mtime", mtime_text.as_str())];
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
    }
}
