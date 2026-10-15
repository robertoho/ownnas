//! Browser terminals: shells running in a pseudo-terminal on the server, which keep running
//! (with a scrollback buffer) while no browser is attached, so long agent sessions survive a
//! closed tab or a dropped connection.
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::sync::broadcast;

use crate::agents::{self, Agent};

/// Output kept for replay when a browser (re)attaches.
const SCROLLBACK_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_SESSIONS: usize = 24;
/// How long a correct terminal password unlocks terminals for one sign-in session.
pub const UNLOCK_FOR: Duration = Duration::from_secs(12 * 60 * 60);

pub const SETTING_ENABLED: &str = "terminal_enabled";
pub const SETTING_PASSWORD: &str = "terminal_password_hash";
pub const SETTING_SHELL: &str = "terminal_shell";

#[derive(Clone, Debug)]
pub enum Event {
    Output(Arc<Vec<u8>>),
    Exit(Option<u32>),
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    pub id: String,
    pub path: String,
    pub shell: String,
    pub owner: String,
    pub created: i64,
    pub running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<u32>,
    /// Window title set by the program in the terminal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Name the user gave the terminal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub agent: Agent,
}

struct Screen {
    scrollback: VecDeque<u8>,
    exit: Option<Option<u32>>,
    escapes: Escapes,
    title: Option<String>,
    last_output: Option<Instant>,
    /// When the program drew on its own (not just echoing typing or redrawing after a resize),
    /// for telling a busy agent from one that only redraws now and then.
    drawn: VecDeque<Instant>,
    /// Typing or resizing, after which the program's output is only an answer to that.
    last_poke: Option<Instant>,
}

/// Something worth surfacing that a program wrote to the terminal.
#[derive(Debug, PartialEq)]
enum Signal {
    Bell,
    Notify(String),
    Title(String),
}

/// Finds BEL and OSC sequences (titles; OSC 9, 99, and 777 desktop notifications) in output
/// that may be split across reads.
#[derive(Default)]
struct Escapes {
    state: u8,
    osc: Vec<u8>,
}

impl Escapes {
    const TEXT: u8 = 0;
    const ESC: u8 = 1;
    const OSC: u8 = 2;
    const OSC_ESC: u8 = 3;

    fn feed(&mut self, bytes: &[u8], out: &mut Vec<Signal>) {
        for &byte in bytes {
            self.state = match (self.state, byte) {
                (Self::TEXT, 0x07) => {
                    out.push(Signal::Bell);
                    Self::TEXT
                }
                (Self::TEXT, 0x1b) | (Self::ESC, 0x1b) => Self::ESC,
                (Self::TEXT, _) => Self::TEXT,
                (Self::ESC, b']') => {
                    self.osc.clear();
                    Self::OSC
                }
                (Self::ESC, _) => Self::TEXT,
                (Self::OSC, 0x07) | (Self::OSC_ESC, b'\\') => {
                    if let Some(signal) = Self::parse(&self.osc) {
                        out.push(signal);
                    }
                    Self::TEXT
                }
                (Self::OSC, 0x1b) => Self::OSC_ESC,
                (Self::OSC, _) => {
                    if self.osc.len() < 4096 {
                        self.osc.push(byte);
                    }
                    Self::OSC
                }
                (Self::OSC_ESC, _) => Self::TEXT,
                _ => Self::TEXT,
            };
        }
    }

    fn parse(osc: &[u8]) -> Option<Signal> {
        let text = String::from_utf8_lossy(osc);
        let (code, rest) = text.split_once(';')?;
        let rest = rest.trim();
        match code {
            "0" | "2" => Some(Signal::Title(rest.to_string())),
            // OSC 9;4 is a progress report (ConEmu, Windows Terminal), not a notification.
            "9" if !rest.starts_with("4;") && !rest.is_empty() => Some(Signal::Notify(rest.to_string())),
            "777" => {
                let mut parts = rest.splitn(3, ';');
                (parts.next() == Some("notify")).then(|| {
                    let title = parts.next().unwrap_or("").trim();
                    let body = parts.next().unwrap_or("").trim();
                    Signal::Notify(match (title.is_empty(), body.is_empty()) {
                        (false, false) => format!("{title}: {body}"),
                        (true, _) => body.to_string(),
                        (_, true) => title.to_string(),
                    })
                })
            }
            "99" => rest.split_once(';').map(|(_, body)| body.trim()).filter(|body| !body.is_empty()).map(|body| Signal::Notify(body.to_string())),
            _ => None,
        }
    }
}

