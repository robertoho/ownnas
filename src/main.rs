mod auth;
mod content_index;
mod db;
mod files;
mod http;
mod pdf_ops;
mod preview;
mod thumbs;
mod tls;
mod update;

use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "ownnas",
    version,
    about = "Share one folder, with login, previews, and a SQLite database",
    after_help = "Examples:\n  ownnas serve --root ~/Pictures\n  ownnas serve --root /media --addr 0.0.0.0:8787\n  ownnas user add --username alice --password \"a long passphrase\"\n\nBackground service:\n  ownnas serve stays in the foreground. Install it with launchd (macOS),\n  systemd (Linux), or Task Scheduler / NSSM (Windows). See:\n  ownnas serve --help"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start the web server and share one folder
    #[command(after_help = SERVE_AFTER_HELP)]
    Serve(ServeArgs),
    /// Generate a private HTTPS CA and server certificate
    Tls {
        #[command(subcommand)]
        command: TlsCommand,
    },
    /// Manage accounts stored in the SQLite database
    User {
        #[command(subcommand)]
        command: UserCommand,
    },
    /// Generate keys / sign release manifests for the static update host
    Update {
        #[command(subcommand)]
        command: UpdateCommand,
    },
}

const SERVE_AFTER_HELP: &str = "\
First start:
  If the data folder has no accounts yet, OwnNAS asks in the terminal for
  any missing --root, --username, or --password. Create the account once
  before installing a background service.

Examples:
  ownnas serve --root ~/Pictures
  ownnas serve --root D:\\\\media --username admin --password secretpass
  OWNNAS_PASSWORD=secretpass ownnas serve --root /media --username admin
  ownnas serve --root /media --readonly --addr 127.0.0.1:8787

Background / daemon:
  OwnNAS does not daemonize itself. Point an OS service at the release
  binary with absolute paths for the binary, --root, and --data.

  macOS (launchd user agent):
    1. Create the account with a normal `ownnas serve` once.
    2. Save ~/Library/LaunchAgents/com.ownnas.serve.plist that runs:
         /abs/ownnas serve --root /abs/library --data /abs/ownnas-data --addr 0.0.0.0:8787
       with RunAtLoad=true and KeepAlive=true.
    3. launchctl load ~/Library/LaunchAgents/com.ownnas.serve.plist
       launchctl unload ~/Library/LaunchAgents/com.ownnas.serve.plist

  Linux (systemd):
    1. Create /etc/systemd/system/ownnas.service (or a user unit) with:
         ExecStart=/abs/ownnas serve --root /abs/library --data /abs/ownnas-data --addr 0.0.0.0:8787
         Restart=on-failure
    2. systemctl daemon-reload
       systemctl enable --now ownnas
       systemctl status ownnas

  Windows:
    1. Create the account once in a terminal.
    2. Task Scheduler: at logon run
         C:\\\\path\\\\ownnas.exe serve --root D:\\\\media --data C:\\\\ownnas-data --addr 0.0.0.0:8787
       or install the same command with NSSM as a Windows service.

  Prefer OWNNAS_PASSWORD in the service environment over --password on
  the command line. Full plist and unit samples are in README.md.
";

#[derive(Subcommand)]
enum TlsCommand {
    /// Generate certificates in a new directory; distribute only ca.crt to users
    Generate {
        #[arg(long, default_value = "ownnas-tls")]
        out: PathBuf,
        /// DNS name or IP used to reach this server (repeat for multiple names)
        #[arg(long, required = true)]
        host: Vec<String>,
    },
}

