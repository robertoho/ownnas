//! Everything known about one photo or video, for the details panel: the file, the camera and
//! exposure, where it was taken, video streams, and what OwnNAS knows (albums, tags, people).
//! Only reads the file.

use std::path::Path;
use std::process::{Command, Stdio};

use rusqlite::{params, Connection};
use serde_json::{json, Value};

/// A labelled value. `time` (seconds since the Unix epoch) is a real moment the app shows in the
/// viewer's own time zone; `value` is the fallback text.
fn row(label: &str, value: impl Into<String>) -> Value {
    json!({ "label": label, "value": value.into() })
}

fn time_row(label: &str, secs: i64) -> Value {
    json!({ "label": label, "value": format!("{} UTC", wall_clock(secs)), "time": secs })
}

fn section(title: &str, rows: Vec<Value>) -> Option<Value> {
    (!rows.is_empty()).then(|| json!({ "title": title, "rows": rows }))
}

/// A Unix time written as "2024-05-01 18:30:00" (as UTC).
pub fn wall_clock(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rest = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}", rest / 3600, rest / 60 % 60, rest % 60)
}

pub fn size_text(bytes: u64) -> String {
    let mut value = bytes as f64;
    for unit in ["bytes", "KB", "MB", "GB", "TB"] {
        if value < 1024.0 || unit == "TB" {
            return if unit == "bytes" { format!("{bytes} bytes") } else { format!("{value:.1} {unit} ({bytes} bytes)") };
        }
        value /= 1024.0;
    }
    unreachable!()
}

