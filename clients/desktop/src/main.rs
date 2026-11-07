//! `ownnas-sync`: keeps local folders and folders on an OwnNAS server in sync.
//!
//! Each server folder you pick syncs with a local folder of its own (selective sync):
//! `setup` signs in and links the first one, `add` links more with the same sign-in, and
//! `run` with no folder syncs every linked folder.
mod folders;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};

use ownnas_core::api::{Api, ApiError};
use ownnas_core::model::{self, Config};
use ownnas_core::{engine, local};

use folders::show_remote;

#[derive(Parser)]
#[command(name = "ownnas-sync", version, about = "Two-way folder sync with an OwnNAS server")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Sign in and link a local folder to a folder on the server.
    Setup {
        /// Local folder to sync (created if missing).
        dir: PathBuf,
        /// Server address, for example https://nas.example:8787
        #[arg(long)]
        server: String,
        #[arg(long)]
        username: String,
        /// Password (prompted if omitted).
        #[arg(long, env = "OWNNAS_PASSWORD", hide_env_values = true)]
        password: Option<String>,
        /// Folder on the server to sync, relative to its shared folder (default: all of it).
        #[arg(long, default_value = "")]
        remote: String,
        /// PEM certificate of a private CA to trust (e.g. the `ca.pem` from `ownnas tls`).
        #[arg(long)]
        ca_cert: Option<PathBuf>,
    },
    /// Link one more server folder to a local folder of its own, with the sign-in of a folder
    /// already set up (no password needed).
    Add {
        /// Local folder to sync it with (created if missing).
        dir: PathBuf,
        /// Folder on the server, relative to its shared folder (see `ownnas-sync folders`).
        #[arg(long)]
        remote: String,
        /// Which server, when folders here are signed in to more than one.
        #[arg(long)]
        server: Option<String>,
        /// Create the server folder if it doesn't exist.
        #[arg(long)]
        create: bool,
    },
    /// List the folders on the server, and which of them sync with this computer.
    Folders {
        /// Server folder to look in (default: the shared folder itself).
        #[arg(default_value = "")]
        path: String,
        /// Which server, when folders here are signed in to more than one.
        #[arg(long)]
        server: Option<String>,
    },
    /// The linked folders: each local folder and the server folder it syncs with.
    List,
    /// Stop syncing a folder. Its files stay on this computer and on the server.
    Remove { dir: PathBuf },
    /// Sync once, or keep syncing with --watch. With no folder, syncs every linked folder.
    Run {
        dir: Option<PathBuf>,
        /// Keep running and sync again every --interval seconds.
        #[arg(long)]
        watch: bool,
        #[arg(long, default_value_t = 30)]
        interval: u64,
        /// Show what would change without changing anything.
        #[arg(long)]
        dry_run: bool,
        /// Print every file operation.
        #[arg(short, long)]
        verbose: bool,
    },
    /// Sign this folder's token out on the server and forget it locally (also in the other
    /// folders that share that sign-in).
    Logout { dir: PathBuf },
}

fn api_for(config: &Config, token: Option<String>) -> Result<Api, String> {
    let ca = match &config.ca_cert {
        Some(path) => Some(std::fs::read(path).map_err(|err| format!("Could not read {}: {err}", path.display()))?),
        None => None,
    };
    Api::new(&config.server, token, ca.as_deref())
}

/// Creates the folder if needed and returns its full path.
fn local_dir(dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|err| format!("Could not create {}: {err}", dir.display()))?;
    dir.canonicalize().map_err(|err| err.to_string())
}

/// The full path of a folder that may no longer exist.
fn existing_or_given(dir: &Path) -> PathBuf {
    dir.canonicalize().unwrap_or_else(|_| std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf()))
}

fn setup(
    dir: &Path,
    server: String,
    username: String,
    password: Option<String>,
    remote: String,
    ca_cert: Option<PathBuf>,
) -> Result<(), String> {
    let dir = local_dir(dir)?;
    let ca_cert = match ca_cert {
        Some(path) => Some(path.canonicalize().map_err(|err| format!("Could not find {}: {err}", path.display()))?),
        None => None,
    };
    let remote = folders::clean_remote(&remote);
    let mut config = Config { server, remote, username, token: String::new(), ca_cert };
    folders::check_overlap(&dir, &config)?;
    let password = match password {
        Some(password) => password,
        None => rpassword::prompt_password(format!("Password for {}: ", config.username)).map_err(|err| err.to_string())?,
    };
    let api = api_for(&config, None)?;
    config.token = api.login(&config.username, &password).map_err(|err| format!("Sign-in failed: {err}"))?;
    // Make sure the remote folder exists before the first sync.
    let api = api_for(&config, Some(config.token.clone()))?;
    if !config.remote.is_empty() {
        api.mkdir(&config.remote).map_err(|err| format!("Could not open {} on the server: {err}", config.remote))?;
    }
    api.tree(&config.remote).map_err(|err| err.to_string())?;
    model::save_config(&dir, &config)?;
    folders::add(&dir)?;
    println!(
        "Linked {} to {} on {}. Run `ownnas-sync run --watch` to start syncing; `ownnas-sync add` links more folders.",
        dir.display(),
        show_remote(&config.remote),
        config.server,
    );
    Ok(())
}