#[derive(Parser)]
struct ServeArgs {
    /// Folder to share, including its subfolders. Asked on the first start when omitted.
    #[arg(long, env = "OWNNAS_ROOT", value_name = "DIR")]
    root: Option<PathBuf>,
    /// Listen address. Use 0.0.0.0:8787 so other devices on the VPN can connect.
    #[arg(long, env = "OWNNAS_ADDR", default_value = "0.0.0.0:8787", value_name = "HOST:PORT")]
    addr: String,
    /// Folder for ownnas.db and the thumbnail cache. Must sit outside --root.
    #[arg(long, env = "OWNNAS_DATA", default_value = "ownnas-data", value_name = "DIR")]
    data: PathBuf,
    /// PEM server certificate chain; enables native HTTPS
    #[arg(long, env = "OWNNAS_TLS_CERT", requires = "tls_key")]
    tls_cert: Option<PathBuf>,
    /// PEM private key for the HTTPS server
    #[arg(long, env = "OWNNAS_TLS_KEY", requires = "tls_cert")]
    tls_key: Option<PathBuf>,
    /// Create this account when the database has no users yet. Ignored later.
    #[arg(long, env = "OWNNAS_USER", value_name = "NAME")]
    username: Option<String>,
    /// Password for --username. Prefer OWNNAS_PASSWORD so it stays out of the process list.
    #[arg(long, env = "OWNNAS_PASSWORD", value_name = "SECRET")]
    password: Option<String>,
    /// Browse, preview, and download only. Uploads and renames are blocked.
    #[arg(long, action = clap::ArgAction::SetTrue)]
    readonly: bool,
    /// Mark the session cookie Secure. Turn this on behind HTTPS.
    #[arg(long, action = clap::ArgAction::SetTrue)]
    secure_cookie: bool,
    /// Open a browser window on this computer after the server starts
    #[arg(long, action = clap::ArgAction::SetTrue)]
    open: bool,
    /// HTTPS URL to latest.json on your update host (OWNNAS_UPDATE_URL)
    #[arg(long, env = "OWNNAS_UPDATE_URL", value_name = "URL")]
    update_url: Option<String>,
    /// Hex ed25519 public key for latest.json.sig (OWNNAS_UPDATE_PUBKEY)
    #[arg(long, env = "OWNNAS_UPDATE_PUBKEY", value_name = "HEX")]
    update_pubkey: Option<String>,
    /// Let launchd/systemd/NSSM restart OwnNAS after updates instead of spawning a child
    #[arg(long, env = "OWNNAS_RESTART_BY_SUPERVISOR", action = clap::ArgAction::SetTrue)]
    restart_by_supervisor: bool,
}

#[derive(Subcommand)]
enum UpdateCommand {
    /// Create update.sk / update.pk for signing manifests
    Keygen {
        #[arg(long, default_value = "releases/keys", value_name = "DIR")]
        out: PathBuf,
    },
    /// Sign latest.json → latest.json.sig with update.sk
    Sign {
        #[arg(long, value_name = "PATH")]
        key: PathBuf,
        #[arg(long, value_name = "PATH")]
        manifest: PathBuf,
    },
}

#[derive(Subcommand)]
enum UserCommand {
    /// Add an account
    Add(UserPassArgs),
    /// Change an account password and sign its sessions out
    Password(UserPassArgs),
    /// Delete an account
    Remove(UserNameArgs),
    /// List accounts
    List(DataArgs),
}

#[derive(Parser)]
struct DataArgs {
    #[arg(long, env = "OWNNAS_DATA", default_value = "ownnas-data")]
    data: PathBuf,
}

#[derive(Parser)]
struct UserNameArgs {
    #[command(flatten)]
    data: DataArgs,
    #[arg(long)]
    username: String,
}

#[derive(Parser)]
struct UserPassArgs {
    #[command(flatten)]
    data: DataArgs,
    #[arg(long, env = "OWNNAS_USER")]
    username: String,
    #[arg(long, env = "OWNNAS_PASSWORD")]
    password: String,
}

