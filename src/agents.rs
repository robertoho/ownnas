//! Coding-agent awareness for terminals: which agent runs in a terminal, what it is doing,
//! and when it needs the user. Agents report through hooks (Claude Code) or a plugin
//! (opencode) that only act inside OwnNAS terminals; any other program is still detected
//! from the terminal's foreground process, bell, and desktop-notification escape codes.
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{json, Value};

/// Environment given to every terminal shell so hooks can report to this server.
pub const ENV_URL: &str = "OWNNAS_AGENT_URL";
pub const ENV_TOKEN: &str = "OWNNAS_AGENT_TOKEN";
pub const ENV_TERMINAL: &str = "OWNNAS_TERMINAL_ID";

const MAX_FILES: usize = 40;
/// Marker in installed hook commands and plugin files, used to find and remove them.
const MARKER: &str = "agent-hook";
const OPENCODE_PLUGIN: &str = include_str!("../integrations/opencode-ownnas.js");

/// Hook events OwnNAS listens to in Claude Code.
const CLAUDE_EVENTS: &[&str] = &[
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "Notification",
    "Stop",
    "StopFailure",
    "SubagentStart",
];

/// Agents recognized from a terminal's foreground command line, with display names.
const KNOWN_AGENTS: &[(&str, &str)] = &[
    ("claude", "Claude Code"),
    ("opencode", "opencode"),
    ("codex", "Codex"),
    ("gemini", "Gemini CLI"),
    ("aider", "Aider"),
    ("cursor-agent", "Cursor Agent"),
    ("copilot", "Copilot CLI"),
    ("amp", "Amp"),
    ("goose", "Goose"),
    ("crush", "Crush"),
    ("qwen", "Qwen Code"),
];

pub fn display_name(agent: &str) -> &str {
    KNOWN_AGENTS.iter().find(|(id, _)| *id == agent).map(|(_, name)| *name).unwrap_or(agent)
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Agent {
    /// Agent id such as "claude", or `None` when no agent runs in the terminal.
    pub name: Option<String>,
    pub label: Option<String>,
    /// "working", "waiting" (needs the user), "idle", "done", or "error".
    pub status: Option<String>,
    /// One line about the current step: "Editing src/http.rs".
    pub detail: Option<String>,
    /// True when the agent reports through hooks, so `status` is exact rather than guessed.
    pub hooked: bool,
    pub session_id: Option<String>,
    pub model: Option<String>,
    /// Library paths the agent changed, newest first.
    pub files: Vec<String>,
    /// Why the terminal wants the user's attention, until they look at it.
    pub attention: Option<String>,
    /// The end of the agent's last answer.
    pub last_message: Option<String>,
    /// Milliseconds since the Unix epoch.
    pub updated: i64,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn clip(text: &str, max: usize) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= max {
        line
    } else {
        format!("{}…", line.chars().take(max).collect::<String>().trim_end())
    }
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string).filter(|s| !s.is_empty())
}

/// Shows a path relative to the library when possible, else just its file name.
fn show_path(path: &str, rel: &dyn Fn(&str) -> Option<String>) -> String {
    rel(path).unwrap_or_else(|| Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string()))
}

impl Agent {
    fn set(&mut self, status: &str, detail: Option<String>) {
        self.status = Some(status.to_string());
        if detail.is_some() {
            self.detail = detail;
        }
    }

    fn identify(&mut self, name: &str) {
        if self.name.as_deref() != Some(name) {
            *self = Agent { name: Some(name.to_string()), label: Some(display_name(name).to_string()), ..Agent::default() };
        }
        self.hooked = true;
    }

    fn add_file(&mut self, path: String) {
        self.files.retain(|existing| *existing != path);
        self.files.insert(0, path);
        self.files.truncate(MAX_FILES);
    }

    fn ask(&mut self, message: String) {
        self.set("waiting", Some(message.clone()));
        self.attention = Some(message);
    }