fn add(dir: &Path, remote: &str, server: Option<&str>, create: bool) -> Result<(), String> {
    let mut config = folders::sign_in_for(server)?;
    config.remote = folders::clean_remote(remote);
    let dir = existing_or_given(dir);
    folders::check_overlap(&dir, &config)?;
    let api = api_for(&config, Some(config.token.clone()))?;
    match api.list(&config.remote) {
        Ok(_) => {}
        Err(ApiError::NotFound) if create => {
            api.mkdir(&config.remote).map_err(|err| format!("Could not create {} on the server: {err}", show_remote(&config.remote)))?;
        }
        Err(ApiError::NotFound) => {
            return Err(format!(
                "There is no folder {} on {}. See `ownnas-sync folders`, or pass --create to make it.",
                show_remote(&config.remote),
                config.server
            ))
        }
        Err(err) => return Err(format!("Could not open {} on the server: {err}", show_remote(&config.remote))),
    }
    let dir = local_dir(&dir)?;
    model::save_config(&dir, &config)?;
    folders::add(&dir)?;
    println!("Linked {} to {} on {}.", dir.display(), show_remote(&config.remote), config.server);
    Ok(())
}

fn list_server_folders(path: &str, server: Option<&str>) -> Result<(), String> {
    let config = folders::sign_in_for(server)?;
    let api = api_for(&config, Some(config.token.clone()))?;
    let path = folders::clean_remote(path);
    let folder = api.list(&path).map_err(|err| format!("Could not list {}: {err}", show_remote(&path)))?;
    let linked: Vec<folders::Linked> = folders::linked()
        .into_iter()
        .filter(|linked| linked.config.server.trim_end_matches('/') == config.server.trim_end_matches('/') && linked.config.username == config.username)
        .collect();
    let inside = |outer: &str, inner: &str| outer.is_empty() || inner == outer || inner.starts_with(&format!("{outer}/"));
    println!("Folders in {} on {}:", if path.is_empty() { format!("{} (/)", folder.root_name) } else { format!("/{path}") }, config.server);
    let mut subfolders: Vec<_> = folder.entries.iter().filter(|entry| entry.dir).collect();
    subfolders.sort_by_key(|entry| entry.name.to_lowercase());
    if subfolders.is_empty() {
        println!("  (no folders)");
    }
    for entry in subfolders {
        let mark = if let Some(same) = linked.iter().find(|linked| linked.config.remote == entry.path) {
            format!("syncs with {}", same.dir.display())
        } else if let Some(outer) = linked.iter().find(|linked| inside(&linked.config.remote, &entry.path)) {
            format!("syncs with {} (as part of {})", outer.dir.display(), show_remote(&outer.config.remote))
        } else if linked.iter().any(|linked| inside(&entry.path, &linked.config.remote)) {
            "has folders inside that sync".to_string()
        } else {
            String::new()
        };
        if mark.is_empty() {
            println!("  /{}", entry.path);
        } else {
            println!("  /{}  — {mark}", entry.path);
        }
    }
    if folder.truncated {
        println!("  … and more (the server stopped listing at its limit)");
    }
    println!("Link one with `ownnas-sync add <local folder> --remote <server folder>`.");
    Ok(())
}

fn list() -> Result<(), String> {
    let all = folders::all();
    if all.is_empty() {
        println!("No folders linked yet. Run `ownnas-sync setup <folder> --server … --username …`.");
    }
    for (dir, config) in all {
        match config {
            Ok(config) => println!(
                "{}  ⇄  {} on {} as {}{}",
                dir.display(),
                show_remote(&config.remote),
                config.server,
                config.username,
                if config.token.is_empty() { " (signed out)" } else { "" }
            ),
            Err(_) => println!("{}  — its settings are missing (moved or deleted?); `ownnas-sync remove` it", dir.display()),
        }
    }
    Ok(())
}

