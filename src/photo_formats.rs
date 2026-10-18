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

/// Layered pictures from editing apps, shown flattened: Photoshop, GIMP, Krita, OpenRaster,
/// Affinity, Sketch, Procreate, and CorelDRAW (most keep a flattened copy inside).
pub fn is_layered(extension: &str) -> bool {
    matches!(extension, "psd" | "psb" | "xcf" | "kra" | "ora" | "afphoto" | "afdesign" | "sketch" | "procreate" | "cdr")
}

/// Picture formats the image library reads but browsers do not show.
pub fn is_other_raster(extension: &str) -> bool {
    matches!(extension, "tga" | "dds" | "exr" | "hdr" | "qoi" | "pbm" | "pgm" | "ppm" | "pnm" | "pam" | "ff" | "icns")
}

/// Whether a photo with this extension is read here rather than by the image library.
pub fn handles(extension: &str) -> bool {
    is_raw(extension) || is_jxl(extension) || is_layered(extension) || is_other_raster(extension)
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
    if handles(&extension) {
        let picture = decode(path)?;
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
    Err("This picture format is not read here".into())
}

/// The photo decoded and turned upright.
pub fn decode(path: &Path) -> Result<DynamicImage, String> {
    let extension = extension_of(path);
    if is_jxl(&extension) {
        return decode_jxl(path);
    }
    if is_layered(&extension) || is_other_raster(&extension) {
        return decode_other(path, &extension);
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
    if is_layered(&extension) || is_other_raster(&extension) {
        return decode_other(path, &extension).ok().map(|picture| (picture.width(), picture.height()));
    }
    let jpeg = raw_preview(path).ok()?;
    let (width, height) = image::ImageReader::with_format(Cursor::new(&jpeg), image::ImageFormat::Jpeg).into_dimensions().ok()?;
    Some(if matches!(jpeg_orientation(&jpeg), Some(5..=8)) { (height, width) } else { (width, height) })
}

/// The largest file read for a layered or other picture.
const MAX_OTHER: u64 = 512 * 1024 * 1024;

fn decode_other(path: &Path, extension: &str) -> Result<DynamicImage, String> {
    let size = std::fs::metadata(path).map_err(|_| "Could not read the picture".to_string())?.len();
    if size > MAX_OTHER {
        return Err("This picture is too large to preview".into());
    }
    match extension {
        "psd" | "psb" => {
            let bytes = std::fs::read(path).map_err(|_| "Could not read the picture".to_string())?;
            photoshop_composite(&bytes).or_else(|| photoshop_thumbnail(&bytes)).ok_or_else(|| "This Photoshop file could not be read".to_string())
        }
        "xcf" => std::panic::catch_unwind(|| flatten_xcf(path)).unwrap_or_else(|_| Err("This GIMP file could not be read".into())),
        // Zips with a flattened picture inside.
        "kra" | "ora" | "sketch" | "procreate" | "cdr" => {
            let names: &[&str] = match extension {
                "kra" | "ora" => &["mergedimage.png", "Thumbnails/thumbnail.png", "preview.png"],
                "sketch" => &["previews/preview.png"],
                "procreate" => &["QuickLook/Thumbnail.png"],
                _ => &["previews/thumbnail.png", "metadata/thumbnails/thumbnail.png"],
            };
            let file = std::fs::File::open(path).map_err(|_| "Could not read the picture".to_string())?;
            let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file)).map_err(|_| "This file could not be opened".to_string())?;
            for name in names {
                if let Ok(mut part) = zip.by_name(name) {
                    let mut bytes = Vec::new();
                    std::io::Read::read_to_end(&mut part, &mut bytes).map_err(|_| "Could not read the picture".to_string())?;
                    return image::load_from_memory(&bytes).map_err(|_| "The picture inside could not be read".to_string());
                }
            }
            Err("This file keeps no picture of itself inside".into())
        }
        // Affinity keeps PNG previews inside: the largest is shown.
        "afphoto" | "afdesign" => {
            let data = std::fs::read(path).map_err(|_| "Could not read the picture".to_string())?;
            let png = largest_png(&data).ok_or("This Affinity file has no preview inside")?;
            image::load_from_memory_with_format(png, image::ImageFormat::Png).map_err(|_| "The preview could not be read".to_string())
        }
        "icns" => {
            let file = std::fs::File::open(path).map_err(|_| "Could not read the icon".to_string())?;
            let family = icns::IconFamily::read(std::io::BufReader::new(file)).map_err(|_| "This icon file could not be read".to_string())?;
            let biggest = family.available_icons().into_iter().max_by_key(|kind| kind.pixel_width()).ok_or("This icon file is empty")?;
            let icon = family.get_icon_with_type(biggest).map_err(|_| "This icon could not be read".to_string())?;
            let rgba = icon.convert_to(icns::PixelFormat::RGBA);
            image::RgbaImage::from_raw(rgba.width(), rgba.height(), rgba.into_data().into_vec()).map(DynamicImage::ImageRgba8).ok_or_else(|| "This icon could not be read".to_string())
        }
        _ => {
            let mut reader = image::ImageReader::open(path).map_err(|_| "Could not read the picture".to_string())?.with_guessed_format().map_err(|_| "Could not read the picture".to_string())?;
            let mut limits = image::Limits::default();
            limits.max_alloc = Some(512 * 1024 * 1024);
            reader.limits(limits);
            let picture = reader.decode().map_err(|_| "This picture could not be read".to_string())?;
            // High dynamic range pictures (EXR, HDR) are brought down to what a screen shows.
            Ok(match picture {
                DynamicImage::ImageRgb32F(_) | DynamicImage::ImageRgba32F(_) => tone_map(&picture),
                other => other,
            })
        }
    }
}

