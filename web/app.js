const state = {
  me: null,
  path: "",
  entries: [],
  truncated: false,
  total: 0,
  view: normalizeView(localStorage.getItem("ownnas-view")),
  sort: localStorage.getItem("ownnas-sort") || "name",
  direction: localStorage.getItem("ownnas-dir") || "asc",
  hidden: localStorage.getItem("ownnas-hidden") === "1",
  filter: "",
  current: null,
  selected: new Set(),
  anchor: "",
  clipboard: null,
  menuEntries: [],
  pasteInto: "",
  bookmarks: [],
};

function normalizeView(value) {
  if (value === "grid") return "masonry";
  if (value === "masonry" || value === "list" || value === "icons") return value;
  return "icons";
}

const THEMES = [
  { id: "ownnas", name: "OwnNAS", mode: "dark", swatches: ["#12140f", "#1b1e16", "#dff25a", "#f4f2e8"] },
  { id: "tokyo-night", name: "Tokyo Night", mode: "dark", swatches: ["#1a1b26", "#24283b", "#7aa2f7", "#c0caf5"] },
  { id: "catppuccin", name: "Catppuccin", mode: "dark", swatches: ["#1e1e2e", "#313244", "#89b4fa", "#cdd6f4"] },
  { id: "catppuccin-latte", name: "Catppuccin Latte", mode: "light", swatches: ["#eff1f5", "#e6e9ef", "#1e66f5", "#4c4f69"] },
  { id: "everforest", name: "Everforest", mode: "dark", swatches: ["#2d353b", "#374247", "#7fbbb3", "#d3c6aa"] },
  { id: "gruvbox", name: "Gruvbox", mode: "dark", swatches: ["#282828", "#3c3836", "#7daea3", "#ebdbb2"] },
  { id: "kanagawa", name: "Kanagawa", mode: "dark", swatches: ["#1f1f28", "#2a2a37", "#7e9cd8", "#dcd7ba"] },
  { id: "osaka-jade", name: "Osaka Jade", mode: "dark", swatches: ["#111c18", "#1a2a22", "#509475", "#c1c497"] },
  { id: "matte-black", name: "Matte Black", mode: "dark", swatches: ["#121212", "#1c1c1c", "#e68e0d", "#eaeaea"] },
  { id: "rose-pine", name: "Rose Pine", mode: "light", swatches: ["#faf4ed", "#fffaf3", "#56949f", "#575279"] },
];

function normalizeTheme(value) {
  return THEMES.some((theme) => theme.id === value) ? value : "ownnas";
}

function applyTheme(id) {
  const theme = normalizeTheme(id);
  document.documentElement.setAttribute("data-theme", theme);
  localStorage.setItem("ownnas-theme", theme);
  renderThemeGrid();
}

function currentTheme() {
  return normalizeTheme(localStorage.getItem("ownnas-theme") || "ownnas");
}

function renderThemeGrid() {
  const host = $("theme-grid");
  if (!host) return;
  const active = currentTheme();
  host.innerHTML = THEMES.map((theme) => `
    <button type="button" class="theme-card" data-theme-id="${esc(theme.id)}" aria-pressed="${theme.id === active ? "true" : "false"}">
      <span class="theme-swatch" aria-hidden="true"></span>
      <strong>${esc(theme.name)}</strong>
      <span class="muted">${theme.mode}</span>
    </button>
  `).join("");
}

function tagInitials(tag) {
  const parts = String(tag).trim().split(/[\s_-]+/).filter(Boolean);
  if (!parts.length) return "?";
  if (parts.length >= 2) {
    return `${parts[0][0] || ""}${parts[1][0] || ""}`.toUpperCase();
  }
  return parts[0].slice(0, 2).toUpperCase();
}

function tagColor(tag) {
  let hash = 2166136261;
  for (let i = 0; i < tag.length; i += 1) {
    hash ^= tag.charCodeAt(i);
    hash = Math.imul(hash, 16777619);
  }
  const hue = (hash >>> 0) % 360;
  return `hsl(${hue} 58% 40%)`;
}

const $ = (id) => document.getElementById(id);
let loadGen = 0;
let toastTimer = 0;
let skipNextClick = false;
let activeDownload = null;
let activeUpload = null;
let uploadAbort = false;
let marquee = null;

function esc(value) {
  return String(value).replace(/[&<>"']/g, (ch) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;",
  }[ch]));
}

function toast(message, bad = false) {
  const el = $("toast");
  el.textContent = message;
  el.classList.toggle("bad", bad);
  el.classList.add("show");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => el.classList.remove("show"), 3200);
}

async function api(url, options = {}) {
  const headers = new Headers(options.headers || {});
  const method = options.method || "GET";
  if (method !== "GET") headers.set("X-OwnNAS", "1");
  let body = options.body;
  if (options.json !== undefined) {
    headers.set("Content-Type", "application/json");
    body = JSON.stringify(options.json);
  }
  const response = await fetch(url, { method, headers, body, credentials: "same-origin" });
  const text = await response.text();
  let data = null;
  if (text) {
    try { data = JSON.parse(text); } catch { data = { error: text }; }
  }
  if (response.status === 401 && !options.allow401) {
    showLogin(data && data.error ? data.error : "");
    throw new Error("Sign in required");
  }
  if (!response.ok) {
    throw new Error((data && data.error) || "Request failed");
  }
  return data;
}

function showLogin(message) {
  $("boot").hidden = true;
  $("app-view").hidden = true;
  $("preview").hidden = true;
  $("library").hidden = true;
  $("settings").hidden = true;
  $("login-view").hidden = false;
  $("login-error").textContent = message || "";
  state.me = null;
}

function showApp() {
  $("boot").hidden = true;
  $("login-view").hidden = true;
  $("app-view").hidden = false;
  $("who").textContent = state.me.admin ? `${state.me.username} · admin` : state.me.username;
  const write = !state.me.readonly;
  $("mkdir-btn").hidden = !write;
  $("newfile-btn").hidden = !write;
  $("upload-btn").hidden = !write;
  $("folder-btn").hidden = !write;
  $("preview-rename").hidden = !write;
  $("preview-delete").hidden = !write;
  $("preview-duplicate").hidden = !write;
  $("preview-move").hidden = !write;
  $("empty-trash-btn").hidden = true;
  refreshLibrary().catch((err) => toast(err.message, true));
}

function formatSize(bytes, dir) {
  if (dir) return "Folder";
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(value >= 10 ? 0 : 1)} ${units[unit]}`;
}

function formatDate(seconds) {
  if (!seconds) return "";
  return new Date(seconds * 1000).toLocaleString();
}

function pathToHash(path) {
  if (!path) return "#/";
  return `#/${path.split("/").map(encodeURIComponent).join("/")}`;
}

function hashToPath() {
  let hash = location.hash.replace(/^#/, "");
  if (hash.startsWith("/")) hash = hash.slice(1);
  if (!hash) return "";
  return hash.split("/").filter(Boolean).map(decodeURIComponent).join("/");
}

function typeIcon(kind) {
  const icons = {
    folder: `<path d="M2 4.5A1.5 1.5 0 0 1 3.5 3H7l1.2 1.5H12.5A1.5 1.5 0 0 1 14 6v6.5A1.5 1.5 0 0 1 12.5 14h-9A1.5 1.5 0 0 1 2 12.5z" fill="currentColor" opacity="0.22"/><path d="M2 6.5h12v6A1.5 1.5 0 0 1 12.5 14h-9A1.5 1.5 0 0 1 2 12.5z" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M2 4.5A1.5 1.5 0 0 1 3.5 3H7l1.5 1.8H12.5" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>`,
    "archive-folder": `<path d="M3 3.5h10v2H3zm1 2v7.5A1.5 1.5 0 0 0 5.5 14.5h5A1.5 1.5 0 0 0 12 13V5.5" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M6.5 8.5h3M6.5 11h3" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>`,
    "trash-folder": `<path d="M3.5 5h9M6 5V3.8A.8.8 0 0 1 6.8 3h2.4a.8.8 0 0 1 .8.8V5m-.8 0v7.2a1 1 0 0 1-1 1H7.8a1 1 0 0 1-1-1V5" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/><path d="M7 7.5v4M9 7.5v4" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/>`,
    image: `<rect x="2.5" y="3.5" width="11" height="9" rx="1.5" fill="none" stroke="currentColor" stroke-width="1.5"/><circle cx="6" cy="7" r="1.1" fill="currentColor"/><path d="M2.8 11.2 6.2 8.4l2.1 1.7 2.2-2.5 2.7 3.6" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/>`,
    svg: `<rect x="2.5" y="3.5" width="11" height="9" rx="1.5" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M5 10.5 8 5.5l3 5" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>`,
    video: `<rect x="2.5" y="3.5" width="11" height="9" rx="1.5" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M7 6.2v3.6L10.2 8z" fill="currentColor"/>`,
    audio: `<path d="M5 10.5a1.7 1.7 0 1 0 0 .2V6.2L12 4.8v4.5" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/><circle cx="5" cy="11" r="1.7" fill="none" stroke="currentColor" stroke-width="1.5"/><circle cx="12" cy="9.5" r="1.7" fill="none" stroke="currentColor" stroke-width="1.5"/>`,
    pdf: `<path d="M4.5 2.5h5L12.5 5v8.5A1.5 1.5 0 0 1 11 15H5A1.5 1.5 0 0 1 3.5 13.5v-10A1 1 0 0 1 4.5 2.5z" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round"/><path d="M9.5 2.6V5h2.4M5.5 9h5M5.5 11.5h3.5" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/>`,
    text: `<path d="M4.5 2.5h5L12.5 5v8.5A1.5 1.5 0 0 1 11 15H5A1.5 1.5 0 0 1 3.5 13.5v-10A1 1 0 0 1 4.5 2.5z" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round"/><path d="M9.5 2.6V5h2.4M5.8 8.5h4.4M5.8 11h3" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/>`,
    archive: `<path d="M3 3.5h10v2.2H3zm1.2 2.2v7.8A1.5 1.5 0 0 0 5.7 15h4.6a1.5 1.5 0 0 0 1.5-1.5V5.7" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M8 8.2v4.2M6.6 10.3h2.8" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/>`,
    file: `<path d="M4.5 2.5h5L12.5 5v8.5A1.5 1.5 0 0 1 11 15H5A1.5 1.5 0 0 1 3.5 13.5v-10A1 1 0 0 1 4.5 2.5z" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round"/><path d="M9.5 2.6V5h2.4" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round"/>`,
  };
  const body = icons[kind] || icons.file;
  return `<svg class="type-icon" viewBox="0 0 16 16" width="28" height="28" aria-hidden="true">${body}</svg>`;
}

function sortedEntries() {
  const query = state.filter.trim().toLowerCase();
  const items = state.entries.filter((entry) => {
    if (!query) return true;
    if (entry.name.toLowerCase().includes(query)) return true;
    return (entry.tags || []).some((tag) => tag.toLowerCase().includes(query));
  });
  const factor = state.direction === "desc" ? -1 : 1;
  items.sort((a, b) => {
    if (a.dir !== b.dir) return a.dir ? -1 : 1;
    if (state.sort === "size") return (a.size - b.size) * factor;
    if (state.sort === "modified") return (a.modified - b.modified) * factor;
    return a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: "base" }) * factor;
  });
  return items;
}

function renderCrumbs() {
  const nav = $("crumbs");
  const parts = state.path ? state.path.split("/") : [];
  const root = state.me.rootName || "Library";
  let html = `<button type="button" data-go="">${esc(root)}</button>`;
  let walked = "";
  parts.forEach((part, index) => {
    walked = walked ? `${walked}/${part}` : part;
    const current = index === parts.length - 1 ? ' aria-current="page"' : "";
    html += `<span class="muted">/</span><button type="button" data-go="${esc(walked)}"${current}>${esc(specialFolderLabel(part))}</button>`;
  });
  nav.innerHTML = html;
}

