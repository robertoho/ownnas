//! Terminals on the server: listing, opening, and closing them over HTTP, and a live session
//! over the terminal WebSocket that keeps the screen in a VT100 emulator. Apps draw
//! [`Snapshot`]s and send keys; this module does the protocol, reconnects, and colors.
//!
//! The socket speaks the web terminal's protocol: binary frames are terminal bytes both ways,
//! the server's text frames are JSON (`reset`, `exit`), and the client sends JSON `resize`.
//! Every connection starts by replaying the terminal's scrollback.
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;
use tungstenite::client::IntoClientRequest;
use tungstenite::Message;

use crate::api::{map_err, Api, ApiError};

/// Lines kept above the screen for scrolling back.
const SCROLLBACK: usize = 5000;
/// How long the session thread waits for output before checking for keys to send.
const POLL: Duration = Duration::from_millis(15);

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    /// Agent id such as "claude"; `None` when no coding agent runs in the terminal.
    pub name: Option<String>,
    pub label: Option<String>,
    /// "working", "waiting" (needs the user), "idle", "done", or "error".
    pub status: Option<String>,
    /// One line about the current step.
    pub detail: Option<String>,
    /// Why the terminal wants attention, until someone looks at it.
    pub attention: Option<String>,
    pub last_message: Option<String>,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TerminalInfo {
    pub id: String,
    /// Folder it runs in, relative to the shared folder.
    pub path: String,
    pub owner: String,
    /// Seconds since the Unix epoch.
    pub created: i64,
    pub running: bool,
    pub exit_code: Option<u32>,
    /// Window title set by the program in the terminal.
    pub title: Option<String>,
    /// Name someone gave the terminal.
    pub name: Option<String>,
    #[serde(default)]
    pub agent: AgentInfo,
}

#[derive(Deserialize)]
struct TerminalList {
    terminals: Vec<TerminalInfo>,
}

#[derive(Deserialize)]
struct Opened {
    terminal: TerminalInfo,
}

impl Api {
    /// Every open terminal. [`ApiError::Locked`] means: ask for the terminal password and [`Api::unlock_terminals`].
    pub fn terminals(&self) -> Result<Vec<TerminalInfo>, ApiError> {
        let response = self.request("GET", "/api/terminals").call().map_err(map_err)?;
        let body: TerminalList = response.into_json().map_err(|err| ApiError::Other(err.to_string()))?;
        Ok(body.terminals)
    }

    /// Starts a shell in `path` (a folder relative to the shared folder).
    pub fn open_terminal(&self, path: &str, cols: u16, rows: u16) -> Result<TerminalInfo, ApiError> {
        let response = self.post_json("/api/terminals", json!({ "path": path, "cols": cols, "rows": rows }))?;
        let body: Opened = response.into_json().map_err(|err| ApiError::Other(err.to_string()))?;
        Ok(body.terminal)
    }

    /// Ends a terminal and its programs.
    pub fn close_terminal(&self, id: &str) -> Result<(), ApiError> {
        self.request("DELETE", "/api/terminals").query("id", id).call().map_err(map_err)?;
        Ok(())
    }

    pub fn unlock_terminals(&self, password: &str) -> Result<(), ApiError> {
        self.post_json("/api/terminal/unlock", json!({ "password": password }))?;
        Ok(())
    }

    /// Clears the "wants attention" mark once someone has looked at the terminal.
    pub fn mark_terminal_seen(&self, id: &str) -> Result<(), ApiError> {
        self.post_json("/api/terminals/seen", json!({ "id": id }))?;
        Ok(())
    }
}

/// Where a session's connection stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Connection {
    Connecting,
    Live,
    /// Lost; trying again shortly.
    Reconnecting,
    /// The shell exited.
    Exited(Option<u32>),
    /// Will not reconnect: closed on the server, signed out, or closed by the app.
    Closed,
}

/// Told about everything that happens in a session, from the session's own thread.
pub trait Listener: Send + Sync {
    /// The screen changed; take a new [`Session::snapshot`] when convenient.
    fn screen_changed(&self);
    fn connection_changed(&self, connection: Connection, message: Option<String>);
}

