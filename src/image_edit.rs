//! Making photos smaller. The picture is scaled as stored (a camera's rotation stays in its EXIF
//! data), and a JPEG keeps its EXIF and colour profile, so the date taken, the place, and the
//! rotation survive. Which photo a resized one came from is kept by SHA-256 in `db`
//! (`image_versions`), so phones and the details panel know it is the same photo.

use std::io::Cursor;

use image::{DynamicImage, ImageFormat};

/// How small to make a photo, measured as it shows (after the camera's rotation).
#[derive(Clone, Copy, Debug)]
pub enum Size {
    /// A share of the width and height, from 1 to 99.
    Percent(u32),
    /// Inside a box, keeping the shape.
    Fit { width: u32, height: u32 },
}

impl Size {
    /// How the change is described in a photo's history.
    pub fn describe(&self) -> String {
        match self {
            Size::Percent(percent) => format!("Resized to {percent}%"),
            Size::Fit { width, height } => format!("Resized to fit {width} × {height}"),
        }
    }
}

pub struct Resized {
    pub bytes: Vec<u8>,
    /// As the photo shows, before and after.
    pub from: (u32, u32),
    pub to: (u32, u32),
}

/// Formats that can be read and written again. (GIFs may be animated, and HEIC cannot be
/// written.)
pub fn resizable(name: &str) -> bool {
    format_of(name).is_some()
}

fn format_of(name: &str) -> Option<ImageFormat> {
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => Some(ImageFormat::Jpeg),
        "png" => Some(ImageFormat::Png),
        "webp" => Some(ImageFormat::WebP),
        "bmp" => Some(ImageFormat::Bmp),
        "tif" | "tiff" => Some(ImageFormat::Tiff),
        _ => None,
    }
}

fn orientation(bytes: &[u8]) -> u32 {
    exif::Reader::new()
        .read_from_container(&mut Cursor::new(bytes))
        .ok()
        .and_then(|exif| exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY).and_then(|field| field.value.get_uint(0)))
        .unwrap_or(1)
}

/// The size a photo file shows at (after the camera's rotation), read from its header.
pub fn shown_size(path: &std::path::Path) -> Option<(u32, u32)> {
    let (width, height) = image::image_dimensions(path).ok()?;
    let file = std::fs::File::open(path).ok()?;
    let turned = exif::Reader::new()
        .read_from_container(&mut std::io::BufReader::new(file))
        .ok()
        .and_then(|exif| exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY).and_then(|field| field.value.get_uint(0)))
        .is_some_and(|value| (5..=8).contains(&value));
    Some(if turned { (height, width) } else { (width, height) })
}

/// The photo in `bytes` (a file called `name`), made smaller. `quality` is for JPEGs (1–100).
pub fn resize(bytes: &[u8], name: &str, size: Size, quality: u8) -> Result<Resized, String> {
    let format = format_of(name).ok_or("Only JPEG, PNG, WebP, BMP, and TIFF photos can be resized")?;
    let picture = image::load_from_memory_with_format(bytes, format).map_err(|_| "Could not read the photo".to_string())?;
    // Orientations 5 to 8 turn the picture a quarter: what shows as wide is stored as tall.
    let turned = matches!(orientation(bytes), 5..=8);
    let (stored_w, stored_h) = (picture.width(), picture.height());
    let shown = if turned { (stored_h, stored_w) } else { (stored_w, stored_h) };
    let scale = match size {
        Size::Percent(percent) if (1..100).contains(&percent) => f64::from(percent) / 100.0,
        Size::Percent(_) => return Err("Choose a size between 1% and 99%".into()),
        Size::Fit { width, height } if width > 0 && height > 0 => (f64::from(width) / f64::from(shown.0)).min(f64::from(height) / f64::from(shown.1)),
        Size::Fit { .. } => return Err("Choose a width and a height".into()),
    };
    if scale >= 1.0 {
        return Err(format!("It is {} × {} already: choose a smaller size", shown.0, shown.1));
    }
    let to = (((f64::from(shown.0) * scale).round() as u32).max(1), ((f64::from(shown.1) * scale).round() as u32).max(1));
    let stored_to = if turned { (to.1, to.0) } else { to };
    let smaller = picture.resize_exact(stored_to.0, stored_to.1, image::imageops::FilterType::Lanczos3);
    let bytes = encode(&smaller, format, quality, bytes)?;
    Ok(Resized { bytes, from: shown, to })
}

fn encode(picture: &DynamicImage, format: ImageFormat, quality: u8, original: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    if format == ImageFormat::Jpeg {
        let rgb = picture.to_rgb8();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100))
            .encode_image(&rgb)
            .map_err(|_| "Could not save the photo".to_string())?;
        return Ok(with_metadata(out, original));
    }
    // Formats without an alpha channel, or with one the encoder takes as it is.
    let picture = match (format, picture) {
        (ImageFormat::Bmp, picture) if picture.color().has_alpha() => DynamicImage::ImageRgba8(picture.to_rgba8()),
        (_, picture) => picture.clone(),
    };
    picture.write_to(&mut Cursor::new(&mut out), format).map_err(|_| "Could not save the photo".to_string())?;
    Ok(out)
}