function specialFolderLabel(name) {
  if (name === ".ownnas-trash" || name === "Trash") return "Trash";
  if (name === ".ownnas-archive" || name === "Archive") return "Archive";
  return name;
}

function inTrashPath(path) {
  return path === ".ownnas-trash"
    || path.startsWith(".ownnas-trash/")
    || path === "Trash"
    || path.startsWith("Trash/");
}

function renderFiles() {
  const host = $("files");
  host.className = `files ${state.view}`;
  const items = sortedEntries();
  $("empty").hidden = items.length !== 0;
  $("empty").textContent = state.entries.length === 0 ? "This folder is empty." : "Nothing matches that filter.";
  const banner = $("banner");
  if (banner) {
    banner.hidden = !state.truncated;
    banner.textContent = state.truncated ? `Showing the first ${state.entries.length} of ${state.total} items.` : "";
  }
  host.innerHTML = items.map((entry) => {
    const thumb = entry.thumb
      ? `<img alt="" loading="lazy" src="/api/thumb?path=${encodeURIComponent(entry.path)}&v=${entry.modified}">`
      : entry.kind === "svg"
        ? `<img alt="" loading="lazy" src="/api/raw?path=${encodeURIComponent(entry.path)}">`
        : `<span class="glyph kind-${esc(entry.kind)}" aria-hidden="true">${typeIcon(entry.kind)}</span>`;
    const selected = state.selected.has(entry.path);
    const cut = state.clipboard && state.clipboard.mode === "cut" && state.clipboard.items.some((item) => item.path === entry.path);
    const archiveFolder = entry.kind === "archive-folder";
    const trashFolder = entry.kind === "trash-folder";
    const displayName = archiveFolder ? "Archive" : trashFolder ? "Trash" : entry.name;
    const badge = archiveFolder
      ? `<span class="badge">Archive</span>`
      : trashFolder
        ? `<span class="badge trash">Trash</span>`
        : "";
    const sub = trashFolder
      ? "OwnNAS trash"
      : archiveFolder
        ? "OwnNAS archive"
        : entry.original
          ? `From ${entry.original}`
          : esc(formatSize(entry.size, entry.dir));
    const tags = entry.tags || [];
    const shownTags = tags.slice(0, 3);
    const tagStack = shownTags.length
      ? `<div class="tag-stack">${shownTags.map((tag) => {
          const color = tagColor(tag);
          return `<button type="button" class="tag-badge" data-filter-tag="${esc(tag)}" title="${esc(tag)}" style="--tag-bg:${color}">${esc(tagInitials(tag))}</button>`;
        }).join("")}${tags.length > 3 ? `<span class="tag-badge more-tags" title="${esc(tags.slice(3).join(", "))}">+${tags.length - 3}</span>` : ""}</div>`
      : "";
    return `<article class="card${selected ? " selected" : ""}${cut ? " cut" : ""}${archiveFolder ? " archive-folder" : ""}${trashFolder ? " trash-folder" : ""}" data-path="${esc(entry.path)}" data-dir="${entry.dir ? "1" : "0"}" aria-selected="${selected ? "true" : "false"}">
      <div class="thumb">${thumb}${badge}${tagStack}</div>
      <div class="card-body">
        <div class="name" title="${esc(entry.path)}">${esc(displayName)}</div>
        <div class="sub" title="${entry.original ? esc(entry.original) : ""}">${sub}</div>
      </div>
      <button class="more" type="button" data-menu-for="${esc(entry.path)}" aria-label="Actions for ${esc(displayName)}">···</button>
    </article>`;
  }).join("");
  host.querySelectorAll("img").forEach((img) => {
    img.addEventListener("error", () => {
      const wrap = document.createElement("span");
      wrap.className = "glyph kind-file";
      wrap.setAttribute("aria-hidden", "true");
      wrap.innerHTML = typeIcon("file");
      img.replaceWith(wrap);
    });
  });
}

async function load(path) {
  const gen = ++loadGen;
  const hidden = state.hidden ? "&hidden=1" : "";
  const data = await api(`/api/list?path=${encodeURIComponent(path)}${hidden}`);
  if (gen !== loadGen) return;
  state.path = data.path || "";
  state.entries = data.entries || [];
  state.truncated = !!data.truncated;
  state.total = data.total || state.entries.length;
  if (data.rootName) state.me.rootName = data.rootName;
  state.selected = new Set();
  state.anchor = "";
  closePreview();
  renderCrumbs();
  renderFiles();
  $("zip-link").href = `/api/zip?path=${encodeURIComponent(state.path)}`;
  syncBookmarkBtn();
  $("empty-trash-btn").hidden = !(state.me && !state.me.readonly && inTrashPath(state.path));
}

function syncBookmarkBtn() {
  const marked = state.bookmarks.includes(state.path);
  const btn = $("bookmark-btn");
  const label = marked ? "Remove bookmark" : "Bookmark this folder";
  btn.title = label;
  btn.setAttribute("aria-label", label);
  btn.setAttribute("aria-pressed", marked ? "true" : "false");
}

async function go(path) {
  closeLibrary();
  closeSettings();
  const next = pathToHash(path);
  if (location.hash !== next) location.hash = next;
  else await load(path);
}

function findEntry(path) {
  return state.entries.find((entry) => entry.path === path);
}

function closeMenu() {
  $("menu").hidden = true;
}

function parentPath(path) {
  const index = path.lastIndexOf("/");
  return index === -1 ? "" : path.slice(0, index);
}

function selectedEntries() {
  return state.entries.filter((entry) => state.selected.has(entry.path));
}

function paintSelection() {
  document.querySelectorAll("#files .card").forEach((card) => {
    const selected = state.selected.has(card.dataset.path);
    card.classList.toggle("selected", selected);
    card.setAttribute("aria-selected", selected ? "true" : "false");
    const cut = state.clipboard && state.clipboard.mode === "cut" && state.clipboard.items.some((item) => item.path === card.dataset.path);
    card.classList.toggle("cut", !!cut);
  });
}

function selectOnly(path) {
  state.selected = new Set([path]);
  state.anchor = path;
  paintSelection();
}

function toggleSelected(path) {
  if (state.selected.has(path)) state.selected.delete(path);
  else state.selected.add(path);
  state.anchor = path;
  paintSelection();
}

function selectRange(path) {
  const items = sortedEntries();
  const from = items.findIndex((entry) => entry.path === (state.anchor || path));
  const to = items.findIndex((entry) => entry.path === path);
  if (from < 0 || to < 0) {
    selectOnly(path);
    return;
  }
  const [start, end] = from < to ? [from, to] : [to, from];
  state.selected = new Set(items.slice(start, end + 1).map((entry) => entry.path));
  paintSelection();
}

function gridColumnCount() {
  const cards = [...document.querySelectorAll("#files .card")];
  if (cards.length < 2 || state.view === "list") return 1;
  const top = cards[0].offsetTop;
  let cols = 1;
  for (let i = 1; i < cards.length; i += 1) {
    if (Math.abs(cards[i].offsetTop - top) > 2) break;
    cols += 1;
  }
  return Math.max(1, cols);
}

function scrollSelectedIntoView(path) {
  const card = document.querySelector(`#files .card[data-path="${CSS.escape(path)}"]`);
  if (card) card.scrollIntoView({ block: "nearest", inline: "nearest" });
}

function moveSelection(deltaX, deltaY, extend) {
  const items = sortedEntries();
  if (!items.length) return;
  const cols = gridColumnCount();
  let index = items.findIndex((entry) => entry.path === state.anchor);
  if (index < 0) {
    const selected = selectedEntries();
    index = selected.length
      ? items.findIndex((entry) => entry.path === selected[selected.length - 1].path)
      : 0;
  }
  if (index < 0) index = 0;
  let next = index + deltaX + (deltaY * cols);
  next = Math.max(0, Math.min(items.length - 1, next));
  const entry = items[next];
  if (!entry) return;
  if (extend) selectRange(entry.path);
  else selectOnly(entry.path);
  scrollSelectedIntoView(entry.path);
  if (!$("preview").hidden && !entry.dir) {
    openEntry(entry).catch((err) => toast(err.message, true));
  }
}

function renderClipboard() {
  const box = $("clipboard");
  const clip = state.clipboard;
  if (!clip || !clip.items.length) {
    box.hidden = true;
    return;
  }
  const verb = clip.mode === "cut" ? "Moving" : "Copying";
  $("clipboard-title").textContent = clip.items.length === 1 ? `${verb} 1 item` : `${verb} ${clip.items.length} items`;
  $("clipboard-list").innerHTML = clip.items.map((item) => `<li>${esc(item.name)}</li>`).join("");
  $("clipboard-paste").hidden = !(state.me && !state.me.readonly);
  box.hidden = false;
}

function setClipboard(mode, entries) {
  if (!entries.length) return;
  state.clipboard = {
    mode,
    items: entries.map((entry) => ({ path: entry.path, name: entry.name, dir: !!entry.dir })),
  };
  renderClipboard();
  paintSelection();
}

function clearClipboard() {
  state.clipboard = null;
  renderClipboard();
  paintSelection();
}

function openSelectionMenu(x, y, pasteInto) {
  const entries = selectedEntries();
  const write = state.me && !state.me.readonly;
  const clip = state.clipboard && state.clipboard.items.length;
  // Empty-space menu: New folder and Paste. Selection menu needs at least one item.
  if (!entries.length && !write) return;
  state.menuEntries = entries;
  state.pasteInto = pasteInto;
  const one = entries.length === 1 ? entries[0] : null;
  const allInTrash = entries.length > 0 && entries.every((entry) => inTrashPath(entry.path));
  const buttons = [];
  if (entries.length) {
    buttons.push(`<div class="menu-label">${entries.length === 1 ? esc(entries[0].name) : `${entries.length} items`}</div>`);
  } else {
    buttons.push(`<div class="menu-label">${esc(state.path || "Library")}</div>`);
  }
  if (!entries.length && write) {
    buttons.push(`<button type="button" data-menu="mkdir">New folder</button>`);
    buttons.push(`<button type="button" data-menu="newfile">New file…</button>`);
  }
  if (one) buttons.push(`<button type="button" data-menu="open">Open</button>`);
  if (entries.length) buttons.push(`<button type="button" data-menu="download">Download</button>`);
  if (write && allInTrash) buttons.push(`<button type="button" data-menu="restore">Restore</button>`);
  if (entries.length) buttons.push(`<button type="button" data-menu="copy">Copy</button>`);
  if (write && entries.length && !allInTrash) {
    buttons.push(`<button type="button" data-menu="cut">Cut</button>`);
  }
  if (write && clip) buttons.push(`<button type="button" data-menu="paste">Paste</button>`);
  if (write && entries.length && !allInTrash) {
    buttons.push(`<button type="button" data-menu="duplicate">Duplicate</button>`);
    buttons.push(`<button type="button" data-menu="move">Move</button>`);
  }
  if (write && entries.length >= 2 && !allInTrash) {
    buttons.push(`<button type="button" data-menu="folderWith">New folder with selection</button>`);
  }
  if (write && one && !allInTrash) buttons.push(`<button type="button" data-menu="rename">Rename</button>`);
  if (write && entries.length) {
    buttons.push(`<button type="button" data-menu="delete" class="danger">${allInTrash ? "Delete forever" : "Move to Trash"}</button>`);
  }
  const menu = $("menu");
  menu.innerHTML = buttons.join("");
  menu.hidden = false;
  const rect = menu.getBoundingClientRect();
  menu.style.left = `${Math.max(8, Math.min(x, window.innerWidth - rect.width - 8))}px`;
  menu.style.top = `${Math.max(8, Math.min(y, window.innerHeight - rect.height - 8))}px`;
}

