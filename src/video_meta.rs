//! When and where a video was filmed, read from an MP4 or QuickTime file itself (no ffprobe):
//! Apple's `com.apple.quicktime.creationdate` (the local time, as phones show it) or the movie
//! header's creation time (UTC), and the place from `com.apple.quicktime.location.ISO6709` or
//! the `©xyz` box that Android phones and cameras write.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Seconds from 1904 (QuickTime's epoch) to 1970.
const FROM_1904: i64 = 2_082_844_800;
/// The movie box is read only up to this size.
const MAX_MOOV: u64 = 64 * 1024 * 1024;

#[derive(Debug, Default, PartialEq)]
pub struct Filmed {
    /// A local date and time with no time zone, stored as if it were UTC (as for photos).
    pub local: Option<i64>,
    /// A real instant (seconds since 1970, UTC).
    pub utc: Option<i64>,
    pub place: Option<(f64, f64)>,
}

pub fn read(path: &Path) -> Filmed {
    let extension = path.extension().map(|ext| ext.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if !matches!(extension.as_str(), "mp4" | "m4v" | "mov" | "3gp" | "insv") {
        return Filmed::default();
    }
    movie_box(path).map(|moov| parse_moov(&moov)).unwrap_or_default()
}

/// The `moov` box's contents, wherever it is in the file.
fn movie_box(path: &Path) -> Option<Vec<u8>> {
    let mut file = std::fs::File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let mut at = 0u64;
    while at + 8 <= length {
        file.seek(SeekFrom::Start(at)).ok()?;
        let mut header = [0u8; 16];
        file.read_exact(&mut header[..8]).ok()?;
        let mut size = u64::from(u32::from_be_bytes(header[..4].try_into().ok()?));
        let mut skip = 8u64;
        if size == 1 {
            file.read_exact(&mut header[8..16]).ok()?;
            size = u64::from_be_bytes(header[8..16].try_into().ok()?);
            skip = 16;
        } else if size == 0 {
            size = length - at;
        }
        if size < skip {
            return None;
        }
        if &header[4..8] == b"moov" {
            let inner = size - skip;
            if inner > MAX_MOOV {
                return None;
            }
            let mut data = vec![0u8; inner as usize];
            file.read_exact(&mut data).ok()?;
            return Some(data);
        }
        at += size;
    }
    None
}

/// The boxes in `data`: (type, contents).
fn boxes(data: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at + 8 <= data.len() {
        let size = u32::from_be_bytes(data[at..at + 4].try_into().unwrap()) as usize;
        let kind: [u8; 4] = data[at + 4..at + 8].try_into().unwrap();
        let (start, end) = match size {
            0 => (at + 8, data.len()),
            1 if at + 16 <= data.len() => {
                let big = u64::from_be_bytes(data[at + 8..at + 16].try_into().unwrap()) as usize;
                (at + 16, at.saturating_add(big))
            }
            _ => (at + 8, at.saturating_add(size)),
        };
        if end > data.len() || end < start {
            break;
        }
        out.push((kind, &data[start..end]));
        at = end;
    }
    out
}

fn parse_moov(moov: &[u8]) -> Filmed {
    let mut filmed = Filmed::default();
    for (kind, inner) in boxes(moov) {
        match &kind {
            b"mvhd" => filmed.utc = header_time(inner),
            b"udta" => {
                for (kind, inner) in boxes(inner) {
                    if &kind == b"\xa9xyz" && inner.len() > 4 {
                        // A 16-bit length and a language, then the text.
                        let text = String::from_utf8_lossy(&inner[4..]);
                        filmed.place = filmed.place.or_else(|| crate::photo_details::iso6709(text.trim_end_matches('\0')));
                    }
                    if &kind == b"meta" {
                        apple_meta(inner, &mut filmed);
                    }
                }
            }
            b"meta" => apple_meta(inner, &mut filmed),
            _ => {}
        }
    }
    filmed
}

/// The movie header's creation time, when it is set.
fn header_time(mvhd: &[u8]) -> Option<i64> {
    let version = *mvhd.first()?;
    let seconds = if version == 1 {
        i64::try_from(u64::from_be_bytes(mvhd.get(4..12)?.try_into().ok()?)).ok()?
    } else {
        i64::from(u32::from_be_bytes(mvhd.get(4..8)?.try_into().ok()?))
    };
    let unix = seconds - FROM_1904;
    // Unset (0, or 1904) or nonsense: no date.
    (unix > 86_400 * 365).then_some(unix)
}

/// Apple's keys and values (`meta` with `keys` and `ilst`).
fn apple_meta(meta: &[u8], filmed: &mut Filmed) {
    // QuickTime's meta box has no version and flags; the ISO one does.
    let children = if meta.len() >= 8 && &meta[4..8] != b"hdlr" && &meta[4..8] != b"keys" { boxes(meta.get(4..).unwrap_or_default()) } else { boxes(meta) };
    let mut keys: Vec<String> = Vec::new();
    for (kind, inner) in &children {
        if kind == b"keys" && inner.len() >= 8 {
            let mut at = 8usize;
            while at + 8 <= inner.len() {
                let size = u32::from_be_bytes(inner[at..at + 4].try_into().unwrap()) as usize;
                if size < 8 || at + size > inner.len() {
                    break;
                }
                keys.push(String::from_utf8_lossy(&inner[at + 8..at + size]).into_owned());
                at += size;
            }
        }
    }
    for (kind, inner) in &children {
        if kind != b"ilst" {
            continue;
        }
        for (index, item) in boxes(inner) {
            let Some(key) = (u32::from_be_bytes(index) as usize).checked_sub(1).and_then(|i| keys.get(i)) else { continue };
            let Some(value) = boxes(item).into_iter().find(|(kind, _)| kind == b"data").and_then(|(_, data)| data.get(8..)) else { continue };
            let text = String::from_utf8_lossy(value);
            match key.as_str() {
                "com.apple.quicktime.creationdate" => filmed.local = filmed.local.or_else(|| local_time(&text)),
                "com.apple.quicktime.location.ISO6709" => filmed.place = filmed.place.or_else(|| crate::photo_details::iso6709(&text)),
                _ => {}
            }
        }
    }
}

/// "2024-07-15T18:30:02+0200" as its local date and time (the offset left out), in seconds.
fn local_time(text: &str) -> Option<i64> {
    let number = |range: std::ops::Range<usize>| text.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13).unwrap_or(0), number(14..16).unwrap_or(0), number(17..19).unwrap_or(0));
    if !(1971..=9999).contains(&year) || !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second)
}

