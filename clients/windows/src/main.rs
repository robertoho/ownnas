//! `ownnas-cloud`: puts OwnNAS servers in Windows Explorer, with online-only files.
//!
//! `ownnas-cloud setup --server https://nas.example:8787 --username ana` signs in, adds the
//! server to Explorer's navigation pane, and starts `ownnas-cloud-agent` (now, and at each
//! sign-in), which keeps it in sync in the background.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "ownnas-cloud", version, about = "OwnNAS servers in Windows Explorer, with online-only files")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Sign in to a server and add it to Explorer.
    Setup {
        /// Server address, for example https://nas.example:8787
        #[arg(long)]
        server: String,
        #[arg(long)]
        username: String,
        /// Password (asked for if left out).
        #[arg(long, env = "OWNNAS_PASSWORD", hide_env_values = true)]
        password: Option<String>,
        /// The folder Explorer shows it in (default: "OwnNAS - <name>" in your user folder).
        #[arg(long)]
        folder: Option<PathBuf>,
        /// PEM certificate of a private CA to trust (the ca.crt from `ownnas tls generate`).
        #[arg(long)]
        ca_cert: Option<PathBuf>,
        /// A server certificate fingerprint to trust (as `ownnas serve` prints it).
        #[arg(long)]
        pin: Vec<String>,
    },
    /// The servers signed in to, and where they are.
    Status,
    /// Take a server out of Explorer and sign out of it (files on this computer stay).
    Remove {
        /// Its address or name, as `status` shows.
        server: String,
    },
    /// Keep everything in sync in this window, writing what happens (for troubleshooting;
    /// the agent does this in the background).
    Run,
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

#[cfg(not(windows))]
fn run(_command: Command) -> Result<(), String> {
    Err("ownnas-cloud works on Windows. On macOS use the OwnNAS app (clients/macos); on Linux, ownnas-sync (clients/desktop).".into())
}

#[cfg(windows)]
fn run(command: Command) -> Result<(), String> {
    use ownnas_cloud::{load_settings, save_settings, secret, sync, Account};
    use ownnas_core::api::{Api, ApiError};
    use ownnas_core::trust::Trust;

    let api_for = |server: &str, token: Option<String>, ca_pem: Option<&str>, pinned: &[String]| {
        Api::with_trust(server, token, &Trust { ca_pem: ca_pem.map(|pem| pem.as_bytes().to_vec()), pinned: pinned.to_vec() })
    };
    match command {
        Command::Setup { server, username, password, folder, ca_cert, pin } => {
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
            let existing = settings.accounts.iter().position(|account| account.server == server && account.username == me.username);
            let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
            let account = Account {
                id: existing.map(|at| settings.accounts[at].id.clone()).unwrap_or_else(new_id),
                folder: folder
                    .or_else(|| existing.map(|at| settings.accounts[at].folder.clone()))
                    .unwrap_or_else(|| home.join(ownnas_cloud::folder_name(&me.root_name))),
                server,
                username: me.username,
                name: me.root_name,
                ca_pem,
                pinned: pin,
                token: secret::protect(&token)?,
            };
            sync::register(&account)?;
            match existing {
                Some(at) => settings.accounts[at] = account.clone(),
                None => settings.accounts.push(account.clone()),
            }
            save_settings(&settings)?;
            start_agent_at_sign_in()?;
            start_agent();
            println!("{} is in Explorer now, at {}.", account.name, account.folder.display());
            println!("Files there download when you open them; \"Free up space\" in their menu empties them again.");
            Ok(())
        }
        Command::Status => {
            let settings = load_settings();
            if settings.accounts.is_empty() {
                println!("No servers yet: run `ownnas-cloud setup --server … --username …`.");
            }
            for account in &settings.accounts {
                let registered = sync::root_id(account).ok().and_then(|id| id.is_registered().ok()).unwrap_or(false);
                println!("{} ({} as {}) at {}{}", account.name, account.server, account.username, account.folder.display(), if registered { "" } else { " — not in Explorer: run setup again" });
            }
            println!("Log: {}", ownnas_cloud::data_dir().join("cloud.log").display());
            Ok(())
        }
        Command::Remove { server } => {
            let mut settings = load_settings();
            let at = settings
                .accounts
                .iter()
                .position(|account| account.server == server.trim_end_matches('/') || account.name == server)
                .ok_or_else(|| format!("No server {server}: see `ownnas-cloud status`."))?;
            let account = settings.accounts.remove(at);
            // Windows won't take out a place while the agent has it connected: stop it, and
            // start it again for the places left.
            stop_agent();
            let unregistered = sync::unregister(&account);
            if !settings.accounts.is_empty() {
                start_agent();
            }
            if let Err(err) = unregistered {
                settings.accounts.insert(at, account);
                return Err(err);
            }
            if let Ok(token) = secret::unprotect(&account.token) {
                if let Ok(api) = api_for(&account.server, Some(token), account.ca_pem.as_deref(), &account.pinned) {
                    let _ = api.logout();
                }
            }
            save_settings(&settings)?;
            if settings.accounts.is_empty() {
                let _ = std::process::Command::new("reg").args(["delete", RUN_KEY, "/v", "OwnNAS", "/f"]).output();
            }
            println!("{} is out of Explorer. Its files on this computer are still in {}.", account.name, account.folder.display());
            Ok(())
        }
        Command::Run => ownnas_cloud::sync::run_all(),
    }
}

#[cfg(windows)]
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

#[cfg(windows)]
fn agent_path() -> Result<PathBuf, String> {
    Ok(std::env::current_exe().map_err(|err| err.to_string())?.with_file_name("ownnas-cloud-agent.exe"))
}

/// Starts the agent at each sign-in to Windows.
#[cfg(windows)]
fn start_agent_at_sign_in() -> Result<(), String> {
    let agent = agent_path()?;
    let out = std::process::Command::new("reg")
        .args(["add", RUN_KEY, "/v", "OwnNAS", "/t", "REG_SZ", "/d", &format!("\"{}\"", agent.display()), "/f"])
        .output()
        .map_err(|err| err.to_string())?;
    if out.status.success() { Ok(()) } else { Err(format!("Could not set OwnNAS to start at sign-in: {}", String::from_utf8_lossy(&out.stderr))) }
}

/// Stops the agent (every place it serves goes offline until it starts again).
#[cfg(windows)]
fn stop_agent() {
    let _ = std::process::Command::new("taskkill").args(["/F", "/IM", "ownnas-cloud-agent.exe"]).output();
    // Windows lets go of the places a moment after the process ends.
    std::thread::sleep(std::time::Duration::from_secs(1));
}

/// Starts the agent now (a second one finds the places connected already and stops).
#[cfg(windows)]
fn start_agent() {
    use std::process::Stdio;
    use windows::Win32::Foundation::{SetHandleInformation, HANDLE_FLAGS, HANDLE_FLAG_INHERIT};
    use windows::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
    // Programs started here inherit every handle that may be inherited, whatever their own
    // input and output are: the agent would keep this window's, and whatever reads this
    // program's output would wait for the agent to end.
    for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        if let Ok(handle) = unsafe { GetStdHandle(which) } {
            let _ = unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0)) };
        }
    }
    if let Ok(agent) = agent_path() {
        let _ = std::process::Command::new(agent).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    }
}

#[cfg(windows)]
fn new_id() -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{:x}{:x}", nanos, std::process::id())
}