function closePreview() {
  setPreviewFullscreen(false);
  $("preview").hidden = true;
  state.current = null;
  $("preview-body").innerHTML = "";
  $("preview-path").hidden = true;
  $("preview-path").textContent = "";
  $("preview-notes").hidden = true;
  $("notes-changed").hidden = true;
  setNotesExpanded(false);
  $("preview-tags").innerHTML = "";
  $("preview-comments").innerHTML = "";
  $("notes-summary-tags").innerHTML = "";
  $("notes-summary-comments").hidden = true;
  $("tag-input").value = "";
  $("comment-input").value = "";
}

function setPreviewFullscreen(on) {
  const preview = $("preview");
  const btn = $("preview-expand");
  preview.classList.toggle("is-fullscreen", !!on);
  btn.setAttribute("aria-pressed", on ? "true" : "false");
  btn.title = on ? "Collapse preview" : "Expand preview";
  btn.setAttribute("aria-label", on ? "Collapse preview" : "Expand preview");
}

function closeLibrary() {
  $("library").hidden = true;
}

function closeSettings() {
  $("settings").hidden = true;
}

function setSettingsTab(tab) {
  const theme = tab === "theme";
  $("settings-tab-theme").setAttribute("aria-selected", theme ? "true" : "false");
  $("settings-tab-users").setAttribute("aria-selected", theme ? "false" : "true");
  $("settings-theme").hidden = !theme;
  $("settings-users").hidden = theme;
}

function openSettings(tab) {
  closePreview();
  closeLibrary();
  const admin = !!(state.me && state.me.admin);
  $("settings-tab-users").hidden = !admin;
  if (!admin) tab = "theme";
  setSettingsTab(tab || "theme");
  renderThemeGrid();
  $("settings").hidden = false;
  if (admin) loadUsers().catch((err) => toast(err.message, true));
}

async function loadUsers() {
  const data = await api("/api/users");
  const users = data.users || [];
  const host = $("users-list");
  if (!users.length) {
    host.innerHTML = `<p class="muted">No accounts yet.</p>`;
    return;
  }
  host.innerHTML = `<table class="users-table">
    <thead><tr><th>User</th><th>Role</th><th></th></tr></thead>
    <tbody>
      ${users.map((user) => `
        <tr data-username="${esc(user.username)}">
          <td>${esc(user.username)}</td>
          <td>${user.isAdmin ? `<span class="pill">Admin</span>` : `<span class="muted">Member</span>`}</td>
          <td>
            <div class="user-actions">
              <button type="button" data-user-password="${esc(user.username)}">Reset password</button>
              <button type="button" data-user-admin="${esc(user.username)}" data-admin="${user.isAdmin ? "0" : "1"}">${user.isAdmin ? "Revoke admin" : "Make admin"}</button>
              <button type="button" class="danger" data-user-remove="${esc(user.username)}">Remove</button>
            </div>
          </td>
        </tr>
      `).join("")}
    </tbody>
  </table>`;
}

function setLibraryTab(tab) {
  const bookmarks = tab === "bookmarks";
  $("library-tab-bookmarks").setAttribute("aria-selected", bookmarks ? "true" : "false");
  $("library-tab-recent").setAttribute("aria-selected", bookmarks ? "false" : "true");
  $("library-bookmarks").hidden = !bookmarks;
  $("library-recent").hidden = bookmarks;
}

function openLibrary(tab) {
  closePreview();
  closeSettings();
  if (tab) setLibraryTab(tab);
  $("library").hidden = false;
  refreshLibrary().catch((err) => toast(err.message, true));
}

function libraryLabel(path) {
  if (!path) return state.me.rootName || "Library";
  const parts = path.split("/");
  return parts[parts.length - 1] || path;
}

function libraryPath(path) {
  const root = (state.me && state.me.rootName) || "Library";
  if (!path) return root;
  return `${root}/${path.split("/").map(specialFolderLabel).join("/")}`;
}

async function openEntry(entry) {
  if (!entry) return;
  if (entry.dir) {
    closeLibrary();
    await go(entry.path);
    return;
  }
  closeLibrary();
  state.current = entry;
  $("preview").hidden = false;
  $("preview-title").textContent = entry.name;
  $("preview-path").hidden = false;
  $("preview-path").textContent = libraryPath(entry.path);
  $("preview-path").title = libraryPath(entry.path);
  $("preview-meta").textContent = `${formatSize(entry.size, false)} · ${formatDate(entry.modified)}`;
  $("preview-hash").textContent = "SHA-256";
  $("preview-download").href = `/api/raw?download=1&path=${encodeURIComponent(entry.path)}`;
  $("preview-delete").textContent = inTrashPath(entry.path) ? "Delete forever" : "Move to Trash";
  const write = state.me && !state.me.readonly;
  $("tag-form").hidden = !write;
  $("comment-form").hidden = !write;
  const images = sortedEntries().filter((item) => item.kind === "image" || item.kind === "svg");
  const imageIndex = images.findIndex((item) => item.path === entry.path);
  $("preview-prev").hidden = imageIndex <= 0;
  $("preview-next").hidden = imageIndex < 0 || imageIndex >= images.length - 1;
  if (!inTrashPath(entry.path)) {
    api("/api/recent", { method: "POST", json: { path: entry.path } }).then(() => refreshLibrary()).catch(() => {});
  }
  loadAnnotations(entry.path).catch((err) => toast(err.message, true));
  const body = $("preview-body");
  const raw = `/api/raw?path=${encodeURIComponent(entry.path)}`;
  if (entry.kind === "image" || entry.kind === "svg") {
    body.innerHTML = `<img alt="${esc(entry.name)}" src="${raw}">`;
  } else if (entry.kind === "video") {
    const poster = entry.thumb ? ` poster="/api/thumb?path=${encodeURIComponent(entry.path)}&v=${entry.modified}"` : "";
    body.innerHTML = `<video controls playsinline${poster} src="${raw}"></video>`;
  } else if (entry.kind === "audio") {
    body.innerHTML = `<audio controls src="${raw}"></audio>`;
  } else if (entry.kind === "pdf") {
    body.innerHTML = `<iframe title="${esc(entry.name)}" src="${raw}"></iframe>`;
  } else {
    body.innerHTML = `<p class="muted">Loading preview…</p>`;
    try {
      const meta = await api(`/api/meta?path=${encodeURIComponent(entry.path)}`);
      if (!state.current || state.current.path !== entry.path) return;
      renderMeta(body, entry, meta);
    } catch (err) {
      if (state.current && state.current.path === entry.path) {
        body.innerHTML = `<p class="muted">${esc(err.message)}</p>`;
      }
    }
  }
}

async function loadAnnotations(path) {
  const notes = $("preview-notes");
  notes.hidden = false;
  setNotesExpanded(false);
  $("preview-tags").innerHTML = `<span class="notes-empty">Loading…</span>`;
  $("preview-comments").innerHTML = "";
  $("notes-summary-tags").innerHTML = `<span class="notes-empty-inline">Loading…</span>`;
  $("notes-summary-comments").hidden = true;
  const data = await api(`/api/annotations?path=${encodeURIComponent(path)}`);
  if (!state.current || state.current.path !== path) return;
  renderAnnotations(data);
}

function setNotesExpanded(expanded) {
  const notes = $("preview-notes");
  const details = $("notes-details");
  const toggle = $("notes-toggle");
  notes.dataset.expanded = expanded ? "true" : "false";
  details.hidden = !expanded;
  toggle.setAttribute("aria-expanded", expanded ? "true" : "false");
}

function renderAnnotations(data) {
  const write = state.me && !state.me.readonly;
  const tags = data.tags || [];
  const comments = data.comments || [];
  const changed = !!data.changed;
  $("notes-changed").hidden = !changed;
  $("notes-ack").hidden = !write;
  $("tag-form").hidden = !write;
  $("comment-form").hidden = !write;

  $("notes-summary-tags").innerHTML = tags.length
    ? tags.slice(0, 6).map((tag) => {
        const color = tagColor(tag);
        return `<span class="tag-badge" title="${esc(tag)}" style="--tag-bg:${color}">${esc(tagInitials(tag))}</span>`;
      }).join("") + (tags.length > 6 ? `<span class="notes-empty-inline">+${tags.length - 6}</span>` : "")
    : `<span class="notes-empty-inline">No tags</span>`;

  const hasComments = comments.length > 0;
  $("notes-summary-comments").hidden = !hasComments;
  $("notes-comment-count").textContent = hasComments ? String(comments.length) : "";

  $("preview-tags").innerHTML = tags.length
    ? tags.map((tag) => `<span class="tag-chip"><button type="button" class="tag-link" data-search-tag="${esc(tag)}">${esc(tag)}</button>${write ? `<button type="button" data-remove-tag="${esc(tag)}" aria-label="Remove tag ${esc(tag)}">×</button>` : ""}</span>`).join("")
    : `<span class="notes-empty">No tags yet</span>`;
  $("preview-comments").innerHTML = comments.length
    ? comments.map((item) => `<article class="comment-item" data-comment-id="${item.id}">
        <div class="comment-head">
          <span>${esc(item.username)} · ${esc(formatDate(item.createdAt))}</span>
          ${write ? `<button type="button" data-remove-comment="${item.id}">Remove</button>` : ""}
        </div>
        <div class="comment-body">${esc(item.body)}</div>
      </article>`).join("")
    : `<span class="notes-empty">No comments yet</span>`;
}

function renderMeta(body, entry, meta) {
  if (meta.text && !meta.text.binary) {
    body.innerHTML = renderTextViewer(entry.name, meta.text.content, meta.text.truncated);
    return;
  }
  if (meta.archive) {
    const rows = meta.archive.map((item) => `<li><span>${esc(item.name)}</span><span class="muted">${item.dir ? "Folder" : esc(formatSize(item.size, false))}</span></li>`).join("");
    body.innerHTML = `<ul class="archive-list">${rows || "<li>The archive is empty.</li>"}</ul>`;
    if (meta.archiveTruncated) body.insertAdjacentHTML("beforeend", `<p class="muted">Showing the first entries only.</p>`);
    return;
  }
  body.innerHTML = `<p class="muted">${esc(meta.note || "No inline preview for this file. Download it to open it locally.")}</p>`;
}

function textFormat(name) {
  const lower = name.toLowerCase();
  if (lower.endsWith(".md") || lower.endsWith(".markdown")) return "markdown";
  if (/\.(ini|conf|cfg|toml|env|properties)$/.test(lower)) return "ini";
  if (lower.endsWith(".json")) return "json";
  if (lower.endsWith(".csv") || lower.endsWith(".tsv")) return "table";
  if (/\.(ya?ml|xml|html?|css|js|mjs|ts|tsx|jsx|py|rs|go|java|c|h|cpp|hpp|cs|sh|bash|zsh|ps1|sql|rb|php|lua|log|txt)$/.test(lower)) {
    return lower.endsWith(".log") ? "log" : "text";
  }
  return "text";
}

function formatLabel(format) {
  return {
    markdown: "Markdown",
    ini: "INI",
    json: "JSON",
    table: "Table",
    log: "Log",
    text: "Text",
  }[format] || "Text";
}

function renderTextViewer(name, content, truncated) {
  const format = textFormat(name);
  let body = "";
  if (format === "table") {
    body = renderTable(content, name.toLowerCase().endsWith(".tsv") ? "\t" : ",");
  } else if (format === "json") {
    let pretty = content;
    try { pretty = JSON.stringify(JSON.parse(pretty), null, 2); } catch { /* keep */ }
    body = `<pre class="text-code">${esc(pretty)}</pre>`;
  } else if (format === "markdown") {
    body = `<div class="md-preview">${renderMarkdown(content)}</div>`;
  } else if (format === "ini") {
    body = `<pre class="text-code ini-preview">${renderIni(content)}</pre>`;
  } else {
    body = renderLinedText(content, format === "log");
  }
  const lines = content ? content.split(/\r?\n/).length : 0;
  return `<div class="text-viewer format-${format}">
    <div class="text-viewer-bar">
      <span class="pill">${esc(formatLabel(format))}</span>
      <span class="muted">${lines} line${lines === 1 ? "" : "s"}</span>
    </div>
    ${body}
    ${truncated ? `<p class="muted text-viewer-note">Preview truncated.</p>` : ""}
  </div>`;
}

