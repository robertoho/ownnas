// Terminal panel for the OwnNAS page: tabs of running terminals (each an iframe of /terminal),
// what the coding agent in each one is doing, an overview of all of them, notifications,
// and the Terminal tab in Settings. Loaded on demand by app.js.

let ctx = null;
let panel = null;
let active = "";
let view = "terminal"; // or "overview"
let drawer = ""; // "", "changes", or "files"
let sessions = [];
let stream = null;
let streamRetry = 0;
let streamTimer = 0;
const frames = new Map(); // id -> tile: a header and the terminal's iframe
let tiled = false; // while showing on a wide screen: the files beside a grid of every terminal
let focusOnLoad = ""; // a terminal to give the keyboard once its page has loaded
let maximized = ""; // a terminal (or "w:<window>") expanded to fill the whole grid, in the tiling layout
const windows = new Map(); // id -> { tile, onClose }: other windows tiled with the terminals, such as Settings
const STASHED_KEY = "ownnas-terminals-hidden";
// Terminals put away with ⇧⌘U: still running and in the tabs, but without a tile.
const stashed = new Set(loadStashed());

function loadStashed() {
  try {
    const list = JSON.parse(localStorage.getItem(STASHED_KEY) || "[]");
    return Array.isArray(list) ? list.filter((id) => typeof id === "string") : [];
  } catch {
    return [];
  }
}

function saveStashed() {
  try {
    localStorage.setItem(STASHED_KEY, JSON.stringify([...stashed]));
  } catch {}
}
const shownAttention = new Map(); // id -> attention text already announced

const STATUS_WORDS = {
  working: "Working",
  waiting: "Needs you",
  idle: "Idle",
  done: "Finished",
  error: "Stopped",
};

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

function tabLabel(item) {
  const folder = item.name || folderLabel(item.path);
  if (item.agent && item.agent.label) return `${folder} · ${item.agent.label}`;
  // Shell prompts set titles like "user@host: ~/path"; other programs set their name.
  if (item.title && !item.title.includes("@")) return `${folder} · ${item.title}`;
  return folder;
}

function statusOf(item) {
  if (!item.running) return "ended";
  return (item.agent && item.agent.status) || "shell";
}

function panelVisible() {
  return panel && !panel.hidden && !document.hidden;
}

function build() {
  panel = el("aside", { id: "terminals", class: "terminals", hidden: true, "aria-label": "Terminals" });
  const bar = el("div", { class: "terminals-bar" });
  const overview = el("button", { type: "button", class: "terminals-overview-btn", "aria-pressed": "false", title: "All terminals and their agents" }, "Overview");
  const tabs = el("div", { class: "terminals-tabs", role: "tablist", "aria-label": "Open terminals" });
  const actions = el("div", { class: "terminals-actions" });
  const add = el("button", { type: "button", "data-action": "new", title: "New terminal in the folder you are browsing" }, "New");
  const hide = el("button", { type: "button", "data-action": "hide", title: `Hide the panel (${ctx.shortcutLabel("L")} or Ctrl+\`). Terminals keep running.` }, "Hide");
  // On phones the other buttons move into this menu, leaving one row for the tabs.
  const more = el("button", { type: "button", "data-action": "more", class: "terminals-more-btn", "aria-label": "More terminal actions", "aria-haspopup": "menu" }, "⋯");
  // Pop out, Git changes, and Show folder are on each terminal's own title bar (and in the menu
  // on phones, where there are no tiles).
  actions.append(add, more, hide);
  bar.append(overview, tabs, actions);

  const strip = el("div", { class: "terminals-agent", hidden: true });
  const drawerBox = el("div", { class: "terminals-drawer", hidden: true });
  const body = el("div", { class: "terminals-body" });
  const empty = el("p", { class: "terminals-empty muted" }, "No terminals are running. Choose New to open one in this folder, or right-click a folder and choose Open terminal here.");
  const grid = el("div", { class: "terminals-overview", hidden: true });
  body.append(empty, grid);
  const resize = el("div", { class: "terminals-resize", role: "separator", "aria-orientation": "vertical", "aria-label": "Resize the terminals" });
  panel.append(resize, bar, strip, drawerBox, body);
  document.body.append(panel);

  overview.addEventListener("click", () => setView(view === "overview" ? "terminal" : "overview"));
  add.addEventListener("click", () => openTerminal(ctx.currentPath()).catch((err) => ctx.toast(err.message, true)));
  hide.addEventListener("click", hidePanel);
  more.addEventListener("click", () => {
    const rect = more.getBoundingClientRect();
    openTabMenu(active, rect.right, rect.bottom + 4, { full: true });
  });
  if (window.visualViewport) {
    window.visualViewport.addEventListener("resize", fitToViewport);
    window.visualViewport.addEventListener("scroll", fitToViewport);
  }
  resize.addEventListener("pointerdown", startResize);
  // The best grid depends on the space: work it out again when the panel changes size.
  new ResizeObserver(() => {
    if (tiled && view === "terminal") syncFrames();
  }).observe(body);
  // Phones get the full-screen panel with one terminal at a time instead of tiles.
  narrow.addEventListener("change", () => {
    if (narrow.matches) for (const win of [...windows.values()]) win.onClose();
    if (!panel.hidden) setTiled(!narrow.matches);
  });
  // When the page itself takes the keyboard back, no terminal tile has it.
  window.addEventListener("focus", () => markFocused(""));
  panel.addEventListener("click", (event) => {
    const file = event.target.closest("[data-open-file]");
    if (file) {
      hidePanel();
      ctx.openFile(file.dataset.openFile);
      return;
    }
    const card = event.target.closest("[data-card]");
    if (card) {
      setView("terminal");
      activate(card.dataset.card);
    }
  });
  window.addEventListener("message", (event) => {
    if (event.origin !== location.origin || !event.data || event.data.source !== "ownnas-terminal") return;
    if (event.data.type === "toggle-panel") togglePanel();
    if (event.data.type === "exit" && event.data.id) {
      const item = sessions.find((session) => session.id === event.data.id);
      if (item) cleanEnded([{ ...item, running: false, exitCode: event.data.code }]);
    }
    if (event.data.type === "shortcut") ctx.runShortcut(event.data.name, "terminal");
    if (event.data.type === "focused" && event.data.id) {
      markFocused(event.data.id);
      if (event.data.id !== active && sessions.some((item) => item.id === event.data.id)) {
        active = event.data.id;
        render();
        syncFrames();
      }
    }
  });
  document.addEventListener("keydown", (event) => {
    if (event.ctrlKey && !event.altKey && !event.metaKey && event.code === "Backquote" && !event.repeat) {
      event.preventDefault();
      togglePanel();
    }
  }, true);
  document.addEventListener("visibilitychange", () => {
    const item = sessions.find((session) => session.id === active);
    if (panelVisible() && view === "terminal" && item && item.agent && item.agent.attention) markSeen(item.id);
  });
}

