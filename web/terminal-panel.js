// Terminal panel for the OwnNAS page: tabs of running terminals (each an iframe of /terminal)
// plus the Terminal tab in Settings. Loaded on demand by app.js.

let ctx = null;
let panel = null;
let active = "";
const frames = new Map(); // id -> iframe
const titles = new Map(); // id -> title reported by the shell
let sessions = [];

function el(tag, attrs = {}, text = "") {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (value === false || value === undefined || value === null) continue;
    node.setAttribute(key, value === true ? "" : value);
  }
  if (text) node.textContent = text;
  return node;
}

function folderLabel(path) {
  if (!path) return ctx.rootName() || "Library";
  return path.split("/").pop();
}

function build() {
  panel = el("aside", { id: "terminals", class: "terminals", hidden: true, "aria-label": "Terminals" });
  const bar = el("div", { class: "terminals-bar" });
  const tabs = el("div", { class: "terminals-tabs", role: "tablist", "aria-label": "Open terminals" });
  const actions = el("div", { class: "terminals-actions" });
  const add = el("button", { type: "button", title: "New terminal in the same folder" }, "New");
  const popout = el("button", { type: "button", title: "Open this terminal in its own browser tab" }, "Pop out");
  const end = el("button", { type: "button", class: "danger", title: "Stop the shell and everything running in it" }, "End");
  const hide = el("button", { type: "button", title: "Hide the panel. Terminals keep running." }, "Hide");
  actions.append(add, popout, end, hide);
  bar.append(tabs, actions);
  const body = el("div", { class: "terminals-body" });
  const empty = el("p", { class: "terminals-empty muted" }, "No terminals are running. Right-click a folder and choose Open terminal here.");
  body.append(empty);
  panel.append(bar, body);
  document.body.append(panel);

  add.addEventListener("click", () => {
    const current = sessions.find((item) => item.id === active);
    openTerminal(current ? current.path : ctx.currentPath()).catch((err) => ctx.toast(err.message, true));
  });
  popout.addEventListener("click", () => {
    if (active) window.open(`/terminal?id=${encodeURIComponent(active)}`, "_blank", "noopener");
  });
  end.addEventListener("click", () => endTerminal(active).catch((err) => ctx.toast(err.message, true)));
  hide.addEventListener("click", hidePanel);
  panel.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && event.target === panel) hidePanel();
  });
  window.addEventListener("message", (event) => {
    const data = event.data;
    if (event.origin !== location.origin || !data || data.source !== "ownnas-terminal") return;
    if (data.type === "title") titles.set(data.id, data.title);
    if (data.type === "exit") {
      const item = sessions.find((session) => session.id === data.id);
      if (item) item.running = false;
    }
    renderTabs();
  });
}

function renderTabs() {
  const tabs = panel.querySelector(".terminals-tabs");
  tabs.replaceChildren(
    ...sessions.map((item) => {
      // Shell prompts set titles like "user@host: ~/path"; programs such as agents set their name.
      const title = titles.get(item.id) || "";
      const label = title && !title.includes("@") ? `${folderLabel(item.path)} · ${title}` : folderLabel(item.path);
      const tab = el("button", {
        type: "button",
        role: "tab",
        "aria-selected": item.id === active ? "true" : "false",
        class: item.running ? "" : "ended",
        title: `${item.path || "/"} — ${title || item.shell}${item.running ? "" : " (ended)"}`,
      }, label);
      tab.addEventListener("click", () => activate(item.id));
      return tab;
    }),
  );
  const has = sessions.length > 0;
  panel.querySelector(".terminals-empty").hidden = has;
  for (const button of panel.querySelectorAll(".terminals-actions button")) {
    if (button.textContent !== "Hide") button.disabled = !has;
  }
}

function activate(id) {
  active = id;
  for (const [frameId, frame] of frames) frame.hidden = frameId !== id;
  if (id && !frames.has(id)) {
    const frame = el("iframe", { src: `/terminal?id=${encodeURIComponent(id)}`, title: "Terminal", class: "terminals-frame" });
    frames.set(id, frame);
    panel.querySelector(".terminals-body").append(frame);
  }
  renderTabs();
  const frame = frames.get(id);
  if (frame) {
    requestAnimationFrame(() => {
      frame.focus();
      frame.contentWindow?.postMessage({ type: "ownnas-terminal-focus" }, location.origin);
    });
  }
}

async function refresh() {
  const data = await withUnlock(() => ctx.api("/api/terminals"));
  if (!data) return false;
  sessions = data.terminals || [];
  for (const [id, frame] of frames) {
    if (!sessions.some((item) => item.id === id)) {
      frame.remove();
      frames.delete(id);
      titles.delete(id);
    }
  }
  if (!sessions.some((item) => item.id === active)) active = sessions.length ? sessions[sessions.length - 1].id : "";
  return true;
}