/// A run of cells with the same look. Colors are 0xAARRGGBB; 0 means the theme's default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub col: u16,
    /// Cells it covers: one per character, two for each wide (CJK, emoji) one.
    pub width: u16,
    pub text: String,
    pub fg: u32,
    pub bg: u32,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub rows: u16,
    pub cols: u16,
    /// One entry per screen row, top first.
    pub lines: Vec<Vec<Run>>,
    pub cursor_row: u16,
    pub cursor_col: u16,
    /// False while the program hides it, or while scrolled back.
    pub cursor_visible: bool,
    /// Lines scrolled back from the live screen (0 = live).
    pub scrolled_back: u32,
    /// Arrow keys send `ESC O A` rather than `ESC [ A`.
    pub application_cursor: bool,
    /// Pastes should be wrapped in `ESC [200~` … `ESC [201~`.
    pub bracketed_paste: bool,
    pub title: Option<String>,
}

/// Keeps the title the program sets.
#[derive(Default)]
struct Titles {
    title: Option<String>,
}

impl vt100::Callbacks for Titles {
    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.title = Some(String::from_utf8_lossy(title).into_owned()).filter(|title| !title.is_empty());
    }
}

enum Command {
    Input(Vec<u8>),
    Resize(u16, u16),
    Close,
}

/// A live terminal. Dropping it, or calling [`Session::close`], disconnects; the shell keeps
/// running on the server until [`Api::close_terminal`].
pub struct Session {
    parser: Arc<Mutex<vt100::Parser<Titles>>>,
    commands: Sender<Command>,
}

impl Session {
    pub fn connect(api: &Api, id: &str, rows: u16, cols: u16, listener: Arc<dyn Listener>) -> Result<Session, ApiError> {
        let token = api.token.clone().ok_or(ApiError::Unauthorized)?;
        let target = Target::parse(&api.server, id, token, api.tls.clone())?;
        let parser = Arc::new(Mutex::new(vt100::Parser::new_with_callbacks(rows.max(1), cols.max(1), SCROLLBACK, Titles::default())));
        let (commands, inbox) = mpsc::channel();
        let worker = Worker { target, parser: parser.clone(), inbox, listener, size: (rows.max(1), cols.max(1)) };
        thread::Builder::new()
            .name("ownnas-terminal".into())
            .spawn(move || worker.run())
            .map_err(|err| ApiError::Other(err.to_string()))?;
        Ok(Session { parser, commands })
    }

    /// Bytes typed or pasted, already encoded for the terminal.
    pub fn send(&self, bytes: Vec<u8>) {
        let _ = self.commands.send(Command::Input(bytes));
    }

    /// Text typed on a keyboard. Returns to the live screen first, as terminals do.
    pub fn type_text(&self, text: &str) {
        self.scroll_to(0);
        self.send(text.as_bytes().to_vec());
    }

    /// Pasted text, marked as a paste when the program asked for that.
    pub fn paste(&self, text: &str) {
        let bracketed = self.lock().screen().bracketed_paste();
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        let bytes = if bracketed { format!("\x1b[200~{text}\x1b[201~") } else { text };
        self.type_text(&bytes);
    }

    /// The view now fits `rows` × `cols` cells.
    pub fn resize(&self, rows: u16, cols: u16) {
        let (rows, cols) = (rows.max(1), cols.max(1));
        if self.lock().screen().size() == (rows, cols) {
            return;
        }
        self.lock().screen_mut().set_size(rows, cols);
        let _ = self.commands.send(Command::Resize(rows, cols));
    }

    /// Moves the view `lines` up (positive) or down; returns how far back it now is.
    pub fn scroll_by(&self, lines: i32) -> u32 {
        let mut parser = self.lock();
        let current = parser.screen().scrollback() as i64;
        let wanted = (current + lines as i64).max(0) as usize;
        parser.screen_mut().set_scrollback(wanted);
        parser.screen().scrollback() as u32
    }

    pub fn scroll_to(&self, lines_back: u32) {
        self.lock().screen_mut().set_scrollback(lines_back as usize);
    }