pub struct Session {
    info: Info,
    /// Secret that hooks inside this terminal use to report agent events.
    hook_token: String,
    cwd: PathBuf,
    shell_pid: Option<u32>,
    name: Mutex<Option<String>>,
    /// Folder holding files pasted into this terminal; deleted with the terminal.
    /// Files pasted into this terminal, removed with it.
    uploads: Mutex<Vec<PathBuf>>,
    agent: Mutex<Agent>,
    changes: broadcast::Sender<()>,
    screen: Mutex<Screen>,
    events: broadcast::Sender<Event>,
    input: std::sync::mpsc::Sender<Vec<u8>>,
    #[cfg(not(target_os = "android"))]
    master: Mutex<Box<dyn portable_pty::MasterPty + Send>>,
    #[cfg(not(target_os = "android"))]
    killer: Mutex<Box<dyn portable_pty::ChildKiller + Send + Sync>>,
}

impl Session {
    pub fn info(&self) -> Info {
        let mut info = self.info.clone();
        let screen = self.screen.lock().unwrap_or_else(|err| err.into_inner());
        if let Some(code) = screen.exit {
            info.running = false;
            info.exit_code = code;
        }
        info.title = screen.title.clone();
        drop(screen);
        info.name = self.name.lock().unwrap_or_else(|err| err.into_inner()).clone();
        info.agent = self.agent.lock().unwrap_or_else(|err| err.into_inner()).clone();
        info
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// Remembers the folder of files pasted into this terminal, for cleanup.
    pub fn track_upload(&self, file: &Path) {
        self.uploads.lock().unwrap_or_else(|err| err.into_inner()).push(file.to_path_buf());
    }

    /// Deletes the files this terminal pasted, and their folders only once nothing else is in them.
    fn remove_uploads(&self) {
        let files = std::mem::take(&mut *self.uploads.lock().unwrap_or_else(|err| err.into_inner()));
        let mut folders: Vec<PathBuf> = Vec::new();
        for file in files {
            let _ = std::fs::remove_file(&file);
            if let Some(parent) = file.parent() {
                if !folders.iter().any(|folder| folder == parent) {
                    folders.push(parent.to_path_buf());
                }
            }
        }
        for folder in folders {
            // The `.gitignore` that pasting added goes once it is all that is left.
            let ignore = folder.join(".gitignore");
            let only_ignore = std::fs::read_dir(&folder)
                .map(|entries| entries.flatten().all(|entry| entry.file_name() == ".gitignore"))
                .unwrap_or(false);
            if only_ignore && std::fs::read(&ignore).is_ok_and(|bytes| bytes == PASTED_GITIGNORE.as_bytes()) {
                let _ = std::fs::remove_file(&ignore);
            }
            // Fails, and so keeps the folder, while anyone else's files are in it.
            let _ = std::fs::remove_dir(folder);
        }
    }

    /// The folder the terminal's foreground program (or its shell) is in now.
    pub fn current_dir(&self) -> PathBuf {
        #[cfg(all(unix, not(target_os = "android")))]
        {
            let foreground = self
                .master
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .process_group_leader()
                .and_then(|pid| u32::try_from(pid).ok());
            for pid in foreground.into_iter().chain(self.shell_pid) {
                if let Some(dir) = agents::process_dir(pid) {
                    return dir;
                }
            }
        }
        self.cwd.clone()
    }

    /// Names the terminal; an empty name restores the default label.
    pub fn rename(&self, name: &str) {
        let name: String = name.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(60).collect();
        *self.name.lock().unwrap_or_else(|err| err.into_inner()) = (!name.is_empty()).then_some(name);
        self.notify();
    }

    fn notify(&self) {
        let _ = self.changes.send(());
    }

    /// Changes the agent state and tells listeners if anything changed.
    pub fn update_agent(&self, change: impl FnOnce(&mut Agent)) {
        let changed = {
            let mut agent = self.agent.lock().unwrap_or_else(|err| err.into_inner());
            let before = agent.clone();
            change(&mut agent);
            *agent != before
        };
        if changed {
            self.notify();
        }
    }

    /// The user looked at this terminal.
    pub fn seen(&self) {
        self.update_agent(|agent| agent.attention = None);
    }

    /// Returns the scrollback, whether the shell already exited, and a receiver for everything
    /// after it. Taken under one lock so no output is lost or repeated.
    pub fn attach(&self) -> (Vec<u8>, Option<Option<u32>>, broadcast::Receiver<Event>) {
        let screen = self.screen.lock().unwrap_or_else(|err| err.into_inner());
        let receiver = self.events.subscribe();
        (screen.scrollback.iter().copied().collect(), screen.exit, receiver)
    }

    pub fn write(&self, bytes: Vec<u8>) {
        self.poked();
        let _ = self.input.send(bytes);
        // Typing in a terminal means the user has seen whatever it wanted.
        if self.agent.lock().unwrap_or_else(|err| err.into_inner()).attention.is_some() {
            self.seen();
        }
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        self.poked();
        #[cfg(not(target_os = "android"))]
        {
            let size = pty_size(cols, rows);
            let master = self.master.lock().unwrap_or_else(|err| err.into_inner());
            let _ = master.resize(size);
        }
        #[cfg(target_os = "android")]
        let _ = (cols, rows);
    }

    pub fn kill(&self) {
        #[cfg(not(target_os = "android"))]
        {
            let mut killer = self.killer.lock().unwrap_or_else(|err| err.into_inner());
            let _ = killer.kill();
        }
    }

    fn record(&self, event: Event) {
        let mut screen = self.screen.lock().unwrap_or_else(|err| err.into_inner());
        let mut signals = Vec::new();
        match &event {
            Event::Output(bytes) => {
                let now = Instant::now();
                screen.last_output = Some(now);
                if !screen.last_poke.is_some_and(|at| now.duration_since(at) < Duration::from_millis(300)) {
                    if screen.drawn.len() >= 64 {
                        screen.drawn.pop_front();
                    }
                    screen.drawn.push_back(now);
                }
                screen.escapes.feed(bytes, &mut signals);
                screen.scrollback.extend(bytes.iter());
                if screen.scrollback.len() > SCROLLBACK_BYTES {
                    // Trim to a line start so replay does not begin mid escape sequence.
                    let excess = screen.scrollback.len() - SCROLLBACK_BYTES;
                    let cut = screen
                        .scrollback
                        .iter()
                        .skip(excess)
                        .position(|byte| *byte == b'\n')
                        .map(|offset| excess + offset + 1)
                        .unwrap_or(excess);
                    screen.scrollback.drain(..cut);
                }
            }
            Event::Exit(code) => screen.exit = Some(*code),
        }
        let mut title_changed = false;
        for signal in &signals {
            if let Signal::Title(title) = signal {
                title_changed |= screen.title.as_deref() != Some(title.as_str());
                screen.title = Some(title.clone());
            }
        }
        let exited = matches!(event, Event::Exit(_));
        let _ = self.events.send(event);
        drop(screen);
        for signal in signals {
            match signal {
                Signal::Bell => self.update_agent(|agent| agent.notified(None)),
                Signal::Notify(message) => self.update_agent(|agent| agent.notified(Some(message))),
                Signal::Title(_) => {}
            }
        }
        if title_changed || exited {
            self.notify();
        }
    }

    /// Whether the program keeps drawing on its own, as an agent does while it works (its
    /// spinner moves several times a second). One redraw now and then is not enough.
    fn busy(&self) -> bool {
        let screen = self.screen.lock().unwrap_or_else(|err| err.into_inner());
        let mut slots: Vec<u128> = screen
            .drawn
            .iter()
            .filter(|at| at.elapsed() < Duration::from_millis(1500))
            .map(|at| at.elapsed().as_millis() / 250)
            .collect();
        slots.dedup();
        slots.len() >= 3
    }

    /// Nothing at all has been written for this long.
    fn silent_for(&self, quiet: Duration) -> bool {
        let screen = self.screen.lock().unwrap_or_else(|err| err.into_inner());
        screen.last_output.is_none_or(|at| at.elapsed() >= quiet)
    }

    fn poked(&self) {
        self.screen.lock().unwrap_or_else(|err| err.into_inner()).last_poke = Some(Instant::now());
    }

    /// The program in the foreground of this terminal, if it is a known agent.
    fn foreground_agent(&self) -> Option<Option<&'static str>> {
        #[cfg(all(unix, not(target_os = "android")))]
        {
            let pid = self.master.lock().unwrap_or_else(|err| err.into_inner()).process_group_leader()?;
            let pid = u32::try_from(pid).ok()?;
            if Some(pid) == self.shell_pid {
                return Some(None);
            }
            Some(agents::command_line(pid).and_then(|args| agents::agent_from_command(&args)))
        }
        #[cfg(not(all(unix, not(target_os = "android"))))]
        {
            None
        }
    }
}