function renderLinedText(content, soft) {
  const lines = content.split(/\r?\n/);
  const rows = lines.map((line, index) => {
    const text = line.length ? esc(line) : " ";
    return `<div class="text-line${soft ? " soft" : ""}"><span class="ln">${index + 1}</span><code>${text}</code></div>`;
  }).join("");
  return `<div class="text-lines">${rows || `<div class="text-line"><span class="ln">1</span><code> </code></div>`}</div>`;
}

function renderIni(content) {
  return content.split(/\r?\n/).map((line) => {
    if (/^\s*$/.test(line)) return " ";
    if (/^\s*[#;]/.test(line)) return `<span class="tok-comment">${esc(line)}</span>`;
    if (/^\s*\[[^\]]*\]\s*$/.test(line)) return `<span class="tok-section">${esc(line)}</span>`;
    const match = line.match(/^(\s*)([^=]+?)(\s*=\s*)(.*)$/);
    if (!match) return esc(line);
    return `${esc(match[1])}<span class="tok-key">${esc(match[2])}</span><span class="tok-eq">${esc(match[3])}</span><span class="tok-value">${esc(match[4])}</span>`;
  }).join("\n");
}

function renderMarkdown(source) {
  const lines = source.replace(/\r\n/g, "\n").split("\n");
  const out = [];
  let i = 0;
  let inCode = false;
  let code = [];
  let list = null;

  function closeList() {
    if (!list) return;
    out.push(`</${list}>`);
    list = null;
  }

  function inlineMd(text) {
    let html = esc(text);
    html = html.replace(/`([^`]+)`/g, "<code>$1</code>");
    html = html.replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>");
    html = html.replace(/(^|[^*\w])\*([^*\n]+)\*(?!\*)/g, "$1<em>$2</em>");
    html = html.replace(/\[([^\]]+)\]\((https?:\/\/[^)\s]+)\)/g, '<a href="$2" target="_blank" rel="noopener noreferrer">$1</a>');
    return html;
  }

  while (i < lines.length) {
    const line = lines[i];
    if (line.startsWith("```")) {
      if (inCode) {
        out.push(`<pre class="text-code"><code>${esc(code.join("\n"))}</code></pre>`);
        code = [];
        inCode = false;
      } else {
        closeList();
        inCode = true;
      }
      i += 1;
      continue;
    }
    if (inCode) {
      code.push(line);
      i += 1;
      continue;
    }
    if (/^\s*$/.test(line)) {
      closeList();
      i += 1;
      continue;
    }
    const heading = line.match(/^(#{1,3})\s+(.*)$/);
    if (heading) {
      closeList();
      const level = heading[1].length;
      out.push(`<h${level + 2}>${inlineMd(heading[2])}</h${level + 2}>`);
      i += 1;
      continue;
    }
    if (/^---+$/.test(line.trim())) {
      closeList();
      out.push("<hr>");
      i += 1;
      continue;
    }
    const quote = line.match(/^>\s?(.*)$/);
    if (quote) {
      closeList();
      out.push(`<blockquote>${inlineMd(quote[1])}</blockquote>`);
      i += 1;
      continue;
    }
    const unordered = line.match(/^\s*[-*+]\s+(.*)$/);
    if (unordered) {
      if (list !== "ul") {
        closeList();
        list = "ul";
        out.push("<ul>");
      }
      out.push(`<li>${inlineMd(unordered[1])}</li>`);
      i += 1;
      continue;
    }
    const ordered = line.match(/^\s*\d+\.\s+(.*)$/);
    if (ordered) {
      if (list !== "ol") {
        closeList();
        list = "ol";
        out.push("<ol>");
      }
      out.push(`<li>${inlineMd(ordered[1])}</li>`);
      i += 1;
      continue;
    }
    closeList();
    out.push(`<p>${inlineMd(line)}</p>`);
    i += 1;
  }
  if (inCode) out.push(`<pre class="text-code"><code>${esc(code.join("\n"))}</code></pre>`);
  closeList();
  return out.join("") || "<p class=\"muted\">Empty file</p>";
}

function renderTable(content, separator) {
  const lines = content.split(/\r?\n/).filter((line) => line.length).slice(0, 200);
  const rows = lines.map((line) => `<tr>${line.split(separator).map((cell) => `<td>${esc(cell)}</td>`).join("")}</tr>`);
  return `<div class="table-wrap"><table>${rows.join("")}</table></div>`;
}

function fileExtension(name) {
  const base = String(name || "");
  const index = base.lastIndexOf(".");
  // Leading-dot names like ".gitignore" have no extension unless another dot follows.
  if (index <= 0 || index === base.length - 1) return "";
  return base.slice(index + 1).toLowerCase();
}

function extensionLabel(ext) {
  return ext ? `.${ext}` : "(none)";
}

function askText(title, label, value, okLabel, options = {}) {
  $("text-title").textContent = title;
  $("text-label-copy").textContent = label;
  $("text-input").value = value || "";
  $("text-ok").textContent = okLabel || "Save";
  const warning = $("text-warning");
  const originalName = options.watchExtensionFrom || "";
  const updateWarning = () => {
    if (!originalName) {
      warning.hidden = true;
      warning.textContent = "";
      return;
    }
    const next = $("text-input").value.trim();
    const fromExt = fileExtension(originalName);
    const toExt = fileExtension(next);
    if (next && fromExt !== toExt) {
      warning.hidden = false;
      warning.textContent = `Extension changes from ${extensionLabel(fromExt)} to ${extensionLabel(toExt)}.`;
    } else {
      warning.hidden = true;
      warning.textContent = "";
    }
  };
  updateWarning();
  const dialog = $("text-dialog");
  // Avoid a previous OK/Cancel sticking around when Escape closes the dialog.
  dialog.returnValue = "";
  dialog.showModal();
  $("text-input").focus();
  $("text-input").select();
  $("text-input").addEventListener("input", updateWarning);
  return new Promise((resolve) => {
    const onCancel = () => {
      dialog.returnValue = "cancel";
    };
    dialog.addEventListener("cancel", onCancel, { once: true });
    dialog.addEventListener("close", () => {
      dialog.removeEventListener("cancel", onCancel);
      $("text-input").removeEventListener("input", updateWarning);
      warning.hidden = true;
      warning.textContent = "";
      resolve(dialog.returnValue === "ok" ? $("text-input").value : null);
    }, { once: true });
  });
}

function askConfirm(message, okLabel) {
  $("confirm-copy").textContent = message;
  $("confirm-ok").textContent = okLabel || "OK";
  const dialog = $("confirm-dialog");
  dialog.showModal();
  return new Promise((resolve) => {
    dialog.addEventListener("close", () => resolve(dialog.returnValue === "ok"), { once: true });
  });
}

async function renameEntry(entry) {
  const name = await askText(
    "Rename",
    "New name",
    entry.name,
    "Rename",
    entry.dir ? {} : { watchExtensionFrom: entry.name },
  );
  if (name === null) return;
  const next = name.trim();
  if (!next || next === entry.name) return;
  if (!entry.dir) {
    const fromExt = fileExtension(entry.name);
    const toExt = fileExtension(next);
    if (fromExt !== toExt) {
      const confirmed = await askConfirm(
        `You are changing the file extension from ${extensionLabel(fromExt)} to ${extensionLabel(toExt)}.\n\n“${entry.name}” → “${next}”\n\nPrograms may no longer open this file correctly.`,
        "Change extension",
      );
      if (!confirmed) return;
    }
  }
  await api("/api/rename", { method: "POST", json: { path: entry.path, name: next } });
  toast("Renamed");
  await load(state.path);
}

async function deleteEntry(entry) {
  await deleteEntries([entry]);
}

async function deleteEntries(entries) {
  if (!entries.length) return;
  const forever = entries.every((entry) => inTrashPath(entry.path));
  const message = forever
    ? (entries.length === 1
      ? `Permanently delete “${entries[0].name}”? This cannot be undone.`
      : `Permanently delete ${entries.length} items? This cannot be undone.`)
    : (entries.length === 1
      ? `Move “${entries[0].name}” to the Trash?`
      : `Move ${entries.length} items to the Trash?`);
  if (!await askConfirm(message, forever ? "Delete forever" : "Move to Trash")) return;
  for (const entry of entries) {
    await api(`/api/entry?path=${encodeURIComponent(entry.path)}`, { method: "DELETE" });
  }
  toast(forever
    ? (entries.length === 1 ? "Deleted forever" : `Deleted ${entries.length} items forever`)
    : (entries.length === 1 ? "Moved to Trash" : `Moved ${entries.length} items to Trash`));
  await load(state.path);
}

async function downloadEntries(entries) {
  if (!entries.length) {
    toast("Select a file or folder to download", true);
    return;
  }
  if (entries.length === 1 && !entries[0].dir) {
    location.href = `/api/raw?download=1&path=${encodeURIComponent(entries[0].path)}`;
    return;
  }
  const label = entries.length === 1
    ? entries[0].name
    : `${entries.length} items`;
  const fallbackName = entries.length === 1
    ? `${entries[0].name}.zip`
    : `ownnas-${entries.length}-items.zip`;
  await downloadZipBlob({
    url: "/api/download",
    method: "POST",
    body: JSON.stringify({ paths: entries.map((entry) => entry.path) }),
    preparing: `Compressing ${label}…`,
    fallbackName,
    doneToast: entries.length === 1 ? "Download started" : `Downloading ${entries.length} items`,
  });
}

function showStatus(title, detail = "", percent = null) {
  const panel = $("status-panel");
  const bar = $("status-bar");
  $("status-title").textContent = title;
  $("status-detail").textContent = detail;
  panel.hidden = false;
  if (percent == null) {
    bar.classList.add("indeterminate");
    bar.style.width = "";
  } else {
    bar.classList.remove("indeterminate");
    bar.style.width = `${Math.max(0, Math.min(100, Math.round(percent)))}%`;
  }
}

function hideStatus() {
  const panel = $("status-panel");
  const bar = $("status-bar");
  panel.hidden = true;
  bar.classList.remove("indeterminate");
  bar.style.width = "0";
  $("status-title").textContent = "Working…";
  $("status-detail").textContent = "";
}

function cancelActiveDownload() {
  if (!activeDownload) return;
  const xhr = activeDownload;
  activeDownload = null;
  xhr.abort();
}

function cancelActiveUpload() {
  uploadAbort = true;
  if (!activeUpload) return;
  const xhr = activeUpload;
  activeUpload = null;
  xhr.abort();
}

function cancelActiveTransfer() {
  cancelActiveDownload();
  cancelActiveUpload();
}

function downloadZipBlob({ url, method = "GET", body = null, preparing, fallbackName, doneToast }) {
  return new Promise((resolve, reject) => {
    if (activeDownload) cancelActiveDownload();
    showStatus(preparing || "Preparing archive…", "This can take a moment for large folders");
    const xhr = new XMLHttpRequest();
    activeDownload = xhr;
    xhr.open(method, url);
    xhr.responseType = "blob";
    xhr.withCredentials = true;
    xhr.setRequestHeader("X-OwnNAS", "1");
    if (body != null) xhr.setRequestHeader("Content-Type", "application/json");
    let receiving = false;
    let cancelled = false;
    xhr.onprogress = (event) => {
      if (cancelled) return;
      if (!event.lengthComputable || !event.total) {
        if (receiving) showStatus("Downloading…", "Receiving archive");
        return;
      }
      receiving = true;
      const percent = (event.loaded / event.total) * 100;
      showStatus("Downloading…", `${Math.round(percent)}% · ${formatSize(event.loaded, false)} of ${formatSize(event.total, false)}`, percent);
    };
    xhr.onreadystatechange = () => {
      if (cancelled) return;
      if (xhr.readyState === XMLHttpRequest.HEADERS_RECEIVED && xhr.status >= 200 && xhr.status < 300) {
        receiving = true;
        showStatus("Downloading…", "Receiving archive");
      }
    };
    xhr.onload = async () => {
      if (activeDownload === xhr) activeDownload = null;
      try {
        if (xhr.status === 401) {
          showLogin("");
          throw new Error("Sign in required");
        }
        if (xhr.status < 200 || xhr.status >= 300) {
          let message = "Download failed";
          try {
            const text = await (xhr.response instanceof Blob ? xhr.response.text() : Promise.resolve(""));
            const data = JSON.parse(text || "");
            message = data.error || message;
          } catch { /* keep */ }
          throw new Error(message);
        }
        let filename = fallbackName || "download.zip";
        const disposition = xhr.getResponseHeader("content-disposition") || "";
        const match = disposition.match(/filename\*=UTF-8''([^;]+)|filename=\"([^\"]+)\"/i);
        if (match) {
          filename = decodeURIComponent((match[1] || match[2] || filename).trim());
        }
        const objectUrl = URL.createObjectURL(xhr.response);
        const link = document.createElement("a");
        link.href = objectUrl;
        link.download = filename;
        document.body.appendChild(link);
        link.click();
        link.remove();
        URL.revokeObjectURL(objectUrl);
        if (doneToast) toast(doneToast);
        resolve();
      } catch (err) {
        reject(err);
      } finally {
        hideStatus();
      }
    };
    xhr.onerror = () => {
      if (activeDownload === xhr) activeDownload = null;
      hideStatus();
      reject(new Error("Download failed"));
    };
    xhr.onabort = () => {
      cancelled = true;
      if (activeDownload === xhr) activeDownload = null;
      hideStatus();
      const err = new Error("Download cancelled");
      err.cancel = true;
      reject(err);
    };
    xhr.send(body);
  });
}

async function restoreEntries(entries) {
  if (!entries.length) return;
  for (const entry of entries) {
    await api("/api/restore", { method: "POST", json: { path: entry.path } });
  }
  toast(entries.length === 1 ? "Restored" : `Restored ${entries.length} items`);
  await load(state.path);
}

async function emptyTrash() {
  if (!await askConfirm("Empty the Trash? Every item in it will be permanently deleted.", "Empty Trash")) return;
  const result = await api("/api/trash/empty", { method: "POST" });
  toast(result.removed ? `Emptied ${result.removed} item(s)` : "Trash was already empty");
  await load(state.path);
}

async function duplicateEntries(entries) {
  for (const entry of entries) {
    await api("/api/duplicate", { method: "POST", json: { path: entry.path } });
  }
  toast(entries.length === 1 ? "Duplicated" : `Duplicated ${entries.length} items`);
  await load(state.path);
}

async function moveEntries(entries) {
  if (!entries.length) return;
  const fallback = state.path.includes("/") ? state.path.slice(0, state.path.lastIndexOf("/")) : "";
  const dest = await askText("Move", "Destination folder (empty for the library root)", fallback, "Move");
  if (dest === null) return;
  for (const entry of entries) {
    await api("/api/move", { method: "POST", json: { path: entry.path, dest } });
  }
  toast(entries.length === 1 ? "Moved" : `Moved ${entries.length} items`);
  await load(state.path);
}

function joinRel(parent, name) {
  return parent ? `${parent}/${name}` : name;
}

async function createFolder() {
  const name = (await askText("New folder", "Folder name", "New folder", "Create") || "").trim();
  if (!name) return;
  await api("/api/mkdir", { method: "POST", json: { path: state.path, name } });
  toast("Folder created");
  await load(state.path);
}

const NEW_FILE_KINDS = {
  md: { label: "Markdown", ext: "md", defaultName: "Untitled" },
  csv: { label: "CSV", ext: "csv", defaultName: "Untitled" },
};

function syncCreateFileName(kind, forceDefault) {
  const meta = NEW_FILE_KINDS[kind] || NEW_FILE_KINDS.md;
  const input = $("create-file-name");
  const current = input.value.trim();
  const stem = (() => {
    if (!current || forceDefault) return meta.defaultName;
    const dot = current.lastIndexOf(".");
    if (dot > 0) return current.slice(0, dot);
    return current;
  })();
  input.value = `${stem}.${meta.ext}`;
}

function askCreateFile(preferredKind) {
  const dialog = $("create-file-dialog");
  const kindSelect = $("create-file-kind");
  const kinds = Object.keys(NEW_FILE_KINDS);
  kindSelect.value = kinds.includes(preferredKind) ? preferredKind : "md";
  syncCreateFileName(kindSelect.value, true);
  dialog.returnValue = "";
  dialog.showModal();
  $("create-file-name").focus();
  $("create-file-name").select();
  return new Promise((resolve) => {
    const onCancel = () => {
      dialog.returnValue = "cancel";
    };
    const onKind = () => syncCreateFileName(kindSelect.value, false);
    kindSelect.addEventListener("change", onKind);
    dialog.addEventListener("cancel", onCancel, { once: true });
    dialog.addEventListener("close", () => {
      dialog.removeEventListener("cancel", onCancel);
      kindSelect.removeEventListener("change", onKind);
      if (dialog.returnValue !== "ok") {
        resolve(null);
        return;
      }
      resolve({
        kind: kindSelect.value,
        name: $("create-file-name").value.trim(),
      });
    }, { once: true });
  });
}

async function createFile(preferredKind) {
  const choice = await askCreateFile(preferredKind);
  if (!choice || !choice.name) return;
  const data = await api("/api/create", {
    method: "POST",
    json: { path: state.path, name: choice.name, kind: choice.kind },
  });
  toast("File created");
  await load(state.path);
  const created = state.entries.find((entry) => entry.path === data.path);
  if (created) openEntry(created).catch((err) => toast(err.message, true));
}

async function createFolderWithSelection(entries) {
  const items = (entries || []).filter((entry) => entry && entry.path);
  if (items.length < 2) return;
  const name = (await askText("New folder with selection", "Folder name", "New folder", "Create") || "").trim();
  if (!name) return;
  // Create the destination first, then move the captured selection into it.
  await api("/api/mkdir", { method: "POST", json: { path: state.path, name } });
  const dest = joinRel(state.path, name);
  let moved = 0;
  try {
    for (const entry of items) {
      if (entry.path === dest || entry.path.startsWith(`${dest}/`)) {
        throw new Error(`“${entry.name}” cannot be moved inside itself`);
      }
      await api("/api/move", { method: "POST", json: { path: entry.path, dest } });
      moved += 1;
    }
  } finally {
    await load(state.path);
  }
  toast(moved === items.length
    ? `Moved ${moved} items into “${name}”`
    : `Moved ${moved} of ${items.length} items into “${name}”`);
}

async function pasteClipboard(dest) {
  const clip = state.clipboard;
  if (!clip || !clip.items.length) return;
  const remaining = [];
  let done = 0;
  let error = null;
  for (let index = 0; index < clip.items.length; index += 1) {
    const item = clip.items[index];
    if (clip.mode === "cut" && parentPath(item.path) === dest) continue;
    if (item.dir && (dest === item.path || dest.startsWith(`${item.path}/`))) {
      error = new Error(`“${item.name}” cannot be pasted inside itself`);
      remaining.push(...clip.items.slice(index));
      break;
    }
    try {
      const url = clip.mode === "cut" ? "/api/move" : "/api/copy";
      await api(url, { method: "POST", json: { path: item.path, dest } });
      done += 1;
    } catch (err) {
      error = err;
      remaining.push(...clip.items.slice(index));
      break;
    }
  }
  if (clip.mode === "cut") {
    state.clipboard = remaining.length ? { mode: "cut", items: remaining } : null;
    renderClipboard();
  }
  await load(state.path);
  if (error) toast(error.message, true);
  else if (done) toast(clip.mode === "cut" ? "Moved" : "Copied");
  else toast("Already in this folder");
}

function uploadFiles(fileList, names) {
  const files = [...fileList].map((file, index) => ({
    file,
    name: (names && names[index]) || file.webkitRelativePath || file.name,
  }));
  if (!files.length) return Promise.resolve({ saved: 0, skipped: 0 });
  return sendUploads(files);
}

function conflictNote(item, info) {
  if (info.dir) return "A folder with this name is already here. Keep both saves the upload under a new name.";
  const incomingSecs = Math.floor(item.file.lastModified / 1000);
  const here = `Already here: ${formatSize(info.size, false)}, ${formatDate(info.modified)}`;
  const incoming = `This upload: ${formatSize(item.file.size, false)}, ${formatDate(incomingSecs)}`;
  let which = "Both have the same date.";
  if (info.modified < incomingSecs) which = "The file already here is older.";
  else if (info.modified > incomingSecs) which = "This upload is older.";
  return `${here}. ${incoming}. ${which}`;
}

function askUploadConflict(item, info) {
  const label = item.name;
  $("conflict-copy").textContent = info.dir
    ? `A folder named “${label}” already exists.`
    : `“${label}” already exists.`;
  $("conflict-meta").textContent = conflictNote(item, info);
  $("conflict-rest").checked = false;
  for (const value of ["overwrite", "archive-older", "archive-existing"]) {
    const button = document.querySelector(`#conflict-dialog button[value="${value}"]`);
    button.hidden = !!info.dir;
  }
  const dialog = $("conflict-dialog");
  dialog.showModal();
  const first = [...dialog.querySelectorAll("button")].find((button) => !button.hidden && button.value !== "cancel");
  if (first) first.focus();
  return new Promise((resolve) => {
    dialog.addEventListener("close", () => {
      const choice = dialog.returnValue;
      if (!choice || choice === "cancel") resolve(null);
      else resolve({ choice, rest: $("conflict-rest").checked });
    }, { once: true });
  });
}

function choiceFits(choice, info) {
  if (!info.dir) return true;
  return choice === "keep" || choice === "ignore";
}

async function sendUploads(files) {
  const plan = await api("/api/upload/conflicts", {
    method: "POST",
    json: { path: state.path, names: files.map((item) => item.name) },
  });
  const byName = new Map((plan.items || []).map((item) => [item.name, item]));
  let restChoice = null;
  let saved = 0;
  let skipped = 0;
  const totalBytes = files.reduce((sum, item) => sum + item.file.size, 0) || 1;
  let loadedBefore = 0;
  uploadAbort = false;
  const updateProgress = (item, index, loaded) => {
    const overall = ((loadedBefore + loaded) / totalBytes) * 100;
    const label = files.length === 1
      ? item.name
      : `${index + 1} of ${files.length} · ${item.name}`;
    showStatus("Uploading…", `${label} · ${Math.round(overall)}%`, overall);
  };
  try {
    for (let index = 0; index < files.length; index += 1) {
      if (uploadAbort) {
        const error = new Error("Upload cancelled");
        error.cancel = true;
        throw error;
      }
      const item = files[index];
      const info = byName.get(item.name);
      let choice = null;
      if (info && info.exists) {
        if (restChoice && choiceFits(restChoice, info)) choice = restChoice;
        else {
          hideStatus();
          const answer = await askUploadConflict(item, info);
          if (!answer) {
            const error = new Error("Upload cancelled");
            error.cancel = true;
            throw error;
          }
          choice = answer.choice;
          if (answer.rest) restChoice = choice;
        }
      }
      if (choice === "ignore") {
        skipped += 1;
        loadedBefore += item.file.size;
        continue;
      }
      updateProgress(item, index, 0);
      let posted = await postFile(item, choice, (loaded) => updateProgress(item, index, loaded));
      if (posted && posted.exists) {
        hideStatus();
        const answer = await askUploadConflict(item, posted);
        if (!answer) {
          const error = new Error("Upload cancelled");
          error.cancel = true;
          throw error;
        }
        if (answer.rest) restChoice = answer.choice;
        if (answer.choice === "ignore") {
          skipped += 1;
          loadedBefore += item.file.size;
          continue;
        }
        updateProgress(item, index, 0);
        posted = await postFile(item, answer.choice, (loaded) => updateProgress(item, index, loaded));
        if (posted && posted.exists) throw new Error("An item with that name already exists");
      }
      loadedBefore += item.file.size;
      saved += 1;
    }
  } finally {
    activeUpload = null;
    hideStatus();
    const thin = $("progress");
    if (thin) {
      thin.hidden = true;
      thin.style.width = "0";
    }
  }
  return { saved, skipped };
}

function postFile(item, choice, onProgress) {
  return new Promise((resolve, reject) => {
    if (uploadAbort) {
      const error = new Error("Upload cancelled");
      error.cancel = true;
      reject(error);
      return;
    }
    const xhr = new XMLHttpRequest();
    activeUpload = xhr;
    const params = new URLSearchParams();
    params.set("path", state.path);
    if (choice) params.set("conflict", choice);
    if (choice === "archive-older") params.set("modified", String(Math.floor(item.file.lastModified / 1000)));
    xhr.open("POST", `/api/upload?${params}`);
    xhr.setRequestHeader("X-OwnNAS", "1");
    xhr.upload.onprogress = (event) => {
      if (event.lengthComputable) onProgress(event.loaded);
    };
    xhr.onload = () => {
      if (activeUpload === xhr) activeUpload = null;
      if (xhr.status >= 200 && xhr.status < 300) resolve(null);
      else if (xhr.status === 409) {
        try { resolve(JSON.parse(xhr.responseText)); }
        catch { reject(new Error("An item with that name already exists")); }
      } else {
        let message = "Upload failed";
        try { message = JSON.parse(xhr.responseText).error || message; } catch { /* keep */ }
        reject(new Error(message));
      }
    };
    xhr.onerror = () => {
      if (activeUpload === xhr) activeUpload = null;
      reject(new Error("Upload failed"));
    };
    xhr.onabort = () => {
      if (activeUpload === xhr) activeUpload = null;
      const error = new Error("Upload cancelled");
      error.cancel = true;
      reject(error);
    };
    const body = new FormData();
    body.append("file", item.file, item.name);
    xhr.send(body);
  });
}

function uploadSummary(result) {
  if (!result.saved && result.skipped) {
    return result.skipped === 1 ? "Ignored 1 file" : `Ignored ${result.skipped} files`;
  }
  if (result.skipped) return `Uploaded ${result.saved}, ignored ${result.skipped}`;
  return "Upload finished";
}

async function readDrop(dataTransfer) {
  const items = [...dataTransfer.items || []];
  const collected = [];
  async function walk(entry, prefix) {
    if (entry.isFile) {
      const file = await new Promise((resolve, reject) => entry.file(resolve, reject));
      collected.push({ file, name: prefix + file.name });
    } else if (entry.isDirectory) {
      const reader = entry.createReader();
      const children = [];
      while (true) {
        const batch = await new Promise((resolve, reject) => reader.readEntries(resolve, reject));
        if (!batch.length) break;
        children.push(...batch);
      }
      for (const child of children) await walk(child, `${prefix}${entry.name}/`);
    }
  }
  if (items.length && items[0].webkitGetAsEntry) {
    for (const item of items) {
      const entry = item.webkitGetAsEntry && item.webkitGetAsEntry();
      if (entry) await walk(entry, "");
    }
    if (collected.length) return collected;
  }
  return [...dataTransfer.files].map((file) => ({ file, name: file.webkitRelativePath || file.name }));
}

function syncControls() {
  $("sort").value = state.sort;
  $("sort-dir").textContent = state.direction === "desc" ? "Z–A" : "A–Z";
  document.querySelectorAll(".view-switch [data-view]").forEach((button) => {
    button.setAttribute("aria-pressed", button.dataset.view === state.view ? "true" : "false");
  });
  $("hidden-toggle").checked = state.hidden;
}

async function boot() {
  try {
    state.me = await api("/api/me", { allow401: true });
  } catch {
    showLogin("");
    return;
  }
  if (!state.me || !state.me.username) {
    showLogin("");
    return;
  }
  showApp();
  syncControls();
  try {
    await load(hashToPath());
  } catch (err) {
    toast(err.message, true);
  }
}

boot();
applyTheme(currentTheme());

$("login-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  $("login-error").textContent = "";
  try {
    await api("/api/login", {
      method: "POST",
      allow401: true,
      json: { username: $("login-user").value, password: $("login-pass").value },
    });
    $("login-pass").value = "";
    await boot();
  } catch (err) {
    $("login-error").textContent = err.message;
  }
});

$("logout-btn").addEventListener("click", async () => {
  await api("/api/logout", { method: "POST" });
  showLogin("");
});

$("password-btn").addEventListener("click", () => {
  $("pw-error").textContent = "";
  $("pw-current").value = "";
  $("pw-new").value = "";
  $("pw-repeat").value = "";
  $("password-dialog").showModal();
});

$("password-form").addEventListener("submit", async (event) => {
  if (event.submitter && event.submitter.value === "cancel") return;
  event.preventDefault();
  $("pw-error").textContent = "";
  if ($("pw-new").value !== $("pw-repeat").value) {
    $("pw-error").textContent = "The new passwords do not match.";
    return;
  }
  try {
    await api("/api/password", {
      method: "POST",
      json: { current: $("pw-current").value, new: $("pw-new").value },
    });
    $("password-dialog").close();
    toast("Password updated");
  } catch (err) {
    $("pw-error").textContent = err.message;
  }
});

$("crumbs").addEventListener("click", (event) => {
  const button = event.target.closest("[data-go]");
  if (!button) return;
  go(button.dataset.go).catch((err) => toast(err.message, true));
});

$("brand-home").addEventListener("click", () => {
  go("").catch((err) => toast(err.message, true));
});

$("files").addEventListener("click", (event) => {
  if (skipNextClick) {
    skipNextClick = false;
    return;
  }
  const tagButton = event.target.closest("[data-filter-tag]");
  if (tagButton) {
    event.stopPropagation();
    state.filter = tagButton.dataset.filterTag;
    $("filter").value = state.filter;
    renderFiles();
    return;
  }
  const menuButton = event.target.closest("[data-menu-for]");
  if (menuButton) {
    event.stopPropagation();
    const path = menuButton.dataset.menuFor;
    if (!state.selected.has(path)) selectOnly(path);
    const entry = findEntry(path);
    const rect = menuButton.getBoundingClientRect();
    const pasteInto = state.selected.size === 1 && entry && entry.dir ? entry.path : state.path;
    openSelectionMenu(rect.left, rect.bottom + 4, pasteInto);
    return;
  }
  const card = event.target.closest("[data-path]");
  if (!card) {
    if (event.target === $("files") || $("files").contains(event.target)) {
      // Empty-space click clears unless Ctrl/Cmd keeps the current selection.
      if (!(event.ctrlKey || event.metaKey)) {
        state.selected = new Set();
        state.anchor = "";
        paintSelection();
      }
    }
    return;
  }
  if (event.detail > 1) return;
  const path = card.dataset.path;
  // Windows Explorer-style: Ctrl/Cmd toggles, Shift selects a range.
  if (event.shiftKey) {
    event.preventDefault();
    selectRange(path);
  } else if (event.ctrlKey || event.metaKey) {
    event.preventDefault();
    toggleSelected(path);
  } else {
    selectOnly(path);
    if (!$("preview").hidden) {
      const entry = findEntry(path);
      if (entry && !entry.dir) {
        openEntry(entry).catch((err) => toast(err.message, true));
      }
    }
  }
});

$("files").addEventListener("mousedown", (event) => {
  if (event.button !== 0) return;
  if (event.target.closest(".more, button, a, input, [data-filter-tag], [data-menu-for]")) return;
  // Keep Ctrl/Shift clicks from selecting card text instead of toggling selection.
  if ((event.ctrlKey || event.metaKey || event.shiftKey) && event.target.closest("[data-path]")) {
    event.preventDefault();
  }
  // Rubber-band selection starts on empty space, not on a file card.
  if (event.target.closest("[data-path]")) return;
  beginMarquee(event);
});

function beginMarquee(event) {
  event.preventDefault();
  closeMenu();
  const additive = event.ctrlKey || event.metaKey;
  marquee = {
    x0: event.clientX,
    y0: event.clientY,
    additive,
    origin: additive ? new Set(state.selected) : new Set(),
    moved: false,
  };
  $("files").classList.add("selecting");
  const box = $("marquee");
  box.hidden = false;
  placeMarquee(event.clientX, event.clientY);
  window.addEventListener("mousemove", onMarqueeMove);
  window.addEventListener("mouseup", endMarquee);
}

function placeMarquee(x1, y1) {
  const box = $("marquee");
  const left = Math.min(marquee.x0, x1);
  const top = Math.min(marquee.y0, y1);
  box.style.left = `${left}px`;
  box.style.top = `${top}px`;
  box.style.width = `${Math.abs(x1 - marquee.x0)}px`;
  box.style.height = `${Math.abs(y1 - marquee.y0)}px`;
}

function onMarqueeMove(event) {
  if (!marquee) return;
  const dist = Math.hypot(event.clientX - marquee.x0, event.clientY - marquee.y0);
  if (!marquee.moved && dist < 3) return;
  marquee.moved = true;
  skipNextClick = true;
  placeMarquee(event.clientX, event.clientY);
  applyMarqueeSelection();
}

function applyMarqueeSelection() {
  const band = $("marquee").getBoundingClientRect();
  const next = new Set(marquee.origin);
  let lastHit = "";
  document.querySelectorAll("#files .card").forEach((card) => {
    const box = card.getBoundingClientRect();
    const hit = !(
      box.right < band.left
      || box.left > band.right
      || box.bottom < band.top
      || box.top > band.bottom
    );
    if (!hit) return;
    next.add(card.dataset.path);
    lastHit = card.dataset.path;
  });
  state.selected = next;
  if (lastHit) state.anchor = lastHit;
  paintSelection();
}

function endMarquee() {
  window.removeEventListener("mousemove", onMarqueeMove);
  window.removeEventListener("mouseup", endMarquee);
  $("files").classList.remove("selecting");
  const box = $("marquee");
  box.hidden = true;
  box.style.width = "0";
  box.style.height = "0";
  if (marquee && marquee.moved) skipNextClick = true;
  marquee = null;
}

$("files").addEventListener("dblclick", (event) => {
  const card = event.target.closest("[data-path]");
  if (!card || event.target.closest(".more")) return;
  openEntry(findEntry(card.dataset.path)).catch((err) => toast(err.message, true));
});

$("files").addEventListener("contextmenu", (event) => {
  if (!state.me) return;
  // Ctrl+click is multi-select (Explorer style), not a context menu shortcut.
  if (event.ctrlKey) {
    event.preventDefault();
    const card = event.target.closest("[data-path]");
    if (card) {
      toggleSelected(card.dataset.path);
      skipNextClick = true;
    }
    return;
  }
  event.preventDefault();
  const card = event.target.closest("[data-path]");
  if (card) {
    const path = card.dataset.path;
    if (!state.selected.has(path)) selectOnly(path);
    const entry = findEntry(path);
    const pasteInto = state.selected.size === 1 && entry && entry.dir ? entry.path : state.path;
    openSelectionMenu(event.clientX, event.clientY, pasteInto);
    return;
  }
  state.selected = new Set();
  state.anchor = "";
  paintSelection();
  openSelectionMenu(event.clientX, event.clientY, state.path);
});

$("menu").addEventListener("click", (event) => {
  const button = event.target.closest("[data-menu]");
  if (!button) return;
  event.preventDefault();
  event.stopPropagation();
  const entries = state.menuEntries.slice();
  const pasteInto = state.pasteInto;
  closeMenu();
  const action = button.getAttribute("data-menu");
  const one = entries.length === 1 ? entries[0] : null;
  if (action === "open" && one) openEntry(one).catch((err) => toast(err.message, true));
  if (action === "download") {
    downloadEntries(entries).catch((err) => {
      if (err.cancel) toast("Download cancelled");
      else toast(err.message, true);
    });
  }
  if (action === "copy") setClipboard("copy", entries);
  if (action === "cut") setClipboard("cut", entries);
  if (action === "paste") pasteClipboard(pasteInto).catch((err) => toast(err.message, true));
  if (action === "mkdir") createFolder().catch((err) => toast(err.message, true));
  if (action === "newfile") createFile().catch((err) => toast(err.message, true));
  if (action === "folderWith") createFolderWithSelection(entries).catch((err) => toast(err.message, true));
  if (action === "rename" && one) renameEntry(one).catch((err) => toast(err.message, true));
  if (action === "restore") restoreEntries(entries).catch((err) => toast(err.message, true));
  if (action === "delete") deleteEntries(entries).catch((err) => toast(err.message, true));
  if (action === "duplicate") duplicateEntries(entries).catch((err) => toast(err.message, true));
  if (action === "move") moveEntries(entries).catch((err) => toast(err.message, true));
});

document.addEventListener("click", (event) => {
  if (!event.target.closest("#menu") && !event.target.closest(".more")) closeMenu();
});

$("preview-close").addEventListener("click", closePreview);

$("preview-expand").addEventListener("click", () => {
  setPreviewFullscreen(!$("preview").classList.contains("is-fullscreen"));
});

$("notes-ack").addEventListener("click", async () => {
  if (!state.current) return;
  try {
    const data = await api("/api/annotations", { method: "POST", json: { path: state.current.path } });
    if (!state.current) return;
    renderAnnotations(data);
  } catch (err) {
    toast(err.message, true);
  }
});

$("notes-toggle").addEventListener("click", () => {
  const expanded = $("preview-notes").dataset.expanded === "true";
  setNotesExpanded(!expanded);
});

$("tag-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  if (!state.current) return;
  const tag = $("tag-input").value.trim();
  if (!tag) return;
  try {
    await api("/api/tags", { method: "POST", json: { path: state.current.path, tag } });
    $("tag-input").value = "";
    await loadAnnotations(state.current.path);
  } catch (err) {
    toast(err.message, true);
  }
});