/// Days since 1970 of a date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boxed(kind: &[u8; 4], inner: &[u8]) -> Vec<u8> {
        let mut out = ((inner.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(inner);
        out
    }

    #[test]
    fn reads_apple_and_android_dates_and_places() {
        // mvhd version 0: creation time 2024-07-15 16:30:02 UTC.
        let mut mvhd = vec![0u8; 4];
        mvhd.extend_from_slice(&((1_721_061_002 + FROM_1904) as u32).to_be_bytes());
        mvhd.extend_from_slice(&[0u8; 92]);
        let key = |name: &str| boxed(b"mdta", name.as_bytes());
        let mut keys = vec![0u8; 4];
        keys.extend_from_slice(&2u32.to_be_bytes());
        keys.extend(key("com.apple.quicktime.creationdate"));
        keys.extend(key("com.apple.quicktime.location.ISO6709"));
        let value = |text: &str| {
            let mut data = vec![0, 0, 0, 1, 0, 0, 0, 0];
            data.extend_from_slice(text.as_bytes());
            boxed(b"data", &data)
        };
        let mut ilst = boxed(&1u32.to_be_bytes(), &value("2024-07-15T18:30:02+0200"));
        ilst.extend(boxed(&2u32.to_be_bytes(), &value("+48.8577+002.2950+035.000/")));
        let mut meta = boxed(b"hdlr", &[0u8; 24]);
        meta.extend(boxed(b"keys", &keys));
        meta.extend(boxed(b"ilst", &ilst));
        let mut moov = boxed(b"mvhd", &mvhd);
        moov.extend(boxed(b"meta", &meta));
        let filmed = parse_moov(&moov);
        assert_eq!(filmed.utc, Some(1_721_061_002));
        assert_eq!(filmed.local, Some(1_721_068_202), "18:30 as shown on the phone");
        assert_eq!(filmed.place, Some((48.8577, 2.295)));

        // Android: only the movie header and ©xyz in udta.
        let mut xyz = vec![0, 18, 0x15, 0xc7];
        xyz.extend_from_slice(b"-33.8688+151.2093/");
        let android = [boxed(b"mvhd", &mvhd), boxed(b"udta", &boxed(b"\xa9xyz", &xyz))].concat();
        let filmed = parse_moov(&android);
        assert_eq!((filmed.local, filmed.place), (None, Some((-33.8688, 151.2093))));

        // A file whose moov is after the media data.
        let dir = std::env::temp_dir().join(format!("ownnas-video-{}-{}", std::process::id(), rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = [boxed(b"ftyp", b"qt  "), boxed(b"mdat", &[7u8; 1000]), boxed(b"moov", &moov)].concat();
        std::fs::write(dir.join("clip.MOV"), file).unwrap();
        assert_eq!(read(&dir.join("clip.MOV")).local, Some(1_721_068_202));
        let _ = std::fs::remove_dir_all(dir);
    }
}
