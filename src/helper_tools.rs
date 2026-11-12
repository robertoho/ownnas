//! The outside programs OwnNAS uses when they are installed (OCR, PDF text, video), checked
//! when Settings asks, so it can say which are present and how to install the missing ones.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HelperTool {
    pub name: &'static str,
    /// What OwnNAS does with it.
    pub purpose: &'static str,
    /// What happens without it.
    pub without: &'static str,
    pub found: bool,
    pub path: Option<String>,
    pub version: Option<String>,
    /// Anything else worth knowing, such as a missing OCR language.
    pub note: Option<String>,
    /// How to install it on this system.
    pub install: Option<&'static str>,
}

/// Each tool as it is on this computer right now.
pub fn check() -> Vec<HelperTool> {
    let mut tools = Vec::new();

    let mut tesseract = tool(
        "tesseract",
        &["--version"],
        "Reads the text in photos and scans, so a search finds them by the words in them.",
        "Searches only look in file names, tags, and documents, not in pictures.",
        install("brew install tesseract", "sudo apt install tesseract-ocr", "winget install UB-Mannheim.TesseractOCR (then add C:\\Program Files\\Tesseract-OCR to PATH)"),
    );
    if tesseract.found {
        let languages = languages();
        if !languages.iter().any(|lang| lang == "eng") {
            tesseract.note = Some("The English language data (eng) is missing, and OwnNAS reads text with it.".to_string());
        } else if languages.len() > 12 {
            tesseract.note = Some(format!("{} languages installed, English among them.", languages.len()));
        } else if !languages.is_empty() {
            tesseract.note = Some(format!("Languages installed: {}.", languages.join(", ")));
        }
    }
    tools.push(tesseract);

    tools.push(tool(
        "pdftotext",
        &["-v"],
        "Reads the text of PDFs for search (part of Poppler).",
        "A built-in reader is used instead, which misses the text of some PDFs.",
        install("brew install poppler", "sudo apt install poppler-utils", "winget install oschwartz10612.Poppler"),
    ));
    tools.push(tool(
        "ffmpeg",
        &["-version"],
        "Makes video thumbnails, plays videos the browser cannot, and edits videos.",
        "Videos show a plain icon, and only formats the browser plays open.",
        install("brew install ffmpeg", "sudo apt install ffmpeg", "winget install Gyan.FFmpeg"),
    ));
    tools.push(tool(
        "ffprobe",
        &["-version"],
        "Reads video details and the subtitle tracks inside videos (comes with ffmpeg).",
        "Video details and built-in subtitles are not shown.",
        install("brew install ffmpeg", "sudo apt install ffmpeg", "winget install Gyan.FFmpeg"),
    ));
    if cfg!(target_os = "macos") {
        tools.push(tool(
            "sips",
            &["--help"],
            "Turns HEIC and AVIF photos into thumbnails and previews (part of macOS).",
            "HEIC and AVIF photos show no thumbnail.",
            None,
        ));
    } else {
        let mut heif = tool(
            "heif-convert",
            &["--version"],
            "Turns HEIC and AVIF photos into thumbnails and previews.",
            "ffmpeg is used for HEIC and AVIF photos if it is installed; otherwise they show no thumbnail.",
            install("brew install libheif", "sudo apt install libheif-examples libheif-plugin-libde265", "Not needed on Windows: install ffmpeg instead"),
        );
        if !heif.found && find("ffmpeg").is_some() {
            heif.note = Some("ffmpeg is installed and is used for HEIC and AVIF photos instead.".to_string());
        }
        // Debian and Ubuntu ship libheif's decoders as plugins: without the HEVC one,
        // heif-convert runs but reads no HEIC photo ("Unsupported codec").
        if heif.found && !heic_decoder_installed() {
            heif.note = Some("heif-convert is installed but cannot read HEIC photos: its HEIC decoder is missing. Install it with: sudo apt install libheif-plugin-libde265".to_string());
        }
        tools.push(heif);
    }
    tools.push(tool(
        "smartctl",
        &["--version"],
        "Reads the drives' own health reports for Settings > Health: temperature, wear, and errors (part of smartmontools).",
        "Health shows only whether a drive says it is healthy, where the system tells.",
        install("brew install smartmontools", "sudo apt install smartmontools", "winget install smartmontools.smartmontools"),
    ));
    tools.push(tool(
        "restic",
        &["version"],
        "Backs up the library and OwnNAS's data to a drive, a bucket, or another machine (Settings > Backup).",
        "Backups are not available.",
        install("brew install restic", "sudo apt install restic", "winget install restic.restic"),
    ));
    let mut rclone = tool(
        "rclone",
        &["version"],
        "Reaches the servers and clouds set up in it (SFTP, OneDrive, Nextcloud, S3…): browse them under Remotes, and back up to them.",
        "Remotes are not available.",
        install("brew install rclone", "sudo apt install rclone", "winget install Rclone.Rclone"),
    );
    if !rclone.found {
        // Also where a service's bare PATH does not look.
        if let Some(path) = crate::remotes::program(&crate::remotes::Settings::default()) {
            rclone.found = true;
            rclone.version = crate::remotes::version(&path);
            rclone.path = Some(path.display().to_string());
        }
    }
    tools.push(rclone);
    tools.push(tool(
        "git",
        &["--version"],
        "Shows what changed in a terminal's project folder.",
        "The changes view of terminals is empty.",
        install("xcode-select --install", "sudo apt install git", "winget install Git.Git"),
    ));
    tools
}