$("comment-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  if (!state.current) return;
  const body = $("comment-input").value.trim();
  if (!body) return;
  try {
    await api("/api/comments", { method: "POST", json: { path: state.current.path, body } });
    $("comment-input").value = "";
    await loadAnnotations(state.current.path);
  } catch (err) {
    toast(err.message, true);
  }
});

$("preview-tags").addEventListener("click", async (event) => {
  const searchTag = event.target.closest("[data-search-tag]");
  if (searchTag) {
    openTagSearch(searchTag.dataset.searchTag);
    return;
  }
  const button = event.target.closest("[data-remove-tag]");
  if (!button || !state.current) return;
  try {
    await api(`/api/tags?path=${encodeURIComponent(state.current.path)}&tag=${encodeURIComponent(button.dataset.removeTag)}`, { method: "DELETE" });
    await loadAnnotations(state.current.path);
  } catch (err) {
    toast(err.message, true);
  }
});

$("preview-comments").addEventListener("click", async (event) => {
  const button = event.target.closest("[data-remove-comment]");
  if (!button || !state.current) return;
  try {
    await api(`/api/comments?id=${encodeURIComponent(button.dataset.removeComment)}`, { method: "DELETE" });
    await loadAnnotations(state.current.path);
  } catch (err) {
    toast(err.message, true);
  }
});
$("preview-rename").addEventListener("click", () => {
  if (state.current) renameEntry(state.current).catch((err) => toast(err.message, true));
});
$("preview-delete").addEventListener("click", () => {
  if (state.current) deleteEntry(state.current).catch((err) => toast(err.message, true));
});