/// Written into the folder of pasted files so git ignores everything in it.
pub const PASTED_GITIGNORE: &str = "# Files pasted into OwnNAS terminals; removed when the terminal closes.\n*\n";

impl Drop for Session {
    fn drop(&mut self) {
        // Also covers terminals that end without being closed through `Terminals::remove`.
        self.remove_uploads();
    }
}

pub struct Terminals {
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    /// Sign-in token hash -> when the terminal password unlock expires.
    unlocked: Mutex<HashMap<String, Instant>>,
    /// Fires when a terminal opens, closes, or its agent state changes.
    changes: broadcast::Sender<()>,
}

impl Default for Terminals {
    fn default() -> Self {
        Self {
            sessions: Mutex::default(),
            unlocked: Mutex::default(),
            changes: broadcast::channel(64).0,
        }
    }
}

impl Terminals {
    pub fn subscribe(&self) -> broadcast::Receiver<()> {
        self.changes.subscribe()
    }

    /// Finds the terminal a hook token belongs to.
    pub fn by_hook_token(&self, token: &str) -> Option<Arc<Session>> {
        use sha2::{Digest, Sha256};
        // Compare digests so the lookup time does not depend on how much of a token matches.
        let wanted = Sha256::digest(token.as_bytes());
        let sessions = self.sessions.lock().unwrap_or_else(|err| err.into_inner());
        sessions.values().find(|session| Sha256::digest(session.hook_token.as_bytes()) == wanted).cloned()
    }

