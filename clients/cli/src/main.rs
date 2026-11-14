//! `ownnascli`: copies files to and from an OwnNAS server from the command line.
mod delete;
mod photos;
mod transfer;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::AtomicU64;

use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};

use ownnas_core::api::{Api, ApiError};
use ownnas_core::browse::normalize_address;
use ownnas_core::local;
use ownnas_core::model::{Node, Stamp};
use ownnas_core::trust::{server_fingerprint, Trust};

use delete::{After, Tally};
use transfer::{join, plan, Listing, Options, Plan, Report, Way};

pub(crate) const REMOTE_PREFIX: &str = "ownnas:";

#[derive(Parser)]
#[command(name = "ownnascli", version, about = "Fast file transfer with an OwnNAS server", after_help = "Paths on the server start with ownnas: (ownnas:Photos). ownnas: alone is the top of the library.")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Sign in to a server. The password is asked for, or read from OWNNAS_PASSWORD.
    Login {
        /// Server address, for example https://nas.example:8787
        server: String,
        #[arg(long)]
        username: Option<String>,
        /// PEM certificate of a private CA to trust (the ca.crt from `ownnas tls generate`).
        #[arg(long)]
        ca_cert: Option<PathBuf>,
        /// A server certificate fingerprint to trust (as `ownnas serve` prints it).
        #[arg(long)]
        fingerprint: Option<String>,
    },
    /// Sign out on the server and forget the sign-in here.
    Logout,
    /// Who is signed in, and to which server.
    Status,
    /// List a folder on the server.
    Ls {
        #[arg(default_value = "ownnas:")]
        path: String,
    },
    /// Make a folder on the server, with any folders above it.
    Mkdir { path: String },
    /// Copy a file, or what is inside a folder, to or from the server. Skips files with the same
    /// size and modification time, and replaces those that differ. Removes nothing on the
    /// server. When uploading, --delete-after-upload deletes the files here that the server has.
    Cp {
        source: String,
        destination: String,
        #[command(flatten)]
        flags: Flags,
        #[command(flatten)]
        delete: delete::Flags,
    },
    /// Make a destination folder match a source folder, removing what the source does not have.
    /// Removed files go to the Trash on the server, or to .ownnas-sync/deleted on this computer.
    Sync {
        source: String,
        destination: String,
        #[command(flatten)]
        flags: Flags,
    },
    /// Upload photos and videos to the Gallery, telling and skipping those already in the
    /// library, whatever they are called and wherever they are. Everything goes straight into the
    /// destination folder, and a name already taken gets a number. --delete-after-upload deletes
    /// the files here that the server has.
    Photos {
        /// Files, or folders to look through.
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// The Gallery workspace to upload to.
        #[arg(short, long)]
        workspace: Option<String>,
        /// The folder on the server: needed without --workspace, or for a workspace of several.
        #[arg(long)]
        to: Option<String>,
        /// Upload those already there as well.
        #[arg(long)]
        force: bool,
        /// How many files to handle at once.
        #[arg(short, long, default_value_t = 4)]
        parallel: usize,
        /// Say what would happen, without uploading or deleting anything.
        #[arg(short = 'n', long)]
        dry_run: bool,
        #[command(flatten)]
        delete: delete::Flags,
    },
    /// Move or rename a file or folder on the server.
    Mv { source: String, destination: String },
    /// Move a file or folder on the server to its Trash.
    Rm {
        path: String,
        /// Needed to remove a folder.
        #[arg(short, long)]
        recursive: bool,
    },
}

#[derive(clap::Args)]
struct Flags {
    /// How many files to transfer at once.
    #[arg(short, long, default_value_t = 8)]
    parallel: usize,
    /// Show what would change without changing anything.
    #[arg(short = 'n', long)]
    dry_run: bool,
    /// Print each file.
    #[arg(short, long)]
    verbose: bool,
    /// Also compare the contents of files with the same size and date, by their checksums.
    /// Slower. For after a copy was cut off by a crash or a power cut.
    #[arg(long)]
    checksum: bool,
}

#[derive(Serialize, Deserialize, Default)]
struct Profile {
    server: String,
    username: String,
    token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ca_cert: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pinned: Vec<String>,
}

fn profile_path() -> Result<PathBuf, String> {
    let var = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty()).map(PathBuf::from);
    let base = if cfg!(windows) {
        var("APPDATA")
    } else {
        var("XDG_CONFIG_HOME").or_else(|| var("HOME").map(|home| home.join(".config")))
    };
    Ok(base.ok_or("Could not find a folder for settings")?.join("ownnascli").join("profile.json"))
}

