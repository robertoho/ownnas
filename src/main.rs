mod auth;
mod db;
mod files;
mod http;
mod preview;
mod thumbs;

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
    /// Manage accounts stored in the SQLite database
    User {
        #[command(subcommand)]
        command: UserCommand,
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
    let state = Arc::new(http::new_state(
        root,
        data,
        args.readonly,
        args.secure_cookie,
        thumbs::ffmpeg_available(),
    )?);
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
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|_| "Could not start the async runtime".to_string())?;
    runtime.block_on(http::serve(state, &args.addr, args.open))
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

fn first_run_root(data: &Path, announced: &mut bool) -> Result<PathBuf, String> {
    let conn = open_data(data)?;
    let fresh = db::count_users(&conn)? == 0;
    drop(conn);
    if !fresh || !interactive() {
        return Err("Pass --root with the folder to share.".into());
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
