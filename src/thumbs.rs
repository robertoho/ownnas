//! Thumbnails for pictures and videos, made once and kept in OwnNAS's data folder.
//! Photos become JPEGs (PNG when they have transparency); videos show a frame (via `ffmpeg`);
//! HEIC and AVIF photos, which the image library cannot read, go through `sips` (macOS),
//! `heif-convert`, or `ffmpeg`, whichever is installed.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant, UNIX_EPOCH};

use sha2::{Digest, Sha256};

/// Longest side of a thumbnail, in pixels.
const SIZE: u32 = 480;
const JPEG_QUALITY: u8 = 80;
/// The cache's own folder inside `thumbs/`: a new one when the format changes.
pub const CACHE_VERSION: &str = "v2";

pub fn ffmpeg_available() -> bool {
    static FOUND: OnceLock<bool> = OnceLock::new();
    *FOUND.get_or_init(|| runs("ffmpeg", &["-version"]))
}

fn runs(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// A tool that turns HEIC/AVIF photos into something the image library reads.
#[derive(Clone, Copy, PartialEq)]
enum Converter {
    Sips,
    HeifConvert,
    Ffmpeg,
}

fn converter() -> Option<Converter> {
    static FOUND: OnceLock<Option<Converter>> = OnceLock::new();
    *FOUND.get_or_init(|| {
        if cfg!(target_os = "macos") && Path::new("/usr/bin/sips").is_file() {
            Some(Converter::Sips)
        } else if runs("heif-convert", &["--version"]) {
            Some(Converter::HeifConvert)
        } else if ffmpeg_available() {
            Some(Converter::Ffmpeg)
        } else {
            None
        }
    })
}

/// Photo formats the image library cannot decode, read through a converter instead.
pub fn needs_converter(extension: &str) -> bool {
    matches!(extension, "heic" | "heif" | "avif")
}

pub fn converter_available() -> bool {
    converter().is_some()
}

/// The cached thumbnail for `source`, made now if there is none yet.
pub fn ensure(source: &Path, cache_root: &Path, video: bool) -> Result<PathBuf, String> {
    let meta = std::fs::metadata(source).map_err(|_| "Could not read the file".to_string())?;
    let base = cache_base(cache_root, source, &meta);
    if let Some(found) = cached(&base) {
        return Ok(found);
    }
    if let Some(parent) = base.parent() {
        std::fs::create_dir_all(parent).map_err(|_| "Could not create the thumbnail cache".to_string())?;
    }
    let extension = source.extension().map(|ext| ext.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let stamp = std::time::SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let temp = |ext: &str| base.with_file_name(format!(".{}.{stamp}.{ext}", std::process::id()));
    let (made, ext) = if video {
        let out = temp("jpg");
        (render_video(source, &out).map(|_| out), "jpg")
    } else if needs_converter(&extension) {
        let out = temp("jpg");
        (render_converted(source, &out).map(|_| out), "jpg")
    } else {
        let jpg = temp("jpg");
        match render_image(source, &jpg) {
            Ok(Encoded::Jpeg) => (Ok(jpg), "jpg"),
            Ok(Encoded::Png) => (Ok(jpg), "png"),
            Err(err) => (Err(err), "jpg"),
        }
    };
    let made = match made {
        Ok(path) => path,
        Err(err) => {
            let _ = std::fs::remove_file(temp("jpg"));
            return Err(err);
        }
    };
    let dest = base.with_extension(ext);
    // Another request may have made it meanwhile; either copy is fine.
    std::fs::rename(&made, &dest).map_err(|_| "Could not store the thumbnail".to_string())?;
    Ok(dest)
}

fn cached(base: &Path) -> Option<PathBuf> {
    ["jpg", "png"].into_iter().map(|ext| base.with_extension(ext)).find(|path| path.is_file())
}

/// Where a file's thumbnail is kept, without its extension. It depends on the file's name,
/// size, and modification time, not its folder: moving a photo keeps its thumbnail.
fn cache_base(cache_root: &Path, source: &Path, meta: &std::fs::Metadata) -> PathBuf {
    let mut hasher = Sha256::new();
    hasher.update(source.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default().as_bytes());
    let modified = meta.modified().ok().and_then(|time| time.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
    hasher.update(modified.to_le_bytes());
    hasher.update(meta.len().to_le_bytes());
    let hash = hex::encode(hasher.finalize());
    cache_root.join(&hash[..2]).join(hash)
}

/// The cache folder for this version, and old caches removed in the background.
pub fn prepare_cache(thumbs: &Path) -> Result<PathBuf, String> {
    let current = thumbs.join(CACHE_VERSION);
    std::fs::create_dir_all(&current).map_err(|_| "Could not create the thumbnail cache".to_string())?;
    let thumbs = thumbs.to_path_buf();
    std::thread::spawn(move || {
        let Ok(entries) = std::fs::read_dir(&thumbs) else { return };
        for entry in entries.flatten() {
            // Thumbnails from earlier versions sat in two-letter folders right inside `thumbs/`.
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.len() == 2 && name.chars().all(|c| c.is_ascii_hexdigit()) {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    });
    Ok(current)
}

pub fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("png") => "image/png",
        _ => "image/jpeg",
    }
}

enum Encoded {
    Jpeg,
    Png,
}

/// Writes a JPEG to `dest`, or a PNG (still at `dest`) when the picture has transparency.
fn render_image(source: &Path, dest: &Path) -> Result<Encoded, String> {
    let orientation = read_orientation(source);
    let image = decode(source)?;
    let thumb = apply_orientation(image, orientation).thumbnail(SIZE, SIZE);
    write_thumbnail(&thumb, dest)
}

fn decode(source: &Path) -> Result<image::DynamicImage, String> {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(25_000);
    limits.max_image_height = Some(25_000);
    limits.max_alloc = Some(256 * 1024 * 1024);
    let mut reader = image::ImageReader::open(source)
        .map_err(|_| "Could not open the image".to_string())?
        .with_guessed_format()
        .map_err(|_| "Could not recognize the image".to_string())?;
    reader.limits(limits);
    let image = reader.decode().map_err(|_| "Could not decode the image".to_string())?;
    if image.width() == 0 || image.height() == 0 {
        return Err("The image is empty".into());
    }
    Ok(image)
}

fn write_thumbnail(thumb: &image::DynamicImage, dest: &Path) -> Result<Encoded, String> {
    let transparent = thumb.color().has_alpha() && thumb.to_rgba8().pixels().any(|pixel| pixel[3] < 255);
    if transparent {
        thumb.save_with_format(dest, image::ImageFormat::Png).map_err(|_| "Could not write the thumbnail".to_string())?;
        return Ok(Encoded::Png);
    }
    let file = File::create(dest).map_err(|_| "Could not write the thumbnail".to_string())?;
    let mut writer = std::io::BufWriter::new(file);
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut writer, JPEG_QUALITY)
        .encode_image(&thumb.to_rgb8())
        .map_err(|_| "Could not write the thumbnail".to_string())?;
    Ok(Encoded::Jpeg)
}

/// HEIC/AVIF: converted to a JPEG by an outside tool, then made into a thumbnail as usual.
fn render_converted(source: &Path, dest: &Path) -> Result<(), String> {
    let converter = converter().ok_or("No tool to read this picture is installed")?;
    let full = dest.with_extension("full.jpg");
    let result = (|| {
        match converter {
            // sips scales as it converts, so its output is the thumbnail already.
            Converter::Sips => {
                run(Command::new("/usr/bin/sips")
                    .args(["-s", "format", "jpeg", "-s", "formatOptions", &JPEG_QUALITY.to_string(), "-Z", &SIZE.to_string()])
                    .arg(source)
                    .arg("--out")
                    .arg(dest))?;
                return if dest.is_file() { Ok(()) } else { Err("sips could not read the picture".to_string()) };
            }
            Converter::HeifConvert => run(Command::new("heif-convert").args(["-q", "90"]).arg(source).arg(&full))?,
            Converter::Ffmpeg => run(Command::new("ffmpeg")
                .args(["-y", "-hide_banner", "-loglevel", "error", "-i"])
                .arg(source)
                .args(["-frames:v", "1", "-q:v", "2"])
                .arg(&full))?,
        }
        let thumb = decode(&full)?.thumbnail(SIZE, SIZE);
        write_thumbnail(&thumb, dest).map(|_| ())
    })();
    let _ = std::fs::remove_file(&full);
    result
}

/// Runs a converter, giving up after 30 seconds.
fn run(command: &mut Command) -> Result<(), String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "The picture converter is not available".to_string())?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => return Err("The picture could not be converted".into()),
            Ok(None) if started.elapsed() > Duration::from_secs(30) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Converting the picture took too long".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(30)),
            Err(_) => return Err("The picture could not be converted".into()),
        }
    }
}