function renderTabs() {
  const tabs = panel.querySelector(".terminals-tabs");
  tabs.replaceChildren(
    ...sessions.map((item) => {
      const status = statusOf(item);
      const attention = item.agent && item.agent.attention;
      const tab = el("button", {
        type: "button",
        role: "tab",
        "aria-selected": view === "terminal" && item.id === active ? "true" : "false",
        "data-tab": item.id,
        class: [item.running ? "" : "ended", attention ? "has-attention" : "", stashed.has(item.id) ? "stashed" : ""].join(" ").trim() || null,
        title: `${item.path || "/"}${item.agent && item.agent.detail ? ` — ${item.agent.detail}` : ""}${attention ? `\n${attention}` : ""}`,
      });
      tab.append(el("span", { class: "agent-dot", "data-status": status, "aria-hidden": "true" }), el("span", {}, tabLabel(item)));
      const number = sessions.indexOf(item) + 1;
      if (number <= 9) {
        tab.append(el("span", { class: "terminals-number", "aria-hidden": "true" }, String(number)));
        tab.title += `\n${ctx.selectLabel(number)}`;
      }
      let pressTimer = 0;
      let pressed = false;
      let pressStart = null;
      // Long-press opens the tab menu on touch screens (iPhones have no context menu).
      tab.addEventListener("pointerdown", (event) => {
        if (event.pointerType !== "touch") return;
        pressStart = { x: event.clientX, y: event.clientY };
        pressTimer = setTimeout(() => {
          pressTimer = 0;
          pressed = true;
          const rect = tab.getBoundingClientRect();
          openTabMenu(item.id, rect.left, rect.bottom + 4);
        }, 500);
      });
      const cancelPress = () => {
        clearTimeout(pressTimer);
        pressTimer = 0;
      };
      tab.addEventListener("pointermove", (event) => {
        if (pressStart && Math.hypot(event.clientX - pressStart.x, event.clientY - pressStart.y) > 10) cancelPress();
      });
      tab.addEventListener("pointerup", cancelPress);
      tab.addEventListener("pointercancel", cancelPress);
      tab.addEventListener("click", (event) => {
        if (pressed) {
          pressed = false;
          event.preventDefault();
          return;
        }
        setView("terminal");
        activate(item.id);
      });
      tab.addEventListener("contextmenu", (event) => {
        event.preventDefault();
        if (pressed || pressTimer) return; // the long-press already handles touch
        const rect = tab.getBoundingClientRect();
        // The keyboard's menu key reports (0, 0); open under the tab instead.
        const fromKeyboard = event.clientX === 0 && event.clientY === 0;
        openTabMenu(item.id, fromKeyboard ? rect.left : event.clientX, fromKeyboard ? rect.bottom + 4 : event.clientY);
      });
      return tab;
    }),
  );
  const has = sessions.length > 0;
  const empty = panel.querySelector(".terminals-empty");
  const allStashed = has && sessions.every((item) => stashed.has(item.id));
  empty.hidden = (has && !allStashed) || view === "overview" || windows.size > 0;
  empty.textContent = allStashed
    ? "Every terminal is hidden. They keep running: choose one in the tabs to show it again."
    : "No terminals are running. Choose New to open one in this folder, or right-click a folder and choose Open terminal here.";
  panel.querySelector(".terminals-overview-btn").setAttribute("aria-pressed", view === "overview" ? "true" : "false");
}

/// The line under the tabs: which agent runs here and what it is doing.
function renderStrip() {
  const strip = panel.querySelector(".terminals-agent");
  const item = sessions.find((session) => session.id === active);
  if (!item || view !== "terminal") {
    strip.hidden = true;
    panel.querySelector(".terminals-drawer").hidden = true;
    return;
  }
  const agent = item.agent || {};
  const status = statusOf(item);
  strip.hidden = false;
  // A plain shell has nothing to report; phones hide the line to save room.
  strip.classList.toggle("is-plain", item.running && !agent.name && !agent.attention);
  const line = el("div", { class: "terminals-agent-line" });
  line.append(el("span", { class: "agent-dot", "data-status": status, "aria-hidden": "true" }));
  if (!item.running) {
    line.append(el("span", {}, `This terminal has ended${item.exitCode !== undefined ? ` (exit code ${item.exitCode})` : ""}.`));
  } else if (agent.name) {
    line.append(el("strong", {}, agent.label || agent.name), el("span", { class: "agent-status" }, STATUS_WORDS[agent.status] || ""));
    if (agent.detail) line.append(el("span", { class: "agent-detail" }, agent.detail));
    if (!agent.hooked && ["claude", "opencode"].includes(agent.name)) {
      const hint = el("button", { type: "button", class: "linkish", title: "Show what it is doing, step by step" }, "More detail…");
      hint.addEventListener("click", () => ctx.openSettings("terminal"));
      line.append(hint);
    }
  } else {
    line.append(el("span", { class: "muted" }, `${item.shell} in ${item.path || "/"}`));
  }
  const tools = el("div", { class: "terminals-agent-tools" });
  const edited = (agent.files || []).length;
  const files = el("button", { type: "button", "aria-pressed": drawer === "files" ? "true" : "false", hidden: !edited }, `Edited (${edited})`);
  files.addEventListener("click", () => toggleDrawer("files"));
  tools.append(files);
  // Nothing to say about a plain shell: its tile's title bar has the rest.
  strip.classList.toggle("is-empty", item.running && !agent.name && !agent.attention && !edited);
  for (const [id, tile] of frames) {
    tile.querySelector('[data-tile="changes"]')?.setAttribute("aria-pressed", String(drawer === "changes" && id === active));
  }
  const rows = [line];
  if (agent.attention) rows.push(el("p", { class: "agent-attention" }, agent.attention));
  else if (agent.status === "done" && agent.lastMessage) rows.push(el("p", { class: "agent-message muted" }, agent.lastMessage));
  const text = el("div", { class: "terminals-agent-text" });
  text.append(...rows);
  strip.replaceChildren(text, tools);
  if (drawer === "files") renderFiles(item);
}

function fileButton(path, label = path) {
  return el("button", { type: "button", class: "linkish", "data-open-file": path, title: `Open ${path}` }, label);
}

function renderFiles(item) {
  const box = panel.querySelector(".terminals-drawer");
  const files = (item.agent && item.agent.files) || [];
  box.hidden = false;
  const list = el("ul", { class: "terminals-files" });
  for (const path of files) {
    const row = el("li");
    row.append(fileButton(path));
    list.append(row);
  }
  box.replaceChildren(el("p", { class: "muted" }, "Files the agent edited in this session, newest first."), list);
}

async function renderChanges(item) {
  await fillChanges(panel.querySelector(".terminals-drawer"), item);
}

let drawerTall = false;

/// What a status code from `git status` means, for its colour and its words.
function gitKind(status) {
  if (status === "new" || status.includes("A")) return ["added", status === "new" ? "new" : "added"];
  if (status.includes("D")) return ["deleted", "deleted"];
  if (status.includes("R") || status.includes("C")) return ["renamed", "renamed"];
  if (status.includes("U")) return ["conflict", "conflict"];
  return ["modified", "changed"];
}

