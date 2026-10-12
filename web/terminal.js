// One terminal session, shown in an iframe of the OwnNAS page or on its own in a browser tab.
import { Terminal } from "/assets/vendor/xterm/xterm.mjs";
import { FitAddon } from "/assets/vendor/xterm/addon-fit.mjs";

const THEMES = ["ownnas", "tokyo-night", "catppuccin", "catppuccin-latte", "everforest", "gruvbox", "kanagawa", "osaka-jade", "matte-black", "rose-pine"];
const KEYS = { esc: "\x1b", tab: "\t", up: "\x1b[A", down: "\x1b[B", right: "\x1b[C", left: "\x1b[D", "ctrl-c": "\x03" };

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

applyTheme();
const term = new Terminal({
  cursorBlink: true,
  fontFamily: getComputedStyle(document.documentElement).getPropertyValue("--mono").trim() || "ui-monospace, monospace",
  fontSize: window.matchMedia("(max-width: 600px)").matches ? 12 : 14,
  scrollback: 10000,
  theme: themeColors(),
});
const fit = new FitAddon();
term.loadAddon(fit);
term.open(host);
fit.fit();
term.focus();

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
const encoder = new TextEncoder();
const ctrlButton = document.querySelector('[data-key="ctrl"]');

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

term.onData((data) => {
  // The on-screen Ctrl key turns the next letter into its control character.
  if (ctrlArmed && data.length === 1 && /[a-z@\[\\\]^_]/i.test(data)) {
    armCtrl(false);
    sendBytes(new Uint8Array([data.toUpperCase().charCodeAt(0) & 0x1f]));
    return;
  }
  sendText(data);
});
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
  if (event.origin === location.origin && event.data && event.data.type === "ownnas-terminal-focus") term.focus();
});

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

const keys = document.getElementById("terminal-keys");
keys.hidden = !window.matchMedia("(pointer: coarse)").matches;
if (!keys.hidden) requestAnimationFrame(() => fit.fit());
keys.addEventListener("pointerdown", (event) => event.preventDefault()); // keep the keyboard open
keys.addEventListener("click", async (event) => {
  const button = event.target.closest("[data-key]");
  if (!button) return;
  const key = button.dataset.key;
  if (key === "ctrl") {
    armCtrl(!ctrlArmed);
  } else if (key === "paste") {
    try {
      const text = await navigator.clipboard.readText();
      if (text) term.paste(text);
    } catch {
      setStatus("The browser did not allow pasting here. Use the keyboard's paste instead.");
      setTimeout(() => setStatus(""), 4000);
    }
  } else if (KEYS[key]) {
    sendText(KEYS[key]);
  }
  term.focus();
});

if (id) {
  connect();
} else {
  setStatus("No terminal selected.");
}