fn render_video(source: &Path, dest: &Path) -> Result<(), String> {
    if render_video_at(source, dest, "1").is_ok() {
        return Ok(());
    }
    render_video_at(source, dest, "0")
}

fn render_video_at(source: &Path, dest: &Path, seek: &str) -> Result<(), String> {
    run(Command::new("ffmpeg")
        .args(["-y", "-hide_banner", "-loglevel", "error", "-ss", seek, "-i"])
        .arg(source)
        .args(["-frames:v", "1", "-vf", &format!("scale={SIZE}:{SIZE}:force_original_aspect_ratio=decrease"), "-f", "image2", "-c:v", "mjpeg", "-q:v", "4"])
        .arg(dest))
    .map_err(|_| "ffmpeg could not make a thumbnail".to_string())?;
    if dest.is_file() {
        Ok(())
    } else {
        Err("ffmpeg could not make a thumbnail".into())
    }
}

fn read_orientation(path: &Path) -> u32 {
    let Ok(file) = File::open(path) else {
        return 1;
    };
    let mut reader = std::io::BufReader::new(file);
    let Ok(exif) = exif::Reader::new().read_from_container(&mut reader) else {
        return 1;
    };
    exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY)
        .and_then(|field| field.value.get_uint(0))
        .unwrap_or(1)
}