    /// Applies one Claude Code hook event.
    pub fn apply_claude(&mut self, event: &Value, rel: &dyn Fn(&str) -> Option<String>) {
        let name = text(event, "hook_event_name").unwrap_or_default();
        if name == "SessionEnd" {
            *self = Agent { updated: now_ms(), ..Agent::default() };
            return;
        }
        self.identify("claude");
        if let Some(id) = text(event, "session_id") {
            self.session_id = Some(id);
        }
        let tool = text(event, "tool_name").unwrap_or_default();
        let input = event.get("tool_input").cloned().unwrap_or(Value::Null);
        match name.as_str() {
            "SessionStart" => {
                self.model = event.get("model").and_then(|m| m.as_str().map(str::to_string).or_else(|| text(m, "display_name")));
                self.set("idle", Some("Ready".into()));
            }
            "UserPromptSubmit" => {
                self.attention = None;
                self.last_message = None;
                let prompt = text(event, "prompt").map(|p| format!("Working on: {}", clip(&p, 90)));
                self.set("working", prompt.or(Some("Working".into())));
            }
            "PreToolUse" => self.set("working", Some(describe_claude(&tool, &input, rel))),
            "PostToolUse" | "PostToolUseFailure" => {
                if matches!(tool.as_str(), "Edit" | "MultiEdit" | "Write" | "NotebookEdit") {
                    let path = text(&input, "file_path").or_else(|| text(&input, "notebook_path"));
                    if let Some(path) = path.and_then(|p| rel(&p)) {
                        self.add_file(path);
                    }
                }
                self.status = Some("working".into());
            }
            "PermissionRequest" => {
                let step = describe_claude(&tool, &input, rel);
                self.ask(format!("Needs permission: {step}"));
            }
            "Notification" => {
                let message = text(event, "message");
                match text(event, "notification_type").as_deref() {
                    Some("permission_prompt") | Some("agent_needs_input") | Some("elicitation_dialog") => {
                        self.ask(message.unwrap_or_else(|| "Needs your input".into()));
                    }
                    Some("idle_prompt") => {
                        self.set("idle", None);
                        self.attention = Some(message.unwrap_or_else(|| "Waiting for your input".into()));
                    }
                    _ => {}
                }
            }
            "Stop" => {
                self.last_message = text(event, "last_assistant_message").map(|m| clip(&m, 400));
                self.set("done", Some("Finished".into()));
                self.attention = Some("Finished".into());
            }
            "StopFailure" => {
                let error = text(event, "error_type").unwrap_or_else(|| "unknown".into()).replace('_', " ");
                self.set("error", Some(format!("Stopped: {error}")));
                self.attention = Some(format!("Stopped: {error}"));
            }
            "SubagentStart" => {
                let kind = text(event, "agent_type").unwrap_or_else(|| "subagent".into());
                self.set("working", Some(format!("Running the {kind} subagent")));
            }
            _ => {}
        }
        self.updated = now_ms();
    }

    /// Applies one event forwarded by the OwnNAS opencode plugin.
    pub fn apply_opencode(&mut self, event: &Value, rel: &dyn Fn(&str) -> Option<String>) {
        self.identify("opencode");
        let kind = text(event, "type").unwrap_or_default();
        let props = event.get("properties").cloned().unwrap_or(Value::Null);
        if let Some(id) = text(&props, "sessionID").or_else(|| props.get("info").and_then(|i| text(i, "id"))) {
            self.session_id = Some(id);
        }
        match kind.as_str() {
            "plugin.loaded" => self.set("idle", Some("Ready".into())),
            "chat.message" => {
                self.attention = None;
                self.last_message = None;
                self.set("working", Some("Working".into()));
            }
            "session.status" => {
                let status = props.get("status").and_then(|s| s.as_str().map(str::to_string).or_else(|| text(s, "type")));
                match status.as_deref() {
                    Some("idle") => self.set("idle", None),
                    Some(_) => self.status = Some("working".into()),
                    None => {}
                }
            }
            "session.idle" => {
                self.set("done", Some("Finished".into()));
                self.attention = Some("Finished".into());
            }
            "session.error" => {
                let message = props
                    .get("error")
                    .and_then(|e| e.get("data").and_then(|d| text(d, "message")).or_else(|| text(e, "name")))
                    .unwrap_or_else(|| "error".into());
                self.set("error", Some(format!("Stopped: {}", clip(&message, 120))));
                self.attention = Some(format!("Stopped: {}", clip(&message, 120)));
            }
            "permission.asked" | "permission.updated" => {
                let what = text(&props, "title")
                    .or_else(|| text(&props, "permission"))
                    .or_else(|| text(&props, "type"))
                    .unwrap_or_else(|| "a step".into());
                self.ask(format!("Needs permission: {}", clip(&what, 120)));
            }
            "question.asked" => self.ask("Has a question for you".into()),
            "permission.replied" | "question.replied" => {
                self.attention = None;
                self.status = Some("working".into());
            }
            "file.edited" => {
                if let Some(path) = text(&props, "file").and_then(|p| rel(&p)) {
                    self.add_file(path);
                }
            }
            "tool.execute.before" => {
                let tool = text(event, "tool").unwrap_or_default();
                let args = event.get("args").cloned().unwrap_or(Value::Null);
                self.set("working", Some(describe_opencode(&tool, &args, rel)));
            }
            "tool.execute.after" => {
                let tool = text(event, "tool").unwrap_or_default();
                if matches!(tool.as_str(), "edit" | "write" | "patch" | "multiedit") {
                    let args = event.get("args").cloned().unwrap_or(Value::Null);
                    if let Some(path) = text(&args, "filePath").and_then(|p| rel(&p)) {
                        self.add_file(path);
                    }
                }
            }
            "plugin.unloaded" => {
                *self = Agent::default();
            }
            _ => {}
        }
        self.updated = now_ms();
    }

