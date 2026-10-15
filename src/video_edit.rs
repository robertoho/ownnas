//! Converting videos with ffmpeg: to another format, smaller, and faster or slower. Each one runs
//! in the background as a job that the page follows (and can cancel); the result is written
//! beside the original and never replaces it.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;

/// What to make of a video.
#[derive(Clone, Debug)]
pub struct Plan {
    /// "same", or one of `FORMATS`.
    pub format: String,
    /// 1.0 is unchanged; 2.0 plays twice as fast.
    pub speed: f64,
    /// At most this tall (keeping the shape); none keeps the size.
    pub height: Option<u32>,
    pub audio: bool,
}

/// The formats a video can become: extension, and whether it keeps the picture.
pub const FORMATS: &[(&str, bool)] = &[
    ("mp4", true),
    ("webm", true),
    ("mov", true),
    ("mkv", true),
    ("gif", true),
    ("mp3", false),
    ("m4a", false),
];

impl Plan {
    pub fn check(&self) -> Result<(), String> {
        if !(0.25..=16.0).contains(&self.speed) {
            return Err("Choose a speed from 0.25× to 16×".into());
        }
        if self.format != "same" && !FORMATS.iter().any(|(ext, _)| *ext == self.format) {
            return Err("That format is not one OwnNAS can make".into());
        }
        if self.format == "same" && (self.speed - 1.0).abs() < 0.001 && self.height.is_none() {
            return Err("Choose a format, a speed, or a size".into());
        }
        if self.height.is_some_and(|h| !(120..=4320).contains(&h)) {
            return Err("Choose a size from 120 to 4320 pixels tall".into());
        }
        Ok(())
    }

    /// The extension of the result, given the original's.
    pub fn extension(&self, original: &str) -> String {
        if self.format != "same" {
            return self.format.clone();
        }
        // Formats ffmpeg writes well keep theirs; anything else becomes MP4.
        match original.to_ascii_lowercase().as_str() {
            ext @ ("mp4" | "m4v" | "mov" | "mkv" | "webm") => ext.to_string(),
            _ => "mp4".into(),
        }
    }