/// `encoded` (a new JPEG) with the original JPEG's EXIF, XMP, colour profile, and IPTC segments,
/// which hold the date taken, the place, the rotation, and the colours.
fn with_metadata(encoded: Vec<u8>, original: &[u8]) -> Vec<u8> {
    let kept = metadata_segments(original);
    if kept.is_empty() || encoded.len() < 2 || encoded[..2] != [0xFF, 0xD8] {
        return encoded;
    }
    // The encoder's own header (APP0, JFIF) goes: EXIF wants to come first.
    let mut rest = &encoded[2..];
    if rest.len() > 4 && rest[0] == 0xFF && rest[1] == 0xE0 {
        let length = usize::from(u16::from_be_bytes([rest[2], rest[3]]));
        if rest.len() > 2 + length {
            rest = &rest[2 + length..];
        }
    }
    let mut out = Vec::with_capacity(encoded.len() + kept.iter().map(|s| s.len()).sum::<usize>());
    out.extend_from_slice(&[0xFF, 0xD8]);
    for segment in kept {
        out.extend_from_slice(segment);
    }
    out.extend_from_slice(rest);
    out
}

/// The APP1 (EXIF, XMP), APP2 (ICC profile), and APP13 (IPTC) segments of a JPEG, as they are.
fn metadata_segments(jpeg: &[u8]) -> Vec<&[u8]> {
    let mut found = Vec::new();
    if jpeg.len() < 4 || jpeg[..2] != [0xFF, 0xD8] {
        return found;
    }
    let mut at = 2;
    while at + 4 <= jpeg.len() && jpeg[at] == 0xFF {
        let marker = jpeg[at + 1];
        // Start of scan: the picture itself follows, and no more headers.
        if marker == 0xDA || marker == 0xD9 {
            break;
        }
        let length = usize::from(u16::from_be_bytes([jpeg[at + 2], jpeg[at + 3]]));
        let end = at + 2 + length;
        if length < 2 || end > jpeg.len() {
            break;
        }
        if matches!(marker, 0xE1 | 0xE2 | 0xED) {
            found.push(&jpeg[at..end]);
        }
        at = end;
    }
    found
}

/// A name for the smaller copy beside the original: "IMG_1234 (1600 × 1200).jpg".
pub fn copy_name(name: &str, to: (u32, u32)) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!("{stem} ({} × {}).{ext}", to.0, to.1),
        _ => format!("{name} ({} × {})", to.0, to.1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg(width: u32, height: u32, orientation: Option<u16>) -> Vec<u8> {
        let picture = DynamicImage::ImageRgb8(image::RgbImage::from_fn(width, height, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 90])));
        let mut plain = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut plain, 90).encode_image(&picture.to_rgb8()).unwrap();
        let Some(orientation) = orientation else { return plain };
        // A minimal EXIF segment (big-endian TIFF) with just the orientation.
        let mut tiff = vec![b'M', b'M', 0, 42, 0, 0, 0, 8, 0, 1, 0x01, 0x12, 0, 3, 0, 0, 0, 1];
        tiff.extend_from_slice(&orientation.to_be_bytes());
        tiff.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        let mut segment = vec![0xFF, 0xE1];
        let payload_len = 2 + 6 + tiff.len();
        segment.extend_from_slice(&(payload_len as u16).to_be_bytes());
        segment.extend_from_slice(b"Exif\0\0");
        segment.extend_from_slice(&tiff);
        let mut out = vec![0xFF, 0xD8];
        out.extend_from_slice(&segment);
        out.extend_from_slice(&plain[2..]);
        out
    }

    #[test]
    fn percent_and_fit_keep_the_shape() {
        let source = jpeg(400, 300, None);
        let half = resize(&source, "a.jpg", Size::Percent(50), 85).unwrap();
        assert_eq!((half.from, half.to), ((400, 300), (200, 150)));
        assert_eq!(image::load_from_memory(&half.bytes).unwrap().width(), 200);
        let fit = resize(&source, "a.jpg", Size::Fit { width: 100, height: 100 }, 85).unwrap();
        assert_eq!(fit.to, (100, 75));
        assert!(resize(&source, "a.jpg", Size::Fit { width: 800, height: 800 }, 85).is_err());
        assert!(resize(&source, "a.gif", Size::Percent(50), 85).is_err());
    }

    #[test]
    fn rotated_photos_are_measured_as_they_show_and_keep_their_exif() {
        // Stored 400 × 300, shown 300 × 400 (orientation 6).
        let source = jpeg(400, 300, Some(6));
        let fit = resize(&source, "a.jpg", Size::Fit { width: 150, height: 1000 }, 85).unwrap();
        assert_eq!((fit.from, fit.to), ((300, 400), (150, 200)));
        let stored = image::load_from_memory(&fit.bytes).unwrap();
        assert_eq!((stored.width(), stored.height()), (200, 150));
        assert_eq!(orientation(&fit.bytes), 6);
    }

    #[test]
    fn png_resizes_and_names_the_copy() {
        let picture = DynamicImage::ImageRgba8(image::RgbaImage::new(64, 32));
        let mut png = Vec::new();
        picture.write_to(&mut Cursor::new(&mut png), ImageFormat::Png).unwrap();
        let half = resize(&png, "shot.png", Size::Percent(25), 85).unwrap();
        assert_eq!(half.to, (16, 8));
        assert_eq!(copy_name("IMG_1.jpg", (16, 8)), "IMG_1 (16 × 8).jpg");
    }
}