/// The details of `rel` (a library path), whose file is at `full`.
pub fn details(db: &std::sync::Mutex<Connection>, rel: &str, full: &Path) -> Value {
    let name = full.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let meta = std::fs::metadata(full).ok();
    let kind = crate::files::kind_of(&name);
    // Read the file before taking the database, which everything else shares.
    let exif = read_exif(full);
    let video = if kind == "video" { probe_video(full) } else { None };
    let format = image::ImageReader::open(full).ok().and_then(|r| r.with_guessed_format().ok()).and_then(|r| r.format());
    let decoded_size = image::image_dimensions(full).ok();
    let conn = db.lock().unwrap_or_else(|err| err.into_inner());
    let conn = &*conn;
    let indexed: Option<(Option<i64>, Option<String>, Option<i64>, Option<i64>)> = conn
        .query_row("SELECT taken_at, date_source, width, height FROM media_index WHERE path = ?1", [rel], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .ok();

    let mut sections = Vec::new();
    let mut location: Option<(f64, f64)> = None;

    // The file.
    let mut file = vec![row("Name", &name), row("Folder", rel.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("/"))];
    if let Some(meta) = &meta {
        file.push(row("Size", size_text(meta.len())));
        let secs = |time: std::io::Result<std::time::SystemTime>| {
            time.ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64)
        };
        if let Some(modified) = secs(meta.modified()) {
            file.push(time_row("Modified", modified));
        }
        if let Some(created) = secs(meta.created()) {
            file.push(time_row("Created on the server", created));
        }
    }
    let mime = mime_guess::from_path(full).first().map(|m| m.essence_str().to_string());
    if let Some(mime) = mime {
        file.push(row("Type", mime));
    }
    if let Some(format) = format {
        file.push(row("Format", format!("{format:?}").to_uppercase()));
    }
    file.push(row("Path", rel));
    let hash: Option<String> = conn.query_row("SELECT sha256 FROM file_hashes WHERE path = ?1", [rel], |r| r.get(0)).ok();
    if let Some(hash) = &hash {
        file.push(row("SHA-256", hash));
    }
    sections.extend(section("File", file));

    // Which photo this one was made from, and the smaller copies made from it.
    if let Some(hash) = &hash {
        let size = |w: Option<i64>, h: Option<i64>| match (w, h) {
            (Some(w), Some(h)) => format!("{w} × {h}"),
            _ => "?".into(),
        };
        let mut versions = Vec::new();
        for step in crate::db::earlier_versions(conn, hash) {
            versions.push(row(
                &step.operation,
                format!(
                    "From {} ({}), {} UTC by {}",
                    step.parent_path,
                    size(step.parent_width, step.parent_height),
                    wall_clock(step.created_at),
                    if step.created_by.is_empty() { "someone" } else { &step.created_by }
                ),
            ));
            versions.push(row("Made from SHA-256", &step.parent));
        }
        for copy in crate::db::later_versions(conn, hash) {
            let now = crate::db::paths_with_hash(conn, &copy.sha256).into_iter().map(|(p, _, _)| p).find(|p| !crate::files::is_under_trash(p));
            versions.push(row(
                "Smaller copy",
                format!("{} ({}, {}){}", now.as_deref().unwrap_or(&copy.path), size(copy.width, copy.height), copy.operation.to_lowercase(), if now.is_some() { "" } else { " · no longer in the library" }),
            ));
        }
        sections.extend(section("Versions", versions));
    }

    // The picture or video itself.
    let mut picture = Vec::new();
    if let Some((taken, source, width, height)) = &indexed {
        if let Some(taken) = taken {
            let from = match source.as_deref() {
                Some("exif") => " (from the camera)",
                _ => "",
            };
            // A camera's date is its own wall clock; a file's date is a real moment.
            if source.as_deref() == Some("file") {
                picture.push(time_row("Taken (file date)", *taken));
            } else {
                picture.push(row("Taken", format!("{}{from}", wall_clock(*taken))));
            }
        }
        if let (Some(w), Some(h)) = (width, height) {
            picture.push(row("Dimensions", dimensions(*w, *h)));
        }
    }
    if picture.iter().all(|r| r["label"] != "Dimensions") {
        if let Some((w, h)) = decoded_size {
            picture.push(row("Dimensions", dimensions(w.into(), h.into())));
        }
    }
    if let Some(exif) = &exif {
        if let Some(orientation) = exif_uint(exif, exif::Tag::Orientation) {
            let text = match orientation {
                1 => "Normal",
                2 => "Mirrored",
                3 => "Rotated 180°",
                4 => "Mirrored, rotated 180°",
                5 => "Mirrored, rotated 90° anticlockwise",
                6 => "Rotated 90° clockwise",
                7 => "Mirrored, rotated 90° clockwise",
                8 => "Rotated 90° anticlockwise",
                _ => "Unknown",
            };
            picture.push(row("Orientation", text));
        }
        if let Some(space) = exif_text(exif, exif::Tag::ColorSpace) {
            picture.push(row("Colour space", space));
        }
        for (label, tag) in [("Resolution X", exif::Tag::XResolution), ("Resolution Y", exif::Tag::YResolution)] {
            if let Some(value) = exif_text(exif, tag) {
                picture.push(row(label, value));
            }
        }
    }
    sections.extend(section(if kind == "video" { "Video" } else { "Photo" }, picture));

    if let Some(exif) = &exif {
        // When, as the camera wrote it.
        let mut dates = Vec::new();
        for (label, tag, offset) in [
            ("Taken", exif::Tag::DateTimeOriginal, exif::Tag::OffsetTimeOriginal),
            ("Digitized", exif::Tag::DateTimeDigitized, exif::Tag::OffsetTimeDigitized),
            ("Changed", exif::Tag::DateTime, exif::Tag::OffsetTime),
        ] {
            if let Some(value) = exif_text(exif, tag) {
                let zone = exif_text(exif, offset).map(|z| format!(" {z}")).unwrap_or_default();
                dates.push(row(label, format!("{}{zone}", value.trim_matches('"'))));
            }
        }
        if let Some(sub) = exif_text(exif, exif::Tag::SubSecTimeOriginal) {
            dates.push(row("Sub-second", sub.trim_matches('"').to_string()));
        }
        sections.extend(section("Camera dates", dates));

        let camera = labelled(exif, &[
            ("Make", exif::Tag::Make),
            ("Model", exif::Tag::Model),
            ("Lens make", exif::Tag::LensMake),
            ("Lens", exif::Tag::LensModel),
            ("Lens specification", exif::Tag::LensSpecification),
            ("Body serial number", exif::Tag::BodySerialNumber),
            ("Lens serial number", exif::Tag::LensSerialNumber),
            ("Software", exif::Tag::Software),
            ("Owner", exif::Tag::CameraOwnerName),
            ("Artist", exif::Tag::Artist),
            ("Copyright", exif::Tag::Copyright),
            ("Description", exif::Tag::ImageDescription),
            ("Comment", exif::Tag::UserComment),
        ]);
        sections.extend(section("Camera", camera));

        let exposure = labelled(exif, &[
            ("Exposure time", exif::Tag::ExposureTime),
            ("Aperture", exif::Tag::FNumber),
            ("ISO", exif::Tag::PhotographicSensitivity),
            ("Focal length", exif::Tag::FocalLength),
            ("Focal length (35 mm)", exif::Tag::FocalLengthIn35mmFilm),
            ("Exposure program", exif::Tag::ExposureProgram),
            ("Exposure mode", exif::Tag::ExposureMode),
            ("Exposure compensation", exif::Tag::ExposureBiasValue),
            ("Metering", exif::Tag::MeteringMode),
            ("Flash", exif::Tag::Flash),
            ("White balance", exif::Tag::WhiteBalance),
            ("Light source", exif::Tag::LightSource),
            ("Scene", exif::Tag::SceneCaptureType),
            ("Digital zoom", exif::Tag::DigitalZoomRatio),
            ("Brightness", exif::Tag::BrightnessValue),
            ("Shutter speed", exif::Tag::ShutterSpeedValue),
            ("Max aperture", exif::Tag::MaxApertureValue),
            ("Subject distance", exif::Tag::SubjectDistance),
            ("Subject area", exif::Tag::SubjectArea),
            ("Contrast", exif::Tag::Contrast),
            ("Saturation", exif::Tag::Saturation),
            ("Sharpness", exif::Tag::Sharpness),
            ("Sensing method", exif::Tag::SensingMethod),
        ]);
        sections.extend(section("Exposure", exposure));

        let mut place = Vec::new();
        if let Some((lat, lon)) = gps_position(exif) {
            location = Some((lat, lon));
            place.push(row("Coordinates", format!("{lat:.6}, {lon:.6}")));
        }
        for (label, tag) in [
            ("Altitude", exif::Tag::GPSAltitude),
            ("Direction", exif::Tag::GPSImgDirection),
            ("Speed", exif::Tag::GPSSpeed),
            ("Accuracy", exif::Tag::GPSHPositioningError),
            ("GPS time", exif::Tag::GPSTimeStamp),
            ("GPS date", exif::Tag::GPSDateStamp),
        ] {
            if let Some(value) = exif_text(exif, tag) {
                let value = if tag == exif::Tag::GPSAltitude && exif_uint(exif, exif::Tag::GPSAltitudeRef) == Some(1) { format!("-{value} (below sea level)") } else { value };
                place.push(row(label, value));
            }
        }
        sections.extend(section("Location", place));
    }

    if let Some(video) = &video {
        let (rows, place) = video_rows(video);
        sections.extend(section("Streams", rows));
        if location.is_none() {
            if let Some((lat, lon)) = place {
                location = Some((lat, lon));
                sections.extend(section("Location", vec![row("Coordinates", format!("{lat:.6}, {lon:.6}"))]));
            }
        }
    }

    // What OwnNAS knows about it.
    let mut library = Vec::new();
    let albums = crate::albums::albums_for_path(conn, rel).unwrap_or_default();
    let tags = crate::db::list_tags(conn, rel).unwrap_or_default();
    let faces = crate::smart::faces_in(conn, rel).unwrap_or_default();
    let hidden: bool = conn.query_row("SELECT 1 FROM timeline_hidden WHERE path = ?1", [rel], |_| Ok(())).is_ok();
    library.push(row("In Photos", if hidden { "Hidden from Photos" } else { "Shown" }));
    library.push(row("Albums", if albums.is_empty() { "None".to_string() } else { albums.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>().join(", ") }));
    library.push(row("Tags", if tags.is_empty() { "None".to_string() } else { tags.join(", ") }));
    if !faces.is_empty() {
        let named: Vec<&str> = faces.iter().filter_map(|(_, _, name)| name.as_deref()).collect();
        let unnamed = faces.len() - named.len();
        let mut text = named.join(", ");
        if unnamed > 0 {
            text += &format!("{}{unnamed} unnamed", if text.is_empty() { "" } else { ", " });
        }
        library.push(row("People", format!("{} face{}: {text}", faces.len(), if faces.len() == 1 { "" } else { "s" })));
    }
    let comments = crate::db::list_comments(conn, rel).map(|c| c.len()).unwrap_or(0);
    if comments > 0 {
        library.push(row("Comments", comments.to_string()));
    }
    let described: bool = conn.query_row("SELECT 1 FROM image_vectors WHERE path = ?1", params![rel], |_| Ok(())).is_ok();
    let scanned: Option<i64> = conn.query_row("SELECT faces FROM face_scans WHERE path = ?1", params![rel], |r| r.get(0)).ok();
    library.push(row("Smart search", if described { "Indexed" } else { "Not indexed yet" }));
    library.push(row("Face scan", match scanned { Some(n) => format!("Done ({n} found)"), None => "Not scanned yet".into() }));
    sections.push(json!({ "title": "In OwnNAS", "rows": library }));

    // Everything, for the curious.
    if let Some(exif) = &exif {
        let mut all = Vec::new();
        for field in exif.fields() {
            if matches!(field.tag, exif::Tag::MakerNote) {
                all.push(row(&format!("{}", field.tag), "(maker data)"));
                continue;
            }
            let mut value = field.display_value().with_unit(exif).to_string();
            if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
                value = value[1..value.len() - 1].to_string();
            }
            if value.len() > 300 {
                value = format!("{}… ({} characters)", &value[..value.floor_char_boundary(300)], value.len());
            }
            let ifd = if field.ifd_num == exif::In::THUMBNAIL { " (thumbnail)" } else { "" };
            all.push(row(&format!("{}{ifd}", field.tag), value));
        }
        sections.extend(section("All EXIF metadata", all));
    }
    if let Some(video) = &video {
        let mut all = Vec::new();
        flatten("", video, &mut all);
        sections.extend(section("All video metadata", all));
    }

    json!({
        "name": name,
        "path": rel,
        "kind": kind,
        "location": location.map(|(lat, lon)| json!({ "lat": lat, "lon": lon })),
        "albums": albums.iter().map(|(id, name)| json!({ "id": id, "name": name })).collect::<Vec<_>>(),
        "tags": tags,
        "sections": sections,
    })
}