    /// How the change is described in the file's history and the new file's name.
    pub fn describe(&self, ext: &str) -> String {
        let mut parts = Vec::new();
        if (self.speed - 1.0).abs() >= 0.001 {
            parts.push(format!("{} {}×", if self.speed > 1.0 { "Sped up" } else { "Slowed down" }, speed_text(self.speed)));
        }
        if let Some(height) = self.height {
            parts.push(format!("made {height}p"));
        }
        if self.format != "same" {
            parts.push(format!("converted to {}", ext.to_uppercase()));
        }
        if !self.audio && !matches!(ext, "gif" | "mp3" | "m4a") {
            parts.push("without sound".into());
        }
        let text = parts.join(", ");
        let mut chars = text.chars();
        chars.next().map(|first| first.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or(text)
    }

    /// A short tag for the new file's name: "clip (2x, 720p).mp4".
    pub fn tag(&self) -> String {
        let mut parts = Vec::new();
        if (self.speed - 1.0).abs() >= 0.001 {
            parts.push(format!("{}x", speed_text(self.speed)));
        }
        if let Some(height) = self.height {
            parts.push(format!("{height}p"));
        }
        if !self.audio {
            parts.push("no sound".into());
        }
        parts.join(", ")
    }
}

fn speed_text(speed: f64) -> String {
    let text = format!("{speed:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// ffmpeg's `atempo` takes 0.5 to 2 at a time on older versions: chain it for the rest.
fn atempo(speed: f64) -> String {
    let mut left = speed;
    let mut steps = Vec::new();
    while left > 2.0 {
        steps.push("atempo=2.0".to_string());
        left /= 2.0;
    }
    while left < 0.5 {
        steps.push("atempo=0.5".to_string());
        left /= 0.5;
    }
    steps.push(format!("atempo={left:.6}"));
    steps.join(",")
}

/// The ffmpeg arguments (after `-i <source>`) that make `out` with `ext`.
pub fn arguments(plan: &Plan, ext: &str, has_audio: bool) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    let sped = (plan.speed - 1.0).abs() >= 0.001;
    let picture = FORMATS.iter().find(|(e, _)| *e == ext).map_or(true, |(_, picture)| *picture);
    let scale = plan.height.map(|h| format!("scale=-2:'min(ih,{h})'"));
    if ext == "gif" {
        let mut chain = Vec::new();
        if sped {
            chain.push(format!("setpts=PTS/{}", plan.speed));
        }
        chain.push("fps=12".into());
        chain.push(format!("scale=-2:'min(ih,{})':flags=lanczos", plan.height.unwrap_or(480)));
        args.extend(["-filter_complex".into(), format!("[0:v]{},split[a][b];[a]palettegen[p];[b][p]paletteuse", chain.join(","))]);
        args.extend(["-an".into(), "-loop".into(), "0".into()]);
        return args;
    }
    if picture {
        let mut video = Vec::new();
        if sped {
            video.push(format!("setpts=PTS/{}", plan.speed));
        }
        if let Some(scale) = scale {
            video.push(scale);
        }
        if !video.is_empty() {
            args.extend(["-filter:v".into(), video.join(",")]);
        }
        match ext {
            "webm" => args.extend(
                ["-c:v", "libvpx-vp9", "-crf", "33", "-b:v", "0", "-deadline", "good", "-cpu-used", "4", "-row-mt", "1"].map(String::from),
            ),
            _ => args.extend(["-c:v", "libx264", "-preset", "medium", "-crf", "21", "-pix_fmt", "yuv420p"].map(String::from)),
        }
    } else {
        args.push("-vn".into());
    }
    if plan.audio && has_audio {
        if sped {
            args.extend(["-filter:a".into(), atempo(plan.speed)]);
        }
        match ext {
            "webm" => args.extend(["-c:a", "libopus", "-b:a", "128k"].map(String::from)),
            "mp3" => args.extend(["-c:a", "libmp3lame", "-q:a", "2"].map(String::from)),
            _ => args.extend(["-c:a", "aac", "-b:a", "192k"].map(String::from)),
        }
    } else {
        args.push("-an".into());
    }
    if matches!(ext, "mp4" | "mov" | "m4a") {
        args.extend(["-movflags".into(), "+faststart".into()]);
    }
    args
}

/// How long a video is, in seconds, and whether it has sound (ffprobe).
pub fn probe(path: &Path) -> Option<(f64, bool)> {
    let output = Command::new("ffprobe")
        .args(["-v", "quiet", "-print_format", "json", "-show_format", "-show_streams"])
        .arg(path)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let info: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let duration = info["format"]["duration"].as_str().and_then(|d| d.parse::<f64>().ok())?;
    let audio = info["streams"].as_array().is_some_and(|streams| streams.iter().any(|s| s["codec_type"] == "audio"));
    Some((duration, audio))
}

// ---------------------------------------------------------------------------
// Jobs
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub id: String,
    pub source: String,
    /// Where the result goes (and is, once done).
    pub output: String,
    pub progress: f64,
    pub done: bool,
    pub error: Option<String>,
}

pub struct Job {
    pub status: Mutex<Status>,
    cancel: AtomicBool,
    child: Mutex<Option<Child>>,
}

impl Job {
    pub fn status(&self) -> Status {
        self.status.lock().unwrap_or_else(|err| err.into_inner()).clone()
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(child) = self.child.lock().unwrap_or_else(|err| err.into_inner()).as_mut() {
            let _ = child.kill();
        }
    }

    fn update(&self, change: impl FnOnce(&mut Status)) {
        change(&mut self.status.lock().unwrap_or_else(|err| err.into_inner()));
    }
}

fn jobs() -> &'static Mutex<HashMap<String, Arc<Job>>> {
    static JOBS: OnceLock<Mutex<HashMap<String, Arc<Job>>>> = OnceLock::new();
    JOBS.get_or_init(Mutex::default)
}

pub fn job(id: &str) -> Option<Arc<Job>> {
    jobs().lock().unwrap_or_else(|err| err.into_inner()).get(id).cloned()
}

/// Starts making `output` (a library path, written at `output_full`) from `source_full`. `done`
/// runs on the job's thread once the file is in place.
pub fn start(
    source: String,
    source_full: PathBuf,
    output: String,
    output_full: PathBuf,
    plan: Plan,
    done: impl FnOnce(&Path) + Send + 'static,
) -> Arc<Job> {
    let id = format!("{:016x}", rand::random::<u64>());
    let job = Arc::new(Job {
        status: Mutex::new(Status { id: id.clone(), source, output, ..Status::default() }),
        cancel: AtomicBool::new(false),
        child: Mutex::new(None),
    });
    {
        let mut all = jobs().lock().unwrap_or_else(|err| err.into_inner());
        // Finished jobs are forgotten after a while.
        all.retain(|_, old| !old.status().done);
        all.insert(id, job.clone());
    }
    let worker = job.clone();
    std::thread::spawn(move || {
        let outcome = run(&worker, &source_full, &output_full, &plan);
        match outcome {
            Ok(()) => {
                done(&output_full);
                worker.update(|s| {
                    s.progress = 1.0;
                    s.done = true;
                });
            }
            Err(err) => worker.update(|s| {
                s.done = true;
                s.error = Some(err);
            }),
        }
    });
    job
}