/// The parts of a Photoshop file: where the flattened picture is, and how it is stored.
struct Photoshop<'a> {
    data: &'a [u8],
    big: bool,
    channels: usize,
    height: usize,
    width: usize,
    depth: u16,
    mode: u16,
    /// Where the image resources start, and where the flattened picture starts.
    resources: usize,
    image: usize,
}

fn be(data: &[u8], at: usize, bytes: usize) -> Option<u64> {
    data.get(at..at + bytes).map(|b| b.iter().fold(0u64, |n, &x| (n << 8) | u64::from(x)))
}

fn photoshop(data: &[u8]) -> Option<Photoshop<'_>> {
    if data.get(..4)? != b"8BPS" {
        return None;
    }
    let big = be(data, 4, 2)? == 2;
    let channels = be(data, 12, 2)? as usize;
    let height = be(data, 14, 4)? as usize;
    let width = be(data, 18, 4)? as usize;
    let depth = be(data, 22, 2)? as u16;
    let mode = be(data, 24, 2)? as u16;
    let mut at = 26;
    at += 4 + be(data, at, 4)? as usize;
    let resources = at;
    at += 4 + be(data, at, 4)? as usize;
    let layer_len = if big { be(data, at, 8)? as usize + 8 } else { be(data, at, 4)? as usize + 4 };
    at += layer_len;
    Some(Photoshop { data, big, channels, height, width, depth, mode, resources, image: at })
}

/// The flattened picture Photoshop saves after the layers ("maximize compatibility").
fn photoshop_composite(data: &[u8]) -> Option<DynamicImage> {
    let psd = photoshop(data)?;
    let (width, height) = (psd.width, psd.height);
    if width == 0 || height == 0 || width * height > 200_000_000 || !matches!(psd.depth, 8 | 16) {
        return None;
    }
    // Grayscale, RGB, and CMYK; the first channels are the colours, the next the transparency.
    let colours = match psd.mode {
        1 => 1,
        3 => 3,
        4 => 4,
        _ => return None,
    };
    if psd.channels < colours {
        return None;
    }
    let bytes_per = usize::from(psd.depth / 8);
    let row = width * bytes_per;
    let compression = be(data, psd.image, 2)?;
    let mut at = psd.image + 2;
    let wanted = colours + usize::from(psd.channels > colours);
    let mut planes: Vec<Vec<u8>> = Vec::with_capacity(wanted);
    match compression {
        0 => {
            for _ in 0..wanted {
                planes.push(data.get(at..at + row * height)?.to_vec());
                at += row * height;
            }
        }
        1 => {
            // The length of every row of every channel comes first, then the rows (PackBits).
            let count_size = if psd.big { 4 } else { 2 };
            let counts: Vec<usize> = (0..psd.channels * height).map(|i| be(data, at + i * count_size, count_size).map(|n| n as usize)).collect::<Option<_>>()?;
            at += psd.channels * height * count_size;
            for channel in 0..wanted {
                let mut plane = Vec::with_capacity(row * height);
                for y in 0..height {
                    let length = counts[channel * height + y];
                    unpack_bits(data.get(at..at + length)?, &mut plane, row);
                    at += length;
                }
                planes.push(plane);
            }
        }
        _ => return None,
    }
    let sample = |plane: &Vec<u8>, i: usize| -> u8 { plane.get(i * bytes_per).copied().unwrap_or(0) };
    let mut out = image::RgbaImage::new(width as u32, height as u32);
    for (i, pixel) in out.pixels_mut().enumerate() {
        let alpha = if wanted > colours { sample(&planes[colours], i) } else { 255 };
        *pixel = match colours {
            1 => {
                let v = sample(&planes[0], i);
                image::Rgba([v, v, v, alpha])
            }
            3 => image::Rgba([sample(&planes[0], i), sample(&planes[1], i), sample(&planes[2], i), alpha]),
            _ => {
                // Photoshop stores CMYK inverted (255 is no ink).
                let k = f32::from(sample(&planes[3], i)) / 255.0;
                let channel = |c: u8| (f32::from(c) * k).round() as u8;
                image::Rgba([channel(sample(&planes[0], i)), channel(sample(&planes[1], i)), channel(sample(&planes[2], i)), alpha])
            }
        };
    }
    Some(DynamicImage::ImageRgba8(out))
}

