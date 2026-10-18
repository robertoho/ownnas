//! Photos the image library cannot read on its own: cameras' RAW files and JPEG XL.
//!
//! A RAW file keeps the camera's own JPEG of the picture next to the sensor data (full size on
//! most cameras), so showing one needs no RAW decoding: the largest JPEG inside is taken as it
//! is. JPEG XL is decoded in pure Rust (`jxl-oxide`). Files are only ever read.

use std::io::Cursor;
use std::path::Path;

use image::DynamicImage;

/// The largest RAW file read whole (the embedded JPEG can sit anywhere in it).
const MAX_RAW: u64 = 400 * 1024 * 1024;

/// Cameras' RAW formats.
pub fn is_raw(extension: &str) -> bool {
    matches!(
        extension,
        "cr2" | "cr3" | "crw" | "nef" | "nrw" | "arw" | "srf" | "sr2" | "dng" | "raf" | "orf" | "rw2" | "rwl" | "pef"
            | "srw" | "x3f" | "3fr" | "iiq" | "erf" | "mef" | "mos" | "kdc" | "dcr"
    )
}

pub fn is_jxl(extension: &str) -> bool {
    extension == "jxl"
}

/// Whether a photo with this extension is read here rather than by the image library.
pub fn handles(extension: &str) -> bool {
    is_raw(extension) || is_jxl(extension)
}

fn extension_of(path: &Path) -> String {
    path.extension().map(|ext| ext.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

/// The photo as a browser shows it: the camera's JPEG for a RAW file (with the rotation it
/// should show at), a JPEG or PNG for JPEG XL. Returns the bytes and their media type.
pub fn displayable(path: &Path) -> Result<(Vec<u8>, &'static str), String> {
    let extension = extension_of(path);
    if is_raw(&extension) {
        return Ok((raw_preview(path)?, "image/jpeg"));
    }
    if is_jxl(&extension) {
        let picture = decode_jxl(path)?;
        let mut out = Vec::new();
        if picture.color().has_alpha() {
            picture.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png).map_err(|_| "Could not convert the photo".to_string())?;
            return Ok((out, "image/png"));
        }
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90)
            .encode_image(&picture.to_rgb8())
            .map_err(|_| "Could not convert the photo".to_string())?;
        return Ok((out, "image/jpeg"));
    }
    Err("This is not a RAW or JPEG XL photo".into())
}

/// The photo decoded and turned upright.
pub fn decode(path: &Path) -> Result<DynamicImage, String> {
    let extension = extension_of(path);
    if is_jxl(&extension) {
        return decode_jxl(path);
    }
    let jpeg = raw_preview(path)?;
    let picture = image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg).map_err(|_| "Could not read the camera's preview".to_string())?;
    Ok(crate::thumbs::apply_orientation(picture, jpeg_orientation(&jpeg).unwrap_or(1)))
}

/// The size the photo shows at (after the camera's rotation).
pub fn shown_size(path: &Path) -> Option<(u32, u32)> {
    let extension = extension_of(path);
    if is_jxl(&extension) {
        let image = jxl_oxide::JxlImage::builder().open(path).ok()?;
        return Some((image.width(), image.height()));
    }
    let jpeg = raw_preview(path).ok()?;
    let (width, height) = image::ImageReader::with_format(Cursor::new(&jpeg), image::ImageFormat::Jpeg).into_dimensions().ok()?;
    Some(if matches!(jpeg_orientation(&jpeg), Some(5..=8)) { (height, width) } else { (width, height) })
}

fn decode_jxl(path: &Path) -> Result<DynamicImage, String> {
    let file = std::fs::File::open(path).map_err(|_| "Could not open the photo".to_string())?;
    let decoder = jxl_oxide::integration::JxlDecoder::new(std::io::BufReader::new(file)).map_err(|_| "Could not read the JPEG XL photo".to_string())?;
    DynamicImage::from_decoder(decoder).map_err(|_| "Could not decode the JPEG XL photo".to_string())
}