    /// Updates from foreground-process detection. Returns true if anything changed.
    pub fn detected(&mut self, name: Option<&str>, recently_active: bool) -> bool {
        let before = self.clone();
        match name {
            None if self.name.is_some() => {
                // The agent exited (possibly without a SessionEnd hook).
                let attention = self.attention.take();
                *self = Agent { attention, updated: now_ms(), ..Agent::default() };
            }
            None => {}
            Some(name) => {
                if self.name.as_deref() != Some(name) {
                    *self = Agent { name: Some(name.to_string()), label: Some(display_name(name).to_string()), ..Agent::default() };
                }
                if !self.hooked && self.status.as_deref() != Some("waiting") {
                    self.status = Some(if recently_active { "working" } else { "idle" }.into());
                }
            }
        }
        let changed = *self != before;
        if changed {
            self.updated = now_ms();
        }
        changed
    }

    /// The terminal rang its bell or sent a desktop notification.
    pub fn notified(&mut self, message: Option<String>) {
        let message = message.map(|m| clip(&m, 160)).unwrap_or_else(|| match &self.label {
            Some(label) => format!("{label} wants your attention"),
            None => "The terminal rang its bell".into(),
        });
        if self.name.is_some() && !self.hooked {
            self.status = Some("waiting".into());
            self.detail = Some(message.clone());
        }
        self.attention = Some(message);
        self.updated = now_ms();
    }
}

fn describe_claude(tool: &str, input: &Value, rel: &dyn Fn(&str) -> Option<String>) -> String {
    let path = || {
        text(input, "file_path")
            .or_else(|| text(input, "notebook_path"))
            .map(|p| show_path(&p, rel))
            .unwrap_or_else(|| "a file".into())
    };
    match tool {
        "Edit" | "MultiEdit" | "NotebookEdit" => format!("Editing {}", path()),
        "Write" => format!("Writing {}", path()),
        "Read" => format!("Reading {}", path()),
        "Bash" => format!("Running {}", clip(&text(input, "command").unwrap_or_default(), 80)),
        "Grep" => format!("Searching for {}", clip(&text(input, "pattern").unwrap_or_default(), 60)),
        "Glob" => format!("Finding {}", clip(&text(input, "pattern").unwrap_or_default(), 60)),
        "WebFetch" => format!("Reading {}", clip(&text(input, "url").unwrap_or_default(), 80)),
        "WebSearch" => format!("Searching the web for {}", clip(&text(input, "query").unwrap_or_default(), 60)),
        "Task" | "Agent" => format!("Delegating: {}", clip(&text(input, "description").unwrap_or_default(), 70)),
        "TodoWrite" => "Updating its to-do list".into(),
        "" => "Working".into(),
        other if other.starts_with("mcp__") => format!("Using {}", other.trim_start_matches("mcp__").replace("__", " → ")),
        other => format!("Using {other}"),
    }
}