fn install(mac: &'static str, linux: &'static str, windows: &'static str) -> Option<&'static str> {
    if cfg!(target_os = "macos") {
        Some(mac)
    } else if cfg!(windows) {
        Some(windows)
    } else {
        Some(linux)
    }
}

/// Whether heif-convert has a HEIC decoder (libheif 1.16 and later list them; older ones had
/// the decoder built in).
fn heic_decoder_installed() -> bool {
    let Ok(output) = std::process::Command::new("heif-convert").arg("--list-decoders").stdin(std::process::Stdio::null()).output() else { return true };
    let text = String::from_utf8_lossy(&output.stdout);
    let Some(section) = text.split("HEIC decoders:").nth(1) else { return true };
    let listed = section.lines().skip(1).take_while(|line| line.starts_with("- ") || line.trim().is_empty()).any(|line| line.starts_with("- "));
    // The section right after the heading: "- libde265 = …" lines, until the next heading.
    listed || section.lines().nth(1).is_some_and(|line| line.starts_with("- "))
}

fn tool(name: &'static str, args: &[&str], purpose: &'static str, without: &'static str, install: Option<&'static str>) -> HelperTool {
    let path = find(name);
    let version = path.as_ref().and_then(|_| version(name, args));
    HelperTool {
        name,
        purpose,
        without,
        found: path.is_some(),
        path: path.map(|p| p.display().to_string()),
        version,
        note: None,
        install,
    }
}

/// Where `name` is on the PATH.
pub(crate) fn find(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    let names: Vec<String> = if cfg!(windows) { vec![format!("{name}.exe"), format!("{name}.cmd"), name.to_string()] } else { vec![name.to_string()] };
    std::env::split_paths(&paths)
        .flat_map(|dir| names.iter().map(move |n| dir.join(n)))
        .find(|candidate| is_program(candidate))
}

fn is_program(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata().is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// The first line the program prints about its version (some print it to stderr).
fn version(name: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(name).args(args).stdin(Stdio::null()).output().ok()?;
    let text = if output.stdout.iter().any(|b| !b.is_ascii_whitespace()) { output.stdout } else { output.stderr };
    let line = String::from_utf8_lossy(&text).lines().map(str::trim).find(|line| !line.is_empty())?.to_string();
    // "sips --help" prints what it is, not a version.
    if name == "sips" || line.to_ascii_lowercase().starts_with("usage") {
        return None;
    }
    let line = line.split(" Copyright").next().unwrap_or(&line);
    Some(line.chars().take(120).collect())
}

fn languages() -> Vec<String> {
    let Ok(output) = Command::new("tesseract").arg("--list-langs").stdin(Stdio::null()).output() else {
        return Vec::new();
    };
    // The first line is a heading ("List of available languages in ...").
    let text = String::from_utf8_lossy(&output.stdout).into_owned() + &String::from_utf8_lossy(&output.stderr);
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.contains(' ') && !line.ends_with(':') && !line.starts_with("script/") && *line != "osd" && *line != "equ")
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_every_tool_and_finds_a_common_one() {
        let tools = check();
        assert!(tools.iter().any(|t| t.name == "tesseract"));
        assert!(tools.iter().any(|t| t.name == "ffmpeg"));
        // A program every test machine has.
        let shell = if cfg!(windows) { "cmd" } else { "sh" };
        assert!(find(shell).is_some());
        assert!(find("ownnas-no-such-program").is_none());
    }
}