/// The largest JPEG inside a RAW file, with the camera's rotation written into it when it
/// has none of its own (so browsers and thumbnails show it the right way up).
pub fn raw_preview(path: &Path) -> Result<Vec<u8>, String> {
    let size = std::fs::metadata(path).map_err(|_| "Could not read the photo".to_string())?.len();
    if size > MAX_RAW {
        return Err("This RAW file is too large to preview".into());
    }
    let data = std::fs::read(path).map_err(|_| "Could not read the photo".to_string())?;
    let (start, end) = largest_jpeg(&data).ok_or("This RAW file has no preview picture inside")?;
    let jpeg = &data[start..end];
    if jpeg_orientation(jpeg).is_some() {
        return Ok(jpeg.to_vec());
    }
    let orientation = container_orientation(path, &data).unwrap_or(1);
    Ok(if orientation == 1 { jpeg.to_vec() } else { with_orientation(jpeg, orientation as u16) })
}

/// The span of the largest baseline or progressive JPEG in `data`.
fn largest_jpeg(data: &[u8]) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize, u64)> = None;
    let mut at = 0;
    while at + 3 < data.len() {
        let Some(found) = data[at..].windows(3).position(|w| w == [0xFF, 0xD8, 0xFF]) else { break };
        let start = at + found;
        match jpeg_span(data, start) {
            Some((end, pixels)) => {
                if best.is_none_or(|(_, _, most)| pixels > most) {
                    best = Some((start, end, pixels));
                }
                // A thumbnail can sit inside a bigger JPEG's EXIF: look inside too.
                at = start + 3;
            }
            None => at = start + 3,
        }
    }
    best.map(|(start, end, _)| (start, end))
}

/// Where the JPEG starting at `start` ends, and its pixel count, if it is one the image
/// library can decode (not the lossless kind RAW files keep their sensor data in).
fn jpeg_span(data: &[u8], start: usize) -> Option<(usize, u64)> {
    let mut at = start + 2;
    let mut pixels = 0u64;
    loop {
        // Markers may be padded with extra 0xFF bytes.
        while at + 1 < data.len() && data[at] == 0xFF && data[at + 1] == 0xFF {
            at += 1;
        }
        if at + 1 >= data.len() || data[at] != 0xFF {
            return None;
        }
        let marker = data[at + 1];
        at += 2;
        match marker {
            0xD9 => return (pixels > 0).then_some((at, pixels)),
            0xD8 => return None,
            0x01 | 0xD0..=0xD7 => continue,
            _ => {}
        }
        if at + 2 > data.len() {
            return None;
        }
        let length = usize::from(u16::from_be_bytes([data[at], data[at + 1]]));
        if length < 2 || at + length > data.len() {
            return None;
        }
        match marker {
            0xC0..=0xC2 if length >= 7 => {
                let height = u16::from_be_bytes([data[at + 3], data[at + 4]]);
                let width = u16::from_be_bytes([data[at + 5], data[at + 6]]);
                if width == 0 || height == 0 {
                    return None;
                }
                pixels = u64::from(width) * u64::from(height);
            }
            // Lossless, hierarchical, or arithmetic-coded: not something to show.
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => return None,
            _ => {}
        }
        at += length;
        if marker == 0xDA {
            if pixels == 0 {
                return None;
            }
            // The compressed picture runs until the next marker that is not stuffing or a restart.
            loop {
                let next = data[at..].iter().position(|&b| b == 0xFF)?;
                at += next;
                let following = *data.get(at + 1)?;
                if following == 0x00 || following == 0xFF || (0xD0..=0xD7).contains(&following) {
                    at += if following == 0xFF { 1 } else { 2 };
                    continue;
                }
                break;
            }
        }
    }
}

fn orientation_of(exif: &exif::Exif) -> Option<u32> {
    exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY).and_then(|field| field.value.get_uint(0))
}

/// The rotation written in a JPEG's own EXIF, if any.
fn jpeg_orientation(jpeg: &[u8]) -> Option<u32> {
    let exif = exif::Reader::new().read_from_container(&mut Cursor::new(jpeg)).ok()?;
    orientation_of(&exif)
}