fn apply_orientation(img: image::DynamicImage, orientation: u32) -> image::DynamicImage {
    match orientation {
        2 => img.fliph(),
        3 => img.rotate180(),
        4 => img.flipv(),
        5 => img.rotate90().fliph(),
        6 => img.rotate90(),
        7 => img.rotate270().fliph(),
        8 => img.rotate270(),
        _ => img,
    }
}

pub fn read_cached(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|_| "Could not read the thumbnail".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn scratch() -> Scratch {
        let path = std::env::temp_dir().join(format!("ownnas-thumbs-{}", rand::random::<u64>()));
        std::fs::create_dir_all(path.join("photos/trip")).unwrap();
        Scratch(path)
    }

    #[test]
    fn photos_become_small_jpegs_that_survive_moves() {
        let dir = scratch();
        let cache = prepare_cache(&dir.0.join("thumbs")).unwrap();
        let photo = dir.0.join("photos/trip/sunset.png");
        let mut image = image::RgbImage::new(1600, 1200);
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            *pixel = image::Rgb([(x % 255) as u8, (y % 255) as u8, ((x + y) % 255) as u8]);
        }
        image.save(&photo).unwrap();
        let thumb = ensure(&photo, &cache, false).unwrap();
        assert_eq!(thumb.extension().unwrap(), "jpg");
        assert_eq!(content_type(&thumb), "image/jpeg");
        let (width, height) = image::image_dimensions(&thumb).unwrap();
        assert_eq!((width, height), (480, 360));

        // Moving the photo (same name, size, and time) finds the same thumbnail.
        let moved = dir.0.join("photos/sunset.png");
        std::fs::rename(&photo, &moved).unwrap();
        assert_eq!(ensure(&moved, &cache, false).unwrap(), thumb);
    }

    #[test]
    fn transparent_pictures_stay_png() {
        let dir = scratch();
        let cache = prepare_cache(&dir.0.join("thumbs")).unwrap();
        let icon = dir.0.join("photos/icon.png");
        let mut image = image::RgbaImage::new(64, 64);
        image.put_pixel(10, 10, image::Rgba([255, 0, 0, 255]));
        image.save(&icon).unwrap();
        let thumb = ensure(&icon, &cache, false).unwrap();
        assert_eq!(thumb.extension().unwrap(), "png");
        assert_eq!(content_type(&thumb), "image/png");
    }

    #[test]
    fn old_caches_are_cleared() {
        let dir = scratch();
        let thumbs = dir.0.join("thumbs");
        std::fs::create_dir_all(thumbs.join("ab")).unwrap();
        std::fs::write(thumbs.join("ab/old.png"), b"x").unwrap();
        prepare_cache(&thumbs).unwrap();
        for _ in 0..100 {
            if !thumbs.join("ab").exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!thumbs.join("ab").exists());
        assert!(thumbs.join(CACHE_VERSION).is_dir());
    }
}