$("filter").addEventListener("input", (event) => {
  state.filter = event.target.value;
  renderFiles();
});

$("sort").addEventListener("change", (event) => {
  state.sort = event.target.value;
  localStorage.setItem("ownnas-sort", state.sort);
  renderFiles();
});

$("sort-dir").addEventListener("click", () => {
  state.direction = state.direction === "asc" ? "desc" : "asc";
  localStorage.setItem("ownnas-dir", state.direction);
  syncControls();
  renderFiles();
});

$("view-icons").addEventListener("click", () => setView("icons"));
$("view-masonry").addEventListener("click", () => setView("masonry"));
$("view-list").addEventListener("click", () => setView("list"));

function setView(view) {
  state.view = normalizeView(view);
  localStorage.setItem("ownnas-view", state.view);
  syncControls();
  renderFiles();
}

$("hidden-toggle").addEventListener("change", (event) => {
  state.hidden = event.target.checked;
  localStorage.setItem("ownnas-hidden", state.hidden ? "1" : "0");
  load(state.path).catch((err) => toast(err.message, true));
});

$("mkdir-btn").addEventListener("click", () => {
  createFolder().catch((err) => toast(err.message, true));
});

$("newfile-btn").addEventListener("click", () => {
  createFile().catch((err) => toast(err.message, true));
});