    /// Refreshes agent detection and activity for every terminal.
    pub fn poll_agents(&self) {
        let sessions: Vec<_> = self.sessions.lock().unwrap_or_else(|err| err.into_inner()).values().cloned().collect();
        for session in sessions {
            let Some(name) = session.foreground_agent() else { continue };
            let busy = session.busy();
            let silent = session.silent_for(Duration::from_secs(10));
            session.update_agent(|agent| {
                agent.detected(name, busy);
                if silent {
                    agent.went_quiet();
                }
            });
        }
    }

    pub fn list(&self) -> Vec<Info> {
        let sessions = self.sessions.lock().unwrap_or_else(|err| err.into_inner());
        let mut list: Vec<Info> = sessions.values().map(|session| session.info()).collect();
        list.sort_by_key(|info| info.created);
        list
    }

    pub fn get(&self, id: &str) -> Option<Arc<Session>> {
        self.sessions.lock().unwrap_or_else(|err| err.into_inner()).get(id).cloned()
    }

    pub fn remove(&self, id: &str) -> Option<Arc<Session>> {
        let session = self.sessions.lock().unwrap_or_else(|err| err.into_inner()).remove(id)?;
        session.kill();
        session.remove_uploads();
        let _ = self.changes.send(());
        Some(session)
    }

    pub fn kill_all(&self) {
        let sessions: Vec<_> = self.sessions.lock().unwrap_or_else(|err| err.into_inner()).drain().collect();
        for (_, session) in sessions {
            session.kill();
            session.remove_uploads();
        }
        let _ = self.changes.send(());
    }

    pub fn unlock(&self, token_hash: &str) {
        let mut unlocked = self.unlocked.lock().unwrap_or_else(|err| err.into_inner());
        let now = Instant::now();
        unlocked.retain(|_, until| *until > now);
        unlocked.insert(token_hash.to_string(), now + UNLOCK_FOR);
    }

    pub fn lock(&self, token_hash: Option<&str>) {
        let mut unlocked = self.unlocked.lock().unwrap_or_else(|err| err.into_inner());
        match token_hash {
            Some(hash) => {
                unlocked.remove(hash);
            }
            None => unlocked.clear(),
        }
    }

    pub fn is_unlocked(&self, token_hash: &str) -> bool {
        self.unlocked
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .get(token_hash)
            .is_some_and(|until| *until > Instant::now())
    }