fn describe_opencode(tool: &str, args: &Value, rel: &dyn Fn(&str) -> Option<String>) -> String {
    let path = || text(args, "filePath").map(|p| show_path(&p, rel)).unwrap_or_else(|| "a file".into());
    match tool {
        "edit" | "multiedit" | "patch" => format!("Editing {}", path()),
        "write" => format!("Writing {}", path()),
        "read" => format!("Reading {}", path()),
        "bash" => format!("Running {}", clip(&text(args, "command").unwrap_or_default(), 80)),
        "grep" => format!("Searching for {}", clip(&text(args, "pattern").unwrap_or_default(), 60)),
        "glob" => format!("Finding {}", clip(&text(args, "pattern").unwrap_or_default(), 60)),
        "webfetch" => format!("Reading {}", clip(&text(args, "url").unwrap_or_default(), 80)),
        "task" => format!("Delegating: {}", clip(&text(args, "description").unwrap_or_default(), 70)),
        "todowrite" => "Updating its to-do list".into(),
        "" => "Working".into(),
        other => format!("Using {other}"),
    }
}

/// Recognizes an agent from a command line (`claude`, `node …/codex.js`, …).
pub fn agent_from_command(args: &[String]) -> Option<&'static str> {
    args.iter().take(3).find_map(|arg| {
        let base = Path::new(arg).file_name()?.to_string_lossy().to_ascii_lowercase();
        let stem = base.split('.').next().unwrap_or("");
        KNOWN_AGENTS.iter().map(|(id, _)| *id).find(|id| stem == *id)
    })
}

/// Command line of a process, if this platform can read it.
pub fn command_line(pid: u32) -> Option<Vec<String>> {
    #[cfg(target_os = "linux")]
    {
        let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
        let args: Vec<String> = raw
            .split(|byte| *byte == 0)
            .filter(|part| !part.is_empty())
            .map(|part| String::from_utf8_lossy(part).to_string())
            .collect();
        (!args.is_empty()).then_some(args)
    }
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("ps").args(["-o", "args=", "-p", &pid.to_string()]).output().ok()?;
        let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!line.is_empty()).then(|| line.split_whitespace().map(str::to_string).collect())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = pid;
        None
    }
}

/// Current folder of a process, if this platform can read it.
pub fn process_dir(pid: u32) -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
    }
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("lsof").args(["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"]).output().ok()?;
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .find_map(|line| line.strip_prefix('n').map(PathBuf::from))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = pid;
        None
    }
}

// ---------------------------------------------------------------------------
// Installing the integrations
// ---------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Integration {
    pub agent: &'static str,
    pub label: &'static str,
    pub installed: bool,
    /// The agent's config folder exists, so it is probably installed for this account.
    pub found: bool,
    pub location: String,
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

fn claude_dir() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).or_else(|| home().map(|h| h.join(".claude")))
}

fn opencode_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join(".config")))
        .map(|config| config.join("opencode"))
}

fn claude_settings() -> Result<PathBuf, String> {
    claude_dir().map(|dir| dir.join("settings.json")).ok_or_else(|| "Could not find the home folder".into())
}

fn opencode_plugin() -> Result<PathBuf, String> {
    opencode_dir().map(|dir| dir.join("plugins").join("ownnas.js")).ok_or_else(|| "Could not find the home folder".into())
}

fn is_ours(hook: &Value) -> bool {
    hook.get("command").and_then(Value::as_str).is_some_and(|c| c.contains(&format!("{MARKER} claude")))
}

fn read_settings(path: &Path) -> Result<Value, String> {
    match std::fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok(json!({})),
        Ok(text) => serde_json::from_str(&text).map_err(|err| format!("{} is not valid JSON ({err}); fix it first", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(err) => Err(format!("Could not read {}: {err}", path.display())),
    }
}

fn write_file(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("Could not create {}: {err}", parent.display()))?;
    }
    let temp = path.with_extension("ownnas-tmp");
    std::fs::write(&temp, contents).map_err(|err| format!("Could not write {}: {err}", path.display()))?;
    std::fs::rename(&temp, path).map_err(|err| format!("Could not write {}: {err}", path.display()))
}

