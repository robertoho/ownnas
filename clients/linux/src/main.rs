//! `ownnas-mount`: OwnNAS servers as folders on Linux, with online-only files.
//!
//! `ownnas-mount setup --server https://nas.example:8787 --username ana` signs in and mounts
//! the server at `~/OwnNAS/<name>` with a systemd user service (`ownnas-mount run`), started
//! now and at each login. `keep` and `free` set files and folders to always stay on this
//! computer, or drop them from it; file managers' menus do the same.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "ownnas-mount", version, about = "OwnNAS servers as folders, with online-only files")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Sign in to a server and mount it.
    Setup {
        /// Server address, for example https://nas.example:8787
        #[arg(long)]
        server: String,
        #[arg(long)]
        username: String,
        /// Password (asked for if left out).
        #[arg(long, env = "OWNNAS_PASSWORD", hide_env_values = true)]
        password: Option<String>,
        /// Where its files show (default: ~/OwnNAS/<name>).
        #[arg(long)]
        folder: Option<PathBuf>,
        /// PEM certificate of a private CA to trust (the ca.crt from `ownnas tls generate`).
        #[arg(long)]
        ca_cert: Option<PathBuf>,
        /// A server certificate fingerprint to trust (as `ownnas serve` prints it).
        #[arg(long)]
        pin: Vec<String>,
        /// How much space opened files may take on this computer, in GB (default 10).
        #[arg(long)]
        cache_gb: Option<u64>,
        /// Don't install or start the systemd service (run `ownnas-mount run` yourself).
        #[arg(long)]
        no_service: bool,
    },
    /// The servers signed in to, and where they are.
    Status,
    /// Unmount a server and sign out of it.
    Remove {
        /// Its address or name, as `status` shows.
        server: String,
        /// Also when changes made here have not reached the server yet (they are lost).
        #[arg(long)]
        force: bool,
    },
    /// Mount every server and keep them in sync, writing what happens (the service runs this).
    Run,
    /// Always keep these files and folders on this computer.
    Keep {
        #[arg(required = true)]
        paths: Vec<PathBuf>,
    },
    /// Free up the space these files and folders take on this computer (they stay on the server).
    Free {
        #[arg(required = true)]
        paths: Vec<PathBuf>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli.command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn run(_command: Command) -> Result<(), String> {
    Err("ownnas-mount works on Linux. On macOS use the OwnNAS app (clients/macos); on Windows, ownnas-cloud (clients/windows).".into())
}

#[cfg(target_os = "linux")]
fn run(command: Command) -> Result<(), String> {
    use ownnas_core::api::ApiError;
    use ownnas_mount::{load_settings, save_settings, Account};

    match command {
        Command::Setup { server, username, password, folder, ca_cert, pin, cache_gb, no_service } => {
            let server = server.trim_end_matches('/').to_string();
            let ca_pem = match ca_cert {
                Some(path) => Some(std::fs::read_to_string(&path).map_err(|err| format!("Could not read {}: {err}", path.display()))?),
                None => None,
            };
            let password = match password {
                Some(password) => password,
                None => rpassword::prompt_password(format!("Password for {username}: ")).map_err(|err| err.to_string())?,
            };
            let api = api_for(&server, None, ca_pem.as_deref(), &pin)?;
            let token = match api.login(&username, &password) {
                Ok(token) => token,
                Err(ApiError::Untrusted) => {
                    let fingerprint = ownnas_core::trust::server_fingerprint(&server).unwrap_or_default();
                    return Err(format!(
                        "The server's certificate is not publicly trusted. If this fingerprint is the one `ownnas serve` prints, run setup again with --pin {fingerprint}"
                    ));
                }
                Err(err) => return Err(format!("Sign-in failed: {err}")),
            };
            let me = api_for(&server, Some(token.clone()), ca_pem.as_deref(), &pin)?.account().map_err(|err| err.to_string())?;
            let mut settings = load_settings();
            if let Some(gb) = cache_gb {
                settings.cache_gb = gb;
            }
            let existing = settings.accounts.iter().position(|account| account.server == server && account.username == me.username);
            let account = Account {
                id: existing.map(|at| settings.accounts[at].id.clone()).unwrap_or_else(new_id),
                folder: folder
                    .or_else(|| existing.map(|at| settings.accounts[at].folder.clone()))
                    .unwrap_or_else(|| ownnas_mount::home().join("OwnNAS").join(ownnas_mount::folder_name(&me.root_name))),
                server,
                username: me.username,
                name: me.root_name,
                ca_pem,
                pinned: pin,
                token,
            };
            std::fs::create_dir_all(&account.folder).map_err(|err| format!("Could not create {}: {err}", account.folder.display()))?;
            match existing {
                Some(at) => settings.accounts[at] = account.clone(),
                None => settings.accounts.push(account.clone()),
            }
            save_settings(&settings)?;
            desktop::bookmark(&account.folder, &account.name, true);
            desktop::install_menus()?;
            if no_service {
                println!("{} is set up at {}. Run `ownnas-mount run` to mount it.", account.name, account.folder.display());
            } else {
                match desktop::start_service() {
                    Ok(()) => println!("{} is at {} now.", account.name, account.folder.display()),
                    Err(err) => println!("{} is set up at {}, but the service did not start ({err}). Run `ownnas-mount run` to mount it.", account.name, account.folder.display()),
                }
            }
            println!("Files there download when you open them. To keep some on this computer, or free the space they take, use your file manager's menu or `ownnas-mount keep|free <paths>`.");
            Ok(())
        }
        Command::Status => {
            let settings = load_settings();
            if settings.accounts.is_empty() {
                println!("No servers yet: run `ownnas-mount setup --server … --username …`.");
            }
            let mounts = std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
            for account in &settings.accounts {
                let mounted = mounts.lines().any(|line| line.split(' ').nth(4).map(unescape).as_deref() == Some(account.folder.as_os_str().to_string_lossy().as_ref()));
                let index = std::fs::read(ownnas_mount::cache_dir().join(&account.id).join("index.json"))
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<ownnas_mount::Index>(&bytes).ok())
                    .unwrap_or_default();
                let used: u64 = index.files.values().map(|cached| cached.size).sum();
                let waiting = index.files.values().filter(|cached| cached.dirty).count();
                println!(
                    "{} ({} as {}) at {}{}: {} on this computer{}",
                    account.name,
                    account.server,
                    account.username,
                    account.folder.display(),
                    if mounted { "" } else { " (not mounted)" },
                    size_text(used),
                    if waiting > 0 { format!(", {waiting} changes not sent yet") } else { String::new() }
                );
            }
            println!("Cache limit: {} GB. Log: {}", settings.cache_gb, ownnas_mount::state_dir().join("mount.log").display());
            Ok(())
        }
        Command::Remove { server, force } => {
            let mut settings = load_settings();
            let at = settings
                .accounts
                .iter()
                .position(|account| account.server == server.trim_end_matches('/') || account.name == server)
                .ok_or_else(|| format!("No server {server}: see `ownnas-mount status`."))?;
            let account = settings.accounts[at].clone();
            let cache = ownnas_mount::cache_dir().join(&account.id);
            let index: ownnas_mount::Index =
                std::fs::read(cache.join("index.json")).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
            let waiting: Vec<&String> = index.files.iter().filter(|(_, cached)| cached.dirty).map(|(path, _)| path).collect();
            if !waiting.is_empty() && !force {
                let list: Vec<&str> = waiting.iter().take(10).map(|path| path.as_str()).collect();
                return Err(format!(
                    "Changes here have not reached the server yet ({}{}). Wait for them (see the log), or remove with --force to lose them.",
                    list.join(", "),
                    if waiting.len() > 10 { ", …" } else { "" }
                ));
            }
            settings.accounts.remove(at);
            save_settings(&settings)?;
            // The service mounts what is left.
            if settings.accounts.is_empty() { desktop::stop_service() } else { desktop::restart_service() }
            for program in ["fusermount3", "fusermount"] {
                if std::process::Command::new(program).arg("-u").arg(&account.folder).output().is_ok_and(|out| out.status.success()) {
                    break;
                }
            }
            if let Ok(api) = api_for(&account.server, Some(account.token.clone()), account.ca_pem.as_deref(), &account.pinned) {
                let _ = api.logout();
            }
            let _ = std::fs::remove_dir_all(&cache);
            let _ = std::fs::remove_dir(&account.folder);
            desktop::bookmark(&account.folder, &account.name, false);
            println!("{} is unmounted and signed out.", account.name);
            Ok(())
        }
        Command::Run => {
            let settings = load_settings();
            if settings.accounts.is_empty() {
                return Err("No servers yet: run `ownnas-mount setup --server … --username …`.".into());
            }
            // Stopping (systemd's SIGTERM, or Ctrl+C) unmounts first: a mount whose program is
            // gone fails everything in it until unmounted.
            let signals = unsafe {
                let mut set: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut set);
                libc::sigaddset(&mut set, libc::SIGTERM);
                libc::sigaddset(&mut set, libc::SIGINT);
                libc::sigaddset(&mut set, libc::SIGHUP);
                libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
                set
            };
            let limit = settings.cache_gb.saturating_mul(1_000_000_000);
            let mut mounts = Vec::new();
            for account in &settings.accounts {
                let api = api_for(&account.server, Some(account.token.clone()), account.ca_pem.as_deref(), &account.pinned)?;
                match ownnas_mount::mount::start(account, api, limit) {
                    Ok(mount) => mounts.push(mount),
                    Err(err) => ownnas_mount::log(&err),
                }
            }
            if mounts.is_empty() {
                return Err("Nothing could be mounted (see above).".into());
            }
            let mut signal = 0;
            unsafe { libc::sigwait(&signals, &mut signal) };
            ownnas_mount::log("stopping");
            for (inner, session) in mounts {
                inner.save();
                drop(session);
            }
            Ok(())
        }
        Command::Keep { paths } => set_keep(&paths, true),
        Command::Free { paths } => set_keep(&paths, false),
    }
}