    /// Starts `shell` (or the platform default) in `cwd`. `agent_url` is where hooks inside
    /// the terminal report agent events.
    pub fn spawn(&self, cwd: &Path, rel: &str, shell: Option<&str>, owner: &str, cols: u16, rows: u16, agent_url: Option<&str>) -> Result<Info, String> {
        if self.sessions.lock().unwrap_or_else(|err| err.into_inner()).len() >= MAX_SESSIONS {
            return Err(format!("Close a terminal first (at most {MAX_SESSIONS} can run at once)"));
        }
        let session = spawn_session(cwd, rel, shell, owner, cols, rows, agent_url, self.changes.clone())?;
        let info = session.info();
        self.sessions
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .insert(info.id.clone(), session);
        let _ = self.changes.send(());
        Ok(info)
    }
}

/// The shell used when no override is configured.
pub fn default_shell() -> String {
    #[cfg(windows)]
    {
        let on_path = |exe: &str| {
            std::env::var_os("PATH")
                .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(exe).is_file()))
                .unwrap_or(false)
        };
        if on_path("pwsh.exe") {
            "pwsh.exe".to_string()
        } else {
            "powershell.exe".to_string()
        }
    }
    #[cfg(not(windows))]
    {
        std::env::var("SHELL")
            .ok()
            .filter(|shell| !shell.trim().is_empty())
            .unwrap_or_else(|| {
                if cfg!(target_os = "macos") {
                    "/bin/zsh".to_string()
                } else if Path::new("/bin/bash").exists() {
                    "/bin/bash".to_string()
                } else {
                    "/bin/sh".to_string()
                }
            })
    }
}

pub fn available() -> bool {
    cfg!(not(target_os = "android"))
}

#[cfg(not(target_os = "android"))]
fn pty_size(cols: u16, rows: u16) -> portable_pty::PtySize {
    portable_pty::PtySize {
        rows: rows.clamp(2, 500),
        cols: cols.clamp(2, 1000),
        pixel_width: 0,
        pixel_height: 0,
    }
}

#[cfg(target_os = "android")]
#[allow(clippy::too_many_arguments)]
fn spawn_session(_: &Path, _: &str, _: Option<&str>, _: &str, _: u16, _: u16, _: Option<&str>, _: broadcast::Sender<()>) -> Result<Arc<Session>, String> {
    Err("Terminals are not available on Android".into())
}

#[cfg(not(target_os = "android"))]
#[allow(clippy::too_many_arguments)]
fn spawn_session(
    cwd: &Path,
    rel: &str,
    shell: Option<&str>,
    owner: &str,
    cols: u16,
    rows: u16,
    agent_url: Option<&str>,
    changes: broadcast::Sender<()>,
) -> Result<Arc<Session>, String> {
    use portable_pty::{native_pty_system, CommandBuilder};
    use std::io::{Read, Write};

    let shell = shell
        .map(str::trim)
        .filter(|shell| !shell.is_empty())
        .map(str::to_string)
        .unwrap_or_else(default_shell);
    let pair = native_pty_system()
        .openpty(pty_size(cols, rows))
        .map_err(|err| format!("Could not open a terminal: {err}"))?;
    let mut command = CommandBuilder::new(&shell);
    #[cfg(not(windows))]
    {
        // A login shell reads the user's profile, so tools in ~/.local/bin and the like are on PATH.
        let name = Path::new(&shell).file_name().and_then(|name| name.to_str()).unwrap_or("");
        if matches!(name, "bash" | "zsh" | "sh" | "fish" | "ksh" | "dash") {
            command.arg("-l");
        }
    }
    command.cwd(cwd);
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    // macOS and BSD `ls` only color their output when asked; GNU `ls` ignores this.
    command.env("CLICOLOR", "1");
    command.env("TERM_PROGRAM", "OwnNAS");
    // Never hand server secrets (such as OWNNAS_PASSWORD) to the shell.
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().to_ascii_uppercase().starts_with("OWNNAS_") {
            command.env_remove(&key);
        }
    }
    let mut id_bytes = [0u8; 12];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut id_bytes);
    let id = hex::encode(id_bytes);
    let hook_token = crate::auth::new_token();
    if let Some(url) = agent_url {
        command.env(agents::ENV_URL, url);
        command.env(agents::ENV_TOKEN, &hook_token);
        command.env(agents::ENV_TERMINAL, &id);
    }
    let mut child = pair
        .slave
        .spawn_command(command)
        .map_err(|err| format!("Could not start {shell}: {err}"))?;
    drop(pair.slave);
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|err| format!("Could not read the terminal: {err}"))?;
    let mut writer = pair
        .master
        .take_writer()
        .map_err(|err| format!("Could not write to the terminal: {err}"))?;
    let killer = child.clone_killer();
    let shell_pid = child.process_id();

    let (input, input_rx) = std::sync::mpsc::channel::<Vec<u8>>();
    let (events, _) = broadcast::channel(1024);
    let created = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let session = Arc::new(Session {
        info: Info {
            id,
            path: rel.to_string(),
            shell: Path::new(&shell)
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or(shell.clone()),
            owner: owner.to_string(),
            created,
            running: true,
            exit_code: None,
            title: None,
            name: None,
            agent: Agent::default(),
        },
        hook_token,
        cwd: cwd.to_path_buf(),
        shell_pid,
        name: Mutex::new(None),
        uploads: Mutex::new(Vec::new()),
        agent: Mutex::new(Agent::default()),
        changes,
        screen: Mutex::new(Screen {
            scrollback: VecDeque::new(),
            exit: None,
            escapes: Escapes::default(),
            title: None,
            last_output: None,
            drawn: VecDeque::new(),
            last_poke: None,
        }),
        events,
        input,
        master: Mutex::new(pair.master),
        killer: Mutex::new(killer),
    });

    // Writes can block when the program is not reading, so they get their own thread.
    std::thread::spawn(move || {
        while let Ok(bytes) = input_rx.recv() {
            if writer.write_all(&bytes).and_then(|_| writer.flush()).is_err() {
                break;
            }
        }
    });

    let output = Arc::downgrade(&session);
    std::thread::spawn(move || {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(read) => match output.upgrade() {
                    Some(session) => session.record(Event::Output(Arc::new(buf[..read].to_vec()))),
                    None => break,
                },
            }
        }
        let code = child.wait().ok().map(|status| status.exit_code());
        if let Some(session) = output.upgrade() {
            session.record(Event::Exit(code));
        }
    });
    Ok(session)
}