/// Removes OwnNAS hooks from Claude settings. Returns true if any were found.
fn strip_claude_hooks(settings: &mut Value) -> bool {
    let Some(hooks) = settings.get_mut("hooks").and_then(Value::as_object_mut) else { return false };
    let mut found = false;
    for groups in hooks.values_mut() {
        let Some(groups) = groups.as_array_mut() else { continue };
        for group in groups.iter_mut() {
            if let Some(list) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                let before = list.len();
                list.retain(|hook| !is_ours(hook));
                found |= list.len() != before;
            }
        }
        groups.retain(|group| group.get("hooks").and_then(Value::as_array).is_none_or(|list| !list.is_empty()));
    }
    hooks.retain(|_, groups| groups.as_array().is_none_or(|list| !list.is_empty()));
    if hooks.is_empty() {
        settings.as_object_mut().map(|object| object.remove("hooks"));
    }
    found
}

fn claude_installed() -> bool {
    claude_settings()
        .ok()
        .and_then(|path| read_settings(&path).ok())
        .and_then(|settings| settings.get("hooks").cloned())
        .and_then(|hooks| hooks.as_object().cloned())
        .is_some_and(|hooks| {
            hooks.values().filter_map(Value::as_array).flatten().any(|group| {
                group.get("hooks").and_then(Value::as_array).is_some_and(|list| list.iter().any(is_ours))
            })
        })
}

pub fn integrations() -> Vec<Integration> {
    let opencode_path = opencode_plugin().ok();
    vec![
        Integration {
            agent: "claude",
            label: "Claude Code",
            installed: claude_installed(),
            found: claude_dir().is_some_and(|dir| dir.is_dir()),
            location: claude_settings().map(|p| p.display().to_string()).unwrap_or_default(),
        },
        Integration {
            agent: "opencode",
            label: "opencode",
            installed: opencode_path
                .as_ref()
                .and_then(|path| std::fs::read_to_string(path).ok())
                .is_some_and(|text| text.contains("OWNNAS_AGENT_URL")),
            found: opencode_dir().is_some_and(|dir| dir.is_dir()),
            location: opencode_path.map(|p| p.display().to_string()).unwrap_or_default(),
        },
    ]
}

/// The command Claude Code runs for each hook. Quoted so paths with spaces work in bash.
fn hook_command(exe: &Path) -> String {
    let exe = exe.display().to_string().replace('\\', "/");
    format!("\"{exe}\" {MARKER} claude")
}

pub fn install(agent: &str, exe: &Path) -> Result<(), String> {
    match agent {
        "claude" => {
            let path = claude_settings()?;
            let mut settings = read_settings(&path)?;
            if !settings.is_object() {
                return Err(format!("{} does not hold a JSON object", path.display()));
            }
            if path.exists() && !claude_installed() {
                let backup = path.with_extension("json.before-ownnas");
                if !backup.exists() {
                    std::fs::copy(&path, &backup).map_err(|err| format!("Could not back up {}: {err}", path.display()))?;
                }
            }
            strip_claude_hooks(&mut settings);
            let hooks = settings
                .as_object_mut()
                .unwrap()
                .entry("hooks")
                .or_insert_with(|| json!({}));
            let Some(hooks) = hooks.as_object_mut() else {
                return Err(format!("\"hooks\" in {} is not an object", path.display()));
            };
            for event in CLAUDE_EVENTS {
                let groups = hooks.entry(*event).or_insert_with(|| json!([]));
                let Some(groups) = groups.as_array_mut() else {
                    return Err(format!("\"hooks.{event}\" in {} is not a list", path.display()));
                };
                groups.push(json!({
                    "hooks": [{ "type": "command", "command": hook_command(exe), "async": true, "timeout": 10 }]
                }));
            }
            let text = serde_json::to_string_pretty(&settings).map_err(|err| err.to_string())?;
            write_file(&path, &format!("{text}\n"))
        }
        "opencode" => write_file(&opencode_plugin()?, OPENCODE_PLUGIN),
        _ => Err("Unknown agent".into()),
    }
}

