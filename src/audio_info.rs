//! What a song file says about itself: its title, artist, album, and cover picture (read with
//! `lofty`, in pure Rust). A song without a picture of its own shows the folder's cover
//! ("cover.jpg", "folder.jpg", and the like), as music players do.

use std::path::{Path, PathBuf};

use lofty::prelude::*;
use serde::Serialize;

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioInfo {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub genre: Option<String>,
    pub year: Option<i64>,
    pub track: Option<u32>,
    pub track_total: Option<u32>,
    pub disc: Option<u32>,
    pub duration: Option<f64>,
    /// Kilobits per second.
    pub bitrate: Option<u32>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u8>,
    pub bit_depth: Option<u8>,
    pub has_cover: bool,
}

/// Names music players look for beside the songs.
const FOLDER_COVERS: [&str; 8] = ["cover", "folder", "front", "album", "albumart", "albumartsmall", "Cover", "Folder"];

pub fn info(path: &Path) -> AudioInfo {
    let mut info = AudioInfo::default();
    if let Ok(file) = lofty::read_from_path(path) {
        let properties = file.properties();
        let duration = properties.duration().as_secs_f64();
        info.duration = (duration > 0.0).then_some(duration);
        info.bitrate = properties.audio_bitrate();
        info.sample_rate = properties.sample_rate();
        info.channels = properties.channels();
        info.bit_depth = properties.bit_depth();
        if let Some(tag) = file.primary_tag().or_else(|| file.first_tag()) {
            let text = |value: Option<std::borrow::Cow<'_, str>>| value.map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
            info.title = text(tag.title());
            info.artist = text(tag.artist());
            info.album = text(tag.album());
            info.genre = text(tag.genre());
            info.album_artist = tag.get_string(ItemKey::AlbumArtist).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
            info.year = tag.date().map(|date| i64::from(date.year));
            info.track = tag.track();
            info.track_total = tag.track_total();
            info.disc = tag.disk();
            info.has_cover = !tag.pictures().is_empty();
        }
    }
    if !info.has_cover {
        info.has_cover = folder_cover(path).is_some();
    }
    info
}

/// The song's own picture (the front cover first), or the folder's cover.
pub fn cover(path: &Path) -> Option<(Vec<u8>, String)> {
    if let Ok(file) = lofty::read_from_path(path) {
        let pictures: Vec<&lofty::picture::Picture> = file.tags().iter().flat_map(|tag| tag.pictures()).collect();
        let front = pictures.iter().find(|p| p.pic_type() == lofty::picture::PictureType::CoverFront).or(pictures.first());
        if let Some(picture) = front {
            let mime = picture.mime_type().map(|m| m.as_str().to_string()).unwrap_or_else(|| "image/jpeg".to_string());
            return Some((picture.data().to_vec(), mime));
        }
    }
    let found = folder_cover(path)?;
    let mime = if found.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("png")) { "image/png" } else { "image/jpeg" };
    Some((std::fs::read(found).ok()?, mime.to_string()))
}

fn folder_cover(path: &Path) -> Option<PathBuf> {
    let dir = path.parent()?;
    for name in FOLDER_COVERS {
        for ext in ["jpg", "jpeg", "png"] {
            let candidate = dir.join(format!("{name}.{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falls_back_to_the_folders_cover() {
        let dir = std::env::temp_dir().join(format!("ownnas-audio-{}-{}", std::process::id(), rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        // Not really a song: nothing to read, but the folder has a cover.
        std::fs::write(dir.join("song.mp3"), b"not audio").unwrap();
        assert!(!info(&dir.join("song.mp3")).has_cover);
        std::fs::write(dir.join("cover.jpg"), b"jpeg bytes").unwrap();
        assert!(info(&dir.join("song.mp3")).has_cover);
        assert_eq!(cover(&dir.join("song.mp3")).unwrap(), (b"jpeg bytes".to_vec(), "image/jpeg".to_string()));
        let _ = std::fs::remove_dir_all(dir);
    }
}