/// A diff with its lines coloured: added, removed, where each change is, and Git's own lines.
function paintDiff(pre, text, truncated) {
  pre.replaceChildren();
  for (const line of text.replace(/\n$/, "").split("\n")) {
    let kind = "";
    if (line.startsWith("+++") || line.startsWith("---") || /^(diff --git|index |new file|deleted file|similarity|rename |old mode|new mode|Binary files)/.test(line)) kind = "diff-meta";
    else if (line.startsWith("@@")) kind = "diff-hunk";
    else if (line.startsWith("+")) kind = "diff-add";
    else if (line.startsWith("-")) kind = "diff-del";
    else if (line.startsWith("\\")) kind = "diff-meta";
    pre.append(el("span", kind ? { class: kind } : {}, line || " "), "\n");
  }
  if (truncated) pre.append(el("span", { class: "diff-meta" }, "… (too long to show it all)"));
}

/// Fills `box` with the Git changes of terminal `item`: a summary with its buttons, then each
/// changed file with its added and removed lines, which opens to show its diff in colour. `popup`
/// is the window it is in when popped out.
async function fillChanges(box, item, popup = null) {
  box.replaceChildren(el("p", { class: "muted" }, "Reading Git status…"));
  let data;
  try {
    data = await ctx.api(`/api/terminals/changes?id=${encodeURIComponent(item.id)}`);
  } catch (err) {
    box.replaceChildren(el("p", { class: "form-error" }, err.message));
    return;
  }
  if (!popup && (drawer !== "changes" || active !== item.id)) return;
  const tool = (label, title, run) => {
    const button = el("button", { type: "button", class: "terminals-drawer-btn", title }, label);
    button.addEventListener("click", run);
    return button;
  };
  // The summary, with the buttons at its right, on one line.
  const headed = (text) => {
    const row = el("div", { class: "terminals-drawer-head" });
    row.append(el("p", { class: "muted" }, text));
    if (!popup) {
      const grow = tool(drawerTall ? "Smaller" : "Expand", drawerTall ? "Make the changes take less room" : "Give the changes more room", () => {
        drawerTall = !drawerTall;
        box.classList.toggle("is-tall", drawerTall);
        grow.textContent = drawerTall ? "Smaller" : "Expand";
      });
      row.append(grow, tool("Pop out", "Open the changes in a window of their own", () => popOutChanges(item)));
    }
    row.append(tool("Refresh", "Read the changes again", () => fillChanges(box, item, popup)));
    return row;
  };
  box.classList.toggle("is-tall", !popup && drawerTall);
  if (popup) popup.document.title = `Git changes · ${data.repoName || tabLabel(item)}`;
  if (!data.repo) {
    box.replaceChildren(headed("The folder this terminal is in is not in a Git repository."));
    return;
  }
  const head = headed([
    data.repoName ? `${data.repoName}${data.branch ? `, on ${data.branch}.` : "."}` : data.branch ? `On ${data.branch}.` : "",
    data.total ? `${data.total} changed ${data.total === 1 ? "file" : "files"}${data.summary ? ` (${data.summary})` : ""}.` : "No uncommitted changes.",
  ].join(" ").trim());
  const list = el("ul", { class: "terminals-files git-files", hidden: !data.files.length });
  for (const file of data.files) {
    const [kind, words] = gitKind(file.status);
    const row = el("li", { class: "git-file" });
    const line = el("div", { class: "git-row" });
    const toggle = el("button", { type: "button", class: "git-toggle", "aria-expanded": "false", title: "Show what changed", "aria-label": `Show what changed in ${file.name}` }, "▸");
    line.append(toggle, el("span", { class: `git-status git-${kind}`, title: words }, file.status === "new" ? "new" : file.status));
    // A deleted file has nothing to open.
    line.append(file.path && kind !== "deleted" ? fileButton(file.path, file.name) : el("span", { class: "git-name" }, file.name));
    const stats = el("span", { class: "git-stats" });
    if (file.added !== null && file.added !== undefined && !(kind === "deleted" && !file.added)) stats.append(el("span", { class: "git-add" }, `+${file.added}`));
    if (file.deleted !== null && file.deleted !== undefined && file.deleted > 0) stats.append(el("span", { class: "git-del" }, `−${file.deleted}`));
    if ((file.added === null || file.added === undefined) && kind !== "deleted") stats.append(el("span", { class: "muted" }, "binary"));
    line.append(stats);
    const pre = el("pre", { class: "git-diff", hidden: true });
    let loaded = false;
    const open = async () => {
      const show = pre.hidden;
      pre.hidden = !show;
      toggle.textContent = show ? "▾" : "▸";
      toggle.setAttribute("aria-expanded", String(show));
      if (!show || loaded) return;
      pre.replaceChildren(el("span", { class: "muted" }, "Reading…"));
      try {
        const diff = await ctx.api(`/api/terminals/diff?id=${encodeURIComponent(item.id)}&file=${encodeURIComponent(file.name)}`);
        paintDiff(pre, diff.diff || "(no differences)", diff.truncated);
        loaded = true;
      } catch (err) {
        pre.replaceChildren(el("span", { class: "form-error" }, err.message));
      }
    };
    toggle.addEventListener("click", open);
    stats.addEventListener("click", open);
    row.append(line, pre);
    list.append(row);
  }
  box.replaceChildren(head, list);
}

/// The changes in a window of their own, kept up to date by this page (Refresh, file links).
function popOutChanges(item) {
  const win = window.open("", `ownnas-changes-${item.id}`, "popup,width=980,height=780");
  if (!win) {
    ctx.toast("The browser blocked the new window", true);
    return;
  }
  const doc = win.document;
  let root = doc.getElementById("changes-root");
  if (!root) {
    doc.documentElement.setAttribute("data-theme", document.documentElement.getAttribute("data-theme") || "ownnas");
    doc.documentElement.setAttribute("data-mode", document.documentElement.getAttribute("data-mode") || "dark");
    const style = doc.createElement("link");
    style.rel = "stylesheet";
    style.href = new URL("/assets/app.css", location.href).href;
    doc.head.append(style);
    doc.body.className = "changes-window";
    root = doc.createElement("div");
    root.id = "changes-root";
    root.className = "terminals-drawer is-window";
    doc.body.append(root);
    root.addEventListener("click", (event) => {
      const file = event.target.closest("[data-open-file]");
      if (!file) return;
      ctx.openFile(file.dataset.openFile);
      window.focus();
    });
  }
  fillChanges(root, item, win);
  win.focus();
}

function toggleDrawer(kind) {
  drawer = drawer === kind ? "" : kind;
  const box = panel.querySelector(".terminals-drawer");
  box.hidden = !drawer;
  const item = sessions.find((session) => session.id === active);
  renderStrip();
  if (drawer === "changes" && item) renderChanges(item);
}

