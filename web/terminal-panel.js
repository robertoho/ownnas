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
  const popout = el("button", { type: "button", "data-action": "popout", title: "Open this terminal in its own browser tab" }, "Pop out");
  const end = el("button", { type: "button", "data-action": "end", class: "danger", title: "Stop the shell and everything running in it" }, "Close");
  const hide = el("button", { type: "button", "data-action": "hide", title: `Hide the panel (${ctx.shortcutLabel("L")} or Ctrl+\`). Terminals keep running.` }, "Hide");
  // On phones the other buttons move into this menu, leaving one row for the tabs.
  const more = el("button", { type: "button", "data-action": "more", class: "terminals-more-btn", "aria-label": "More terminal actions", "aria-haspopup": "menu" }, "⋯");
  actions.append(add, popout, end, more, hide);
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
  popout.addEventListener("click", () => {
    if (active) window.open(`/terminal?id=${encodeURIComponent(active)}`, "_blank", "noopener");
  });
  end.addEventListener("click", () => endTerminal(active).catch((err) => ctx.toast(err.message, true)));
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
  // Phones get the full-screen panel with one terminal at a time instead of tiles.
  narrow.addEventListener("change", () => {
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
    if (event.data.type === "shortcut") ctx.runShortcut(event.data.name);
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
        class: [item.running ? "" : "ended", attention ? "has-attention" : ""].join(" ").trim() || null,
        title: `${item.path || "/"}${item.agent && item.agent.detail ? ` — ${item.agent.detail}` : ""}${attention ? `\n${attention}` : ""}`,
      });
      tab.append(el("span", { class: "agent-dot", "data-status": status, "aria-hidden": "true" }), el("span", {}, tabLabel(item)));
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
  panel.querySelector(".terminals-empty").hidden = has || view === "overview";
  // New works with no terminals open too: it starts one in the folder being browsed.
  for (const action of ["popout", "end"]) {
    panel.querySelector(`[data-action="${action}"]`).disabled = !has || view === "overview";
  }
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
  const files = el("button", { type: "button", "aria-pressed": drawer === "files" ? "true" : "false", hidden: !(agent.files && agent.files.length) }, `Edited (${(agent.files || []).length})`);
  const changes = el("button", { type: "button", "aria-pressed": drawer === "changes" ? "true" : "false", title: "Uncommitted changes in this folder's Git repository" }, "Changes");
  const folder = el("button", { type: "button", title: "Show the terminal's current folder in the file browser" }, "Show folder");
  folder.addEventListener("click", () => showFolder(item.id).catch((err) => ctx.toast(err.message, true)));
  files.addEventListener("click", () => toggleDrawer("files"));
  changes.addEventListener("click", () => toggleDrawer("changes"));
  tools.append(files, changes, folder);
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
  const box = panel.querySelector(".terminals-drawer");
  box.hidden = false;
  box.replaceChildren(el("p", { class: "muted" }, "Reading Git status…"));
  let data;
  try {
    data = await ctx.api(`/api/terminals/changes?id=${encodeURIComponent(item.id)}`);
  } catch (err) {
    box.replaceChildren(el("p", { class: "form-error" }, err.message));
    return;
  }
  if (drawer !== "changes" || active !== item.id) return;
  if (!data.repo) {
    box.replaceChildren(el("p", { class: "muted" }, "This folder is not in a Git repository."));
    return;
  }
  const head = el("p", { class: "muted" }, [
    data.branch ? `On ${data.branch}.` : "",
    data.total ? `${data.total} changed ${data.total === 1 ? "file" : "files"}${data.summary ? ` (${data.summary})` : ""}.` : "No uncommitted changes.",
  ].join(" ").trim());
  const list = el("ul", { class: "terminals-files" });
  for (const file of data.files) {
    const row = el("li");
    row.append(el("span", { class: "git-status" }, file.status));
    row.append(file.path ? fileButton(file.path, file.name) : el("span", {}, file.name));
    list.append(row);
  }
  const refresh = el("button", { type: "button" }, "Refresh");
  refresh.addEventListener("click", () => renderChanges(item));
  box.replaceChildren(head, list, refresh);
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
  const head = el("button", { type: "button", class: "terminals-tile-head", tabindex: "-1" });
  head.addEventListener("click", () => {
    setView("terminal");
    activate(id);
  });
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
  if (tiled && view === "terminal") {
    for (const item of sessions) tileFor(item.id);
    // Same order as the tabs. (Moving an iframe in the page would reload it.)
    sessions.forEach((item, index) => { frames.get(item.id).style.order = String(index); });
    const count = sessions.length;
    const cols = count <= 2 ? 1 : count <= 6 ? 2 : 3;
    body.style.setProperty("--tile-cols", String(cols));
    body.style.setProperty("--tile-rows", String(Math.max(1, Math.ceil(count / cols))));
  }
  for (const [id, tile] of frames) {
    tile.hidden = view !== "terminal" || (!tiled && id !== active);
    // A hidden terminal would keep the keyboard and swallow every key.
    if (tile.hidden && tile.contains(document.activeElement)) document.activeElement.blur();
    tile.classList.toggle("is-active", id === active);
    const item = sessions.find((session) => session.id === id);
    if (!item) continue;
    const head = tile.querySelector(".terminals-tile-head");
    head.replaceChildren(el("span", { class: "agent-dot", "data-status": statusOf(item), "aria-hidden": "true" }), el("span", {}, tabLabel(item)));
    head.title = item.path || "/";
  }
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
  const focusedFrame = document.activeElement && document.activeElement.closest && document.activeElement.closest(".terminals-tile");
  const current = tiles.find((tile) => tile.id === (focusedFrame ? focusedFrame.dataset.id : "")) || tiles[0];
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
  if (best.id) {
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
    items.push(action("close", "Close", { class: "danger" }));
  }
  tabMenu.replaceChildren(...items);
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
  hidePanel();
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
  } else if (action === "close") {
    await endTerminal(id);
  }
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

function applyList(list) {
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
  if (!sessions.some((item) => item.id === active)) active = sessions.length ? sessions[sessions.length - 1].id : "";
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