fn load_profile() -> Result<Profile, String> {
    let path = profile_path()?;
    let text = std::fs::read(&path).map_err(|_| "Not signed in. Run `ownnascli login <server>` first.".to_string())?;
    let profile: Profile = serde_json::from_slice(&text).map_err(|err| format!("Could not read {}: {err}", path.display()))?;
    if profile.token.is_empty() {
        return Err("Not signed in. Run `ownnascli login <server>` first.".into());
    }
    Ok(profile)
}

/// Saved readable only by this user, since it holds the sign-in token.
fn save_profile(profile: &Profile) -> Result<(), String> {
    let path = profile_path()?;
    let fail = |err: std::io::Error| format!("Could not save {}: {err}", path.display());
    let folder = path.parent().ok_or("Could not find a folder for settings")?;
    std::fs::create_dir_all(folder).map_err(fail)?;
    let temp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&temp);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(&temp).map_err(fail)?;
    let text = serde_json::to_vec_pretty(profile).map_err(|err| err.to_string())?;
    std::io::Write::write_all(&mut file, &text).map_err(fail)?;
    drop(file);
    std::fs::rename(&temp, &path).map_err(fail)
}

fn api_for(server: &str, token: Option<String>, ca_cert: Option<&Path>, pinned: &[String]) -> Result<Api, String> {
    let ca_pem = match ca_cert {
        Some(path) => Some(std::fs::read(path).map_err(|err| format!("Could not read {}: {err}", path.display()))?),
        None => None,
    };
    Api::with_trust(server, token, &Trust { ca_pem, pinned: pinned.to_vec() })
}

fn signed_in() -> Result<Api, String> {
    let profile = load_profile()?;
    api_for(&profile.server, Some(profile.token), profile.ca_cert.as_deref(), &profile.pinned)
}

/// An error in this tool's words.
pub(crate) fn say(err: &ApiError) -> String {
    match err {
        ApiError::Unauthorized => "The server rejected the sign-in. Run `ownnascli login` again.".into(),
        other => other.to_string(),
    }
}

pub(crate) fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// The path on the server, if this is one.
fn remote_of(path: &str) -> Option<String> {
    path.strip_prefix(REMOTE_PREFIX).map(|rest| rest.trim_matches('/').to_string())
}

/// For commands that only work on the server, where the prefix is optional.
fn on_server(path: &str) -> String {
    remote_of(path).unwrap_or_else(|| path.trim_matches('/').to_string())
}