/// The camera's rotation, from the RAW file's own EXIF: TIFF-based RAWs are read as TIFF, and
/// Canon's CR3 keeps its EXIF in a "CMT1" box.
fn container_orientation(path: &Path, data: &[u8]) -> Option<u32> {
    if let Ok(file) = std::fs::File::open(path) {
        if let Ok(exif) = exif::Reader::new().read_from_container(&mut std::io::BufReader::new(file)) {
            if let Some(orientation) = orientation_of(&exif) {
                return Some(orientation);
            }
        }
    }
    let head = &data[..data.len().min(1024 * 1024)];
    let box_at = head.windows(4).position(|w| w == b"CMT1")?;
    let box_start = box_at.checked_sub(4)?;
    let box_len = u32::from_be_bytes(data.get(box_start..box_at)?.try_into().ok()?) as usize;
    let tiff = data.get(box_at + 4..box_start + box_len)?;
    let exif = exif::Reader::new().read_raw(tiff.to_vec()).ok()?;
    orientation_of(&exif)
}

/// `jpeg` with a small EXIF segment holding just the rotation, right after its start.
fn with_orientation(jpeg: &[u8], orientation: u16) -> Vec<u8> {
    let mut tiff = vec![b'M', b'M', 0, 42, 0, 0, 0, 8, 0, 1, 0x01, 0x12, 0, 3, 0, 0, 0, 1];
    tiff.extend_from_slice(&orientation.to_be_bytes());
    tiff.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    let mut out = Vec::with_capacity(jpeg.len() + 64);
    out.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0xE1]);
    out.extend_from_slice(&((2 + 6 + tiff.len()) as u16).to_be_bytes());
    out.extend_from_slice(b"Exif\0\0");
    out.extend_from_slice(&tiff);
    out.extend_from_slice(&jpeg[2..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg(width: u32, height: u32) -> Vec<u8> {
        let picture = image::RgbImage::from_fn(width, height, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 40]));
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 80).encode_image(&picture).unwrap();
        out
    }

    #[test]
    fn finds_the_largest_jpeg_in_a_raw_file() {
        let small = jpeg(16, 12);
        let big = jpeg(64, 48);
        // A made-up RAW: a TIFF header, a thumbnail, sensor data with stray marker bytes and a
        // lossless JPEG (which cannot be shown), then the big preview.
        let mut raw = b"II*\0\x08\0\0\0".to_vec();
        raw.extend_from_slice(&small);
        raw.extend_from_slice(&[0x12, 0xFF, 0xD8, 0xFF, 0x00, 0x33]);
        raw.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0xC3, 0x00, 0x0B, 8, 0, 4, 0, 4, 1, 1, 0x11, 0, 0xFF, 0xD9]);
        let at = raw.len();
        raw.extend_from_slice(&big);
        raw.extend_from_slice(&[0u8; 100]);
        assert_eq!(largest_jpeg(&raw), Some((at, at + big.len())));

        let dir = std::env::temp_dir().join(format!("ownnas-raw-{}-{}", std::process::id(), rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("photo.nef");
        std::fs::write(&path, &raw).unwrap();
        let (bytes, media) = displayable(&path).unwrap();
        assert_eq!((bytes.len(), media), (big.len(), "image/jpeg"));
        assert_eq!(shown_size(&path), Some((64, 48)));
        assert_eq!(decode(&path).unwrap().width(), 64);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn writes_the_rotation_into_a_preview_without_one() {
        let turned = with_orientation(&jpeg(40, 20), 6);
        assert_eq!(jpeg_orientation(&turned), Some(6));
        let picture = image::load_from_memory(&turned).unwrap();
        assert_eq!((picture.width(), picture.height()), (40, 20));
    }
}

#[cfg(test)]
mod samples {
    /// `OWNNAS_RAW_SAMPLES=dir cargo test samples -- --nocapture`: what each real file gives.
    #[test]
    fn real_samples() {
        let Ok(dir) = std::env::var("OWNNAS_RAW_SAMPLES") else { return };
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            let started = std::time::Instant::now();
            let shown = super::displayable(&path).map(|(bytes, media)| (bytes.len(), media));
            let orientation = super::raw_preview(&path).ok().and_then(|jpeg| super::jpeg_orientation(&jpeg));
            println!("{:?}: {:?} size {:?} orientation {:?} in {:?}", path.file_name().unwrap(), shown, super::shown_size(&path), orientation, started.elapsed());
        }
    }
}
