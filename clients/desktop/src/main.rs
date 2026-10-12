//! `ownnas-sync`: keeps a local folder and a folder on an OwnNAS server in sync.
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};

use ownnas_core::api::Api;
use ownnas_core::model::{self, Config};
use ownnas_core::{engine, local};

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
    /// Sync once, or keep syncing with --watch.
    Run {
        dir: PathBuf,
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
    /// Sign this folder's token out on the server and forget it locally.
    Logout { dir: PathBuf },
}

fn api_for(config: &Config, token: Option<String>) -> Result<Api, String> {
    let ca = match &config.ca_cert {
        Some(path) => Some(std::fs::read(path).map_err(|err| format!("Could not read {}: {err}", path.display()))?),
        None => None,
    };
    Api::new(&config.server, token, ca.as_deref())
}

fn setup(
    dir: &Path,
    server: String,
    username: String,
    password: Option<String>,
    remote: String,
    ca_cert: Option<PathBuf>,
) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|err| format!("Could not create {}: {err}", dir.display()))?;
    let dir = dir.canonicalize().map_err(|err| err.to_string())?;
    let ca_cert = match ca_cert {
        Some(path) => Some(path.canonicalize().map_err(|err| format!("Could not find {}: {err}", path.display()))?),
        None => None,
    };
    let remote = remote.trim_matches('/').to_string();
    let mut config = Config { server, remote, username, token: String::new(), ca_cert };
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
    println!(
        "Linked {} to {}/{}. Run `ownnas-sync run {} --watch` to start syncing.",
        dir.display(),
        config.server,
        config.remote,
        dir.display()
    );
    Ok(())
}

fn run(dir: &Path, watch: bool, interval: u64, dry_run: bool, verbose: bool) -> Result<(), String> {
    let dir = dir.canonicalize().map_err(|err| format!("Could not open {}: {err}", dir.display()))?;
    let config = model::load_config(&dir)?;
    let api = api_for(&config, Some(config.token.clone()))?;
    let syncer = engine::Syncer { root: &dir, config: &config, api: &api, host: engine::host_name(), dry_run, verbose, notices: None };
    loop {
        local::prune_deleted(&dir, 30);
        match syncer.run() {
            Ok(summary) if summary.changes() > 0 || summary.failed > 0 || !watch => println!("{summary}"),
            Ok(_) => {}
            Err(err) if watch => eprintln!("sync failed: {err}"),
            Err(err) => return Err(err),
        }
        if !watch {
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(interval.max(5)));
    }
}

fn logout(dir: &Path) -> Result<(), String> {
    let mut config = model::load_config(dir)?;
    let api = api_for(&config, Some(config.token.clone()))?;
    if let Err(err) = api.logout() {
        eprintln!("Could not reach the server to sign out ({err}); forgetting the token locally.");
    }
    config.token.clear();
    model::save_config(dir, &config)?;
    println!("Signed out. Run `ownnas-sync setup` to link this folder again.");
    Ok(())
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Setup { dir, server, username, password, remote, ca_cert } => {
            setup(&dir, server, username, password, remote, ca_cert)
        }
        Command::Run { dir, watch, interval, dry_run, verbose } => run(&dir, watch, interval, dry_run, verbose),
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
