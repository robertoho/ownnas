mod auth;
mod db;
mod files;
mod http;
mod preview;
mod thumbs;

use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "ownnas",
    version,
    about = "Share one folder, with login, previews, and a SQLite database"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start the web server
    Serve(ServeArgs),
    /// Manage accounts stored in the SQLite database
    User {
        #[command(subcommand)]
        command: UserCommand,
    },
}

#[derive(Parser)]
struct ServeArgs {
    /// Folder to share, including its subfolders
    #[arg(long, env = "OWNNAS_ROOT")]
    root: PathBuf,
    /// Listen address. Use 0.0.0.0:8787 so other devices on the VPN can connect.
    #[arg(long, env = "OWNNAS_ADDR", default_value = "0.0.0.0:8787")]
    addr: String,
    /// Folder for ownnas.db and the thumbnail cache. Must sit outside --root.
    #[arg(long, env = "OWNNAS_DATA", default_value = "ownnas-data")]
    data: PathBuf,
    /// Create this account when the database has no users yet
    #[arg(long, env = "OWNNAS_USER")]
    username: Option<String>,
    /// Password for --username. Prefer the OWNNAS_PASSWORD environment variable.
    #[arg(long, env = "OWNNAS_PASSWORD")]
    password: Option<String>,
    /// Browse, preview, and download only
    #[arg(long, action = clap::ArgAction::SetTrue)]
    readonly: bool,
    /// Mark the session cookie Secure. Turn this on behind HTTPS.
    #[arg(long, action = clap::ArgAction::SetTrue)]
    secure_cookie: bool,
    /// Open a browser window on this computer
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
                    println!("No accounts remain. Create one before starting the server again.");
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
    let (root, data) = files::prepare_paths(&args.root, &args.data)?;
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
            match (&args.username, &args.password) {
                (Some(username), Some(password)) => {
                    db::create_user(&conn, username, password)?;
                    println!("Created account {}", username.trim());
                }
                _ => {
                    return Err(
                        "No account exists yet. Start once with --username and --password, or run `ownnas user add`."
                            .into(),
                    );
                }
            }
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