fn main() {
    let cli = Cli::parse();
    if let Err(err) = run(cli) {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.command {
        Command::Serve(args) => serve(args),
        Command::Tls { command } => match command {
            TlsCommand::Generate { out, host } => tls::generate(&out, host),
        },
        Command::Update { command } => match command {
            UpdateCommand::Keygen { out } => update::keygen(&out),
            UpdateCommand::Sign { key, manifest } => update::sign_manifest(&key, &manifest),
        },
        Command::User { command } => match command {
            UserCommand::Add(args) => {
                let conn = open_data(&args.data.data)?;
                db::create_user(&conn, &args.username, &args.password)?;
                println!("Added account {}", args.username.trim());
                Ok(())
            }
            UserCommand::Password(args) => {
                let conn = open_data(&args.data.data)?;
                db::set_password(&conn, &args.username, &args.password)?;
                println!("Updated the password for {}", args.username.trim());
                Ok(())
            }
            UserCommand::Remove(args) => {
                let conn = open_data(&args.data.data)?;
                db::delete_user(&conn, &args.username)?;
                let left = db::count_users(&conn)?;
                println!("Removed account {}", args.username.trim());
                if left == 0 {
                    println!("No accounts remain. The next serve will ask you to create one.");
                }
                Ok(())
            }
            UserCommand::List(args) => {
                let conn = open_data(&args.data)?;
                for name in db::list_users(&conn)? {
                    println!("{name}");
                }
                Ok(())
            }
        },
    }
}

fn open_data(data: &std::path::Path) -> Result<rusqlite::Connection, String> {
    std::fs::create_dir_all(data)
        .map_err(|_| format!("Could not create the data folder: {}", data.display()))?;
    db::open(&data.join("ownnas.db"))
}

fn serve(args: ServeArgs) -> Result<(), String> {
    let mut announced = false;
    let root = match args.root {
        Some(root) => root,
        None => first_run_root(&args.data, &mut announced)?,
    };
    let (root, data) = files::prepare_paths(&root, &args.data)?;
    let update_url = args
        .update_url
        .map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty());
    let pubkey = match args.update_pubkey.as_deref() {
        Some(value) if !value.trim().is_empty() => Some(update::parse_pubkey_hex(value)?),
        _ if update_url.is_none() => Some(update::parse_pubkey_hex(update::EMBEDDED_PUBKEY)?),
        _ => None,
    };
    let update = update::UpdateConfig {
        url: Some(update_url.unwrap_or_else(|| update::DEFAULT_URL.to_string())),
        pubkey,
    };
    let saved_root = root.clone();
    let mut app_state = http::new_state(
        root,
        data,
        args.readonly,
        args.secure_cookie || args.tls_cert.is_some(),
        thumbs::ffmpeg_available(),
        content_index::tesseract_available(),
        update,
    )?;
    app_state.supervisor_restart = args.restart_by_supervisor;
    let state = Arc::new(app_state);
    {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        let count = db::count_users(&conn)?;
        if count == 0 {
            let (username, password) =
                first_account(args.username, args.password, &mut announced)?;
            db::create_user(&conn, &username, &password)?;
            println!("Created account {}", username.trim());
        } else if args.username.is_some() {
            println!("An account already exists, so --username was left unused.");
        }
    }
    {
        let conn = state.db.lock().unwrap_or_else(|err| err.into_inner());
        save_shared_root(&conn, &saved_root)?;
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|_| "Could not start the async runtime".to_string())?;
    let server_result = runtime.block_on(async {
        let update_state = state.clone();
        tokio::spawn(async move {
            loop {
                if update_state.update.url.is_some() {
                    let config = update_state.update.clone();
                    let result = tokio::task::spawn_blocking(move || update::check(&config)).await;
                    let mut value = match result {
                        Ok(Ok(result)) => serde_json::to_value(result).unwrap_or_default(),
                        Ok(Err(err)) => serde_json::json!({"configured": true, "available": false, "error": err}),
                        Err(_) => serde_json::json!({"configured": true, "available": false, "error": "Update check failed"}),
                    };
                    if let Some(object) = value.as_object_mut() {
                        object.insert("checkedAt".into(), serde_json::Value::String(format!("{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs())));
                    }
                    *update_state.update_status.lock().unwrap_or_else(|e| e.into_inner()) = Some(value);
                }
                tokio::time::sleep(std::time::Duration::from_secs(24 * 60 * 60)).await;
            }
        });
        match (args.tls_cert, args.tls_key) {
            (Some(cert), Some(key)) => http::serve_https(state.clone(), &args.addr, args.open, &cert, &key).await,
            _ => http::serve(state.clone(), &args.addr, args.open).await,
        }
    });
    if state.restart_requested.load(std::sync::atomic::Ordering::SeqCst) {
        if state.supervisor_restart {
            return server_result;
        }
        update::spawn_replaced_binary()?;
    }
    server_result
}

fn interactive() -> bool {
    io::stdin().is_terminal() && io::stderr().is_terminal()
}

fn announce(announced: &mut bool) {
    if *announced {
        return;
    }
    eprintln!("Set up OwnNAS");
    eprintln!();
    *announced = true;
}

fn load_shared_root(conn: &rusqlite::Connection) -> Result<Option<PathBuf>, String> {
    use rusqlite::OptionalExtension;
    let value: Option<String> = conn.query_row(
        "SELECT value FROM settings WHERE key = 'shared_root'", [], |row| row.get(0),
    ).optional().map_err(|err| format!("Could not read the saved shared folder: {err}"))?;
    value.map(|value| serde_json::from_str(&value)
        .map_err(|err| format!("Could not read the saved shared folder; pass --root to replace it: {err}")))
        .transpose()
}

fn save_shared_root(conn: &rusqlite::Connection, root: &Path) -> Result<(), String> {
    let value = serde_json::to_string(root).map_err(|err| err.to_string())?;
    conn.execute(
        "INSERT INTO settings(key, value) VALUES('shared_root', ?1) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [value],
    ).map_err(|err| format!("Could not save the shared folder: {err}"))?;
    Ok(())
}

fn first_run_root(data: &Path, announced: &mut bool) -> Result<PathBuf, String> {
    let conn = open_data(data)?;
    if let Some(root) = load_shared_root(&conn)? {
        return Ok(root);
    }
    drop(conn);
    if !interactive() {
        return Err("No shared folder is saved yet. Pass --root with the folder to share.".into());
    }
    announce(announced);
    loop {
        let entered = prompt_line("Folder to share: ")?;
        if entered.is_empty() {
            eprintln!("Enter the folder to share.");
            continue;
        }
        let root = expand_home(&entered);
        match files::prepare_paths(&root, data) {
            Ok((root, _)) => return Ok(root),
            Err(err) => eprintln!("{err}"),
        }
    }
}

fn first_account(
    username: Option<String>,
    password: Option<String>,
    announced: &mut bool,
) -> Result<(String, String), String> {
    match (username, password) {
        (Some(username), Some(password)) => {
            auth::validate_username(&username).map_err(|err| err.to_string())?;
            auth::validate_password(&password).map_err(|err| err.to_string())?;
            Ok((username, password))
        }
        (username, password) => {
            if !interactive() {
                return Err(
                    "No account exists yet. Start once with --username and --password, or run `ownnas user add`."
                        .into(),
                );
            }
            announce(announced);
            let username = match username {
                Some(username) => {
                    auth::validate_username(&username).map_err(|err| err.to_string())?;
                    username
                }
                None => prompt_username()?,
            };
            let password = match password {
                Some(password) => {
                    auth::validate_password(&password).map_err(|err| err.to_string())?;
                    password
                }
                None => prompt_password()?,
            };
            Ok((username, password))
        }
    }
}

fn prompt_username() -> Result<String, String> {
    loop {
        let entered = prompt_line("Username [admin]: ")?;
        let username = if entered.is_empty() {
            "admin".to_string()
        } else {
            entered
        };
        match auth::validate_username(&username) {
            Ok(()) => return Ok(username),
            Err(err) => eprintln!("{err}"),
        }
    }
}

fn prompt_password() -> Result<String, String> {
    loop {
        let password = prompt_secret("Password: ")?;
        if let Err(err) = auth::validate_password(&password) {
            eprintln!("{err}");
            continue;
        }
        let confirm = prompt_secret("Confirm password: ")?;
        if password != confirm {
            eprintln!("Those passwords do not match.");
            continue;
        }
        return Ok(password);
    }
}

fn prompt_line(label: &str) -> Result<String, String> {
    eprint!("{label}");
    io::stderr()
        .flush()
        .map_err(|_| "Could not show the prompt".to_string())?;
    let mut line = String::new();
    let read = io::stdin()
        .read_line(&mut line)
        .map_err(|_| "Could not read input".to_string())?;
    if read == 0 {
        return Err("Setup cancelled".into());
    }
    Ok(line.trim().to_string())
}

fn prompt_secret(label: &str) -> Result<String, String> {
    rpassword::prompt_password(label).map_err(|_| "Could not read the password".to_string())
}

fn expand_home(path: &str) -> PathBuf {
    let path = path.trim();
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    let Some(home) = home else {
        return PathBuf::from(path);
    };
    if path == "~" {
        return PathBuf::from(home);
    }
    path.strip_prefix("~/")
        .or_else(|| path.strip_prefix("~\\"))
        .map(|rest| PathBuf::from(home).join(rest))
        .unwrap_or_else(|| PathBuf::from(path))
}

#[cfg(test)]
mod tests {
    use super::expand_home;

    #[test]
    fn tilde_expands_to_the_home_directory() {
        let home = std::env::var_os("HOME").expect("home");
        assert_eq!(expand_home("~"), std::path::PathBuf::from(&home));
        assert_eq!(
            expand_home("~/Pictures"),
            std::path::PathBuf::from(home).join("Pictures")
        );
        assert_eq!(
            expand_home("/tmp/library"),
            std::path::PathBuf::from("/tmp/library")
        );
    }
}