fn run(job: &Job, source: &Path, output: &Path, plan: &Plan) -> Result<(), String> {
    let (duration, has_audio) = probe(source).ok_or("Could not read the video (is ffmpeg installed?)")?;
    let ext = output.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if !has_audio && matches!(ext.as_str(), "mp3" | "m4a") {
        return Err("This video has no sound to keep".into());
    }
    let name = output.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    // Written under a hidden name first, so a cancelled or failed job leaves nothing half done.
    let partial = output.with_file_name(format!(".{name}.converting.{ext}"));
    let mut command = Command::new("ffmpeg");
    command
        .args(["-y", "-hide_banner", "-nostdin", "-loglevel", "error", "-progress", "pipe:1", "-nostats", "-i"])
        .arg(source)
        .args(arguments(plan, &ext, has_audio))
        .arg(&partial)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|_| "ffmpeg is not installed on the server".to_string())?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    *job.child.lock().unwrap_or_else(|err| err.into_inner()) = Some(child);
    // ffmpeg's last words, for the error message.
    let errors = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(stderr) = stderr {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                text = line;
            }
        }
        text
    });
    let length = duration / plan.speed;
    if let Some(stdout) = stdout {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(micros) = line.strip_prefix("out_time_us=").and_then(|v| v.trim().parse::<f64>().ok()) {
                if length > 0.0 {
                    job.update(|s| s.progress = (micros / 1_000_000.0 / length).clamp(0.0, 0.99));
                }
            }
        }
    }
    let status = job.child.lock().unwrap_or_else(|err| err.into_inner()).as_mut().and_then(|child| child.wait().ok());
    let last = errors.join().unwrap_or_default();
    if job.cancel.load(Ordering::Relaxed) {
        let _ = std::fs::remove_file(&partial);
        return Err("Cancelled".into());
    }
    if !status.is_some_and(|s| s.success()) {
        let _ = std::fs::remove_file(&partial);
        return Err(if last.is_empty() { "ffmpeg could not convert the video".into() } else { format!("ffmpeg: {last}") });
    }
    // The original's time, so the result sorts with it in the Gallery.
    if let (Ok(time), Ok(file)) = (std::fs::metadata(source).and_then(|m| m.modified()), std::fs::File::options().write(true).open(&partial)) {
        let _ = file.set_modified(time);
    }
    std::fs::rename(&partial, output).map_err(|_| {
        let _ = std::fs::remove_file(&partial);
        "Could not save the converted video".to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(format: &str, speed: f64, height: Option<u32>, audio: bool) -> Plan {
        Plan { format: format.into(), speed, height, audio }
    }

    #[test]
    fn plans_are_checked_and_described() {
        assert!(plan("same", 1.0, None, true).check().is_err());
        assert!(plan("same", 32.0, None, true).check().is_err());
        assert!(plan("exe", 1.0, None, true).check().is_err());
        assert!(plan("same", 2.0, None, true).check().is_ok());
        assert_eq!(plan("same", 2.0, None, true).extension("AVI"), "mp4");
        assert_eq!(plan("same", 2.0, None, true).extension("mov"), "mov");
        assert_eq!(plan("webm", 1.5, Some(720), false).describe("webm"), "Sped up 1.5×, made 720p, converted to WEBM, without sound");
        assert_eq!(plan("same", 0.5, None, true).describe("mp4"), "Slowed down 0.5×");
        assert_eq!(plan("mp4", 4.0, Some(720), false).tag(), "4x, 720p, no sound");
    }

    #[test]
    fn speed_changes_the_picture_and_the_sound() {
        assert_eq!(atempo(4.0), "atempo=2.0,atempo=2.000000");
        assert_eq!(atempo(0.25), "atempo=0.5,atempo=0.500000");
        let args = arguments(&plan("mp4", 2.0, Some(720), true), "mp4", true).join(" ");
        assert!(args.contains("setpts=PTS/2,scale=-2:'min(ih,720)'"), "{args}");
        assert!(args.contains("-filter:a atempo=2.000000") && args.contains("libx264"), "{args}");
        let silent = arguments(&plan("mp4", 2.0, None, false), "mp4", true).join(" ");
        assert!(silent.contains("-an") && !silent.contains("atempo"), "{silent}");
        let sound = arguments(&plan("mp3", 1.0, None, true), "mp3", true).join(" ");
        assert!(sound.contains("-vn") && sound.contains("libmp3lame"), "{sound}");
        assert!(arguments(&plan("gif", 1.0, None, true), "gif", true).join(" ").contains("palettegen"));
    }
}