#[cfg(target_os = "linux")]
fn api_for(server: &str, token: Option<String>, ca_pem: Option<&str>, pinned: &[String]) -> Result<ownnas_core::api::Api, String> {
    use ownnas_core::trust::Trust;
    ownnas_core::api::Api::with_trust(server, token, &Trust { ca_pem: ca_pem.map(|pem| pem.as_bytes().to_vec()), pinned: pinned.to_vec() })
}

/// Sets "always keep" on files and folders in a mount (the mount does the rest).
#[cfg(target_os = "linux")]
fn set_keep(paths: &[PathBuf], keep: bool) -> Result<(), String> {
    use std::os::unix::ffi::OsStrExt;
    let name = std::ffi::CString::new(ownnas_mount::KEEP_XATTR).unwrap();
    let value: &[u8] = if keep { b"1" } else { b"0" };
    for path in paths {
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|err| err.to_string())?;
        let done = unsafe { libc::setxattr(c_path.as_ptr(), name.as_ptr(), value.as_ptr().cast(), value.len(), 0) };
        if done != 0 {
            let err = std::io::Error::last_os_error();
            return Err(match err.raw_os_error() {
                Some(libc::ENOTSUP) => format!("{} is not in an OwnNAS folder", path.display()),
                _ => format!("{}: {err}", path.display()),
            });
        }
    }
    println!(
        "{}",
        if keep { "Kept on this computer: what is not downloaded yet is downloading." } else { "Freed: these download again when opened." }
    );
    Ok(())
}

