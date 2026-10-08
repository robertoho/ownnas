//! Self-update: fetch a static `latest.json` from a VPS, verify, replace binary.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const DEFAULT_URL: &str = "https://github.com/robertoho/ownnas/releases/latest/download/latest.json";
pub const EMBEDDED_PUBKEY: &str = include_str!("../releases/keys/update.pk");

#[derive(Clone, Debug, Default)]
pub struct UpdateConfig {
    /// Absolute URL to `latest.json` (HTTPS required except localhost).
    pub url: Option<String>,
    /// Optional ed25519 public key (32 raw bytes) for `latest.json.sig`.
    pub pubkey: Option<[u8; 32]>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub published_at: String,
    #[serde(default)]
    pub min_version: Option<String>,
    pub artifacts: std::collections::BTreeMap<String, Artifact>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    pub url: String,
    pub sha256: String,
    #[serde(default)]
    pub size: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckResult {
    pub current: String,
    pub latest: String,
    pub notes: String,
    pub available: bool,
    pub target: String,
    pub artifact_url: Option<String>,
    pub signed: bool,
    pub update_url: String,
}

pub fn current_target() -> &'static str {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        return "x86_64-pc-windows-msvc";
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        return "x86_64-apple-darwin";
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        return "aarch64-apple-darwin";
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        return "x86_64-unknown-linux-gnu";
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        return "aarch64-unknown-linux-gnu";
    }
    #[allow(unreachable_code)]
    "unknown"
}

pub fn parse_key32_hex(value: &str) -> Result<[u8; 32], String> {
    let clean: String = value.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = hex::decode(&clean).map_err(|_| "Key must be hex".to_string())?;
    if bytes.len() != 32 {
        return Err("Key must be 32 bytes (64 hex chars)".into());
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

pub fn parse_pubkey_hex(value: &str) -> Result<[u8; 32], String> {
    parse_key32_hex(value).map_err(|_| "Update public key must be 32 bytes (64 hex chars)".into())
}

pub fn keygen(out_dir: &Path) -> Result<(), String> {
    fs::create_dir_all(out_dir).map_err(|_| format!("Could not create {}", out_dir.display()))?;
    let signing = SigningKey::generate(&mut OsRng);
    let sk = signing.to_bytes();
    let pk = signing.verifying_key().to_bytes();
    let sk_path = out_dir.join("update.sk");
    let pk_path = out_dir.join("update.pk");
    fs::write(&sk_path, hex::encode(sk)).map_err(|_| "Could not write update.sk".to_string())?;
    fs::write(&pk_path, hex::encode(pk)).map_err(|_| "Could not write update.pk".to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&sk_path, fs::Permissions::from_mode(0o600));
    }
    println!("Wrote {}", sk_path.display());
    println!("Wrote {}", pk_path.display());
    println!("Keep update.sk private. Pass update.pk to OwnNAS as --update-pubkey.");
    Ok(())
}

pub fn sign_manifest(key_path: &Path, manifest_path: &Path) -> Result<(), String> {
    let sk_hex = fs::read_to_string(key_path).map_err(|_| "Could not read the private key".to_string())?;
    let sk_bytes = parse_key32_hex(sk_hex.trim())?;
    let signing = SigningKey::from_bytes(&sk_bytes);
    let body = fs::read(manifest_path).map_err(|_| "Could not read latest.json".to_string())?;
    let sig = signing.sign(&body);
    let sig_path = PathBuf::from(format!("{}.sig", manifest_path.display()));
    fs::write(&sig_path, hex::encode(sig.to_bytes()))
        .map_err(|_| "Could not write the signature file".to_string())?;
    println!("Wrote {}", sig_path.display());
    Ok(())
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(120))
        .user_agent(concat!("OwnNAS/", env!("CARGO_PKG_VERSION")))
        .build()
}

fn require_https_url(url: &str) -> Result<(), String> {
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("https://") {
        return Ok(());
    }
    if lower.starts_with("http://127.0.0.1") || lower.starts_with("http://localhost") {
        return Ok(());
    }
    Err("Update URL must use HTTPS (http is only allowed for localhost)".into())
}

fn fetch_bytes(url: &str) -> Result<Vec<u8>, String> {
    require_https_url(url)?;
    let response = agent()
        .get(url)
        .call()
        .map_err(|err| format!("Could not download update metadata: {err}"))?;
    if !(200..300).contains(&response.status()) {
        return Err(format!("Update server returned HTTP {}", response.status()));
    }
    let mut buf = Vec::new();
    response
        .into_reader()
        .take(2 * 1024 * 1024)
        .read_to_end(&mut buf)
        .map_err(|_| "Could not read the update response".to_string())?;
    Ok(buf)
}