function renderOverview() {
  const grid = panel.querySelector(".terminals-overview");
  grid.hidden = view !== "overview";
  syncFrames();
  if (view !== "overview") return;
  const cards = sessions.map((item) => {
    const agent = item.agent || {};
    const card = el("article", { class: `agent-card${agent.attention ? " has-attention" : ""}` });
    const open = el("button", { type: "button", class: "agent-card-open", "data-card": item.id });
    open.append(el("span", { class: "agent-dot", "data-status": statusOf(item), "aria-hidden": "true" }), el("strong", {}, tabLabel(item)));
    card.append(open);
    const state = !item.running ? "Ended" : agent.name ? `${STATUS_WORDS[agent.status] || "Running"}${agent.hooked ? "" : " (detected)"}` : "Shell";
    card.append(el("p", { class: "muted" }, `${item.path || "/"} · ${state}`));
    if (agent.detail) card.append(el("p", {}, agent.detail));
    if (agent.attention) card.append(el("p", { class: "agent-attention" }, agent.attention));
    if (agent.lastMessage && agent.status === "done") card.append(el("p", { class: "agent-message muted" }, agent.lastMessage));
    if (agent.files && agent.files.length) {
      const files = el("p", { class: "agent-card-files" });
      files.append(el("span", { class: "muted" }, "Edited: "));
      agent.files.slice(0, 5).forEach((path, index) => {
        if (index) files.append(", ");
        files.append(fileButton(path, path.split("/").pop()));
      });
      if (agent.files.length > 5) files.append(el("span", { class: "muted" }, ` and ${agent.files.length - 5} more`));
      card.append(files);
    }
    return card;
  });
  const head = el("div", { class: "agent-overview-head" });
  head.append(el("p", { class: "muted" }, sessions.length ? `${sessions.length} ${sessions.length === 1 ? "terminal" : "terminals"}` : "No terminals are running."));
  if (window.isSecureContext && "Notification" in window && Notification.permission === "default") {
    const enable = el("button", { type: "button" }, "Notify me when an agent needs me");
    enable.addEventListener("click", async () => {
      await Notification.requestPermission();
      renderOverview();
    });
    head.append(enable);
  }
  grid.replaceChildren(head, ...cards);
}

function render() {
  renderTabs();
  renderStrip();
  renderOverview();
  ctx.setBadge(sessions.filter((item) => item.agent && item.agent.attention).length);
}

function setView(next) {
  view = next;
  if (view === "terminal") activate(active);
  else render();
}

function activate(id) {
  active = id;
  if (id && stashed.delete(id)) saveStashed();
  if (maximized && id) maximized = id;
  if (id) tileFor(id);
  syncFrames();
  const item = sessions.find((session) => session.id === id);
  if (drawer === "changes" && item) renderChanges(item);
  if (item && item.agent && item.agent.attention && panelVisible()) markSeen(id);
  render();
  if (id && view === "terminal") {
    focusOnLoad = id;
    requestAnimationFrame(() => focusTerminal(id));
  }
}

/// The tile showing terminal `id`, made the first time it is needed.
function tileFor(id) {
  let tile = frames.get(id);
  if (tile) return tile;
  tile = el("div", { class: "terminals-tile", "data-id": id });
  const head = el("div", { class: "terminals-tile-head" });
  const title = el("button", { type: "button", class: "terminals-tile-title", tabindex: "-1" });
  title.addEventListener("click", () => {
    setView("terminal");
    activate(id);
  });
  title.addEventListener("dblclick", () => toggleMaximized(id));
  const tool = (name, label, svg, run) => {
    const button = el("button", { type: "button", class: "terminals-tile-btn", "data-tile": name, tabindex: "-1", title: label, "aria-label": label });
    button.innerHTML = `<svg viewBox="0 0 16 16" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${svg}</svg>`;
    button.addEventListener("click", run);
    return button;
  };
  const changes = tool("changes", "Git changes: what is not committed in this folder's repository",
    '<circle cx="4.5" cy="3.5" r="1.5"/><circle cx="4.5" cy="12.5" r="1.5"/><circle cx="11.5" cy="4.5" r="1.5"/><path d="M4.5 5v6M11.5 6c0 3-3 3.5-6.2 5.3"/>',
    () => {
      setView("terminal");
      activate(id);
      toggleDrawer("changes");
    });
  const folder = tool("folder", "Show the terminal's current folder in the files",
    '<path d="M2 4.5A1.5 1.5 0 0 1 3.5 3H7l1.2 1.5h4.3A1.5 1.5 0 0 1 14 6v6.5a1.5 1.5 0 0 1-1.5 1.5h-9A1.5 1.5 0 0 1 2 12.5z"/>',
    () => showFolder(id).catch((err) => ctx.toast(err.message, true)));
  const popout = tool("popout", "Pop out: open this terminal in its own browser tab",
    '<path d="M9.5 2.5h4v4M13.5 2.5 8 8M12 9.5v3a1 1 0 0 1-1 1H3.5a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1h3"/>',
    () => window.open(`/terminal?id=${encodeURIComponent(id)}`, "_blank", "noopener"));
  const expand = el("button", { type: "button", class: "terminals-tile-btn", "data-tile": "expand", tabindex: "-1" });
  expand.addEventListener("click", () => toggleMaximized(id));
  const close = el("button", {
    type: "button",
    class: "terminals-tile-btn",
    tabindex: "-1",
    title: `Hide (${ctx.shortcutLabel("U")}). It keeps running, and its tab brings it back; ${ctx.shortcutLabel("X")} closes it for good.`,
    "aria-label": "Hide this terminal",
  }, "✕");
  close.addEventListener("click", () => stashTerminal(id));
  head.append(title, changes, folder, popout, expand, close);
  const frame = el("iframe", { src: `/terminal?id=${encodeURIComponent(id)}`, title: "Terminal", class: "terminals-frame is-loading", "data-id": id });
  // A page is white until its styles load; show the panel's background meanwhile.
  frame.addEventListener("load", () => {
    frame.classList.remove("is-loading");
    frame.contentWindow?.postMessage({ type: "ownnas-terminal-tiled", on: tiled }, location.origin);
    // Focusing a frame before its page loads does not always stick.
    if (focusOnLoad === id && id === active && panelVisible()) focusTerminal(id);
  }, { once: true });
  tile.append(head, frame);
  frames.set(id, tile);
  panel.querySelector(".terminals-body").append(tile);
  return tile;
}

function focusTerminal(id) {
  const frame = frames.get(id)?.querySelector("iframe");
  if (!frame) return;
  frame.focus();
  frame.contentWindow?.postMessage({ type: "ownnas-terminal-focus" }, location.origin);
}