#[cfg(test)]
mod escape_tests {
    use super::*;

    #[test]
    fn finds_bells_notifications_and_titles_across_reads() {
        let mut escapes = Escapes::default();
        let mut out = Vec::new();
        escapes.feed(b"hi\x07 \x1b]0;claude: fixing\x07\x1b]9;Claude needs ", &mut out);
        escapes.feed(b"your permission\x1b\\\x1b]9;4;3;50\x07\x1b[31mred\x1b]777;notify;Codex;Turn complete\x07", &mut out);
        escapes.feed(b"\x1b]99;i=1;Done here\x1b\\", &mut out);
        assert_eq!(
            out,
            vec![
                Signal::Bell,
                Signal::Title("claude: fixing".into()),
                Signal::Notify("Claude needs your permission".into()),
                Signal::Notify("Codex: Turn complete".into()),
                Signal::Notify("Done here".into()),
            ]
        );
    }
}

#[cfg(all(test, not(target_os = "android"), unix))]
mod tests {
    use super::*;

    #[test]
    fn runs_a_shell_in_the_folder_and_replays_output() {
        let dir = std::env::temp_dir().join(format!("ownnas-term-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("OWNNAS_PASSWORD", "must-not-leak");
        let terminals = Terminals::default();
        let info = terminals.spawn(&dir, "work", Some("/bin/sh"), "admin", 80, 24, Some("http://127.0.0.1:9/agent-events")).unwrap();
        let session = terminals.get(&info.id).unwrap();
        session.write(b"pwd; echo \"secret=$OWNNAS_PASSWORD. url=$OWNNAS_AGENT_URL\"; printf '\\033]777;notify;Build;done\\007'; exit 3\n".to_vec());
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let (scrollback, exit, _) = session.attach();
            if exit.is_some() {
                let text = String::from_utf8_lossy(&scrollback).to_string();
                assert!(text.contains(dir.file_name().unwrap().to_str().unwrap()), "{text}");
                assert!(text.contains("secret=. url=http://127.0.0.1:9/agent-events"), "{text}");
                assert_eq!(session.info().agent.attention.as_deref(), Some("Build: done"));
                assert!(terminals.by_hook_token(&session.hook_token).is_some());
                assert!(terminals.by_hook_token("wrong").is_none());
                assert_eq!(exit, Some(Some(3)));
                break;
            }
            assert!(Instant::now() < deadline, "shell did not exit");
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!terminals.list()[0].running);
        terminals.remove(&info.id);
        assert!(terminals.list().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