fn remove(dir: &Path) -> Result<(), String> {
    let dir = existing_or_given(dir);
    let config = model::load_config(&dir).ok();
    let listed = folders::forget(&dir)?;
    if config.is_none() && !listed {
        return Err(format!("{} is not a linked folder: see `ownnas-sync list`.", dir.display()));
    }
    if let Some(config) = config {
        // Other folders signed in with the same token keep using it.
        let shared = folders::linked().iter().any(|other| other.config.token == config.token);
        if !shared && !config.token.is_empty() {
            if let Ok(api) = api_for(&config, Some(config.token.clone())) {
                let _ = api.logout();
            }
        }
        let meta = model::meta_dir(&dir);
        for name in ["config.json", "state.json"] {
            let _ = std::fs::remove_file(meta.join(name));
        }
    }
    println!("{} no longer syncs. Its files are still here and on the server.", dir.display());
    Ok(())
}

struct RunOptions {
    dry_run: bool,
    verbose: bool,
}

/// One pass over one folder.
fn sync_folder(dir: &Path, config: &Config, options: &RunOptions) -> Result<engine::Summary, String> {
    let api = api_for(config, Some(config.token.clone()))?;
    if config.token.is_empty() {
        return Err("signed out; run `ownnas-sync setup` again".to_string());
    }
    let syncer = engine::Syncer {
        root: dir,
        config,
        api: &api,
        host: engine::host_name(),
        dry_run: options.dry_run,
        verbose: options.verbose,
        notices: None,
    };
    local::prune_deleted(dir, 30);
    syncer.run()
}

fn run(dir: Option<&Path>, watch: bool, interval: u64, options: RunOptions) -> Result<(), String> {
    let only = match dir {
        Some(dir) => {
            let dir = dir.canonicalize().map_err(|err| format!("Could not open {}: {err}", dir.display()))?;
            model::load_config(&dir)?;
            // Folders set up before there was a list join it the first time they run.
            folders::add(&dir)?;
            Some(dir)
        }
        None => None,
    };
    loop {
        // Read again each time, so folders added or removed meanwhile are picked up.
        let targets: Vec<(PathBuf, Result<Config, String>)> = match &only {
            Some(dir) => vec![(dir.clone(), model::load_config(dir))],
            None => folders::all(),
        };
        if targets.is_empty() && !watch {
            return Err("No folders linked yet. Run `ownnas-sync setup <folder> --server … --username …`.".to_string());
        }
        let labelled = only.is_none();
        let mut failures = 0;
        for (dir, config) in &targets {
            let label = if labelled { format!("{}: ", dir.display()) } else { String::new() };
            let result = config.as_ref().map_err(String::clone).and_then(|config| sync_folder(dir, config, &options));
            match result {
                Ok(summary) if summary.changes() > 0 || summary.failed > 0 || !watch => println!("{label}{summary}"),
                Ok(_) => {}
                Err(err) => {
                    failures += 1;
                    eprintln!("{label}sync failed: {err}");
                }
            }
        }
        if !watch {
            return match failures {
                0 => Ok(()),
                _ if targets.len() == 1 => Err("sync failed".to_string()),
                n => Err(format!("{n} of {} folders failed to sync", targets.len())),
            };
        }
        std::thread::sleep(Duration::from_secs(interval.max(5)));
    }
}

fn logout(dir: &Path) -> Result<(), String> {
    let dir = existing_or_given(dir);
    let mut config = model::load_config(&dir)?;
    let api = api_for(&config, Some(config.token.clone()))?;
    if let Err(err) = api.logout() {
        eprintln!("Could not reach the server to sign out ({err}); forgetting the token locally.");
    }
    // The server no longer takes the token, so neither can the folders that share it.
    for mut other in folders::linked() {
        if other.dir != dir && !config.token.is_empty() && other.config.token == config.token {
            other.config.token.clear();
            model::save_config(&other.dir, &other.config)?;
            println!("Also signed out {}.", other.dir.display());
        }
    }
    config.token.clear();
    model::save_config(&dir, &config)?;
    println!("Signed out. Run `ownnas-sync setup` to link this folder again.");
    Ok(())
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Setup { dir, server, username, password, remote, ca_cert } => {
            setup(&dir, server, username, password, remote, ca_cert)
        }
        Command::Add { dir, remote, server, create } => add(&dir, &remote, server.as_deref(), create),
        Command::Folders { path, server } => list_server_folders(&path, server.as_deref()),
        Command::List => list(),
        Command::Remove { dir } => remove(&dir),
        Command::Run { dir, watch, interval, dry_run, verbose } => {
            run(dir.as_deref(), watch, interval, RunOptions { dry_run, verbose })
        }
        Command::Logout { dir } => logout(&dir),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::FAILURE
        }
    }
}
