use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, UNIX_EPOCH};

use sha2::{Digest, Sha256};

pub fn ffmpeg_available() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

pub fn ensure(source: &Path, cache_root: &Path, video: bool) -> Result<PathBuf, String> {
    let meta = std::fs::metadata(source).map_err(|_| "Could not read the file".to_string())?;
    let dest = cache_file(cache_root, source, &meta)?;
    if dest.is_file() {
        return Ok(dest);
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|_| "Could not create the thumbnail cache".to_string())?;
    }
    let tmp = temp_png(&dest);
    let generated = if video {
        render_video(source, &tmp)
    } else {
        render_image(source, &tmp)
    };
    if let Err(err) = generated {
        let _ = std::fs::remove_file(&tmp);
        return Err(err);
    }
    if dest.is_file() {
        let _ = std::fs::remove_file(&tmp);
        return Ok(dest);
    }
    std::fs::rename(&tmp, &dest).map_err(|_| "Could not store the thumbnail".to_string())?;
    Ok(dest)
}

fn temp_png(dest: &Path) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    dest.with_file_name(format!(".{}.{stamp}.png", std::process::id()))
}

fn cache_file(cache_root: &Path, source: &Path, meta: &std::fs::Metadata) -> Result<PathBuf, String> {
    let mut hasher = Sha256::new();
    hasher.update(source.to_string_lossy().as_bytes());
    let modified = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    hasher.update(modified.to_le_bytes());
    hasher.update(meta.len().to_le_bytes());
    let hash = hex::encode(hasher.finalize());
    let dir = cache_root.join(&hash[..2]);
    Ok(dir.join(format!("{hash}.png")))
}

fn render_image(source: &Path, dest: &Path) -> Result<(), String> {
    let orientation = read_orientation(source);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(25_000);
    limits.max_image_height = Some(25_000);
    limits.max_alloc = Some(256 * 1024 * 1024);
    let mut reader = image::ImageReader::open(source)
        .map_err(|_| "Could not open the image".to_string())?
        .with_guessed_format()
        .map_err(|_| "Could not recognize the image".to_string())?;
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|_| "Could not decode the image".to_string())?;
    if image.width() == 0 || image.height() == 0 {
        return Err("The image is empty".into());
    }
    let oriented = apply_orientation(image, orientation);
    let thumb = oriented.thumbnail(480, 480);
    thumb
        .save_with_format(dest, image::ImageFormat::Png)
        .map_err(|_| "Could not write the thumbnail".to_string())
}

fn render_video(source: &Path, dest: &Path) -> Result<(), String> {
    if render_video_at(source, dest, "1").is_ok() {
        return Ok(());
    }
    render_video_at(source, dest, "0")
}

fn render_video_at(source: &Path, dest: &Path, seek: &str) -> Result<(), String> {
    let mut child = Command::new("ffmpeg")
        .arg("-y")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-ss")
        .arg(seek)
        .arg("-i")
        .arg(source)
        .arg("-frames:v")
        .arg("1")
        .arg("-vf")
        .arg("scale=480:480:force_original_aspect_ratio=decrease")
        .arg("-f")
        .arg("image2")
        .arg("-c:v")
        .arg("png")
        .arg(dest)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "ffmpeg is not available".to_string())?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() && dest.is_file() => return Ok(()),
            Ok(Some(_)) => return Err("ffmpeg could not make a thumbnail".into()),
            Ok(None) if started.elapsed() > Duration::from_secs(25) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("ffmpeg timed out".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(40)),
            Err(_) => return Err("ffmpeg could not make a thumbnail".into()),
        }
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