    pub fn snapshot(&self) -> Snapshot {
        let parser = self.lock();
        snapshot_of(parser.screen(), parser.callbacks().title.clone())
    }

    /// Disconnects; the shell keeps running on the server.
    pub fn close(&self) {
        let _ = self.commands.send(Command::Close);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, vt100::Parser<Titles>> {
        self.parser.lock().unwrap_or_else(|err| err.into_inner())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.close();
    }
}

/// Where and how to connect a terminal socket.
struct Target {
    host: String,
    port: u16,
    tls: Option<Arc<rustls::ClientConfig>>,
    url: String,
    token: String,
}

impl Target {
    fn parse(server: &str, id: &str, token: String, tls: Arc<rustls::ClientConfig>) -> Result<Target, ApiError> {
        let invalid = || ApiError::Other(format!("Not a server address: {server}"));
        let (secure, rest) = if let Some(rest) = server.strip_prefix("https://") {
            (true, rest)
        } else if let Some(rest) = server.strip_prefix("http://") {
            (false, rest)
        } else {
            return Err(invalid());
        };
        let (authority, prefix) = match rest.find('/') {
            Some(slash) => (&rest[..slash], rest[slash..].trim_end_matches('/')),
            None => (rest, ""),
        };
        let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
            // An IPv6 address: [::1]:8787
            let (host, after) = bracketed.split_once(']').ok_or_else(invalid)?;
            (host.to_string(), after.strip_prefix(':').map(str::parse).transpose().map_err(|_| invalid())?)
        } else {
            match authority.rsplit_once(':') {
                Some((host, port)) => (host.to_string(), Some(port.parse().map_err(|_| invalid())?)),
                None => (authority.to_string(), None),
            }
        };
        let port = port.unwrap_or(if secure { 443 } else { 80 });
        let scheme = if secure { "wss" } else { "ws" };
        let query: String = id.bytes().map(|b| if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
        Ok(Target {
            url: format!("{scheme}://{authority}{prefix}/api/terminals/ws?id={query}"),
            host,
            port,
            tls: secure.then_some(tls),
            token,
        })
    }
}

/// Plain TCP, or TLS over it.
enum Stream {
    Plain(TcpStream),
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
}

impl Stream {
    fn tcp(&self) -> &TcpStream {
        match self {
            Stream::Plain(tcp) => tcp,
            Stream::Tls(tls) => &tls.sock,
        }
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Stream::Plain(tcp) => tcp.read(buf),
            Stream::Tls(tls) => tls.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Stream::Plain(tcp) => tcp.write(buf),
            Stream::Tls(tls) => tls.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Stream::Plain(tcp) => tcp.flush(),
            Stream::Tls(tls) => tls.flush(),
        }
    }
}

type Socket = tungstenite::WebSocket<Stream>;

/// Why a connection ended.
enum Ended {
    /// The app closed the session.
    ByApp,
    Exited(Option<u32>),
    Lost(String),
    /// The server refused the socket; reconnecting will not help.
    Refused(String),
}

struct Worker {
    target: Target,
    parser: Arc<Mutex<vt100::Parser<Titles>>>,
    inbox: Receiver<Command>,
    listener: Arc<dyn Listener>,
    size: (u16, u16),
}

impl Worker {
    fn run(mut self) {
        let mut attempts = 0u32;
        loop {
            let state = if attempts == 0 { Connection::Connecting } else { Connection::Reconnecting };
            self.listener.connection_changed(state, None);
            let ended = match self.connect() {
                Ok(socket) => {
                    attempts = 0;
                    self.listener.connection_changed(Connection::Live, None);
                    self.pump(socket)
                }
                Err(ended) => ended,
            };
            match ended {
                Ended::ByApp => return self.listener.connection_changed(Connection::Closed, None),
                Ended::Exited(code) => return self.listener.connection_changed(Connection::Exited(code), None),
                Ended::Refused(message) => return self.listener.connection_changed(Connection::Closed, Some(message)),
                Ended::Lost(message) => {
                    attempts += 1;
                    self.listener.connection_changed(Connection::Reconnecting, Some(message));
                    // Back off like the web terminal: 0.5 s, 1 s, 2 s, … up to 10 s.
                    let delay = Duration::from_millis((500u64 << attempts.min(5)).min(10_000));
                    if self.wait(delay) {
                        return self.listener.connection_changed(Connection::Closed, None);
                    }
                }
            }
        }
    }