/// Runs `request`; if terminals are locked, asks for the terminal password and retries once.
async function withUnlock(request) {
  try {
    return await request();
  } catch (err) {
    if (!err.data || !err.data.locked) throw err;
  }
  const password = await ctx.askText("Terminal password", "Password", "", "Unlock", { password: true });
  if (password === null) return null;
  await ctx.api("/api/terminal/unlock", { method: "POST", json: { password } });
  return request();
}

function showPanel() {
  panel.hidden = false;
  document.body.classList.add("terminals-open");
  activate(active);
}

function hidePanel() {
  panel.hidden = true;
  document.body.classList.remove("terminals-open");
}

async function openTerminal(path) {
  const data = await withUnlock(() => ctx.api("/api/terminals", { method: "POST", json: { path: path || "", cols: 100, rows: 30 } }));
  if (!data) return;
  await refresh();
  showPanel();
  activate(data.terminal.id);
}

async function endTerminal(id) {
  const item = sessions.find((session) => session.id === id);
  if (!item) return;
  if (item.running && !(await ctx.askConfirm(`End the terminal in ${item.path || "/"}? Programs running in it are stopped.`, "End terminal"))) return;
  await ctx.api(`/api/terminals?id=${encodeURIComponent(id)}`, { method: "DELETE" });
  await refresh();
  if (!sessions.length) hidePanel();
  else activate(active);
}

export function init(context) {
  ctx = context;
  if (!panel) build();
}

export async function open(path) {
  await openTerminal(path);
}

export async function show() {
  if (await refresh()) showPanel();
}

/// Fills the Terminal tab of Settings.
export async function renderSettings(section) {
  const config = await ctx.api("/api/terminal/config");
  section.replaceChildren();
  section.append(el("p", { class: "muted" }, "Open a shell in any folder from the right-click menu, for example to run Claude Code or opencode on these files. Terminals run as the account OwnNAS runs under and keep running when you close the page."));
  if (!config.available) {
    section.append(el("p", { class: "form-error" }, "Terminals are not available on this server (it is read-only or running on Android)."));
    return;
  }
  if (!config.secureTransport) {
    section.append(el("p", { class: "text-warning" }, "You are connected over plain HTTP from outside your network, so terminals will refuse to open. Use HTTPS or your VPN."));
  }
  const form = el("form", { class: "user-form terminal-settings" });
  const enabled = el("input", { type: "checkbox", id: "terminal-enabled" });
  enabled.checked = config.enabled;
  const enabledLabel = el("label", { class: "check" });
  enabledLabel.append(enabled, " Allow terminals (administrators only)");
  const shell = el("input", { id: "terminal-shell", autocomplete: "off", placeholder: config.defaultShell, value: config.shell || "" });
  const shellLabel = el("label", {}, "Shell");
  shellLabel.append(shell);
  const status = el("p", { class: "muted" }, config.passwordSet
    ? "A terminal password is set. It is asked once per sign-in, for up to 12 hours."
    : "No terminal password is set. Signing in to OwnNAS is enough to open a terminal.");
  const password = el("input", { type: "password", id: "terminal-password", autocomplete: "new-password", minlength: "8", placeholder: config.passwordSet ? "Leave empty to keep it" : "Optional" });
  const passwordLabel = el("label", {}, config.passwordSet ? "New terminal password" : "Terminal password");
  passwordLabel.append(password);
  const remove = el("input", { type: "checkbox", id: "terminal-password-remove" });
  const removeLabel = el("label", { class: "check", hidden: !config.passwordSet });
  removeLabel.append(remove, " Remove the terminal password");
  const account = el("input", { type: "password", id: "terminal-account-password", autocomplete: "current-password", required: true });
  const accountLabel = el("label", {}, "Your account password (required to save)");
  accountLabel.append(account);
  const save = el("button", { type: "submit" }, "Save terminal settings");
  const error = el("p", { class: "form-error" });
  form.append(enabledLabel, shellLabel, status, passwordLabel, removeLabel, accountLabel, save, error);
  section.append(form);
  if (config.passwordSet && config.unlocked) {
    const lock = el("button", { type: "button" }, "Lock terminals now");
    lock.addEventListener("click", async () => {
      await ctx.api("/api/terminal/lock", { method: "POST" });
      await renderSettings(section);
    });
    section.append(lock);
  }
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    error.textContent = "";
    const body = { accountPassword: account.value, enabled: enabled.checked, shell: shell.value };
    if (remove.checked) body.password = "";
    else if (password.value) body.password = password.value;
    try {
      await ctx.api("/api/terminal/config", { method: "POST", json: body });
      ctx.toast("Terminal settings saved");
      if (!enabled.checked) {
        sessions = [];
        for (const frame of frames.values()) frame.remove();
        frames.clear();
        hidePanel();
      }
      await ctx.refreshMe();
      await renderSettings(section);
    } catch (err) {
      error.textContent = err.message;
    }
  });
}
