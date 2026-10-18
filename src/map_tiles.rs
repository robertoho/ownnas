//! Map pictures (OpenStreetMap's standard tiles) for drawing GPS tracks on, fetched by the
//! server and kept in OwnNAS's data folder, so the page only ever loads from OwnNAS and a tile
//! is downloaded once. OpenStreetMap's tile policy asks for an identifying user agent and for
//! tiles to be cached, which this does; tiles older than a month are fetched again.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

const SOURCE: &str = "https://tile.openstreetmap.org";
const KEEP: Duration = Duration::from_secs(30 * 24 * 3600);
const MAX_ZOOM: u32 = 19;

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(15))
        .user_agent(concat!("OwnNAS/", env!("CARGO_PKG_VERSION"), " (self-hosted file server; +https://github.com/robertoho/ownnas)"))
        .build()
}

/// Where tile `z/x/y` is kept, if those are a tile that exists.
fn cached_path(cache: &Path, z: u32, x: u32, y: u32) -> Option<PathBuf> {
    if z > MAX_ZOOM || x >= 1 << z || y >= 1 << z {
        return None;
    }
    Some(cache.join(z.to_string()).join(x.to_string()).join(format!("{y}.png")))
}

/// The PNG for tile `z/x/y`, from the cache or fetched now.
pub fn tile(cache: &Path, z: u32, x: u32, y: u32) -> Result<Vec<u8>, String> {
    let path = cached_path(cache, z, x, y).ok_or("There is no such map tile")?;
    let fresh = std::fs::metadata(&path).ok().and_then(|meta| meta.modified().ok()).and_then(|time| time.elapsed().ok()).is_some_and(|age| age < KEEP);
    if fresh {
        if let Ok(bytes) = std::fs::read(&path) {
            return Ok(bytes);
        }
    }
    let fetched = (|| {
        let response = agent().get(&format!("{SOURCE}/{z}/{x}/{y}.png")).call().map_err(|_| "The map could not be downloaded".to_string())?;
        let mut bytes = Vec::new();
        response.into_reader().take(2 * 1024 * 1024).read_to_end(&mut bytes).map_err(|_| "The map could not be downloaded".to_string())?;
        if !bytes.starts_with(b"\x89PNG") {
            return Err("The map server sent something else".to_string());
        }
        Ok(bytes)
    })();
    match fetched {
        Ok(bytes) => {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let temp = path.with_extension(format!("{}.part", std::process::id()));
            if std::fs::write(&temp, &bytes).is_ok() {
                let _ = std::fs::rename(&temp, &path);
            }
            Ok(bytes)
        }
        // Offline: an old tile is better than none.
        Err(err) => std::fs::read(&path).map_err(|_| err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_real_tiles_have_a_place() {
        let cache = Path::new("/c");
        assert_eq!(cached_path(cache, 2, 3, 1), Some(PathBuf::from("/c/2/3/1.png")));
        assert_eq!(cached_path(cache, 2, 4, 0), None);
        assert_eq!(cached_path(cache, 20, 0, 0), None);
    }
}
