// One terminal session, shown in an iframe of the OwnNAS page or on its own in a browser tab.
import { Terminal } from "/assets/vendor/xterm/xterm.mjs";
import { FitAddon } from "/assets/vendor/xterm/addon-fit.mjs";

const THEMES = ["ownnas", "tokyo-night", "catppuccin", "catppuccin-latte", "everforest", "gruvbox", "kanagawa", "osaka-jade", "matte-black", "rose-pine"];
const KEYS = {
  esc: "\x1b",
  tab: "\t",
  "shift-tab": "\x1b[Z",
  up: "\x1b[A",
  down: "\x1b[B",
  right: "\x1b[C",
  left: "\x1b[D",
  home: "\x1b[H",
  end: "\x1b[F",
  pgup: "\x1b[5~",
  pgdn: "\x1b[6~",
  "ctrl-c": "\x03",
  pipe: "|",
  slash: "/",
  tilde: "~",
  dash: "-",
};
const FONT_KEY = "ownnas-terminal-font";

const id = new URLSearchParams(location.search).get("id") || "";
const host = document.getElementById("terminal");
const statusLine = document.getElementById("terminal-status");
const embedded = window.parent !== window;

function storedTheme() {
  try {
    const theme = localStorage.getItem("ownnas-theme");
    return THEMES.includes(theme) ? theme : "ownnas";
  } catch {
    return "ownnas";
  }
}

function themeColors() {
  const css = getComputedStyle(document.documentElement);
  const value = (name, fallback) => css.getPropertyValue(name).trim() || fallback;
  const background = value("--panel-deep", "#151810");
  return {
    background,
    foreground: value("--text", "#f4f2e8"),
    cursor: value("--accent", "#dff25a"),
    cursorAccent: background,
    selectionBackground: value("--line", "#3a4030"),
  };
}

function applyTheme() {
  document.documentElement.setAttribute("data-theme", storedTheme());
  document.body.style.background = themeColors().background;
}

function storedFontSize() {
  try {
    const size = Number(localStorage.getItem(FONT_KEY));
    if (size >= 8 && size <= 28) return size;
  } catch {}
  return window.matchMedia("(max-width: 600px)").matches ? 12 : 14;
}

applyTheme();
const term = new Terminal({
  cursorBlink: true,
  fontFamily: getComputedStyle(document.documentElement).getPropertyValue("--mono").trim() || "ui-monospace, monospace",
  fontSize: storedFontSize(),
  scrollback: 10000,
  theme: themeColors(),
});
const fit = new FitAddon();
term.loadAddon(fit);
term.open(host);
fit.fit();
// In the OwnNAS panel, take focus only if the panel gave it to this terminal (it may have
// moved on, for example to a menu, while this page was loading).
let focusOnLoad = true;
try {
  focusOnLoad = !embedded || window.parent.document.activeElement === window.frameElement;
} catch {}
if (focusOnLoad) term.focus();

window.addEventListener("storage", (event) => {
  if (event.key !== "ownnas-theme") return;
  applyTheme();
  term.options.theme = themeColors();
});

function notifyParent(message) {
  if (embedded) window.parent.postMessage({ source: "ownnas-terminal", id, ...message }, location.origin);
}

function setStatus(text) {
  statusLine.textContent = text;
  statusLine.hidden = !text;
}

let socket = null;
let finished = false;
let attempts = 0;
let ctrlArmed = false;
let altArmed = false;
const encoder = new TextEncoder();
const ctrlButton = document.querySelector('[data-key="ctrl"]');
const altButton = document.querySelector('[data-key="alt"]');

function sendBytes(bytes) {
  if (socket && socket.readyState === WebSocket.OPEN) socket.send(bytes);
}

function sendText(text) {
  sendBytes(encoder.encode(text));
}

function sendResize() {
  if (socket && socket.readyState === WebSocket.OPEN) {
    socket.send(JSON.stringify({ type: "resize", cols: term.cols, rows: term.rows }));
  }
}

function armCtrl(on) {
  ctrlArmed = on;
  ctrlButton.setAttribute("aria-pressed", on ? "true" : "false");
}