/// PackBits run-length decoding of one row, `row` bytes long.
fn unpack_bits(input: &[u8], out: &mut Vec<u8>, row: usize) {
    let end = out.len() + row;
    let mut at = 0;
    while at < input.len() && out.len() < end {
        let header = input[at] as i8;
        at += 1;
        if header >= 0 {
            let count = header as usize + 1;
            out.extend_from_slice(&input[at..(at + count).min(input.len())]);
            at += count;
        } else if header != -128 {
            let count = (1 - i32::from(header)) as usize;
            if let Some(&value) = input.get(at) {
                out.extend(std::iter::repeat_n(value, count));
            }
            at += 1;
        }
    }
    out.resize(end, 0);
}

/// The small JPEG thumbnail Photoshop keeps in its image resources (number 1036).
fn photoshop_thumbnail(data: &[u8]) -> Option<DynamicImage> {
    let psd = photoshop(data)?;
    let end = psd.resources + 4 + be(data, psd.resources, 4)? as usize;
    let mut at = psd.resources + 4;
    while at + 12 <= end {
        if data.get(at..at + 4)? != b"8BIM" {
            return None;
        }
        let id = be(data, at + 4, 2)?;
        let name_len = be(data, at + 6, 1)? as usize;
        let name_total = (1 + name_len + 1) & !1;
        let size_at = at + 6 + name_total;
        let size = be(data, size_at, 4)? as usize;
        let body = size_at + 4;
        if id == 1036 || id == 1033 {
            // A 28-byte header, then the JPEG.
            return image::load_from_memory(data.get(body + 28..body + size)?).ok();
        }
        at = body + ((size + 1) & !1);
    }
    None
}

/// Linear light as the screen shows it: gamma-corrected, with values brighter than white
/// squeezed into range (Reinhard) only when the picture has any.
fn tone_map(picture: &DynamicImage) -> DynamicImage {
    let float = picture.to_rgba32f();
    let brightest = float.pixels().flat_map(|p| [p[0], p[1], p[2]]).fold(0.0f32, f32::max);
    let squeeze = brightest > 1.0;
    let mut out = image::RgbaImage::new(float.width(), float.height());
    for (source, target) in float.pixels().zip(out.pixels_mut()) {
        let map = |v: f32| {
            let v = v.max(0.0);
            let v = if squeeze { v / (1.0 + v) } else { v };
            (v.powf(1.0 / 2.2) * 255.0).round().min(255.0) as u8
        };
        *target = image::Rgba([map(source[0]), map(source[1]), map(source[2]), (source[3].clamp(0.0, 1.0) * 255.0) as u8]);
    }
    DynamicImage::ImageRgba8(out)
}