/// Shows the active terminal, or every terminal as tiles; labels the tiles.
function syncFrames() {
  if (!panel) return;
  const body = panel.querySelector(".terminals-body");
  const windowMaximized = maximized.startsWith("w:");
  if (windowMaximized ? !windows.has(maximized.slice(2)) : maximized && (stashed.has(maximized) || !sessions.some((item) => item.id === maximized))) maximized = "";
  // An expanded tile has the grid to itself; the others keep running behind it.
  const shown = sessions.filter((item) => !stashed.has(item.id) && (!maximized || item.id === maximized));
  const shownWindows = [...windows].filter(([id]) => !maximized || maximized === `w:${id}`);
  if (tiled && view === "terminal") {
    for (const item of shown) tileFor(item.id);
    // Windows come first, so Settings opens at the top left.
    layoutTiles([...shownWindows.map(([, win]) => win.tile), ...shown.map((item) => frames.get(item.id))]);
  }
  for (const [id, win] of windows) {
    win.tile.hidden = !tiled || view !== "terminal" || !shownWindows.some(([shownId]) => shownId === id);
    const expand = win.tile.querySelector('[data-tile="expand"]');
    const big = maximized === `w:${id}`;
    expand.textContent = big ? "⤡" : "⤢";
    expand.title = big ? "Back to the grid" : "Fill the whole space";
    expand.setAttribute("aria-pressed", big ? "true" : "false");
  }
  for (const [id, tile] of frames) {
    tile.hidden = view !== "terminal" || (tiled ? !shown.some((item) => item.id === id) : id !== active);
    // A hidden terminal would keep the keyboard and swallow every key.
    if (tile.hidden && tile.contains(document.activeElement)) document.activeElement.blur();
    tile.classList.toggle("is-active", id === active);
    const item = sessions.find((session) => session.id === id);
    if (!item) continue;
    const title = tile.querySelector(".terminals-tile-title");
    const number = sessions.indexOf(item) + 1;
    title.replaceChildren(
      el("span", { class: "agent-dot", "data-status": statusOf(item), "aria-hidden": "true" }),
      el("span", { class: "terminals-tile-name" }, tabLabel(item)),
      ...(number <= 9 ? [el("span", { class: "terminals-number", title: ctx.selectLabel(number) }, String(number))] : []),
    );
    title.title = item.path || "/";
    const expand = tile.querySelector('[data-tile="expand"]');
    const big = id === maximized;
    expand.textContent = big ? "⤡" : "⤢";
    expand.title = `${big ? "Back to the grid" : "Fill the whole space"} (${ctx.shortcutLabel("↵")}, or double-click the title)`;
    expand.setAttribute("aria-label", big ? "Back to the grid" : "Expand this terminal");
    expand.setAttribute("aria-pressed", big ? "true" : "false");
  }
}

// Terminals look best around 80×24 characters, about 1.6 times wider than tall.
const TILE_ASPECT = 1.6;

/// Picks the grid that keeps every tile closest to a terminal's shape: how many columns,
/// and whether a short last row widens its tiles or the tiles above it grow down into the gap.
function planTiles(count, width, height) {
  const fit = (w, h) => Math.abs(Math.log(w / h / TILE_ASPECT));
  let best = null;
  for (let cols = 1; cols <= Math.min(count, 4); cols++) {
    const rows = Math.ceil(count / cols);
    const last = count - cols * (rows - 1); // tiles in the last row, 1..cols
    const w = width / cols;
    const h = height / rows;
    const plans = [{ cols, rows, last, grow: "wide", score: (count - last) * fit(w, h) + last * fit(width / last, h) }];
    if (last < cols && rows > 1) {
      const tall = cols - last;
      plans.push({ cols, rows, last, grow: "tall", score: (count - tall) * fit(w, h) + tall * fit(w, h * 2) });
    }
    for (const plan of plans) if (!best || plan.score < best.score - 1e-9) best = plan;
  }
  return best;
}

const gcd = (a, b) => (b ? gcd(b, a % b) : a);

/// Places the tiles (in tab order) on the grid planned for the space they have.
function layoutTiles(tiles) {
  const body = panel.querySelector(".terminals-body");
  if (!tiles.length) {
    body.style.gridTemplateColumns = "";
    body.style.gridTemplateRows = "";
    return;
  }
  const plan = planTiles(tiles.length, Math.max(1, body.clientWidth), Math.max(1, body.clientHeight));
  const { cols, rows, last, grow } = plan;
  // Enough grid columns to split both a full row and the last row evenly.
  const span = grow === "wide" ? cols * last / gcd(cols, last) : cols;
  body.style.gridTemplateColumns = `repeat(${span}, minmax(0, 1fr))`;
  body.style.gridTemplateRows = `repeat(${rows}, minmax(0, 1fr))`;
  const full = tiles.length - last;
  tiles.forEach((tile, index) => {
    let row;
    let col;
    let colSpan;
    let rowSpan = 1;
    if (index < full) {
      row = Math.floor(index / cols);
      col = index % cols;
      colSpan = span / cols;
      // Above the gap in the last row: grow down into it.
      if (grow === "tall" && row === rows - 2 && col >= last) rowSpan = 2;
      col *= colSpan;
    } else {
      row = rows - 1;
      colSpan = grow === "wide" ? span / last : 1;
      col = (index - full) * colSpan;
    }
    tile.style.gridRow = `${row + 1} / span ${rowSpan}`;
    tile.style.gridColumn = `${col + 1} / span ${colSpan}`;
  });
}

function markFocused(id) {
  if (id) focusOnLoad = "";
  for (const [tileId, tile] of frames) tile.classList.toggle("is-focused", tileId === id);
}

const OPEN_KEY = "ownnas-terminals-open";
const TILES_WIDTH_KEY = "ownnas-terminals-tiles-width";
const narrow = window.matchMedia("(max-width: 820px)");