fn parent_and_name(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

/// A remote listing, or an empty one for a folder that does not exist yet.
fn remote_listing(api: &Api, folder: &str) -> Result<(bool, Listing), String> {
    match api.tree(folder) {
        Ok(tree) => Ok((tree.readonly, tree.nodes.into_iter().filter(|(path, _)| !local::ignored_path(path)).collect())),
        Err(ApiError::NotFound) => Ok((false, BTreeMap::new())),
        Err(err) => Err(format!("Could not list {REMOTE_PREFIX}{folder}: {}", say(&err))),
    }
}

fn is_remote_folder(api: &Api, path: &str) -> Result<Option<bool>, String> {
    if path.is_empty() {
        return Ok(Some(true));
    }
    match api.list(path) {
        Ok(_) => Ok(Some(true)),
        Err(ApiError::NotFound) => Ok(None),
        Err(ApiError::Unauthorized) => Err(say(&ApiError::Unauthorized)),
        Err(_) => Ok(Some(false)),
    }
}

/// A file's version on the server, found through its folder's listing.
fn remote_stamp(api: &Api, path: &str) -> Result<Option<Stamp>, String> {
    let (parent, name) = parent_and_name(path);
    match api.list(parent) {
        Ok(folder) => Ok(folder.entries.into_iter().find(|entry| entry.name == name && !entry.dir).map(|entry| Stamp { size: entry.size, mtime: entry.mtime_ms })),
        Err(ApiError::NotFound) => Ok(None),
        Err(err) => Err(say(&err)),
    }
}

fn summarize(report: &Report, unchanged: u64) -> Result<(), String> {
    let rate = human_size((report.bytes as f64 / report.took.as_secs_f64().max(0.001)) as u64);
    let mut line = format!("{} copied ({}, {rate}/s), {unchanged} already there", report.copied, human_size(report.bytes));
    if report.removed > 0 {
        line.push_str(&format!(", {} removed", report.removed));
    }
    println!("{line}, in {:.1} s", report.took.as_secs_f64());
    for (path, why) in &report.failed {
        eprintln!("failed: {path}: {why}");
    }
    if report.failed.is_empty() {
        Ok(())
    } else {
        Err(format!("{} failed.", report.failed.len()))
    }
}

fn show_plan(plan: &Plan, source: &Listing) {
    for path in &plan.mkdir {
        println!("would make {path}/");
    }
    for path in &plan.copy {
        println!("would copy {path} ({})", human_size(source[path].stamp.size));
    }
    for path in &plan.remove {
        println!("would remove {path}");
    }
    println!("{} to copy, {} to remove, {} already there", plan.copy.len(), plan.remove.len(), plan.unchanged);
}

fn folder_transfer(api: &Api, way: Way, source: &Listing, dest: &Listing, delete: bool, after: After, flags: &Flags) -> Result<(), String> {
    if delete && !dest.is_empty() && !source.values().any(|node| !node.dir) {
        return Err("The source has no files, so this would remove everything in the destination. Nothing was changed.".into());
    }
    let mut work = plan(source, dest, delete);
    // Those with the same size and date on the server, which are not sent.
    let mut same: Vec<String> = Vec::new();
    if flags.checksum || after.deletes_there() {
        same = source.iter().filter(|(path, node)| !node.dir && dest.get(*path) == Some(node)).map(|(path, _)| path.clone()).collect();
    }
    if flags.checksum {
        let changed = transfer::differing(api, &way, same.clone(), flags.parallel);
        same.retain(|path| !changed.contains(path));
        work.unchanged -= changed.len() as u64;
        work.copy.extend(changed);
        work.copy.sort();
    }
    let (Way::Up { local: root, remote } | Way::Down { remote, local: root }) = &way;
    let item = |path: &String| (local::full_path(root, path), Some(source[path].stamp), join(remote, path));
    let there: Vec<_> = if after.deletes_there() { same.iter().map(item).collect() } else { Vec::new() };
    if flags.dry_run {
        show_plan(&work, source);
        if after.deletes() {
            let tally = Tally::new(true);
            for path in &work.copy {
                tally.planned(&local::full_path(root, path));
            }
            tally.settle_all(api, there, flags.parallel);
            println!("{}", tally.summary().trim_start_matches(", "));
            return tally.result();
        }
        return Ok(());
    }
    let report = transfer::run(api, &way, &work, source, dest, &Options { parallel: flags.parallel, verbose: flags.verbose });
    let copied = summarize(&report, work.unchanged);
    if !after.deletes() {
        return copied;
    }
    let tally = Tally::new(false);
    if report.copied > 0 && report.done.is_empty() {
        eprintln!("Deleted nothing here, because the server could not make sure the files are on its disk.");
    } else {
        let mut items: Vec<_> = report.done.iter().map(item).collect();
        items.extend(there);
        tally.settle_all(api, items, flags.parallel);
        println!("{}", tally.summary().trim_start_matches(", "));
    }
    copied.and(tally.result())
}

fn local_listing(folder: &Path) -> Result<Listing, String> {
    if folder.exists() {
        local::scan(folder)
    } else {
        Ok(BTreeMap::new())
    }
}

fn single_file_note(flags: &Flags, what: &str) -> bool {
    if flags.dry_run {
        println!("would copy {what}");
    }
    flags.dry_run
}

fn upload(api: &Api, source: &Path, remote: String, delete: bool, after: After, flags: &Flags) -> Result<(), String> {
    let meta = std::fs::metadata(source).map_err(|err| format!("Could not open {}: {err}", source.display()))?;
    if meta.is_dir() {
        let (readonly, dest) = remote_listing(api, &remote)?;
        if readonly {
            return Err("This server is read-only.".into());
        }
        let listing = local::scan(source)?;
        return folder_transfer(api, Way::Up { local: source.to_path_buf(), remote }, &listing, &dest, delete, after, flags);
    }
    if delete {
        return Err("sync works on folders. Use cp for one file.".into());
    }
    let name = source.file_name().and_then(|name| name.to_str()).ok_or("The file's name is not valid Unicode")?;
    let target = if is_remote_folder(api, &remote)? == Some(true) { join(&remote, name) } else { remote };
    if single_file_note(flags, &format!("{} to {REMOTE_PREFIX}{target}", source.display())) && !after.deletes() {
        return Ok(());
    }
    let Node { stamp, .. } = local::node_of(&meta);
    let base = remote_stamp(api, &target)?;
    let tally = Tally::new(flags.dry_run);
    if flags.dry_run {
        match (base == Some(stamp), after.deletes_there()) {
            (false, _) => tally.planned(source),
            (true, false) => println!("would keep {} here: it is already there, so it is not uploaded", source.display()),
            (true, true) => tally.settle(source, Some(stamp), transfer::with_retries(|| api.sha256(&target)).map_err(|err| say(&err))),
        }
        return tally.result();
    }
    let settle = || tally.settle(source, Some(stamp), transfer::with_retries(|| api.sha256(&target)).map_err(|err| say(&err)));
    if base == Some(stamp) {
        println!("Already there.");
        if after.deletes_there() {
            settle();
        }
        return tally.result();
    }
    transfer::upload_one(api, source, &target, stamp, base, false, &AtomicU64::new(0)).map_err(|err| say(&err))?;
    println!("Copied to {REMOTE_PREFIX}{target} ({})", human_size(stamp.size));
    if after.deletes() {
        settle();
    }
    tally.result()
}

fn download(api: &Api, remote: String, dest: &Path, delete: bool, flags: &Flags) -> Result<(), String> {
    match is_remote_folder(api, &remote)? {
        None => return Err(format!("{REMOTE_PREFIX}{remote} is not on the server.")),
        Some(true) => {
            let (_, source) = remote_listing(api, &remote)?;
            let existing = local_listing(dest)?;
            return folder_transfer(api, Way::Down { remote, local: dest.to_path_buf() }, &source, &existing, delete, After::Keep, flags);
        }
        Some(false) => {}
    }
    if delete {
        return Err("sync works on folders. Use cp for one file.".into());
    }
    let (_, name) = parent_and_name(&remote);
    let target = if dest.is_dir() { dest.join(name) } else { dest.to_path_buf() };
    if single_file_note(flags, &format!("{REMOTE_PREFIX}{remote} to {}", target.display())) {
        return Ok(());
    }
    let stamp = remote_stamp(api, &remote)?.ok_or_else(|| format!("{REMOTE_PREFIX}{remote} is not on the server."))?;
    if std::fs::metadata(&target).is_ok_and(|meta| local::node_of(&meta).stamp == stamp) {
        println!("Already there.");
        return Ok(());
    }
    let resumed = transfer::download_one(api, &remote, &target, stamp, false, &AtomicU64::new(0)).map_err(|err| say(&err))?;
    let from = resumed.map_or(String::new(), |from| format!(", went on from {}", human_size(from)));
    println!("Copied to {} ({}{from})", target.display(), human_size(stamp.size));
    Ok(())
}

fn copy(source: &str, destination: &str, delete: bool, after: After, flags: &Flags) -> Result<(), String> {
    match (remote_of(source), remote_of(destination)) {
        (Some(_), Some(_)) => Err("Both paths are on the server. Use mv, or copy through this computer.".into()),
        (None, None) => Err(format!("One of the paths has to be on the server, starting with {REMOTE_PREFIX}")),
        (None, Some(remote)) => upload(&signed_in()?, Path::new(source), remote, delete, after, flags),
        (Some(_), None) if after.deletes() => Err("--delete-after-upload deletes what was uploaded, so it is only for copying to the server.".into()),
        (Some(remote), None) => download(&signed_in()?, remote, Path::new(destination), delete, flags),
    }
}

fn login(server: String, username: Option<String>, ca_cert: Option<PathBuf>, fingerprint: Option<String>) -> Result<(), String> {
    let server = normalize_address(&server)?;
    let username = match username {
        Some(name) => name,
        None => {
            eprint!("Username: ");
            let mut line = String::new();
            std::io::stdin().read_line(&mut line).map_err(|err| err.to_string())?;
            line.trim().to_string()
        }
    };
    let password = match std::env::var("OWNNAS_PASSWORD") {
        Ok(password) if !password.is_empty() => password,
        _ => rpassword::prompt_password(format!("Password for {username}: ")).map_err(|err| err.to_string())?,
    };
    let ca_cert = match ca_cert {
        Some(path) => Some(path.canonicalize().map_err(|err| format!("Could not find {}: {err}", path.display()))?),
        None => None,
    };
    let pinned: Vec<String> = fingerprint.into_iter().collect();
    let api = api_for(&server, None, ca_cert.as_deref(), &pinned)?;
    let token = match api.login(&username, &password) {
        Ok(token) => token,
        Err(ApiError::Untrusted) => {
            let seen = server_fingerprint(&server).unwrap_or_else(|_| "unknown".into());
            return Err(format!(
                "The server's certificate is not trusted. Its fingerprint is:\n  {seen}\nIf that is your server's, run this again with --fingerprint {seen}"
            ));
        }
        Err(err) => return Err(format!("Sign-in failed: {err}")),
    };
    save_profile(&Profile { server: server.clone(), username: username.clone(), token, ca_cert, pinned })?;
    println!("Signed in to {server} as {username}.");
    Ok(())
}

fn logout() -> Result<(), String> {
    let mut profile = load_profile()?;
    let api = api_for(&profile.server, Some(profile.token.clone()), profile.ca_cert.as_deref(), &profile.pinned)?;
    if let Err(err) = api.logout() {
        eprintln!("Could not reach the server to sign out ({err}); forgetting the sign-in here.");
    }
    profile.token.clear();
    save_profile(&profile)?;
    println!("Signed out.");
    Ok(())
}

fn status() -> Result<(), String> {
    let profile = load_profile()?;
    let api = api_for(&profile.server, Some(profile.token.clone()), profile.ca_cert.as_deref(), &profile.pinned)?;
    let account = api.account().map_err(|err| say(&err))?;
    println!("{} as {} (server version {}{})", profile.server, account.username, account.version, if account.readonly { ", read-only" } else { "" });
    Ok(())
}

fn list(path: &str) -> Result<(), String> {
    let path = on_server(path);
    let folder = signed_in()?.list(&path).map_err(|err| format!("Could not list {REMOTE_PREFIX}{path}: {}", say(&err)))?;
    for entry in &folder.entries {
        let size = if entry.dir { "-".to_string() } else { human_size(entry.size) };
        println!("{size:>10}  {}{}", entry.name, if entry.dir { "/" } else { "" });
    }
    if folder.truncated {
        eprintln!("The server stopped listing at its limit, so this is not everything.");
    }
    Ok(())
}

/// A move into an existing folder, or to a new name, or both.
fn move_item(source: &str, destination: &str) -> Result<(), String> {
    let wants_folder = destination.ends_with('/');
    let (source, destination) = (on_server(source), on_server(destination));
    if source.is_empty() {
        return Err("The top of the library cannot be moved.".into());
    }
    let api = signed_in()?;
    let into_folder = wants_folder || is_remote_folder(&api, &destination)? == Some(true);
    let moved = if into_folder {
        api.move_to(&source, &destination)
    } else {
        let (from_parent, _) = parent_and_name(&source);
        let (to_parent, new_name) = parent_and_name(&destination);
        if from_parent == to_parent {
            api.rename(&source, new_name)
        } else {
            api.move_to(&source, to_parent).and_then(|at| api.rename(&at, new_name))
        }
    };
    let at = moved.map_err(|err| format!("Could not move {REMOTE_PREFIX}{source}: {}", say(&err)))?;
    println!("Moved to {REMOTE_PREFIX}{at}");
    Ok(())
}

fn remove(path: &str, recursive: bool) -> Result<(), String> {
    let path = on_server(path);
    if path.is_empty() {
        return Err("The top of the library cannot be removed.".into());
    }
    // On something already in the Trash, the server's call deletes it for good.
    let first = path.split('/').next().unwrap_or_default();
    if first == ".ownnas-trash" || first == "Trash" {
        return Err("That is in the Trash. Empty it from the web app.".into());
    }
    let api = signed_in()?;
    match is_remote_folder(&api, &path)? {
        None => return Err(format!("{REMOTE_PREFIX}{path} is not on the server.")),
        Some(true) if !recursive => return Err(format!("{REMOTE_PREFIX}{path} is a folder. Add -r to remove it.")),
        _ => {}
    }
    api.trash(&path).map_err(|err| format!("Could not remove {REMOTE_PREFIX}{path}: {}", say(&err)))?;
    println!("Moved {REMOTE_PREFIX}{path} to the Trash.");
    Ok(())
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Login { server, username, ca_cert, fingerprint } => login(server, username, ca_cert, fingerprint),
        Command::Logout => logout(),
        Command::Status => status(),
        Command::Ls { path } => list(&path),
        Command::Mkdir { path } => signed_in().and_then(|api| api.mkdir(&on_server(&path)).map_err(|err| say(&err))),
        Command::Cp { source, destination, flags, delete } => copy(&source, &destination, false, delete.after(), &flags),
        Command::Sync { source, destination, flags } => copy(&source, &destination, true, After::Keep, &flags),
        Command::Photos { paths, workspace, to, force, parallel, dry_run, delete } => signed_in().and_then(|api| {
            let options = photos::Options { force, parallel, dry_run, after: delete.after() };
            photos::run(&api, &paths, workspace.as_deref(), to.as_deref().map(on_server).as_deref(), &options)
        }),
        Command::Mv { source, destination } => move_item(&source, &destination),
        Command::Rm { path, recursive } => remove(&path, recursive),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::FAILURE
        }
    }
}