/// Mount points in /proc/self/mountinfo escape spaces and such as `\040`.
#[cfg(target_os = "linux")]
fn unescape(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'\\' && at + 3 < bytes.len() && bytes[at + 1..at + 4].iter().all(|b| (b'0'..=b'7').contains(b)) {
            out.push(u8::from_str_radix(&text[at + 1..at + 4], 8).unwrap_or(b'?'));
            at += 4;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(target_os = "linux")]
fn size_text(bytes: u64) -> String {
    match bytes {
        b if b >= 1_000_000_000 => format!("{:.1} GB", b as f64 / 1e9),
        b if b >= 1_000_000 => format!("{:.1} MB", b as f64 / 1e6),
        b if b >= 1_000 => format!("{:.0} KB", b as f64 / 1e3),
        b => format!("{b} bytes"),
    }
}

#[cfg(target_os = "linux")]
fn new_id() -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{:x}{:x}", nanos, std::process::id())
}

/// The desktop's parts: the systemd user service, file managers' sidebars and menus.
#[cfg(target_os = "linux")]
mod desktop {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    const SERVICE: &str = "ownnas-mount.service";

    fn exe() -> String {
        std::env::current_exe().map(|path| path.display().to_string()).unwrap_or_else(|_| "ownnas-mount".into())
    }