function stored(key) {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function store(key, value) {
  try {
    localStorage.setItem(key, value);
  } catch {}
}

function applyTilesWidth(fraction) {
  const value = Math.min(0.8, Math.max(0.25, Number(fraction) || 0.6));
  document.body.style.setProperty("--tiles-width", `${(value * 100).toFixed(2)}vw`);
  return value;
}

/// The tiling layout: the files on the left, every terminal in a grid on the right.
function setTiled(on) {
  const changed = tiled !== on;
  tiled = on;
  if (on) applyTilesWidth(stored(TILES_WIDTH_KEY));
  panel.classList.toggle("is-tiled", on);
  document.body.classList.toggle("is-tiled", on);
  if (!changed) return;
  for (const tile of frames.values()) {
    tile.querySelector("iframe").contentWindow?.postMessage({ type: "ownnas-terminal-tiled", on }, location.origin);
  }
  if (!panel.hidden && view === "terminal") activate(active);
}

/// Dragging the panel's left edge in the tiling layout.
function startResize(event) {
  if (!tiled) return;
  event.preventDefault();
  const handle = event.currentTarget;
  handle.setPointerCapture(event.pointerId);
  // The terminals' iframes would swallow the pointer while it passes over them.
  panel.classList.add("is-resizing");
  let fraction = 0;
  const move = (moved) => {
    fraction = applyTilesWidth((window.innerWidth - moved.clientX) / window.innerWidth);
  };
  const end = () => {
    panel.classList.remove("is-resizing");
    handle.removeEventListener("pointermove", move);
    if (fraction) store(TILES_WIDTH_KEY, String(fraction));
  };
  handle.addEventListener("pointermove", move);
  handle.addEventListener("pointerup", end, { once: true });
  handle.addEventListener("pointercancel", end, { once: true });
}

/// Moves the keyboard to the next tile (the files count as one) in a direction.
function focusTile(direction) {
  if (!tiled || panel.hidden) return;
  const files = document.querySelector(".explorer-content");
  const tiles = [{ id: "", rect: files.getBoundingClientRect() }];
  for (const [id, tile] of frames) if (!tile.hidden) tiles.push({ id, rect: tile.getBoundingClientRect() });
  for (const [id, win] of windows) if (!win.tile.hidden) tiles.push({ id: `w:${id}`, rect: win.tile.getBoundingClientRect(), tile: win.tile });
  const focusedFrame = document.activeElement && document.activeElement.closest && document.activeElement.closest(".terminals-tile");
  const focusedId = focusedFrame ? (focusedFrame.dataset.window ? `w:${focusedFrame.dataset.window}` : focusedFrame.dataset.id) : "";
  const current = tiles.find((tile) => tile.id === focusedId) || tiles[0];
  const center = (rect) => ({ x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 });
  const from = center(current.rect);
  let best = null;
  let bestScore = Infinity;
  for (const tile of tiles) {
    if (tile === current) continue;
    const to = center(tile.rect);
    const dx = to.x - from.x;
    const dy = to.y - from.y;
    const ahead = { left: -dx, right: dx, up: -dy, down: dy }[direction];
    if (ahead <= 1) continue;
    const aside = direction === "left" || direction === "right" ? Math.abs(dy) : Math.abs(dx);
    const score = ahead + aside * 2;
    if (score < bestScore) {
      bestScore = score;
      best = tile;
    }
  }
  if (!best) return;
  if (best.tile) {
    markFocused("");
    best.tile.querySelector(".terminals-window-body button, .terminals-window-body input, .terminals-window-body [tabindex]")?.focus();
  } else if (best.id) {
    activate(best.id);
  } else {
    markFocused("");
    window.focus();
    ctx.focusFiles();
  }
}

let tabMenu = null;

function closeTabMenu() {
  if (!tabMenu || tabMenu.hidden) return;
  tabMenu.hidden = true;
  document.removeEventListener("pointerdown", onPointerOutside, true);
  document.removeEventListener("keydown", onMenuKey, true);
}

function onPointerOutside(event) {
  if (!tabMenu.contains(event.target)) closeTabMenu();
}

function onMenuKey(event) {
  if (event.key === "Escape") {
    // Only the menu closes, not Settings or anything else behind it.
    event.preventDefault();
    event.stopPropagation();
    const id = tabMenu.dataset.id;
    closeTabMenu();
    panel.querySelector(`.terminals-tabs [data-tab="${CSS.escape(id)}"]`)?.focus();
    return;
  }
  if (event.key === "ArrowDown" || event.key === "ArrowUp") {
    event.preventDefault();
    const buttons = [...tabMenu.querySelectorAll("button:not([disabled])")];
    const index = buttons.indexOf(document.activeElement);
    const next = event.key === "ArrowDown" ? index + 1 : index - 1;
    buttons[(next + buttons.length) % buttons.length]?.focus();
  }
}

function openTabMenu(id, x, y, { full = false } = {}) {
  const item = sessions.find((session) => session.id === id);
  if (!item && !full) return;
  if (!tabMenu) {
    tabMenu = el("div", { class: "menu terminal-tab-menu", role: "menu", hidden: true });
    document.body.append(tabMenu);
    // Like other menus, close when focus moves elsewhere (including into a terminal).
    tabMenu.addEventListener("focusout", () => {
      setTimeout(() => {
        if (!tabMenu.contains(document.activeElement) || !document.hasFocus()) closeTabMenu();
      }, 0);
    });
    tabMenu.addEventListener("click", (event) => {
      const button = event.target.closest("[data-tab-action]");
      if (!button) return;
      const target = tabMenu.dataset.id;
      closeTabMenu();
      runTabAction(button.dataset.tabAction, target).catch((err) => ctx.toast(err.message, true));
    });
  }
  tabMenu.dataset.id = id || "";
  const action = (name, label, extra = {}) => el("button", { type: "button", role: "menuitem", "data-tab-action": name, ...extra }, label);
  const items = [];
  if (full) items.push(action("overview", view === "overview" ? "Back to the terminal" : "All terminals"));
  if (full && !item) items.push(action("new-here", "New terminal here"));
  if (item) {
    items.unshift(el("div", { class: "menu-label" }, tabLabel(item)));
    items.push(action("rename", "Rename…"));
    items.push(action("duplicate", full ? "New terminal here" : "Duplicate", { title: `New terminal in ${item.path || "/"}` }));
    items.push(action("folder", "Show folder in files"));
    if (full) {
      const edited = (item.agent && item.agent.files) || [];
      if (edited.length) items.push(action("edited", `Edited files (${edited.length})`));
      items.push(action("changes", "Git changes"));
    }
    items.push(action("popout", "Pop out"));
    items.push(action("hide", "Hide (keeps running)", { title: ctx.shortcutLabel("U") }));
    items.push(action("close", "Close", { class: "danger", title: ctx.shortcutLabel("X") }));
  }
  tabMenu.replaceChildren(...items);
  ctx.decorateMenu(tabMenu);
  tabMenu.hidden = false;
  const rect = tabMenu.getBoundingClientRect();
  tabMenu.style.left = `${Math.max(8, Math.min(x, window.innerWidth - rect.width - 8))}px`;
  tabMenu.style.top = `${Math.max(8, Math.min(y, window.innerHeight - rect.height - 8))}px`;
  document.addEventListener("pointerdown", onPointerOutside, true);
  document.addEventListener("keydown", onMenuKey, true);
  tabMenu.querySelector("button")?.focus();
}

/// Shows the folder the terminal is in now (it may have changed with `cd`) in the file browser.
async function showFolder(id) {
  const data = await ctx.api(`/api/terminals/cwd?id=${encodeURIComponent(id)}`);
  if (data.outside) ctx.toast(`The terminal is in ${data.current}, outside the library. Showing the folder it started in.`);
  // Tiled terminals sit beside the files, so they can stay.
  if (!tiled) hidePanel();
  ctx.goToFolder(data.path);
}

async function runTabAction(action, id) {
  if (action === "overview") {
    setView(view === "overview" ? "terminal" : "overview");
    return;
  }
  if (action === "new-here") {
    await openTerminal(ctx.currentPath());
    return;
  }
  const item = sessions.find((session) => session.id === id);
  if (!item) return;
  if (action === "edited" || action === "changes") {
    setView("terminal");
    if (drawer !== (action === "edited" ? "files" : "changes")) toggleDrawer(action === "edited" ? "files" : "changes");
    return;
  }
  if (action === "rename") {
    const name = await ctx.askText("Rename terminal", "Name (leave empty for the folder name)", item.name || "", "Rename");
    if (name === null) return;
    const data = await ctx.api("/api/terminals/rename", { method: "POST", json: { id, name } });
    applyList(sessions.map((session) => (session.id === id ? data.terminal : session)));
  } else if (action === "duplicate") {
    await openTerminal(item.path);
  } else if (action === "folder") {
    await showFolder(id);
  } else if (action === "popout") {
    window.open(`/terminal?id=${encodeURIComponent(id)}`, "_blank", "noopener");
  } else if (action === "hide") {
    stashTerminal(id);
  } else if (action === "close") {
    await endTerminal(id);
  }
}

/// Opens `element` (such as Settings) as a window tiled with the terminals, opening the panel if
/// needed. Resolves false where there is no tiling (phones, or terminals off or locked); the
/// caller then shows it its usual way. `onClose` runs when the window's ✕ or the panel closes it.
export async function openWindow(id, title, element, onClose) {
  if (!panel || narrow.matches) return false;
  if (panel.hidden) {
    try {
      // Not through `refresh`: opening Settings must never ask for the terminal password.
      const data = await ctx.api("/api/terminals");
      applyList(data.terminals || []);
      watch();
    } catch {
      return false;
    }
    view = "terminal";
    showPanel();
  }
  if (!tiled) return false;
  if (view !== "terminal") view = "terminal";
  if (!windows.has(id)) {
    const tile = el("div", { class: "terminals-tile is-window", "data-window": id });
    const head = el("div", { class: "terminals-tile-head" });
    // `el` takes text only: the name goes in as a child.
    const name = el("button", { type: "button", class: "terminals-tile-title", tabindex: "-1" });
    name.append(el("span", { class: "terminals-tile-name" }, title));
    name.addEventListener("dblclick", () => toggleMaximized(`w:${id}`));
    const expand = el("button", { type: "button", class: "terminals-tile-btn", "data-tile": "expand", tabindex: "-1" });
    expand.addEventListener("click", () => toggleMaximized(`w:${id}`));
    const close = el("button", { type: "button", class: "terminals-tile-btn", tabindex: "-1", title: `Close ${title}`, "aria-label": `Close ${title}` }, "✕");
    close.addEventListener("click", () => onClose());
    head.append(name, expand, close);
    const content = el("div", { class: "terminals-window-body" });
    content.append(element);
    tile.append(head, content);
    panel.querySelector(".terminals-body").append(tile);
    windows.set(id, { tile, onClose });
  }
  syncFrames();
  render();
  return true;
}

/// Takes a window out of the tiles and hands back what was in it (to put back in the page).
export function closeWindow(id) {
  const win = windows.get(id);
  if (!win) return null;
  windows.delete(id);
  if (maximized === `w:${id}`) maximized = "";
  const content = win.tile.querySelector(".terminals-window-body").firstElementChild;
  win.tile.remove();
  syncFrames();
  return content;
}

/// Expands a terminal to fill the grid, or puts it back among the others.
function toggleMaximized(id) {
  if (!tiled || !id) return;
  maximized = maximized === id ? "" : id;
  if (id.startsWith("w:")) {
    // Not through setView: activating a terminal would move the expansion to it.
    view = "terminal";
    syncFrames();
    render();
    return;
  }
  setView("terminal");
  activate(id);
}

/// Puts a terminal away without stopping it, and moves on to the next one.
function stashTerminal(id) {
  const index = sessions.findIndex((item) => item.id === id);
  if (index < 0) return;
  stashed.add(id);
  saveStashed();
  if (id !== active) {
    render();
    syncFrames();
    return;
  }
  const shown = sessions.filter((item) => !stashed.has(item.id));
  const next = shown.find((item) => sessions.indexOf(item) > index) || shown[shown.length - 1];
  if (next) {
    activate(next.id);
    return;
  }
  active = "";
  render();
  syncFrames();
  if (tiled) ctx.focusFiles();
  else hidePanel();
}

function markSeen(id) {
  const item = sessions.find((session) => session.id === id);
  if (item && item.agent) item.agent.attention = null;
  shownAttention.delete(id);
  ctx.api("/api/terminals/seen", { method: "POST", json: { id } }).catch(() => {});
}

/// Announces new attention requests unless the user is already looking at that terminal.
function announce(item) {
  const message = item.agent.attention;
  if (panelVisible() && view === "terminal" && item.id === active) {
    markSeen(item.id);
    return;
  }
  const label = tabLabel(item);
  ctx.toast(`${label}: ${message}`);
  if (document.hidden && "Notification" in window && Notification.permission === "granted") {
    const notice = new Notification(`OwnNAS · ${label}`, { body: message, tag: `ownnas-${item.id}` });
    notice.addEventListener("click", () => {
      window.focus();
      showPanel();
      setView("terminal");
      activate(item.id);
      notice.close();
    });
  }
}

const cleaning = new Set();

/// Closes terminals whose shell has exited (`exit`, or a crash), so they do not linger.
/// Only an unexpected end is mentioned, with its exit code.
function cleanEnded(list) {
  for (const item of list) {
    if (item.running || cleaning.has(item.id)) continue;
    cleaning.add(item.id);
    if (item.exitCode !== undefined && item.exitCode !== null && item.exitCode !== 0) {
      ctx.toast(`“${tabLabel(item)}” ended with exit code ${item.exitCode}`);
    }
    ctx.api(`/api/terminals?id=${encodeURIComponent(item.id)}`, { method: "DELETE" })
      .catch(() => {})
      .then(() => refresh())
      .then(() => {
        if (panel && !panel.hidden && !sessions.length) hidePanel();
        else if (tiled && !active) ctx.focusFiles();
      })
      .catch(() => {})
      .finally(() => cleaning.delete(item.id));
  }
}

function applyList(list) {
  cleanEnded(list);
  list = list.filter((item) => item.running);
  sessions = list;
  for (const [id, frame] of frames) {
    if (!sessions.some((item) => item.id === id)) {
      frame.remove();
      frames.delete(id);
      shownAttention.delete(id);
    }
  }
  for (const item of sessions) {
    const attention = item.agent && item.agent.attention;
    if (attention && shownAttention.get(item.id) !== attention) {
      shownAttention.set(item.id, attention);
      announce(item);
    } else if (!attention) {
      shownAttention.delete(item.id);
    }
  }
  for (const id of [...stashed]) {
    if (!sessions.some((item) => item.id === id)) stashed.delete(id);
  }
  saveStashed();
  if (!sessions.some((item) => item.id === active)) {
    const shown = sessions.filter((item) => !stashed.has(item.id));
    active = shown.length ? shown[shown.length - 1].id : "";
  }
  if (panel && !panel.hidden) {
    syncFrames();
    if (active && !frames.has(active) && view === "terminal") activate(active);
  }
  render();
}

/// Follows terminal and agent changes from the server; retries when the connection drops.
function watch() {
  if (stream) return;
  clearTimeout(streamTimer);
  stream = new EventSource("/api/agents/stream");
  stream.addEventListener("terminals", (event) => {
    streamRetry = 0;
    try {
      applyList(JSON.parse(event.data).terminals || []);
    } catch {}
  });
  stream.addEventListener("error", () => {
    stream.close();
    stream = null;
    const delay = Math.min(60000, 3000 * 2 ** streamRetry++);
    streamTimer = setTimeout(() => {
      ctx.api("/api/terminal/config")
        .then((config) => {
          if (config.enabled && (!config.passwordSet || config.unlocked)) watch();
          else streamTimer = setTimeout(() => watch(), 60000);
        })
        .catch(() => {});
    }, delay);
  });
}

async function refresh() {
  const data = await withUnlock(() => ctx.api("/api/terminals"));
  if (!data) return false;
  applyList(data.terminals || []);
  watch();
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

/// On phones, keep the panel inside the part of the screen the on-screen keyboard leaves
/// (iPhones cover the page instead of resizing it).
function fitToViewport() {
  const viewport = window.visualViewport;
  if (!panel || panel.hidden || !viewport || !window.matchMedia("(max-width: 820px)").matches) {
    if (panel) {
      panel.style.height = "";
      panel.style.top = "";
    }
    return;
  }
  panel.style.height = `${viewport.height}px`;
  panel.style.top = `${viewport.offsetTop}px`;
}

function showPanel() {
  panel.hidden = false;
  document.body.classList.add("terminals-open");
  store(OPEN_KEY, "1");
  setTiled(!narrow.matches);
  fitToViewport();
  if (view === "terminal") activate(active);
  else render();
}

function hidePanel() {
  closeTabMenu();
  for (const win of [...windows.values()]) win.onClose();
  store(OPEN_KEY, "0");
  setTiled(false);
  const hadFocus = panel.contains(document.activeElement);
  panel.hidden = true;
  document.body.classList.remove("terminals-open");
  // Keep the keyboard in the page, so the shortcut can bring the panel back.
  if (hadFocus) document.getElementById("terminals-btn")?.focus();
}

/// Ctrl+`: hide the panel, or bring it back (opening the newest terminal).
function togglePanel() {
  if (!panel.hidden) {
    hidePanel();
    return;
  }
  refresh()
    .then((ok) => {
      if (!ok) return;
      view = "terminal";
      showPanel();
    })
    .catch((err) => ctx.toast(err.message, true));
}

async function openTerminal(path) {
  const data = await withUnlock(() => ctx.api("/api/terminals", { method: "POST", json: { path: path || "", cols: 100, rows: 30 } }));
  if (!data) return;
  await refresh();
  view = "terminal";
  showPanel();
  activate(data.terminal.id);
}

async function endTerminal(id) {
  const item = sessions.find((session) => session.id === id);
  if (!item) return;
  if (item.running && !(await ctx.askConfirm(`Close “${tabLabel(item)}”? The shell and anything running in it will stop.`, "Close terminal"))) return;
  await ctx.api(`/api/terminals?id=${encodeURIComponent(id)}`, { method: "DELETE" });
  await refresh();
  if (!sessions.length) hidePanel();
  else activate(active);
  // Every terminal left is hidden: the keyboard goes back to the files.
  if (!active && tiled) ctx.focusFiles();
}

export function init(context) {
  ctx = context;
  if (!panel) build();
}

export async function open(path) {
  await openTerminal(path);
}

export function toggle() {
  togglePanel();
}

/// ⇧⌘U and ⇧⌘X: hide or end the terminal being used.
export function hideActive() {
  if (panel && !panel.hidden && active) stashTerminal(active);
}

export async function closeActive() {
  if (panel && !panel.hidden && active) await endTerminal(active);
}

/// ⌃1…⌃9 (Alt+1… elsewhere): the terminal with that number, showing the panel if needed.
export async function select(number) {
  if (panel.hidden && !(await refresh())) return;
  const item = sessions[number - 1];
  if (!item) return;
  view = "terminal";
  if (panel.hidden) showPanel();
  activate(item.id);
}

export function toggleExpanded() {
  if (panel && !panel.hidden) toggleMaximized(active);
}

export function moveFocus(direction) {
  focusTile(direction);
}

export function isTiled() {
  return tiled;
}

export async function show() {
  if (await refresh()) showPanel();
}

/// Starts following terminals in the background (for badges and notifications) without
/// asking for the terminal password.
export async function background() {
  try {
    const config = await ctx.api("/api/terminal/config");
    if (config.enabled && (!config.passwordSet || config.unlocked)) {
      watch();
      // Come back to the terminals after a reload.
      if (stored(OPEN_KEY) === "1" && !narrow.matches && panel.hidden && (await refresh())) {
        view = "terminal";
        showPanel();
      }
    }
  } catch {}
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
  if (config.enabled) {
    const box = el("div", { class: "agent-integrations" });
    section.append(box);
    renderIntegrations(box).catch((err) => box.replaceChildren(el("p", { class: "form-error" }, err.message)));
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
        ctx.setBadge(0);
      }
      await ctx.refreshMe();
      await renderSettings(section);
    } catch (err) {
      error.textContent = err.message;
    }
  });
}