$("upload-btn").addEventListener("click", () => $("upload-input").click());
$("folder-btn").addEventListener("click", () => $("folder-input").click());

async function onPicked(input) {
  try {
    const result = await uploadFiles(input.files);
    toast(uploadSummary(result));
  } catch (err) {
    toast(err.cancel ? "Upload cancelled" : err.message, !err.cancel);
  }
  input.value = "";
  if (state.me) load(state.path).catch((err) => toast(err.message, true));
}
$("upload-input").addEventListener("change", () => onPicked($("upload-input")));
$("folder-input").addEventListener("change", () => onPicked($("folder-input")));

window.addEventListener("hashchange", () => {
  if (!state.me) return;
  load(hashToPath()).catch((err) => toast(err.message, true));
});

window.addEventListener("keydown", (event) => {
  if (event.key === "Escape") {
    // Let open dialogs handle Escape themselves (and avoid clearing selection mid-prompt).
    if (document.querySelector("dialog[open]")) return;
    closeMenu();
    if ($("preview").classList.contains("is-fullscreen")) {
      setPreviewFullscreen(false);
      return;
    }
    closePreview();
    closeLibrary();
    closeSettings();
    if (marquee) endMarquee();
    if (state.selected.size) {
      state.selected = new Set();
      state.anchor = "";
      paintSelection();
    }
    return;
  }
  // Enter in the name dialog should confirm, not hit Cancel (first submit button).
  if (event.key === "Enter" && event.target && (event.target.id === "text-input" || event.target.id === "create-file-name")) {
    event.preventDefault();
    if (event.target.id === "create-file-name") $("create-file-ok").click();
    else $("text-ok").click();
    return;
  }
  if (document.querySelector("dialog[open]")) return;
  const typing = event.target.closest("input, textarea, select");
  if (typing) return;
  const key = event.key.toLowerCase();
  const command = event.metaKey || event.ctrlKey;
  if (command && key === "a") {
    event.preventDefault();
    const items = sortedEntries();
    state.selected = new Set(items.map((entry) => entry.path));
    state.anchor = items.length ? items[0].path : "";
    paintSelection();
  }
  if (command && key === "c") {
    const entries = selectedEntries();
    if (!entries.length) return;
    event.preventDefault();
    setClipboard("copy", entries);
  }
  if (command && key === "x" && state.me && !state.me.readonly) {
    const entries = selectedEntries();
    if (!entries.length) return;
    event.preventDefault();
    setClipboard("cut", entries);
  }
  if (event.key === "Delete" && state.me && !state.me.readonly) {
    const entries = selectedEntries();
    if (!entries.length) return;
    event.preventDefault();
    deleteEntries(entries).catch((err) => toast(err.message, true));
  }
  if (event.key === "ArrowUp" || event.key === "ArrowDown" || event.key === "ArrowLeft" || event.key === "ArrowRight") {
    if (!state.me || $("app-view").hidden) return;
    event.preventDefault();
    const extend = event.shiftKey;
    if (event.key === "ArrowUp") moveSelection(0, -1, extend);
    if (event.key === "ArrowDown") moveSelection(0, 1, extend);
    if (event.key === "ArrowLeft") moveSelection(-1, 0, extend);
    if (event.key === "ArrowRight") moveSelection(1, 0, extend);
  }
  if (event.key === "Enter") {
    const entries = selectedEntries();
    if (entries.length !== 1) return;
    event.preventDefault();
    openEntry(entries[0]).catch((err) => toast(err.message, true));
  }
});

window.addEventListener("paste", (event) => {
  if (!state.me || state.me.readonly) return;
  if ($("login-view") && !$("login-view").hidden) return;
  if (document.querySelector("dialog[open]")) return;

  const data = event.clipboardData;
  if (!data) return;

  const editingSensitive = event.target.closest("#login-view, #password-dialog, #text-dialog, #create-file-dialog, #comment-input, #tag-input, #search-input");
  const hasFiles = clipboardHasFiles(data);

  // File paste should upload even when the folder filter (or similar) is focused.
  if (hasFiles) {
    if (editingSensitive) return;
    event.preventDefault();
    event.stopPropagation();
    const pasted = filesFromPaste(data);
    if (!pasted.length) {
      toast("Nothing to upload from the clipboard", true);
      return;
    }
    uploadFiles(pasted.map((item) => item.file), pasted.map((item) => item.name))
      .then(async (result) => {
        toast(uploadSummary(result));
        if (state.me) await load(state.path);
      })
      .catch((err) => toast(err.cancel ? "Upload cancelled" : err.message, !err.cancel));
    return;
  }

  if (event.target.closest("input, textarea, select, [contenteditable]")) return;
  if (state.clipboard && state.clipboard.items.length) {
    event.preventDefault();
    pasteClipboard(state.path).catch((err) => toast(err.message, true));
  }
}, true);

function clipboardHasFiles(clipboardData) {
  if (!clipboardData) return false;
  if (clipboardData.files && clipboardData.files.length) return true;
  const types = clipboardData.types ? [...clipboardData.types] : [];
  if (types.includes("Files") || types.some((type) => type.startsWith("image/"))) return true;
  for (const item of clipboardData.items || []) {
    if (item.kind === "file") return true;
  }
  return false;
}

function filesFromPaste(clipboardData) {
  if (!clipboardData) return [];
  const out = [];
  const seen = new Set();
  const add = (file) => {
    if (!file) return;
    const key = `${file.name}|${file.size}|${file.lastModified}|${file.type}`;
    if (seen.has(key)) return;
    seen.add(key);
    out.push({ file, name: pasteFileName(file) });
  };

  // Prefer DataTransferItemList: screenshots and some OS pastes leave `files` empty.
  for (const item of clipboardData.items || []) {
    if (item.kind === "file") add(item.getAsFile());
  }
  if (out.length) return out;

  if (clipboardData.files && clipboardData.files.length) {
    for (const file of clipboardData.files) add(file);
  }
  return out;
}

function pasteFileName(file) {
  const raw = (file.name || "").trim();
  if (raw && raw !== "blob") return raw.includes("/") ? raw.split("/").pop() : raw;
  const type = file.type || "";
  const ext = type === "image/png" ? "png"
    : type === "image/jpeg" ? "jpg"
    : type === "image/webp" ? "webp"
    : type === "image/gif" ? "gif"
    : type === "image/svg+xml" ? "svg"
    : type.startsWith("text/") ? "txt"
    : "bin";
  const stamp = new Date().toISOString().replace(/[:.]/g, "-").slice(0, 19);
  return `paste-${stamp}.${ext}`;
}