pub fn uninstall(agent: &str) -> Result<(), String> {
    match agent {
        "claude" => {
            let path = claude_settings()?;
            let mut settings = read_settings(&path)?;
            if strip_claude_hooks(&mut settings) {
                let text = serde_json::to_string_pretty(&settings).map_err(|err| err.to_string())?;
                write_file(&path, &format!("{text}\n"))?;
            }
            Ok(())
        }
        "opencode" => {
            let path = opencode_plugin()?;
            match std::fs::read_to_string(&path) {
                Ok(text) if text.contains("OWNNAS_AGENT_URL") => {
                    std::fs::remove_file(&path).map_err(|err| format!("Could not remove {}: {err}", path.display()))
                }
                Ok(_) => Err(format!("{} was not written by OwnNAS; remove it yourself", path.display())),
                Err(_) => Ok(()),
            }
        }
        _ => Err("Unknown agent".into()),
    }
}

// ---------------------------------------------------------------------------
// `ownnas agent-hook <agent>`: run by Claude Code for each hook event
// ---------------------------------------------------------------------------

/// Drops large fields (file contents, tool output) before an event is sent.
fn slim(mut event: Value) -> Value {
    if let Some(object) = event.as_object_mut() {
        object.remove("tool_response");
        if let Some(input) = object.get_mut("tool_input").and_then(Value::as_object_mut) {
            for key in ["content", "new_string", "old_string", "edits", "new_source"] {
                input.remove(key);
            }
        }
        for value in object.values_mut() {
            if let Value::String(text) = value {
                if text.len() > 4000 {
                    *text = text.chars().take(4000).collect();
                }
            }
        }
    }
    event
}

