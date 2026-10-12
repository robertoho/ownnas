//! Browser terminals: shells running in a pseudo-terminal on the server, which keep running
//! (with a scrollback buffer) while no browser is attached, so long agent sessions survive a
//! closed tab or a dropped connection.
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::sync::broadcast;

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
}

struct Screen {
    scrollback: VecDeque<u8>,
    exit: Option<Option<u32>>,
}

pub struct Session {
    info: Info,
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
        info
    }

    /// Returns the scrollback, whether the shell already exited, and a receiver for everything
    /// after it. Taken under one lock so no output is lost or repeated.
    pub fn attach(&self) -> (Vec<u8>, Option<Option<u32>>, broadcast::Receiver<Event>) {
        let screen = self.screen.lock().unwrap_or_else(|err| err.into_inner());
        let receiver = self.events.subscribe();
        (screen.scrollback.iter().copied().collect(), screen.exit, receiver)
    }

    pub fn write(&self, bytes: Vec<u8>) {
        let _ = self.input.send(bytes);
    }

    pub fn resize(&self, cols: u16, rows: u16) {
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
        match &event {
            Event::Output(bytes) => {
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
        let _ = self.events.send(event);
    }
}

#[derive(Default)]
pub struct Terminals {
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    /// Sign-in token hash -> when the terminal password unlock expires.
    unlocked: Mutex<HashMap<String, Instant>>,
}

impl Terminals {
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
        Some(session)
    }

    pub fn kill_all(&self) {
        let sessions: Vec<_> = self.sessions.lock().unwrap_or_else(|err| err.into_inner()).drain().collect();
        for (_, session) in sessions {
            session.kill();
        }
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

    /// Starts `shell` (or the platform default) in `cwd`.
    pub fn spawn(&self, cwd: &Path, rel: &str, shell: Option<&str>, owner: &str, cols: u16, rows: u16) -> Result<Info, String> {
        if self.sessions.lock().unwrap_or_else(|err| err.into_inner()).len() >= MAX_SESSIONS {
            return Err(format!("Close a terminal first (at most {MAX_SESSIONS} can run at once)"));
        }
        let session = spawn_session(cwd, rel, shell, owner, cols, rows)?;
        let info = session.info();
        self.sessions
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .insert(info.id.clone(), session);
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
fn spawn_session(_: &Path, _: &str, _: Option<&str>, _: &str, _: u16, _: u16) -> Result<Arc<Session>, String> {
    Err("Terminals are not available on Android".into())
}

#[cfg(not(target_os = "android"))]
fn spawn_session(cwd: &Path, rel: &str, shell: Option<&str>, owner: &str, cols: u16, rows: u16) -> Result<Arc<Session>, String> {
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
    command.env("TERM_PROGRAM", "OwnNAS");
    // Never hand server secrets (such as OWNNAS_PASSWORD) to the shell.
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().to_ascii_uppercase().starts_with("OWNNAS_") {
            command.env_remove(&key);
        }
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

    let (input, input_rx) = std::sync::mpsc::channel::<Vec<u8>>();
    let (events, _) = broadcast::channel(1024);
    let mut id_bytes = [0u8; 12];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut id_bytes);
    let created = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let session = Arc::new(Session {
        info: Info {
            id: hex::encode(id_bytes),
            path: rel.to_string(),
            shell: Path::new(&shell)
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or(shell.clone()),
            owner: owner.to_string(),
            created,
            running: true,
            exit_code: None,
        },
        screen: Mutex::new(Screen { scrollback: VecDeque::new(), exit: None }),
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

#[cfg(all(test, not(target_os = "android"), unix))]
mod tests {
    use super::*;

    #[test]
    fn runs_a_shell_in_the_folder_and_replays_output() {
        let dir = std::env::temp_dir().join(format!("ownnas-term-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("OWNNAS_PASSWORD", "must-not-leak");
        let terminals = Terminals::default();
        let info = terminals.spawn(&dir, "work", Some("/bin/sh"), "admin", 80, 24).unwrap();
        let session = terminals.get(&info.id).unwrap();
        session.write(b"pwd; echo \"secret=$OWNNAS_PASSWORD.\"; exit 3\n".to_vec());
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let (scrollback, exit, _) = session.attach();
            if exit.is_some() {
                let text = String::from_utf8_lossy(&scrollback).to_string();
                assert!(text.contains(dir.file_name().unwrap().to_str().unwrap()), "{text}");
                assert!(text.contains("secret=."), "{text}");
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