let fileDragDepth = 0;

function draggingFiles(event) {
  const types = event.dataTransfer && event.dataTransfer.types;
  return !!types && [...types].includes("Files");
}

function showDrop() {
  $("drop").hidden = false;
  $("drop").classList.add("show");
}

function hideDrop() {
  fileDragDepth = 0;
  $("drop").classList.remove("show");
  $("drop").hidden = true;
}

window.addEventListener("dragenter", (event) => {
  if (!state.me || state.me.readonly || !draggingFiles(event)) return;
  event.preventDefault();
  fileDragDepth += 1;
  showDrop();
});
window.addEventListener("dragover", (event) => {
  if (!state.me || state.me.readonly || !draggingFiles(event)) return;
  event.preventDefault();
});
window.addEventListener("dragleave", () => {
  if (fileDragDepth === 0) return;
  fileDragDepth -= 1;
  if (fileDragDepth === 0) hideDrop();
});
window.addEventListener("dragend", hideDrop);
window.addEventListener("drop", async (event) => {
  hideDrop();
  if (!state.me || state.me.readonly || !draggingFiles(event)) return;
  event.preventDefault();
  try {
    const items = await readDrop(event.dataTransfer);
    const result = await uploadFiles(items.map((item) => item.file), items.map((item) => item.name));
    toast(uploadSummary(result));
  } catch (err) {
    toast(err.cancel ? "Upload cancelled" : err.message, !err.cancel);
  }
  if (state.me) load(state.path).catch((err) => toast(err.message, true));
});

async function refreshLibrary() {
  if (!state.me) return;
  const [bookmarks, recent] = await Promise.all([
    api("/api/bookmarks"),
    api("/api/recent"),
  ]);
  state.bookmarks = bookmarks.paths || [];
  $("bookmarks").innerHTML = state.bookmarks.length
    ? state.bookmarks.map((path) => {
      const label = libraryLabel(path);
      const detail = path || (state.me.rootName || "Library");
      return `<button type="button" data-go="${esc(path)}"><strong>${esc(label)}</strong><span class="path">${esc(detail)}</span></button>`;
    }).join("")
    : `<p class="empty-note">No bookmarks yet. Use Bookmark on a folder to save it here.</p>`;
  const items = recent.items || [];
  $("recents").innerHTML = items.length
    ? items.map((item) => {
      const parent = item.path.includes("/") ? item.path.slice(0, item.path.lastIndexOf("/")) : (state.me.rootName || "Library");
      return `<button type="button" data-open="${esc(item.path)}"><strong>${esc(item.name)}</strong><span class="path">${esc(parent || (state.me.rootName || "Library"))}</span></button>`;
    }).join("")
    : `<p class="empty-note">Open a file to see it here.</p>`;
  syncBookmarkBtn();
}

function imageStep(delta) {
  if (!state.current) return;
  const images = sortedEntries().filter((item) => item.kind === "image" || item.kind === "svg");
  const index = images.findIndex((item) => item.path === state.current.path);
  const next = images[index + delta];
  if (next) openEntry(next).catch((err) => toast(err.message, true));
}

async function duplicateEntry(entry) {
  await api("/api/duplicate", { method: "POST", json: { path: entry.path } });
  toast("Duplicated");
  await load(state.path);
}

async function moveEntry(entry) {
  const dest = await askText("Move", "Destination folder (empty for the library root)", state.path.includes("/") ? state.path.slice(0, state.path.lastIndexOf("/")) : "", "Move");
  if (dest === null) return;
  await api("/api/move", { method: "POST", json: { path: entry.path, dest } });
  toast("Moved");
  await load(state.path);
}

$("bookmark-btn").addEventListener("click", async () => {
  try {
    if (state.bookmarks.includes(state.path)) {
      await api(`/api/bookmarks?path=${encodeURIComponent(state.path)}`, { method: "DELETE" });
    } else {
      await api("/api/bookmarks", { method: "POST", json: { path: state.path } });
    }
    await refreshLibrary();
  } catch (err) {
    toast(err.message, true);
  }
});

$("library-btn").addEventListener("click", () => openLibrary());
$("library-close").addEventListener("click", closeLibrary);
$("settings-btn").addEventListener("click", () => openSettings());
$("settings-close").addEventListener("click", closeSettings);
$("settings-tab-theme").addEventListener("click", () => setSettingsTab("theme"));
$("settings-tab-users").addEventListener("click", () => {
  setSettingsTab("users");
  loadUsers().catch((err) => toast(err.message, true));
});
$("theme-grid").addEventListener("click", (event) => {
  const button = event.target.closest("[data-theme-id]");
  if (!button) return;
  applyTheme(button.dataset.themeId);
});
$("user-create-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  $("user-create-error").textContent = "";
  try {
    await api("/api/users", {
      method: "POST",
      json: {
        username: $("user-create-name").value.trim(),
        password: $("user-create-pass").value,
        admin: $("user-create-admin").checked,
      },
    });
    $("user-create-form").reset();
    toast("Account created");
    await loadUsers();
  } catch (err) {
    $("user-create-error").textContent = err.message;
  }
});
$("users-list").addEventListener("click", async (event) => {
  const passwordBtn = event.target.closest("[data-user-password]");
  const adminBtn = event.target.closest("[data-user-admin]");
  const removeBtn = event.target.closest("[data-user-remove]");
  try {
    if (passwordBtn) {
      const username = passwordBtn.dataset.userPassword;
      const password = await askText(`Reset password for ${username}`, "New password", "", "Save");
      if (password === null) return;
      if (password.trim().length < 8) {
        toast("Password must be at least 8 characters", true);
        return;
      }
      await api("/api/users/password", { method: "POST", json: { username, password } });
      toast("Password updated");
      return;
    }
    if (adminBtn) {
      const username = adminBtn.dataset.userAdmin;
      const admin = adminBtn.dataset.admin === "1";
      await api("/api/users/admin", { method: "POST", json: { username, admin } });
      toast(admin ? "Administrator granted" : "Administrator revoked");
      await loadUsers();
      return;
    }
    if (removeBtn) {
      const username = removeBtn.dataset.userRemove;
      if (!await askConfirm(`Remove account “${username}”?`, "Remove")) return;
      await api("/api/users/remove", { method: "POST", json: { username } });
      toast("Account removed");
      await loadUsers();
    }
  } catch (err) {
    toast(err.message, true);
  }
});
$("library-tab-bookmarks").addEventListener("click", () => setLibraryTab("bookmarks"));
$("library-tab-recent").addEventListener("click", () => setLibraryTab("recent"));
$("empty-trash-btn").addEventListener("click", () => {
  emptyTrash().catch((err) => toast(err.message, true));
});

$("usage-btn").addEventListener("click", async () => {
  try {
    const usage = await api(`/api/usage?path=${encodeURIComponent(state.path)}`);
    const extra = usage.truncated ? " Count stopped early." : "";
    toast(`${formatSize(usage.bytes, false)} in ${usage.files} files.${extra}`);
  } catch (err) {
    toast(err.message, true);
  }
});

$("download-btn").addEventListener("click", () => {
  downloadEntries(selectedEntries()).catch((err) => {
    if (err.cancel) toast("Download cancelled");
    else toast(err.message, true);
  });
});

$("zip-link").addEventListener("click", (event) => {
  event.preventDefault();
  const name = state.path ? state.path.split("/").pop() : (state.me.rootName || "Library");
  downloadZipBlob({
    url: `/api/zip?path=${encodeURIComponent(state.path)}`,
    preparing: `Compressing ${name}…`,
    fallbackName: `${name}.zip`,
    doneToast: "Download started",
  }).catch((err) => {
    if (err.cancel) toast("Download cancelled");
    else toast(err.message, true);
  });
});

$("status-cancel").addEventListener("click", () => cancelActiveTransfer());

function openTagSearch(tag) {
  $("search-results").innerHTML = "";
  $("search-input").value = `tag:${tag}`;
  $("search-dialog").showModal();
  runSearch().catch((err) => toast(err.message, true));
}

async function runSearch() {
  const data = await api(`/api/search?path=${encodeURIComponent(state.path)}&q=${encodeURIComponent($("search-input").value)}`);
  const hits = data.hits || [];
  $("search-results").innerHTML = hits.length
    ? hits.map((hit) => {
      const tag = hit.matchedTag ? ` <span class="tag-chip mini">#${esc(hit.matchedTag)}</span>` : "";
      return `<button type="button" data-hit="${esc(hit.path)}" data-dir="${hit.dir ? "1" : "0"}">${esc(hit.path)}${tag}</button>`;
    }).join("")
    : `<span class="muted">No matches</span>`;
}

$("search-btn").addEventListener("click", () => {
  $("search-results").innerHTML = "";
  $("search-dialog").showModal();
  $("search-input").focus();
});
$("search-close").addEventListener("click", () => $("search-dialog").close());
$("search-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  try {
    await runSearch();
  } catch (err) {
    toast(err.message, true);
  }
});
$("search-results").addEventListener("click", (event) => {
  const button = event.target.closest("[data-hit]");
  if (!button) return;
  $("search-dialog").close();
  if (button.dataset.dir === "1") go(button.dataset.hit).catch((err) => toast(err.message, true));
  else openEntry({ path: button.dataset.hit, name: button.dataset.hit.split("/").pop(), dir: false, size: 0, modified: 0, kind: "file" }).catch((err) => toast(err.message, true));
});

$("activity-btn").addEventListener("click", async () => {
  try {
    const data = await api("/api/activity");
    const events = data.events || [];
    $("activity-list").innerHTML = events.length
      ? events.map((item) => `<li><span>${esc(item.username)} ${esc(item.action)} ${esc(item.detail)}</span><span class="muted">${esc(formatDate(item.createdAt))}</span></li>`).join("")
      : `<li>No activity yet</li>`;
    $("activity-dialog").showModal();
  } catch (err) {
    toast(err.message, true);
  }
});

$("preview-prev").addEventListener("click", () => imageStep(-1));
$("preview-next").addEventListener("click", () => imageStep(1));
$("preview-hash").addEventListener("click", async () => {
  if (!state.current) return;
  $("preview-hash").textContent = "Hashing…";
  try {
    const data = await api(`/api/hash?path=${encodeURIComponent(state.current.path)}`);
    $("preview-meta").textContent = `${$("preview-meta").textContent} · ${data.sha256}`;
    $("preview-hash").textContent = "SHA-256";
  } catch (err) {
    $("preview-hash").textContent = "SHA-256";
    toast(err.message, true);
  }
});
$("preview-duplicate").addEventListener("click", () => {
  if (state.current) duplicateEntry(state.current).catch((err) => toast(err.message, true));
});
$("preview-move").addEventListener("click", () => {
  if (state.current) moveEntry(state.current).catch((err) => toast(err.message, true));
});

$("clipboard-clear").addEventListener("click", clearClipboard);
$("clipboard-paste").addEventListener("click", () => {
  pasteClipboard(state.path).catch((err) => toast(err.message, true));
});

$("bookmarks").addEventListener("click", (event) => {
  const button = event.target.closest("[data-go]");
  if (!button) return;
  closeLibrary();
  go(button.dataset.go).catch((err) => toast(err.message, true));
});
$("recents").addEventListener("click", async (event) => {
  const button = event.target.closest("[data-open]");
  if (!button) return;
  const path = button.dataset.open;
  const parent = path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : "";
  try {
    closeLibrary();
    if (state.path !== parent) await go(parent);
    const entry = findEntry(path);
    if (entry) await openEntry(entry);
    else await openEntry({ path, name: path.split("/").pop(), dir: false, size: 0, modified: 0, kind: "file" });
  } catch (err) {
    toast(err.message, true);
  }
});