/// Forwards one hook event from stdin to the OwnNAS terminal it runs in. Does nothing (and
/// never fails) outside an OwnNAS terminal, so the hook is harmless everywhere else.
pub fn run_hook(agent: &str) {
    use std::io::Read;
    let (Ok(url), Ok(token)) = (std::env::var(ENV_URL), std::env::var(ENV_TOKEN)) else { return };
    let mut input = String::new();
    let _ = std::io::stdin().take(8 * 1024 * 1024).read_to_string(&mut input);
    let Ok(event) = serde_json::from_str::<Value>(&input) else { return };
    let agent_http = ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(3)).build();
    let _ = agent_http
        .post(&url)
        .set("x-ownnas-agent-token", &token)
        .send_json(json!({ "agent": agent, "event": slim(event) }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(path: &str) -> Option<String> {
        path.strip_prefix("/lib/").map(str::to_string)
    }

    #[test]
    fn claude_hooks_drive_status_files_and_attention() {
        let mut agent = Agent::default();
        agent.apply_claude(&json!({"hook_event_name": "SessionStart", "session_id": "s1", "model": "claude-opus-5-5"}), &rel);
        assert_eq!(agent.name.as_deref(), Some("claude"));
        assert_eq!(agent.status.as_deref(), Some("idle"));
        agent.apply_claude(&json!({"hook_event_name": "UserPromptSubmit", "prompt": "Fix   the\nlogin bug"}), &rel);
        assert_eq!(agent.detail.as_deref(), Some("Working on: Fix the login bug"));
        agent.apply_claude(&json!({"hook_event_name": "PreToolUse", "tool_name": "Edit", "tool_input": {"file_path": "/lib/src/a.rs"}}), &rel);
        assert_eq!(agent.detail.as_deref(), Some("Editing src/a.rs"));
        agent.apply_claude(&json!({"hook_event_name": "PostToolUse", "tool_name": "Edit", "tool_input": {"file_path": "/lib/src/a.rs"}}), &rel);
        agent.apply_claude(&json!({"hook_event_name": "PostToolUse", "tool_name": "Write", "tool_input": {"file_path": "/elsewhere/b.rs"}}), &rel);
        assert_eq!(agent.files, vec!["src/a.rs".to_string()], "only library files are listed");
        agent.apply_claude(&json!({"hook_event_name": "PermissionRequest", "tool_name": "Bash", "tool_input": {"command": "cargo test"}}), &rel);
        assert_eq!(agent.status.as_deref(), Some("waiting"));
        assert_eq!(agent.attention.as_deref(), Some("Needs permission: Running cargo test"));
        agent.apply_claude(&json!({"hook_event_name": "Stop", "last_assistant_message": "All done."}), &rel);
        assert_eq!(agent.status.as_deref(), Some("done"));
        assert_eq!(agent.last_message.as_deref(), Some("All done."));
        agent.apply_claude(&json!({"hook_event_name": "SessionEnd"}), &rel);
        assert!(agent.name.is_none());
    }

    #[test]
    fn opencode_events_drive_status() {
        let mut agent = Agent::default();
        agent.apply_opencode(&json!({"type": "tool.execute.before", "tool": "bash", "args": {"command": "npm test"}}), &rel);
        assert_eq!(agent.detail.as_deref(), Some("Running npm test"));
        agent.apply_opencode(&json!({"type": "file.edited", "properties": {"file": "/lib/web/app.js"}}), &rel);
        assert_eq!(agent.files, vec!["web/app.js".to_string()]);
        agent.apply_opencode(&json!({"type": "permission.asked", "properties": {"title": "Run rm -rf build"}}), &rel);
        assert_eq!(agent.status.as_deref(), Some("waiting"));
        agent.apply_opencode(&json!({"type": "session.idle", "properties": {"sessionID": "x"}}), &rel);
        assert_eq!(agent.status.as_deref(), Some("done"));
        assert_eq!(agent.session_id.as_deref(), Some("x"));
    }

    #[test]
    fn detection_recognizes_agents_and_their_exit() {
        let args = |line: &str| line.split(' ').map(str::to_string).collect::<Vec<_>>();
        assert_eq!(agent_from_command(&args("/home/u/.local/bin/claude --resume")), Some("claude"));
        assert_eq!(agent_from_command(&args("node /usr/lib/node_modules/@openai/codex/bin/codex.js")), Some("codex"));
        assert_eq!(agent_from_command(&args("vim notes.txt")), None);
        let mut agent = Agent::default();
        assert!(agent.detected(Some("codex"), true));
        assert_eq!(agent.status.as_deref(), Some("working"));
        assert!(agent.detected(Some("codex"), false));
        assert_eq!(agent.status.as_deref(), Some("idle"));
        assert!(!agent.detected(Some("codex"), false));
        agent.notified(None);
        assert_eq!(agent.attention.as_deref(), Some("Codex wants your attention"));
        assert!(agent.detected(None, false));
        assert!(agent.name.is_none());
        assert!(agent.attention.is_some(), "attention survives the agent exiting");
    }

    #[test]
    fn claude_hooks_install_idempotently_and_uninstall_cleanly() {
        let dir = std::env::temp_dir().join(format!("ownnas-agents-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).unwrap();
        // Only this test reads CLAUDE_CONFIG_DIR.
        std::env::set_var("CLAUDE_CONFIG_DIR", &dir);
        let original = "{\n  \"model\": \"opus\",\n  \"hooks\": {\"Stop\": [{\"hooks\": [{\"type\": \"command\", \"command\": \"say done\"}]}]}\n}\n";
        std::fs::write(dir.join("settings.json"), original).unwrap();
        let exe = Path::new("/opt/own nas/ownnas");
        install("claude", exe).unwrap();
        install("claude", exe).unwrap();
        let settings = read_settings(&dir.join("settings.json")).unwrap();
        assert_eq!(settings["hooks"]["Stop"].as_array().unwrap().len(), 2, "user hook kept, ours added once");
        assert_eq!(settings["hooks"]["Stop"][1]["hooks"][0]["command"], "\"/opt/own nas/ownnas\" agent-hook claude");
        assert_eq!(settings["hooks"]["PreToolUse"][0]["hooks"][0]["async"], true);
        let keys: Vec<_> = settings.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, vec!["model", "hooks"], "key order is preserved");
        assert!(claude_installed());
        assert_eq!(std::fs::read_to_string(dir.join("settings.json.before-ownnas")).unwrap(), original);
        uninstall("claude").unwrap();
        assert!(!claude_installed());
        let settings = read_settings(&dir.join("settings.json")).unwrap();
        assert_eq!(settings, serde_json::from_str::<Value>(original).unwrap());
        std::env::remove_var("CLAUDE_CONFIG_DIR");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn slim_drops_file_contents() {
        let event = slim(json!({"tool_input": {"file_path": "a", "content": "big"}, "tool_response": {"x": 1}}));
        assert_eq!(event, json!({"tool_input": {"file_path": "a"}}));
    }
}