fn dimensions(w: i64, h: i64) -> String {
    let megapixels = (w * h) as f64 / 1_000_000.0;
    format!("{w} × {h} ({megapixels:.1} MP)")
}

fn read_exif(path: &Path) -> Option<exif::Exif> {
    let file = std::fs::File::open(path).ok()?;
    exif::Reader::new().read_from_container(&mut std::io::BufReader::new(file)).ok()
}

fn exif_text(exif: &exif::Exif, tag: exif::Tag) -> Option<String> {
    let field = exif.get_field(tag, exif::In::PRIMARY)?;
    let text = field.display_value().with_unit(exif).to_string();
    let text = text.trim().trim_matches('"').trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn exif_uint(exif: &exif::Exif, tag: exif::Tag) -> Option<u32> {
    exif.get_field(tag, exif::In::PRIMARY)?.value.get_uint(0)
}

fn labelled(exif: &exif::Exif, tags: &[(&str, exif::Tag)]) -> Vec<Value> {
    tags.iter().filter_map(|(label, tag)| exif_text(exif, *tag).map(|value| row(label, value.trim_matches('"').to_string()))).collect()
}

fn gps_position(exif: &exif::Exif) -> Option<(f64, f64)> {
    let degrees = |tag, reference, negative: &[u8]| -> Option<f64> {
        let field = exif.get_field(tag, exif::In::PRIMARY)?;
        let exif::Value::Rational(ref parts) = field.value else { return None };
        let value = parts.iter().take(3).enumerate().map(|(i, part)| part.to_f64() / 60f64.powi(i as i32)).sum::<f64>();
        let sign = match exif.get_field(reference, exif::In::PRIMARY).map(|f| &f.value) {
            Some(exif::Value::Ascii(text)) if text.first().is_some_and(|t| t.first().is_some_and(|c| negative.contains(c))) => -1.0,
            _ => 1.0,
        };
        value.is_finite().then_some(value * sign)
    };
    let lat = degrees(exif::Tag::GPSLatitude, exif::Tag::GPSLatitudeRef, b"Ss")?;
    let lon = degrees(exif::Tag::GPSLongitude, exif::Tag::GPSLongitudeRef, b"Ww")?;
    (lat.abs() <= 90.0 && lon.abs() <= 180.0 && !(lat == 0.0 && lon == 0.0)).then_some((lat, lon))
}

/// ffprobe's view of a video, when ffprobe is installed.
fn probe_video(path: &Path) -> Option<Value> {
    let output = Command::new("ffprobe")
        .args(["-v", "quiet", "-print_format", "json", "-show_format", "-show_streams"])
        .arg(path)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice(&output.stdout).ok()
}

fn video_rows(probe: &Value) -> (Vec<Value>, Option<(f64, f64)>) {
    let mut rows = Vec::new();
    let format = &probe["format"];
    if let Some(duration) = format["duration"].as_str().and_then(|d| d.parse::<f64>().ok()) {
        let secs = duration.round() as i64;
        rows.push(row("Duration", format!("{}:{:02}:{:02} ({duration:.2} s)", secs / 3600, secs / 60 % 60, secs % 60)));
    }
    if let Some(rate) = format["bit_rate"].as_str().and_then(|b| b.parse::<f64>().ok()) {
        rows.push(row("Bit rate", format!("{:.1} Mbit/s", rate / 1_000_000.0)));
    }
    if let Some(name) = format["format_long_name"].as_str() {
        rows.push(row("Container", name));
    }
    for stream in probe["streams"].as_array().into_iter().flatten() {
        let codec = stream["codec_long_name"].as_str().or(stream["codec_name"].as_str()).unwrap_or("?");
        match stream["codec_type"].as_str() {
            Some("video") => {
                let mut text = codec.to_string();
                if let (Some(w), Some(h)) = (stream["width"].as_i64(), stream["height"].as_i64()) {
                    text += &format!(", {w} × {h}");
                }
                if let Some(rate) = stream["avg_frame_rate"].as_str().and_then(fraction) {
                    text += &format!(", {rate:.2} fps");
                }
                if let Some(profile) = stream["profile"].as_str() {
                    text += &format!(", {profile}");
                }
                if let Some(pixels) = stream["pix_fmt"].as_str() {
                    text += &format!(", {pixels}");
                }
                if let Some(transfer) = stream["color_transfer"].as_str() {
                    if matches!(transfer, "arib-std-b67" | "smpte2084") {
                        text += ", HDR";
                    }
                }
                rows.push(row("Video stream", text));
                let rotation = stream["side_data_list"].as_array().into_iter().flatten().find_map(|s| s["rotation"].as_i64());
                if let Some(rotation) = rotation.filter(|r| *r != 0) {
                    rows.push(row("Rotation", format!("{rotation}°")));
                }
            }
            Some("audio") => {
                let mut text = codec.to_string();
                if let Some(rate) = stream["sample_rate"].as_str() {
                    text += &format!(", {rate} Hz");
                }
                if let Some(layout) = stream["channel_layout"].as_str() {
                    text += &format!(", {layout}");
                }
                rows.push(row("Audio stream", text));
            }
            Some(other) => rows.push(row(&format!("{} stream", capitalize(other)), codec)),
            None => {}
        }
    }
    let tags = &format["tags"];
    for (label, key) in [
        ("Recorded", "creation_time"),
        ("Make", "com.apple.quicktime.make"),
        ("Model", "com.apple.quicktime.model"),
        ("Software", "com.apple.quicktime.software"),
        ("Recorded (camera)", "com.apple.quicktime.creationdate"),
    ] {
        if let Some(value) = tags[key].as_str() {
            rows.push(row(label, value));
        }
    }
    let place = ["location", "com.apple.quicktime.location.ISO6709", "location-eng"]
        .iter()
        .find_map(|key| tags[*key].as_str())
        .and_then(iso6709);
    (rows, place)
}

fn fraction(text: &str) -> Option<f64> {
    let (a, b) = text.split_once('/')?;
    let (a, b): (f64, f64) = (a.parse().ok()?, b.parse().ok()?);
    (b != 0.0 && a != 0.0).then(|| a / b)
}

/// "+48.8577+002.2950+035.000/" → (48.8577, 2.295).
fn iso6709(text: &str) -> Option<(f64, f64)> {
    let mut numbers = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices() {
        if c == '+' || c == '-' || c == '/' {
            if let Some(s) = start {
                numbers.push(text[s..i].parse::<f64>().ok()?);
            }
            start = (c != '/').then_some(i);
        }
    }
    if let Some(s) = start {
        numbers.push(text[s..].parse::<f64>().ok()?);
    }
    match numbers[..] {
        [lat, lon, ..] if lat.abs() <= 90.0 && lon.abs() <= 180.0 => Some((lat, lon)),
        _ => None,
    }
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
}

/// Every scalar in ffprobe's answer, as "streams.0.codec_name: h264".
fn flatten(prefix: &str, value: &Value, out: &mut Vec<Value>) {
    match value {
        Value::Object(map) => {
            for (key, inner) in map {
                flatten(&if prefix.is_empty() { key.clone() } else { format!("{prefix}.{key}") }, inner, out);
            }
        }
        Value::Array(items) => {
            for (i, inner) in items.iter().enumerate() {
                flatten(&format!("{prefix}.{i}"), inner, out);
            }
        }
        Value::String(text) => out.push(row(prefix, text.clone())),
        Value::Null => {}
        other => out.push(row(prefix, other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_places() {
        assert_eq!(wall_clock(0), "1970-01-01 00:00:00");
        assert_eq!(wall_clock(1_700_000_000), "2023-11-14 22:13:20");
        assert_eq!(iso6709("+48.8577+002.2950+035.000/"), Some((48.8577, 2.295)));
        assert_eq!(iso6709("-33.8688+151.2093/"), Some((-33.8688, 151.2093)));
        assert_eq!(iso6709("nonsense"), None);
        assert_eq!(fraction("30000/1001").map(|f| (f * 100.0).round()), Some(2997.0));
    }
}
