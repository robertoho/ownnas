//! Playing videos the browser cannot, and subtitles for them.
//!
//! A video in a container or codec the browser does not play (AVI, WMV, MPEG, HEVC or MPEG-4
//! Part 2 in MKV) is turned into a streaming MP4 by `ffmpeg` while it plays: copied as it is
//! when it already holds H.264 and AAC or MP3 (only the container changes, which is instant),
//! otherwise converted to H.264 at most 1080 pixels high. Playing from a point starts a new
//! stream there. SubRip subtitles (.srt) become WebVTT, which browsers show.

use std::path::Path;
use std::process::{Command, Stdio};

use serde::Serialize;

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Streams {
    pub video: Option<String>,
    pub audio: Option<String>,
    pub duration: Option<f64>,
    pub width: Option<u64>,
    pub height: Option<u64>,
}

/// The codecs and length of a video, from `ffprobe`.
pub fn probe(path: &Path) -> Option<Streams> {
    let output = Command::new("ffprobe")
        .args(["-v", "quiet", "-print_format", "json", "-show_format", "-show_streams"])
        .arg(path)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let info: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let streams = info["streams"].as_array()?;
    let first = |kind: &str| streams.iter().find(|s| s["codec_type"] == kind && s["disposition"]["attached_pic"] != 1);
    let video = first("video");
    Some(Streams {
        video: video.and_then(|s| s["codec_name"].as_str()).map(str::to_string),
        audio: first("audio").and_then(|s| s["codec_name"].as_str()).map(str::to_string),
        duration: info["format"]["duration"].as_str().and_then(|d| d.parse().ok()),
        width: video.and_then(|s| s["width"].as_u64()),
        height: video.and_then(|s| s["height"].as_u64()),
    })
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Subtitle {
    pub path: String,
    pub label: String,
    /// The language code from the name ("movie.en.srt"), for the browser.
    pub language: Option<String>,
}

/// Subtitle files named after the video beside it: "movie.srt", "movie.en.srt", "movie.English.vtt".
pub fn subtitles_beside(video: &Path, rel: &str) -> Vec<Subtitle> {
    let (Some(dir), Some(stem)) = (video.parent(), video.file_stem().map(|s| s.to_string_lossy().into_owned())) else { return Vec::new() };
    let rel_dir = rel.rsplit_once('/').map_or("", |(dir, _)| dir);
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut found: Vec<Subtitle> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let (base, ext) = name.rsplit_once('.')?;
            if !matches!(ext.to_ascii_lowercase().as_str(), "srt" | "vtt") {
                return None;
            }
            let middle = if base == stem { "" } else { base.strip_prefix(&stem)?.strip_prefix('.')? };
            let language = (!middle.is_empty() && middle.len() <= 3 && middle.chars().all(|c| c.is_ascii_alphabetic())).then(|| middle.to_ascii_lowercase());
            Some(Subtitle {
                path: if rel_dir.is_empty() { name.clone() } else { format!("{rel_dir}/{name}") },
                label: if middle.is_empty() { "Subtitles".to_string() } else { middle.to_string() },
                language,
            })
        })
        .collect();
    found.sort_by_key(|subtitle| subtitle.label.to_lowercase());
    found
}

/// `ffmpeg`'s arguments for a streaming MP4 of `path` from `start` seconds, written to its output.
pub fn stream_arguments(path: &Path, start: f64, streams: &Streams) -> Vec<String> {
    let mut args: Vec<String> = vec!["-hide_banner".into(), "-loglevel".into(), "error".into(), "-nostdin".into()];
    if start > 0.0 {
        args.extend(["-ss".into(), format!("{start:.3}")]);
    }
    args.extend(["-i".into(), path.to_string_lossy().into_owned()]);
    args.extend(["-map".into(), "0:v:0".into(), "-map".into(), "0:a:0?".into(), "-sn".into(), "-dn".into()]);
    let copy_video = streams.video.as_deref() == Some("h264");
    if copy_video {
        args.extend(["-c:v".into(), "copy".into()]);
    } else {
        args.extend(
            ["-c:v", "libx264", "-preset", "veryfast", "-crf", "23", "-pix_fmt", "yuv420p", "-vf", "scale=-2:'min(1080,ih)'"]
                .map(String::from),
        );
    }
    match streams.audio.as_deref() {
        Some("aac") | Some("mp3") => args.extend(["-c:a".into(), "copy".into()]),
        Some(_) => args.extend(["-c:a", "aac", "-b:a", "160k", "-ac", "2"].map(String::from)),
        None => {}
    }
    args.extend(["-movflags", "frag_keyframe+empty_moov+default_base_moof", "-f", "mp4", "pipe:1"].map(String::from));
    args
}