    /// Sleeps, but keeps track of resizes and wakes for a close. True when closed.
    fn wait(&mut self, delay: Duration) -> bool {
        let deadline = std::time::Instant::now() + delay;
        while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
            match self.inbox.recv_timeout(left.min(Duration::from_millis(100))) {
                Ok(Command::Close) | Err(mpsc::RecvTimeoutError::Disconnected) => return true,
                Ok(Command::Resize(rows, cols)) => self.size = (rows, cols),
                Ok(Command::Input(_)) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
        false
    }

    fn connect(&self) -> Result<Socket, Ended> {
        let lost = |err: &dyn std::fmt::Display| Ended::Lost(err.to_string());
        let address = (self.target.host.as_str(), self.target.port)
            .to_socket_addrs()
            .map_err(|err| lost(&err))?
            .next()
            .ok_or_else(|| Ended::Lost(format!("Could not find {}", self.target.host)))?;
        let tcp = TcpStream::connect_timeout(&address, Duration::from_secs(15)).map_err(|err| lost(&err))?;
        tcp.set_nodelay(true).ok();
        let stream = match &self.target.tls {
            None => Stream::Plain(tcp),
            Some(config) => {
                let name = rustls::pki_types::ServerName::try_from(self.target.host.clone()).map_err(|err| lost(&err))?;
                let connection = rustls::ClientConnection::new(config.clone(), name).map_err(|err| lost(&err))?;
                Stream::Tls(Box::new(rustls::StreamOwned::new(connection, tcp)))
            }
        };
        let mut request = self.target.url.as_str().into_client_request().map_err(|err| lost(&err))?;
        let bearer = format!("Bearer {}", self.target.token)
            .parse()
            .map_err(|_| Ended::Refused("The sign-in token is not valid".into()))?;
        request.headers_mut().insert("Authorization", bearer);
        let (mut socket, _) = tungstenite::client(request, stream).map_err(|err| match err {
            tungstenite::HandshakeError::Failure(tungstenite::Error::Http(response)) => {
                let message = response
                    .body()
                    .as_deref()
                    .and_then(|body| serde_json::from_slice::<serde_json::Value>(body).ok())
                    .and_then(|body| body["error"].as_str().map(str::to_string))
                    .unwrap_or_else(|| format!("The server refused the terminal (HTTP {})", response.status().as_u16()));
                Ended::Refused(message)
            }
            other => lost(&other),
        })?;
        socket.get_mut().tcp().set_read_timeout(Some(POLL)).map_err(|err| lost(&err))?;
        let (rows, cols) = self.size;
        socket
            .send(Message::text(json!({ "type": "resize", "cols": cols, "rows": rows }).to_string()))
            .map_err(|err| lost(&err))?;
        Ok(socket)
    }

    /// Moves bytes both ways until the connection ends.
    fn pump(&mut self, mut socket: Socket) -> Ended {
        // The server replays the scrollback first; start from a clean screen for it.
        let mut replay = true;
        loop {
            loop {
                match self.inbox.try_recv() {
                    Ok(Command::Input(bytes)) => {
                        if let Err(err) = socket.send(Message::binary(bytes)) {
                            return Ended::Lost(err.to_string());
                        }
                    }
                    Ok(Command::Resize(rows, cols)) => {
                        self.size = (rows, cols);
                        let resize = json!({ "type": "resize", "cols": cols, "rows": rows }).to_string();
                        if let Err(err) = socket.send(Message::text(resize)) {
                            return Ended::Lost(err.to_string());
                        }
                    }
                    Ok(Command::Close) | Err(TryRecvError::Disconnected) => {
                        let _ = socket.close(None);
                        let _ = socket.flush();
                        return Ended::ByApp;
                    }
                    Err(TryRecvError::Empty) => break,
                }
            }
            match socket.read() {
                Ok(Message::Binary(bytes)) => {
                    let mut parser = self.parser.lock().unwrap_or_else(|err| err.into_inner());
                    if replay {
                        replay = false;
                        reset(&mut parser);
                    }
                    parser.process(&bytes);
                    drop(parser);
                    self.listener.screen_changed();
                }
                Ok(Message::Text(text)) => {
                    let message: serde_json::Value = serde_json::from_str(text.as_str()).unwrap_or_default();
                    match message["type"].as_str() {
                        Some("reset") => {
                            reset(&mut self.parser.lock().unwrap_or_else(|err| err.into_inner()));
                            self.listener.screen_changed();
                        }
                        Some("exit") => return Ended::Exited(message["code"].as_u64().map(|code| code as u32)),
                        _ => {}
                    }
                }
                Ok(Message::Close(frame)) => {
                    let reason = frame.map(|frame| frame.reason.to_string()).filter(|reason| !reason.is_empty());
                    return Ended::Lost(reason.unwrap_or_else(|| "The server closed the connection".into()));
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(err)) if matches!(err.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
                Err(err) => return Ended::Lost(err.to_string()),
            }
        }
    }
}

/// Clears the emulator, keeping its size and the program's title.
fn reset(parser: &mut vt100::Parser<Titles>) {
    let (rows, cols) = parser.screen().size();
    let title = parser.callbacks_mut().title.take();
    *parser = vt100::Parser::new_with_callbacks(rows, cols, SCROLLBACK, Titles { title });
}

fn snapshot_of(screen: &vt100::Screen, title: Option<String>) -> Snapshot {
    let (rows, cols) = screen.size();
    let mut lines = Vec::with_capacity(rows as usize);
    for row in 0..rows {
        let mut runs: Vec<Run> = Vec::new();
        let mut col = 0;
        while col < cols {
            let Some(cell) = screen.cell(row, col) else { break };
            if cell.is_wide_continuation() {
                col += 1;
                continue;
            }
            let width = if cell.is_wide() { 2 } else { 1 };
            let text = if cell.has_contents() { cell.contents() } else { " " };
            let (mut fg, mut bg) = (color(cell.fgcolor(), cell.bold()), color(cell.bgcolor(), false));
            if cell.inverse() {
                // 1 stands for "the default color of the other side", resolved by the app.
                let swap = |c: u32, other_default: u32| if c == 0 { other_default } else { c };
                (fg, bg) = (swap(bg, INVERSE_DEFAULT_BG), swap(fg, INVERSE_DEFAULT_FG));
            }
            let looks = (fg, bg, cell.bold(), cell.dim(), cell.italic(), cell.underline());
            match runs.last_mut() {
                // Wide characters get their own run so every run is evenly spaced.
                Some(last)
                    if width == 1
                        && last.width as usize == last.text.chars().count()
                        && last.col + last.width == col
                        && (last.fg, last.bg, last.bold, last.dim, last.italic, last.underline) == looks =>
                {
                    last.text.push_str(text);
                    last.width += 1;
                }
                _ => runs.push(Run {
                    col,
                    width,
                    text: text.to_string(),
                    fg,
                    bg,
                    bold: cell.bold(),
                    dim: cell.dim(),
                    italic: cell.italic(),
                    underline: cell.underline(),
                }),
            }
            col += width;
        }
        lines.push(runs);
    }
    let (cursor_row, cursor_col) = screen.cursor_position();
    Snapshot {
        rows,
        cols,
        lines,
        cursor_row,
        cursor_col,
        cursor_visible: !screen.hide_cursor() && screen.scrollback() == 0,
        scrolled_back: screen.scrollback() as u32,
        application_cursor: screen.application_cursor(),
        bracketed_paste: screen.bracketed_paste(),
        title,
    }
}

/// Inverse text on the default colors: the app draws the default background as text and the
/// default foreground as the background. Real colors always have an alpha of 0xFF.
pub const INVERSE_DEFAULT_FG: u32 = 1;
pub const INVERSE_DEFAULT_BG: u32 = 2;

/// xterm.js's default palette, so terminals look the same as in the web app.
const ANSI: [u32; 16] = [
    0x2e3436, 0xcc0000, 0x4e9a06, 0xc4a000, 0x3465a4, 0x75507b, 0x06989a, 0xd3d7cf, 0x555753, 0xef2929, 0x8ae234,
    0xfce94f, 0x729fcf, 0xad7fa8, 0x34e2e2, 0xeeeeec,
];

/// 0xAARRGGBB, or 0 for the default. Bold text in the first eight colors uses the bright ones.
fn color(color: vt100::Color, bold: bool) -> u32 {
    let rgb = match color {
        vt100::Color::Default => return 0,
        vt100::Color::Idx(index) if index < 8 && bold => ANSI[index as usize + 8],
        vt100::Color::Idx(index) if index < 16 => ANSI[index as usize],
        vt100::Color::Idx(index) if index < 232 => {
            let steps = [0, 95, 135, 175, 215, 255];
            let i = index as usize - 16;
            (steps[i / 36] << 16) | (steps[(i / 6) % 6] << 8) | steps[i % 6]
        }
        vt100::Color::Idx(index) => {
            let gray = 8 + 10 * (index as u32 - 232);
            (gray << 16) | (gray << 8) | gray
        }
        vt100::Color::Rgb(r, g, b) => ((r as u32) << 16) | ((g as u32) << 8) | b as u32,
    };
    0xff00_0000 | rgb
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tls() -> Arc<rustls::ClientConfig> {
        Arc::new(
            rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_root_certificates(rustls::RootCertStore::empty())
                .with_no_client_auth(),
        )
    }

    #[test]
    fn socket_urls_follow_the_server_address() {
        let target = Target::parse("http://10.0.2.2:8899", "abc", "t".into(), tls()).unwrap();
        assert_eq!((target.host.as_str(), target.port, target.tls.is_some()), ("10.0.2.2", 8899, false));
        assert_eq!(target.url, "ws://10.0.2.2:8899/api/terminals/ws?id=abc");
        let target = Target::parse("https://nas.example/own", "a b", "t".into(), tls()).unwrap();
        assert_eq!((target.port, target.tls.is_some()), (443, true));
        assert_eq!(target.url, "wss://nas.example/own/api/terminals/ws?id=a%20b");
        let target = Target::parse("http://[::1]:8787", "x", "t".into(), tls()).unwrap();
        assert_eq!((target.host.as_str(), target.port), ("::1", 8787));
    }

    #[test]
    fn snapshots_group_cells_into_colored_runs() {
        let mut parser = vt100::Parser::new_with_callbacks(2, 10, 0, Titles::default());
        parser.process(b"\x1b]0;build\x07ab\x1b[31mcd\x1b[0m\xe4\xbd\xa0e");
        let snapshot = snapshot_of(parser.screen(), parser.callbacks().title.clone());
        assert_eq!(snapshot.title.as_deref(), Some("build"));
        let line = &snapshot.lines[0];
        assert_eq!((line[0].text.as_str(), line[0].fg), ("ab", 0));
        assert_eq!((line[1].text.as_str(), line[1].fg, line[1].col), ("cd", 0xffcc0000, 2));
        // The wide character stands alone and covers two cells.
        assert_eq!((line[2].text.as_str(), line[2].col, line[2].width), ("你", 4, 2));
        assert_eq!((line[3].text.as_str(), line[3].col), ("e   ", 6));
        assert_eq!((snapshot.cursor_row, snapshot.cursor_col), (0, 7));
    }

    #[test]
    fn colors_match_the_web_palette() {
        assert_eq!(color(vt100::Color::Idx(1), false), 0xffcc0000);
        assert_eq!(color(vt100::Color::Idx(1), true), 0xffef2929);
        assert_eq!(color(vt100::Color::Idx(196), false), 0xffff0000);
        assert_eq!(color(vt100::Color::Idx(232), false), 0xff080808);
        assert_eq!(color(vt100::Color::Rgb(1, 2, 3), false), 0xff010203);
    }
}
