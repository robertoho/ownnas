//! Photos that look alike: burst shots, resized or re-saved copies, slightly edited versions.
//! Each photo gets a 64-bit "difference hash" from its thumbnail (brighter or darker than its
//! neighbour, over a 9×8 grey picture), kept in `photo_hashes` until the file changes; photos
//! whose hashes differ in only a few bits are grouped. No models are needed.

use crate::files::{self, CancelFlag, FileError};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::path::Path;

/// Photos compared at most in one go.
pub const MOST_PHOTOS: usize = 40_000;

/// How alike photos must be: bits of the 64 that may differ.
pub fn threshold(strength: &str) -> u32 {
    match strength {
        "strict" => 3,
        "loose" => 12,
        _ => 7,
    }
}

/// The difference hash of a picture.
pub fn dhash(image: &image::DynamicImage) -> u64 {
    let small = image.grayscale().resize_exact(9, 8, image::imageops::FilterType::Triangle).to_luma8();
    let mut hash = 0u64;
    for y in 0..8 {
        for x in 0..8 {
            hash <<= 1;
            if small.get_pixel(x, y)[0] > small.get_pixel(x + 1, y)[0] {
                hash |= 1;
            }
        }
    }
    hash
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Photo {
    pub path: String,
    pub size: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub modified: i64,
    /// Bits that differ from the group's first (best) photo.
    pub distance: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    /// Groups of photos that look alike, the best (largest) first in each, biggest groups first.
    pub groups: Vec<Vec<Photo>>,
    pub compared: usize,
    pub truncated: bool,
}

fn stored(conn: &Connection, path: &str, size: i64, mtime: i64) -> Option<u64> {
    conn.query_row("SELECT dhash FROM photo_hashes WHERE path = ?1 AND size = ?2 AND mtime = ?3", params![path, size, mtime], |row| row.get::<_, i64>(0))
        .optional()
        .ok()
        .flatten()
        .map(|hash| hash as u64)
}

fn store(conn: &Connection, path: &str, size: i64, mtime: i64, hash: u64) {
    let _ = conn.execute(
        "INSERT INTO photo_hashes(path, size, mtime, dhash) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(path) DO UPDATE SET size = excluded.size, mtime = excluded.mtime, dhash = excluded.dhash",
        params![path, size, mtime, hash as i64],
    );
}

fn find_root(parent: &mut [usize], mut at: usize) -> usize {
    while parent[at] != at {
        parent[at] = parent[parent[at]];
        at = parent[at];
    }
    at
}

/// Photos under `folder` that look alike. `progress` is told how many of how many photos have
/// a hash so far (making one means making the thumbnail, the first time).
pub fn find(conn: &Connection, root: &Path, thumbs: &Path, folder: &str, strength: &str, cancel: &CancelFlag, progress: &dyn Fn(usize, usize)) -> Result<Report, FileError> {
    let root = root.canonicalize().map_err(|_| FileError::NotFound)?;
    let start = files::resolve(&root, folder)?;
    if !start.full.is_dir() {
        return Err(FileError::NotADirectory);
    }
    let rel = start.rel.clone();
    let photos: Vec<(String, i64, i64)> = {
        let mut stmt = conn
            .prepare(
                "SELECT path, size, mtime FROM file_index WHERE dir = 0 AND kind = 'image' AND size > 0
                   AND (?1 = '' OR (path >= ?1 || '/' AND path < ?1 || '0')) AND path NOT LIKE '.%' AND path NOT LIKE '%/.%'
                 ORDER BY path LIMIT ?2",
            )
            .map_err(|_| FileError::Io("Could not read the file index"))?;
        let rows = stmt.query_map(params![rel, (MOST_PHOTOS + 1) as i64], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).map_err(|_| FileError::Io("Could not read the file index"))?;
        rows.filter_map(Result::ok).filter(|(path, _, _): &(String, i64, i64)| !files::is_under_trash(path)).collect()
    };
    let truncated = photos.len() > MOST_PHOTOS;
    let photos = &photos[..photos.len().min(MOST_PHOTOS)];
    let mut hashed: Vec<(usize, u64)> = Vec::with_capacity(photos.len());
    for (index, (path, size, mtime)) in photos.iter().enumerate() {
        if cancel.is_cancelled() {
            return Err(FileError::Cancelled);
        }
        if index % 50 == 0 {
            progress(index, photos.len());
        }
        // The index may not know yet that a file was moved or deleted.
        if !root.join(path).is_file() {
            continue;
        }
        let hash = match stored(conn, path, *size, *mtime) {
            Some(hash) => hash,
            None => {
                let full = root.join(path);
                let Ok(thumb) = crate::thumbs::ensure(&full, thumbs, false) else { continue };
                let Ok(image) = image::open(&thumb).or_else(|_| image::load_from_memory(&crate::thumbs::read_cached(&thumb).unwrap_or_default())) else { continue };
                let hash = dhash(&image);
                store(conn, path, *size, *mtime, hash);
                hash
            }
        };
        hashed.push((index, hash));
    }
    progress(photos.len(), photos.len());
    let limit = threshold(strength);
    let mut parent: Vec<usize> = (0..hashed.len()).collect();
    for a in 0..hashed.len() {
        if a % 1024 == 0 && cancel.is_cancelled() {
            return Err(FileError::Cancelled);
        }
        for b in a + 1..hashed.len() {
            if (hashed[a].1 ^ hashed[b].1).count_ones() <= limit {
                let (ra, rb) = (find_root(&mut parent, a), find_root(&mut parent, b));
                if ra != rb {
                    parent[rb] = ra;
                }
            }
        }
    }
    let mut groups: std::collections::HashMap<usize, Vec<usize>> = std::collections::HashMap::new();
    for at in 0..hashed.len() {
        let top = find_root(&mut parent, at);
        groups.entry(top).or_default().push(at);
    }
    let mut out: Vec<Vec<Photo>> = Vec::new();
    for members in groups.into_values().filter(|members| members.len() > 1) {
        let mut photos_in: Vec<(Photo, u64)> = members
            .iter()
            .map(|&at| {
                let (path, size, mtime) = &photos[hashed[at].0];
                let dimensions = image::image_dimensions(root.join(path)).ok();
                (
                    Photo { path: path.clone(), size: *size as u64, width: dimensions.map(|d| d.0), height: dimensions.map(|d| d.1), modified: mtime / 1_000_000_000, distance: 0 },
                    hashed[at].1,
                )
            })
            .collect();
        // The best first: the most pixels, then the largest file.
        photos_in.sort_by(|a, b| {
            let pixels = |photo: &Photo| u64::from(photo.width.unwrap_or(0)) * u64::from(photo.height.unwrap_or(0));
            pixels(&b.0).cmp(&pixels(&a.0)).then(b.0.size.cmp(&a.0.size)).then(a.0.path.cmp(&b.0.path))
        });
        let best = photos_in[0].1;
        out.push(photos_in.into_iter().map(|(mut photo, hash)| {
            photo.distance = (hash ^ best).count_ones();
            photo
        }).collect());
    }
    out.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a[0].path.cmp(&b[0].path)));
    Ok(Report { groups: out, compared: hashed.len(), truncated })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(shift: u8, size: u32) -> image::DynamicImage {
        image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(size, size, |x, y| {
            let value = ((x * 255 / size) as u8).wrapping_add(shift) ^ ((y * 97 / size) as u8);
            image::Rgb([value, value / 2, 255 - value])
        }))
    }

    #[test]
    fn alike_pictures_have_close_hashes() {
        let original = dhash(&picture(0, 400));
        let smaller = dhash(&picture(0, 160));
        let brighter = dhash(&picture(6, 400));
        let other = dhash(&image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(400, 400, |x, y| image::Rgb([((x * y) % 255) as u8, 0, (y % 255) as u8]))));
        assert!((original ^ smaller).count_ones() <= threshold("normal"), "resized");
        assert!((original ^ brighter).count_ones() <= threshold("loose"), "a little changed");
        assert!((original ^ other).count_ones() > threshold("loose"), "different");
    }
}