/// The largest whole PNG inside `data`.
fn largest_png(data: &[u8]) -> Option<&[u8]> {
    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    let mut best: Option<&[u8]> = None;
    let mut at = 0;
    while let Some(found) = data[at..].windows(8).position(|w| w == SIGNATURE) {
        let start = at + found;
        if let Some(end) = data[start..].windows(8).position(|w| &w[4..] == b"IEND").map(|i| start + i + 12) {
            if end <= data.len() && best.is_none_or(|b| end - start > b.len()) {
                best = Some(&data[start..end]);
            }
        }
        at = start + 8;
    }
    best
}

/// A GIMP picture's visible layers, laid over each other (normal mode, with their opacity).
fn flatten_xcf(path: &Path) -> Result<DynamicImage, String> {
    let xcf = xcf::Xcf::open(path).map_err(|_| "This GIMP file could not be read".to_string())?;
    let (width, height) = xcf.dimensions();
    if u64::from(width) * u64::from(height) > 200_000_000 {
        return Err("This picture is too large to preview".into());
    }
    let mut canvas = image::RgbaImage::new(width, height);
    let property = |layer: &xcf::Layer, kind: xcf::PropertyIdentifier| {
        layer.properties.iter().find(|p| p.kind == kind).and_then(|p| match &p.payload {
            xcf::PropertyPayload::Unknown(bytes) => Some(bytes.clone()),
            _ => None,
        })
    };
    // The first layer is the top one.
    for layer in xcf.layers.iter().rev() {
        let visible = property(layer, xcf::PropertyIdentifier::PropVisible).and_then(|b| b.get(..4).map(|v| u32::from_be_bytes([v[0], v[1], v[2], v[3]]))).unwrap_or(1);
        if visible == 0 {
            continue;
        }
        let opacity = property(layer, xcf::PropertyIdentifier::PropOpacity).and_then(|b| b.get(..4).map(|v| u32::from_be_bytes([v[0], v[1], v[2], v[3]]))).unwrap_or(255).min(255) as f32 / 255.0;
        let (dx, dy) = property(layer, xcf::PropertyIdentifier::PropOffsets)
            .and_then(|b| (b.len() >= 8).then(|| (i32::from_be_bytes([b[0], b[1], b[2], b[3]]), i32::from_be_bytes([b[4], b[5], b[6], b[7]]))))
            .unwrap_or((0, 0));
        for y in 0..layer.height {
            for x in 0..layer.width {
                let (cx, cy) = (x as i64 + i64::from(dx), y as i64 + i64::from(dy));
                if cx < 0 || cy < 0 || cx >= i64::from(width) || cy >= i64::from(height) {
                    continue;
                }
                let Some(pixel) = layer.pixel(x, y) else { continue };
                let alpha = f32::from(pixel.a()) / 255.0 * opacity;
                if alpha <= 0.0 {
                    continue;
                }
                let below = canvas.get_pixel_mut(cx as u32, cy as u32);
                let under = f32::from(below[3]) / 255.0;
                let out_alpha = alpha + under * (1.0 - alpha);
                let mix = |top: u8, bottom: u8| ((f32::from(top) * alpha + f32::from(bottom) * under * (1.0 - alpha)) / out_alpha).round() as u8;
                *below = image::Rgba([mix(pixel.r(), below[0]), mix(pixel.g(), below[1]), mix(pixel.b(), below[2]), (out_alpha * 255.0).round() as u8]);
            }
        }
    }
    Ok(DynamicImage::ImageRgba8(canvas))
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
    fn reads_photoshops_flattened_picture() {
        // 2 × 1 RGB, RLE: red, then blue.
        let mut psd = b"8BPS\x00\x01\x00\x00\x00\x00\x00\x00".to_vec();
        psd.extend_from_slice(&[0, 3, 0, 0, 0, 1, 0, 0, 0, 2, 0, 8, 0, 3]);
        psd.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        psd.extend_from_slice(&[0, 1]);
        // Row lengths, then each channel's row: a literal run of two bytes.
        psd.extend_from_slice(&[0, 3, 0, 3, 0, 3]);
        psd.extend_from_slice(&[1, 255, 0, 1, 0, 0, 1, 0, 255]);
        let picture = photoshop_composite(&psd).unwrap().to_rgba8();
        assert_eq!((picture.width(), picture.height()), (2, 1));
        assert_eq!(picture.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(picture.get_pixel(1, 0).0, [0, 0, 255, 255]);
        let mut row = Vec::new();
        unpack_bits(&[0xFE, 7, 1, 9], &mut row, 4);
        assert_eq!(row, [7, 7, 7, 9]);
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