fn fetch_to_file(url: &str, dest: &Path, expected_size: u64) -> Result<(), String> {
    require_https_url(url)?;
    let response = agent()
        .get(url)
        .call()
        .map_err(|err| format!("Could not download the update: {err}"))?;
    if !(200..300).contains(&response.status()) {
        return Err(format!("Update download returned HTTP {}", response.status()));
    }
    let mut reader = response.into_reader();
    let mut file = File::create(dest).map_err(|_| "Could not create the download file".to_string())?;
    let mut buf = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|_| "Download interrupted".to_string())?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if expected_size > 0 && total > expected_size.saturating_mul(2).max(expected_size + 8 * 1024 * 1024) {
            return Err("Download is larger than expected".into());
        }
        if total > 200 * 1024 * 1024 {
            return Err("Update binary is too large (200 MB max)".into());
        }
        file.write_all(&buf[..n])
            .map_err(|_| "Could not write the download".to_string())?;
    }
    file.flush().map_err(|_| "Could not finish the download".to_string())?;
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|_| "Could not open the downloaded file".to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|_| "Could not hash the downloaded file".to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn verify_signature(body: &[u8], sig_hex: &str, pubkey: &[u8; 32]) -> Result<(), String> {
    let sig_bytes = hex::decode(sig_hex.trim()).map_err(|_| "Signature is not valid hex".to_string())?;
    let sig = Signature::from_slice(&sig_bytes).map_err(|_| "Signature has the wrong length".to_string())?;
    let key = VerifyingKey::from_bytes(pubkey).map_err(|_| "Update public key is invalid".to_string())?;
    key.verify(body, &sig)
        .map_err(|_| "Update manifest signature is invalid".to_string())
}

fn is_newer(latest: &str, current: &str) -> Result<bool, String> {
    let latest = semver::Version::parse(latest.trim_start_matches('v'))
        .map_err(|_| format!("Invalid latest version: {latest}"))?;
    let current = semver::Version::parse(current.trim_start_matches('v'))
        .map_err(|_| format!("Invalid current version: {current}"))?;
    Ok(latest > current)
}

pub fn fetch_manifest(config: &UpdateConfig) -> Result<(Manifest, Vec<u8>, bool), String> {
    let url = config
        .url
        .as_deref()
        .ok_or_else(|| "Updates are not configured (--update-url)".to_string())?;
    let body = fetch_bytes(url)?;
    let mut signed = false;
    if let Some(pubkey) = config.pubkey {
        let sig_url = if url.ends_with(".json") {
            format!("{url}.sig")
        } else {
            format!("{url}.sig")
        };
        match fetch_bytes(&sig_url) {
            Ok(sig_bytes) => {
                let sig_hex = String::from_utf8_lossy(&sig_bytes);
                verify_signature(&body, &sig_hex, &pubkey)?;
                signed = true;
            }
            Err(err) => {
                return Err(format!(
                    "A public key is configured, but the signature could not be verified: {err}"
                ));
            }
        }
    }
    let manifest: Manifest = serde_json::from_slice(&body)
        .map_err(|_| "latest.json is not valid JSON".to_string())?;
    Ok((manifest, body, signed))
}

pub fn check(config: &UpdateConfig) -> Result<CheckResult, String> {
    let url = config
        .url
        .clone()
        .ok_or_else(|| "Updates are not configured (--update-url)".to_string())?;
    let (manifest, _, signed) = fetch_manifest(config)?;
    let target = current_target().to_string();
    let artifact = manifest.artifacts.get(&target).cloned();
    let available = is_newer(&manifest.version, CURRENT_VERSION)? && artifact.is_some();
    Ok(CheckResult {
        current: CURRENT_VERSION.to_string(),
        latest: manifest.version,
        notes: manifest.notes,
        available,
        target,
        artifact_url: artifact.map(|a| a.url),
        signed,
        update_url: url,
    })
}

/// Downloads, verifies, and replaces the running binary. Caller should re-spawn and exit.
pub fn download_and_replace(config: &UpdateConfig) -> Result<String, String> {
    let (manifest, _, _) = fetch_manifest(config)?;
    if !is_newer(&manifest.version, CURRENT_VERSION)? {
        return Err("Already on the latest version".into());
    }
    if let Some(min) = &manifest.min_version {
        let current = semver::Version::parse(CURRENT_VERSION)
            .map_err(|_| "Invalid current version".to_string())?;
        let min = semver::Version::parse(min.trim_start_matches('v'))
            .map_err(|_| format!("Invalid minVersion: {min}"))?;
        if current < min {
            return Err(format!(
                "This install is too old to update automatically (need {min}+). Install manually."
            ));
        }
    }
    let target = current_target();
    let artifact = manifest
        .artifacts
        .get(target)
        .ok_or_else(|| format!("No build for this platform ({target}) in latest.json"))?
        .clone();
    let expected = artifact.sha256.trim().to_ascii_lowercase();
    if expected.len() != 64 || hex::decode(&expected).is_err() {
        return Err("Artifact sha256 in latest.json is invalid".into());
    }

    let current_exe = std::env::current_exe().map_err(|_| "Could not locate the OwnNAS binary".to_string())?;
    let parent = current_exe
        .parent()
        .ok_or_else(|| "Could not locate the OwnNAS binary folder".to_string())?;
    let staged = parent.join(format!(
        ".ownnas-update-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));

    if let Err(err) = fetch_to_file(&artifact.url, &staged, artifact.size) {
        let _ = fs::remove_file(&staged);
        return Err(err);
    }
    let actual = match sha256_file(&staged) {
        Ok(hash) => hash,
        Err(err) => {
            let _ = fs::remove_file(&staged);
            return Err(err);
        }
    };
    if actual != expected {
        let _ = fs::remove_file(&staged);
        return Err("Downloaded binary failed the SHA-256 check".into());
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&staged, fs::Permissions::from_mode(0o755));
    }

    self_replace::self_replace(&staged).map_err(|err| format!("Could not replace the binary: {err}"))?;
    let _ = fs::remove_file(&staged);
    Ok(manifest.version)
}

pub fn spawn_replaced_binary() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|_| "Could not locate the OwnNAS binary".to_string())?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    Command::new(exe)
        .args(args)
        .spawn()
        .map_err(|_| "Could not start the updated OwnNAS process".to_string())?;
    Ok(())
}