function armAlt(on) {
  altArmed = on;
  altButton.setAttribute("aria-pressed", on ? "true" : "false");
}

/// Applies the on-screen Ctrl and Alt keys (pressed once, they act on the next key) to a
/// key's input, using the xterm encodings for modified arrows and similar keys.
function withModifiers(input) {
  if (!ctrlArmed && !altArmed) return input;
  const modifier = 1 + (altArmed ? 2 : 0) + (ctrlArmed ? 4 : 0);
  let output = input;
  let match;
  if ((match = /^\x1b\[([A-DHF])$/.exec(input))) {
    output = `\x1b[1;${modifier}${match[1]}`;
  } else if ((match = /^\x1b\[(\d+)~$/.exec(input))) {
    output = `\x1b[${match[1]};${modifier}~`;
  } else if (input.length === 1) {
    output = ctrlArmed && /[a-z@\[\\\]^_ ?]/i.test(input)
      ? String.fromCharCode(input === "?" ? 0x7f : input.toUpperCase().charCodeAt(0) & 0x1f)
      : input;
    if (altArmed) output = `\x1b${output}`;
  }
  armCtrl(false);
  armAlt(false);
  return output;
}

term.onData((data) => {
  // Typed text from the on-screen keyboard picks up the sticky Ctrl and Alt keys.
  if ((ctrlArmed || altArmed) && data.length === 1) {
    sendText(withModifiers(data));
    return;
  }
  sendText(data);
});
// The page's ⇧⌘ shortcuts (Ctrl+Shift elsewhere); arrows only move between tiles while tiled.
const isMac = /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
const PAGE_SHORTCUTS = { KeyK: "shortcuts", KeyL: "terminals", KeyE: "new-terminal" };
const TILE_MOVES = { ArrowLeft: "left", ArrowRight: "right", ArrowUp: "up", ArrowDown: "down" };
let tiled = false;

function pageShortcut(event) {
  const command = isMac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
  if (!command || !event.shiftKey || event.altKey) return "";
  if (PAGE_SHORTCUTS[event.code]) return PAGE_SHORTCUTS[event.code];
  if (tiled && TILE_MOVES[event.code]) return `focus-${TILE_MOVES[event.code]}`;
  return "";
}

// Ctrl+` (the key left of 1, on any layout) hides the OwnNAS terminal panel, like VS Code.
term.attachCustomKeyEventHandler((event) => {
  if (embedded && event.ctrlKey && !event.altKey && !event.metaKey && event.code === "Backquote") {
    if (event.type === "keydown") notifyParent({ type: "toggle-panel" });
    return false;
  }
  return true;
});
// Caught on the window, not by xterm, so they work even when the terminal itself has no focus.
window.addEventListener("keydown", (event) => {
  const shortcut = embedded ? pageShortcut(event) : "";
  if (!shortcut) return;
  event.preventDefault();
  event.stopPropagation();
  if (!event.repeat) notifyParent({ type: "shortcut", name: shortcut });
}, true);
term.onBinary((data) => sendBytes(Uint8Array.from(data, (char) => char.charCodeAt(0) & 0xff)));
term.onResize(sendResize);
term.onTitleChange((title) => {
  document.title = `${title} · OwnNAS`;
  notifyParent({ type: "title", title });
});
new ResizeObserver(() => {
  if (host.clientWidth && host.clientHeight) fit.fit();
}).observe(host);
window.addEventListener("message", (event) => {
  if (event.origin !== location.origin || !event.data) return;
  if (event.data.type === "ownnas-terminal-focus") term.focus();
  if (event.data.type === "ownnas-terminal-tiled") tiled = !!event.data.on;
});
// Lets the page mark which tile has the keyboard.
term.textarea?.addEventListener("focus", () => notifyParent({ type: "focused" }));

/// Works out whether a dropped connection is worth retrying.
async function closeReason() {
  try {
    const response = await fetch("/api/terminals", { credentials: "same-origin", cache: "no-store" });
    const data = await response.json().catch(() => ({}));
    if (response.status === 401) return "You were signed out. Sign in to OwnNAS again.";
    if (!response.ok) return data.error || "This terminal is not available.";
    if (!(data.terminals || []).some((item) => item.id === id)) return "This terminal was closed.";
    return "";
  } catch {
    return "";
  }
}

function connect() {
  const scheme = location.protocol === "https:" ? "wss:" : "ws:";
  socket = new WebSocket(`${scheme}//${location.host}/api/terminals/ws?id=${encodeURIComponent(id)}`);
  socket.binaryType = "arraybuffer";
  let replay = true;
  socket.addEventListener("open", () => {
    attempts = 0;
    setStatus("");
    sendResize();
  });
  socket.addEventListener("message", (event) => {
    if (typeof event.data === "string") {
      let message = {};
      try { message = JSON.parse(event.data); } catch { return; }
      if (message.type === "reset") {
        term.reset();
      } else if (message.type === "exit") {
        finished = true;
        const code = message.code === null || message.code === undefined ? "" : ` with code ${message.code}`;
        term.write(`\r\n\x1b[2m[Process exited${code}]\x1b[0m\r\n`);
        setStatus("This terminal has ended.");
        notifyParent({ type: "exit", code: message.code });
      }
      return;
    }
    // Every connection starts with the scrollback, so a reconnect redraws from scratch.
    if (replay) {
      replay = false;
      term.reset();
    }
    term.write(new Uint8Array(event.data));
  });
  socket.addEventListener("close", async () => {
    if (finished) return;
    const reason = await closeReason();
    if (reason) {
      finished = true;
      setStatus(reason);
      return;
    }
    const delay = Math.min(10000, 500 * 2 ** attempts++);
    setStatus("Connection lost. Reconnecting…");
    setTimeout(connect, delay);
  });
}

/// Types a path the way local terminals do for pasted or dropped files.
function shellQuote(path) {
  return /^[\w@%+=:,./\\-]+$/.test(path) ? path : `'${path.replace(/'/g, "'\\''")}'`;
}

function flash(text) {
  setStatus(text);
  setTimeout(() => setStatus(""), 5000);
}

/// Uploads files to this terminal's temporary folder on the server and pastes their paths.
async function pasteFiles(files) {
  if (!files.length || finished) return;
  setStatus(files.length === 1 ? `Uploading ${files[0].name || "the file"}…` : `Uploading ${files.length} files…`);
  const paths = [];
  try {
    for (const file of files) {
      const extension = (file.type.split("/")[1] || "bin").replace(/[^a-z0-9]/gi, "");
      const name = file.name || `pasted.${extension}`;
      const response = await fetch(`/api/terminals/upload?id=${encodeURIComponent(id)}&name=${encodeURIComponent(name)}`, {
        method: "POST",
        headers: { "X-OwnNAS": "1", "Content-Type": "application/octet-stream" },
        body: file,
        credentials: "same-origin",
      });
      const data = await response.json().catch(() => ({}));
      if (!response.ok) throw new Error(data.error || "Could not upload the file");
      paths.push(shellQuote(data.path));
    }
  } catch (err) {
    flash(err.message);
    return;
  }
  setStatus("");
  term.paste(`${paths.join(" ")} `);
  term.focus();
}

// Pasting files (a screenshot, files copied in a file manager) uploads them; text pastes as usual.
document.addEventListener("paste", (event) => {
  const data = event.clipboardData;
  let files = [...((data && data.files) || [])];
  // Some browsers list pasted screenshots only as items.
  if (!files.length && data) {
    files = [...(data.items || [])].filter((item) => item.kind === "file").map((item) => item.getAsFile()).filter(Boolean);
  }
  if (!files.length) return;
  event.preventDefault();
  event.stopImmediatePropagation();
  pasteFiles(files);
}, true);
host.addEventListener("dragover", (event) => {
  if (![...(event.dataTransfer?.types || [])].includes("Files")) return;
  event.preventDefault();
  event.dataTransfer.dropEffect = "copy";
  host.classList.add("is-drop-target");
});
host.addEventListener("dragleave", () => host.classList.remove("is-drop-target"));
host.addEventListener("drop", (event) => {
  host.classList.remove("is-drop-target");
  const files = [...(event.dataTransfer?.files || [])];
  if (!files.length) return;
  event.preventDefault();
  pasteFiles(files);
});

const keys = document.getElementById("terminal-keys");
keys.hidden = !window.matchMedia("(pointer: coarse)").matches;
if (!keys.hidden) requestAnimationFrame(() => fit.fit());
keys.addEventListener("pointerdown", (event) => event.preventDefault()); // keep the keyboard open
function setFontSize(size) {
  term.options.fontSize = Math.max(8, Math.min(28, size));
  try {
    localStorage.setItem(FONT_KEY, String(term.options.fontSize));
  } catch {}
  fit.fit();
}

/// Shows the output as plain text, where the phone's own selection and copy work.
const textView = document.getElementById("terminal-text");
const textBody = document.getElementById("terminal-text-body");

function bufferText() {
  const buffer = term.buffer.active;
  const lines = [];
  for (let index = 0; index < buffer.length; index++) {
    const line = buffer.getLine(index);
    if (!line) continue;
    const text = line.translateToString(true);
    if (line.isWrapped && lines.length) lines[lines.length - 1] += text;
    else lines.push(text);
  }
  while (lines.length && !lines[lines.length - 1].trim()) lines.pop();
  return lines.join("\n");
}

function openTextView() {
  textBody.textContent = bufferText();
  textView.hidden = false;
  term.blur();
  requestAnimationFrame(() => {
    textBody.scrollTop = textBody.scrollHeight;
  });
}

textView.addEventListener("click", async (event) => {
  const action = event.target.closest("[data-text]")?.dataset.text;
  if (action === "close") {
    textView.hidden = true;
    term.focus();
  } else if (action === "copy") {
    try {
      await navigator.clipboard.writeText(textBody.textContent);
      flash("Copied");
    } catch {
      const range = document.createRange();
      range.selectNodeContents(textBody);
      getSelection().removeAllRanges();
      getSelection().addRange(range);
      flash(document.execCommand("copy") ? "Copied" : "Select the text and use your phone's Copy.");
    }
  }
});

keys.addEventListener("click", async (event) => {
  const button = event.target.closest("[data-key]");
  if (!button) return;
  const key = button.dataset.key;
  if (key === "keyboard") {
    // Hiding the keyboard leaves the whole screen for reading the output.
    if (document.activeElement === term.textarea) term.blur();
    else term.focus();
    return;
  } else if (key === "select") {
    openTextView();
    return;
  } else if (key === "smaller" || key === "larger") {
    setFontSize(term.options.fontSize + (key === "larger" ? 1 : -1));
    return;
  } else if (key === "ctrl") {
    armCtrl(!ctrlArmed);
  } else if (key === "alt") {
    armAlt(!altArmed);
  } else if (key === "paste") {
    try {
      // An image on the clipboard (such as a screenshot) is uploaded like a pasted file.
      if (navigator.clipboard.read) {
        for (const item of await navigator.clipboard.read()) {
          const type = item.types.find((kind) => kind.startsWith("image/"));
          if (type) {
            const blob = await item.getType(type);
            await pasteFiles([new File([blob], `pasted.${type.split("/")[1]}`, { type })]);
            term.focus();
            return;
          }
        }
      }
      const text = await navigator.clipboard.readText();
      if (text) term.paste(text);
    } catch {
      flash("The browser did not allow pasting here. Use the keyboard's paste instead.");
    }
  } else if (KEYS[key]) {
    sendText(withModifiers(KEYS[key]));
  }
  term.focus();
});

// On its own tab, follow the on-screen keyboard: iPhones cover the page instead of resizing it.
if (!embedded && window.visualViewport) {
  const fitViewport = () => {
    document.body.style.height = `${window.visualViewport.height}px`;
    window.scrollTo(0, 0);
  };
  window.visualViewport.addEventListener("resize", fitViewport);
  fitViewport();
}

if (id) {
  connect();
} else {
  setStatus("No terminal selected.");
}