    fn data_home() -> PathBuf {
        match std::env::var_os("XDG_DATA_HOME") {
            Some(dir) if !dir.is_empty() => PathBuf::from(dir),
            _ => ownnas_mount::home().join(".local/share"),
        }
    }

    fn systemctl(args: &[&str]) -> Result<(), String> {
        let out = Command::new("systemctl").arg("--user").args(args).output().map_err(|err| format!("systemctl: {err}"))?;
        if out.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&out.stderr).trim().to_string()) }
    }

    /// Installs the user service and (re)starts it, so it mounts every server signed in to.
    pub fn start_service() -> Result<(), String> {
        let unit = format!(
            "[Unit]\nDescription=OwnNAS folders (online-only files)\nAfter=network-online.target\n\n\
             [Service]\nExecStart=\"{}\" run\nRestart=on-failure\nRestartSec=5\n\n\
             [Install]\nWantedBy=default.target\n",
            exe()
        );
        let dir = ownnas_mount::config_dir().parent().map(Path::to_path_buf).unwrap_or_default().join("systemd/user");
        std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
        std::fs::write(dir.join(SERVICE), unit).map_err(|err| err.to_string())?;
        systemctl(&["daemon-reload"])?;
        systemctl(&["enable", SERVICE])?;
        systemctl(&["restart", SERVICE])
    }

    pub fn restart_service() {
        let _ = systemctl(&["try-restart", SERVICE]);
    }

    pub fn stop_service() {
        let _ = systemctl(&["disable", "--now", SERVICE]);
    }

    /// Adds (or removes) the folder in the sidebar of GTK file managers (Files, Nemo, Caja,
    /// Thunar).
    pub fn bookmark(folder: &Path, name: &str, add: bool) {
        let file = ownnas_mount::config_dir().parent().map(Path::to_path_buf).unwrap_or_default().join("gtk-3.0/bookmarks");
        let uri = format!("file://{}", encode(&folder.display().to_string()));
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        let mut lines: Vec<String> = text.lines().filter(|line| line.split(' ').next() != Some(uri.as_str())).map(str::to_string).collect();
        if add {
            lines.push(format!("{uri} {name}"));
        }
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&file, lines.iter().map(|line| format!("{line}\n")).collect::<String>());
    }

    fn encode(path: &str) -> String {
        path.bytes()
            .map(|b| if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
            .collect()
    }

    /// "Always keep on this computer" and "Free up space" in Dolphin's, Nautilus's and Nemo's
    /// menus.
    pub fn install_menus() -> Result<(), String> {
        let exe = exe();
        let data = data_home();
        let files = [
            ("kio/servicemenus/ownnas.desktop", include_str!("../desktop/dolphin.desktop"), true),
            ("nautilus-python/extensions/ownnas.py", include_str!("../desktop/nautilus.py"), false),
            ("nemo/actions/ownnas-keep.nemo_action", include_str!("../desktop/keep.nemo_action"), false),
            ("nemo/actions/ownnas-free.nemo_action", include_str!("../desktop/free.nemo_action"), false),
        ];
        for (path, text, executable) in files {
            let file = data.join(path);
            if let Some(dir) = file.parent() {
                std::fs::create_dir_all(dir).map_err(|err| format!("Could not create {}: {err}", dir.display()))?;
            }
            std::fs::write(&file, text.replace("@EXE@", &exe)).map_err(|err| format!("Could not write {}: {err}", file.display()))?;
            if executable {
                // Dolphin only runs service menus marked executable.
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755));
            }
        }
        Ok(())
    }
}