/// Subtitle text as WebVTT, moved `shift` seconds earlier (for a stream started part way).
/// SubRip (.srt) and WebVTT (.vtt) are read; text that is not UTF-8 is read as Windows-1252,
/// which most older subtitle files are.
pub fn to_webvtt(bytes: &[u8], shift: f64) -> String {
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => bytes.iter().map(|&b| windows_1252(b)).collect(),
    };
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n").replace('\r', "\n");
    let mut out = String::from("WEBVTT\n\n");
    for block in text.split("\n\n") {
        let lines: Vec<&str> = block.lines().filter(|line| !line.trim().is_empty()).collect();
        let Some(timing_at) = lines.iter().position(|line| line.contains("-->")) else { continue };
        let Some((from, to)) = lines[timing_at].split_once("-->") else { continue };
        let to_text = to.trim();
        // WebVTT may add cue settings after the end time.
        let (to_time, settings) = to_text.split_once(char::is_whitespace).unwrap_or((to_text, ""));
        let (Some(from), Some(to)) = (seconds(from.trim()), seconds(to_time)) else { continue };
        let (from, to) = (from - shift, to - shift);
        if to <= 0.0 {
            continue;
        }
        out.push_str(&format!("{} --> {}", stamp(from.max(0.0)), stamp(to)));
        if !settings.trim().is_empty() {
            out.push(' ');
            out.push_str(settings.trim());
        }
        out.push('\n');
        for line in &lines[timing_at + 1..] {
            out.push_str(line);
            out.push('\n');
        }
        out.push('\n');
    }
    out
}

/// "01:02:03,456", "01:02:03.456", or "02:03.456" as seconds.
fn seconds(text: &str) -> Option<f64> {
    let text = text.replace(',', ".");
    let parts: Vec<&str> = text.split(':').collect();
    let (hours, minutes, secs) = match parts.as_slice() {
        [h, m, s] => (h.parse::<f64>().ok()?, m.parse::<f64>().ok()?, s.parse::<f64>().ok()?),
        [m, s] => (0.0, m.parse::<f64>().ok()?, s.parse::<f64>().ok()?),
        _ => return None,
    };
    Some(hours * 3600.0 + minutes * 60.0 + secs)
}

fn stamp(total: f64) -> String {
    let millis = (total * 1000.0).round() as u64;
    format!("{:02}:{:02}:{:02}.{:03}", millis / 3_600_000, millis / 60_000 % 60, millis / 1000 % 60, millis % 1000)
}

fn windows_1252(byte: u8) -> char {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž', '\u{8f}', '\u{90}', '‘', '’', '“',
        '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}', 'ž', 'Ÿ',
    ];
    match byte {
        0x80..=0x9F => HIGH[usize::from(byte - 0x80)],
        _ => char::from(byte),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subrip_becomes_webvtt() {
        let srt = "1\r\n00:00:01,500 --> 00:00:03,000\r\nHello <i>there</i>\r\n\r\n2\r\n00:00:10,000 --> 00:00:12,250\r\nSecond line\r\nand more\r\n";
        assert_eq!(
            to_webvtt(srt.as_bytes(), 0.0),
            "WEBVTT\n\n00:00:01.500 --> 00:00:03.000\nHello <i>there</i>\n\n00:00:10.000 --> 00:00:12.250\nSecond line\nand more\n\n"
        );
        // From 2 seconds in: the first cue is cut short, the second moves earlier.
        let shifted = to_webvtt(srt.as_bytes(), 2.0);
        assert!(shifted.contains("00:00:00.000 --> 00:00:01.000\nHello"));
        assert!(shifted.contains("00:00:08.000 --> 00:00:10.250"));
        assert_eq!(to_webvtt(b"1\n00:00:01,000 --> 00:00:02,000\nCaf\xe9 \x93ok\x94\n", 0.0).lines().nth(3), Some("Café “ok”"));
    }

    #[test]
    fn finds_subtitles_named_after_the_video() {
        let dir = std::env::temp_dir().join(format!("ownnas-subs-{}-{}", std::process::id(), rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["Film.mkv", "Film.srt", "Film.en.srt", "Film.Español.vtt", "Other.srt", "Film.txt"] {
            std::fs::write(dir.join(name), b"").unwrap();
        }
        let found = subtitles_beside(&dir.join("Film.mkv"), "Movies/Film.mkv");
        let paths: Vec<_> = found.iter().map(|s| (s.path.as_str(), s.label.as_str(), s.language.as_deref())).collect();
        assert_eq!(paths, [("Movies/Film.en.srt", "en", Some("en")), ("Movies/Film.Español.vtt", "Español", None), ("Movies/Film.srt", "Subtitles", None)]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn copies_what_the_browser_plays_and_converts_the_rest() {
        let h264 = Streams { video: Some("h264".into()), audio: Some("ac3".into()), ..Streams::default() };
        let args = stream_arguments(Path::new("a.mkv"), 30.0, &h264).join(" ");
        assert!(args.contains("-ss 30.000 -i a.mkv") && args.contains("-c:v copy") && args.contains("-c:a aac"));
        let hevc = Streams { video: Some("hevc".into()), audio: Some("aac".into()), ..Streams::default() };
        let args = stream_arguments(Path::new("a.mkv"), 0.0, &hevc).join(" ");
        assert!(args.contains("-c:v libx264") && args.contains("-c:a copy") && !args.contains("-ss"));
    }
}