/// Agent integrations: hooks that report each step to OwnNAS. They only act inside OwnNAS
/// terminals, so the agents behave as before everywhere else.
async function renderIntegrations(box) {
  const data = await withUnlock(() => ctx.api("/api/agents/integrations"));
  if (!data) {
    box.replaceChildren(el("p", { class: "muted" }, "Unlock terminals to manage agent integrations."));
    return;
  }
  box.replaceChildren(
    el("h3", {}, "Agent integrations"),
    el("p", { class: "muted" }, "Without an integration, OwnNAS still sees which agent runs in a terminal and when it rings for you. With one, it also shows each step, the files it edits, and when it needs permission or finishes. Integrations only act inside OwnNAS terminals."),
  );
  for (const item of data.integrations) {
    const row = el("div", { class: "agent-integration" });
    const text = el("div");
    text.append(el("strong", {}, item.label), el("p", { class: "muted" }, item.installed
      ? `Installed in ${item.location}`
      : item.found ? `Adds a hook to ${item.location}` : `${item.label} does not seem to be set up for this account yet.`));
    const button = el("button", { type: "button", class: item.installed ? "" : "primary" }, item.installed ? "Remove" : "Install");
    button.addEventListener("click", async () => {
      button.disabled = true;
      try {
        await ctx.api("/api/agents/integrations", { method: "POST", json: { agent: item.agent, installed: !item.installed } });
        ctx.toast(item.installed ? `${item.label} integration removed` : `${item.label} integration installed. Restart ${item.label} sessions to use it.`);
        await renderIntegrations(box);
      } catch (err) {
        ctx.toast(err.message, true);
        button.disabled = false;
      }
    });
    row.append(text, button);
    box.append(row);
  }
}
