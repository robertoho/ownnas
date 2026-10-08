const state = {
  me: null,
  path: "",
  entries: [],
  truncated: false,
  total: 0,
  view: normalizeView(localStorage.getItem("ownnas-view")),
  groupBy: normalizeGroup(localStorage.getItem("ownnas-group")),
  collapsedGroups: new Set(),
  detailWidths: readDetailWidths(),
  tileSize: normalizeTileSize(localStorage.getItem("ownnas-tile-size")),
  sort: localStorage.getItem("ownnas-sort") || "name",
  direction: localStorage.getItem("ownnas-dir") || "asc",
  hidden: localStorage.getItem("ownnas-hidden") === "1",
  filter: "",
  searchController: null,
  searchHits: [],
  current: null,
  editor: null,
  folderSizeGen: 0,
  folderSizes: new Map(),
  selected: new Set(),
  anchor: "",
  focus: "",
  clipboard: null,
  menuEntries: [],
  pasteInto: "",
  bookmarks: [],
};
let updateNoticeVersion = "";
let updateStatusTimer = null;

function normalizeGroup(value) {
  return ["type", "date", "tag"].includes(value) ? value : "none";
}

function readDetailWidths() {
  const fallback = [280, 112, 128, 176, 176];
  try {
    const saved = JSON.parse(localStorage.getItem("ownnas-detail-widths"));
    return fallback.map((width, i) => Number.isFinite(saved?.[i]) ? Math.max(i === 0 ? 180 : 72, Math.min(800, saved[i])) : width);
  } catch { return fallback; }
}

function normalizeView(value) {
  if (value === "grid") return "masonry";
  if (value === "masonry" || value === "list" || value === "icons" || value === "columns") return value;
  return "icons";
}

function normalizeTileSize(value) {
  const size = Number(value);
  if (!Number.isFinite(size) || size <= 0) return 180;
  return Math.max(96, Math.min(360, Math.round(size / 12) * 12));
}

function syncTileSize() {
  $("files").style.setProperty("--tile-size", `${state.tileSize}px`);
  $("search-results").style.setProperty("--tile-size", `${state.tileSize}px`);
  $("tile-size").value = state.tileSize;
  $("tile-size").setAttribute("aria-valuetext", `${state.tileSize} pixels`);
  $("tile-size").title = `Tile size: ${state.tileSize} px`;
  $("tile-size-control").hidden = state.view === "list" || state.view === "columns";
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

const undoHistory = [];
let undoBusy = false;
let columnGeneration = 0;
let columnEntries = new Map();
function syncUndo() {
  const button = $("undo-btn");
  if (!button) return;
  button.disabled = undoBusy || !undoHistory.length || !state.me || state.me.readonly;
  button.title = undoHistory.length ? `Undo ${undoHistory.at(-1).label} (Ctrl/Cmd+Z)` : "Nothing to undo";
}
async function undoFileOperation() {
  if (undoBusy || !undoHistory.length || !state.me || state.me.readonly) return;
  undoBusy = true; syncUndo();
  const operation = undoHistory.at(-1);
  try {
    await api("/api/undo-move", { method: "POST", json: { path: operation.path, dest: operation.dest } });
    undoHistory.pop();
    invalidateFolderSizes();
    await load(state.path);
    toast(`Undid ${operation.label}`);
  } catch (err) { toast(err.message, true); }
  finally { undoBusy = false; syncUndo(); }
}
async function renderColumns() {
  const generation = ++columnGeneration;
  const path = state.path;
  const paths = [""];
  for (const part of path.split("/").filter(Boolean)) paths.push(paths.at(-1) ? `${paths.at(-1)}/${part}` : part);
  try {
    const listings = await Promise.all(paths.map(p => p === path ? Promise.resolve({entries: state.entries}) : api(`/api/list?path=${encodeURIComponent(p)}${state.hidden ? "&hidden=1" : ""}`)));
    if (generation !== columnGeneration || state.view !== "columns" || state.path !== path) return;
    columnEntries = new Map();
    $("files").innerHTML = paths.map((p, index) => {
      const entries = index === paths.length - 1 ? sortedEntries() : listings[index].entries;
      return `<section class="finder-column" data-column-path="${esc(p)}" aria-label="${esc(p || state.me.rootName || 'Library')}"><div class="finder-column-title">${esc(p.split('/').pop() || state.me.rootName || 'Library')}</div>${entries.map(entry => {
        columnEntries.set(entry.path, entry);
        return `<button type="button" class="column-entry ${paths[index+1] === entry.path ? 'active' : ''}" draggable="${!state.me.readonly}" data-column-entry="${esc(entry.path)}" data-dir="${entry.dir ? 1 : 0}"><span class="column-glyph">${entryIcon(entry)}</span><span>${esc(entry.name)}</span>${entry.dir ? '<span>›</span>' : ''}</button>`;
      }).join('')}</section>`;
    }).join('');
    $("files").lastElementChild?.scrollIntoView({block: "nearest", inline: "nearest"});
  } catch (err) { if (generation === columnGeneration) toast(err.message, true); }
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
  let response;
  try {
    response = await fetch(url, {
      method,
      headers,
      body,
      credentials: "same-origin",
      signal: options.signal,
    });
  } catch (err) {
    if (err && (err.name === "AbortError" || options.signal?.aborted)) {
      const abortErr = new Error("Cancelled");
      abortErr.name = "AbortError";
      abortErr.cancel = true;
      throw abortErr;
    }
    throw err;
  }
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
    const message = (data && data.error) || "Request failed";
    if (message === "Cancelled" || options.signal?.aborted) {
      const abortErr = new Error("Cancelled");
      abortErr.name = "AbortError";
      abortErr.cancel = true;
      throw abortErr;
    }
    throw new Error(message);
  }
  if (!undoBusy && state.me) {
    if (method === "DELETE" && url.startsWith("/api/entry?") && data.action === "deleted" || url === "/api/trash/empty") { undoHistory.length = 0; syncUndo(); }
    const request = options.json || (["/api/rename", "/api/move"].includes(url) && typeof options.body === "string" ? JSON.parse(options.body) : {});
    let operation;
    if (["/api/rename", "/api/move"].includes(url) && method === "POST") operation = { path: data.path, dest: request.path, label: url.endsWith("rename") ? "rename" : "move" };
    if (method === "DELETE" && url.startsWith("/api/entry?") && data.action === "trashed") operation = { path: data.path, dest: new URL(url, location.href).searchParams.get("path"), label: "move to Trash" };
    if (operation?.path) { undoHistory.push(operation); if (undoHistory.length > 100) undoHistory.shift(); syncUndo(); }
    if (url === "/api/logout") { undoHistory.length = 0; syncUndo(); }
  }
  return data;
}

function isCancelError(err) {
  return !!(err && (err.cancel || err.name === "AbortError" || err.message === "Cancelled"));
}

function showLogin(message) {
  undoHistory.length = 0;
  syncUndo();
  clearEditor();
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
  const whoLabel = state.me.admin ? `${state.me.username} · admin` : state.me.username;
  $("who").textContent = whoLabel;
  const settingsBtn = $("settings-btn");
  settingsBtn.title = `Settings (${whoLabel})`;
  settingsBtn.setAttribute("aria-label", `Settings (${whoLabel})`);
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

function formatFolderSize(info) {
  if (!info) return "Folder";
  const label = formatSize(info.bytes || 0, false);
  return info.truncated ? `${label}+` : label;
}

function invalidateFolderSizes() {
  state.folderSizes.clear();
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function formatDate(seconds) {
  if (!seconds) return "";
  return new Date(seconds * 1000).toLocaleString();
}

function formatDateShort(seconds) {
  if (!seconds) return "";
  return new Date(seconds * 1000).toLocaleString(undefined, {
    year: "2-digit",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
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
    "archive-folder": `
      <path d="M2.2 4.1h11.6v2.35H2.2z" fill="currentColor" opacity="0.28"/>
      <path d="M3.15 6.45h9.7v6.2a1.35 1.35 0 0 1-1.35 1.35h-7a1.35 1.35 0 0 1-1.35-1.35z" fill="currentColor" opacity="0.16"/>
      <path d="M2.2 4.1h11.6v2.35H2.2z" fill="none" stroke="currentColor" stroke-width="1.35" stroke-linejoin="round"/>
      <path d="M3.15 6.45h9.7v6.2a1.35 1.35 0 0 1-1.35 1.35h-7a1.35 1.35 0 0 1-1.35-1.35z" fill="none" stroke="currentColor" stroke-width="1.35" stroke-linejoin="round"/>
      <path d="M2.55 3.15h10.9l.55.95H2z" fill="currentColor" opacity="0.2"/>
      <path d="M2.55 3.15h10.9l.55.95H2z" fill="none" stroke="currentColor" stroke-width="1.25" stroke-linejoin="round"/>
      <rect x="6.85" y="7.55" width="2.3" height="3.55" rx="0.55" fill="currentColor" opacity="0.22"/>
      <rect x="6.85" y="7.55" width="2.3" height="3.55" rx="0.55" fill="none" stroke="currentColor" stroke-width="1.2"/>
      <path d="M7.35 9.05h1.3M7.35 10.35h1.3" stroke="currentColor" stroke-width="1.15" stroke-linecap="round"/>
      <path d="M4.4 12.15h7.2" stroke="currentColor" stroke-width="1.15" stroke-linecap="round" opacity="0.7"/>
    `,
    "trash-folder": `
      <path d="M4.05 5.85h7.9l-.55 7.05a1.4 1.4 0 0 1-1.4 1.3H6a1.4 1.4 0 0 1-1.4-1.3z" fill="currentColor" opacity="0.18"/>
      <path d="M4.05 5.85h7.9l-.55 7.05a1.4 1.4 0 0 1-1.4 1.3H6a1.4 1.4 0 0 1-1.4-1.3z" fill="none" stroke="currentColor" stroke-width="1.35" stroke-linejoin="round"/>
      <path d="M2.65 5.85h10.7" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/>
      <path d="M5.55 5.85V4.05A1.15 1.15 0 0 1 6.7 2.9h2.6a1.15 1.15 0 0 1 1.15 1.15v1.8" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/>
      <path d="M6.7 2.9h2.6" stroke="currentColor" stroke-width="1.25" stroke-linecap="round"/>
      <path d="M6.35 7.7v4.15M8 7.7v4.15M9.65 7.7v4.15" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" opacity="0.9"/>
      <path d="M4.7 5.85l.2-1.05h6.2l.2 1.05" fill="currentColor" opacity="0.22"/>
    `,
    image: `<rect x="2.5" y="3.5" width="11" height="9" rx="1.5" fill="none" stroke="currentColor" stroke-width="1.5"/><circle cx="6" cy="7" r="1.1" fill="currentColor"/><path d="M2.8 11.2 6.2 8.4l2.1 1.7 2.2-2.5 2.7 3.6" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/>`,
    svg: `<rect x="2.5" y="3.5" width="11" height="9" rx="1.5" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M5 10.5 8 5.5l3 5" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>`,
    video: `<rect x="2.5" y="3.5" width="11" height="9" rx="1.5" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M7 6.2v3.6L10.2 8z" fill="currentColor"/>`,
    audio: `<path d="M5 10.5a1.7 1.7 0 1 0 0 .2V6.2L12 4.8v4.5" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/><circle cx="5" cy="11" r="1.7" fill="none" stroke="currentColor" stroke-width="1.5"/><circle cx="12" cy="9.5" r="1.7" fill="none" stroke="currentColor" stroke-width="1.5"/>`,
    pdf: `<path d="M4.5 2.5h5L12.5 5v8.5A1.5 1.5 0 0 1 11 15H5A1.5 1.5 0 0 1 3.5 13.5v-10A1 1 0 0 1 4.5 2.5z" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round"/><path d="M9.5 2.6V5h2.4M5.5 9h5M5.5 11.5h3.5" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/>`,
    model3d: `<path d="M8 2.2 13.2 5v6L8 13.8 2.8 11V5z" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round"/><path d="M8 2.2V8m0 0 5.2 3M8 8 2.8 11" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/>`,
    cad2d: `<path d="M2.5 12.8h11M3.2 11.1l3-3 2.1 1.6 4.5-5.2" fill="none" stroke="currentColor" stroke-width="1.35" stroke-linecap="round" stroke-linejoin="round"/><circle cx="3.2" cy="11.1" r=".8" fill="currentColor"/><circle cx="12.8" cy="4.5" r=".8" fill="currentColor"/>`,
    text: `<path d="M4.5 2.5h5L12.5 5v8.5A1.5 1.5 0 0 1 11 15H5A1.5 1.5 0 0 1 3.5 13.5v-10A1 1 0 0 1 4.5 2.5z" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round"/><path d="M9.5 2.6V5h2.4M5.8 8.5h4.4M5.8 11h3" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/>`,
    archive: `<path d="M2.4 3.4h11.2v2.2H2.4z" fill="currentColor" opacity="0.2"/><path d="M2.4 3.4h11.2v2.2H2.4z" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round"/><path d="M3.4 5.6h9.2v7.1A1.3 1.3 0 0 1 11.3 14H4.7A1.3 1.3 0 0 1 3.4 12.7z" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round"/><path d="M7.15 7.5h1.7v3.2H7.15z" fill="none" stroke="currentColor" stroke-width="1.2"/><path d="M7.4 8.9h1.2" stroke="currentColor" stroke-width="1.15" stroke-linecap="round"/>`,
    file: `<path d="M4.5 2.5h5L12.5 5v8.5A1.5 1.5 0 0 1 11 15H5A1.5 1.5 0 0 1 3.5 13.5v-10A1 1 0 0 1 4.5 2.5z" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round"/><path d="M9.5 2.6V5h2.4" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round"/>`,
  };
  const body = icons[kind] || (kind === "spreadsheet"
    ? '<rect x="2.5" y="2.5" width="11" height="11" rx="1" fill="none" stroke="currentColor" stroke-width="1.4"/><path d="M2.8 6.2h10.4M2.8 9.7h10.4M6.2 2.8v10.4M9.8 2.8v10.4" stroke="currentColor" stroke-width="1.1"/>'
    : icons.file);
  const special = kind === "archive-folder" || kind === "trash-folder";
  return `<svg class="type-icon${special ? " type-icon-special" : ""}" viewBox="0 0 16 16" width="28" height="28" aria-hidden="true">${body}</svg>`;
}

const FOLDER_ICONS = {
  folder: { label: 'Folder', kind: 'folder' },
  documents: { label: 'Documents', kind: 'text' },
  photos: { label: 'Photos', kind: 'image' },
  music: { label: 'Music', kind: 'audio' },
  video: { label: 'Video', kind: 'video' },
  code: { label: 'Code', body: '<path d="m6 4-4 4 4 4m4-8 4 4-4 4" fill="none" stroke="currentColor" stroke-width="1.5"/>' },
  work: { label: 'Work', body: '<rect x="2" y="5" width="12" height="9" rx="1.5" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M5 5V2h6v3M2 9h12M7 8v3h2V8" fill="none" stroke="currentColor" stroke-width="1.5"/>' },
  star: { label: 'Star', body: '<path d="m8 1.5 2 4 4.5.7-3.3 3.2.8 4.5-4-2.1-4 2.1.8-4.5L1.5 6.2 6 5.5z" fill="currentColor"/>' },
  heart: { label: 'Heart', body: '<path d="M8 14S1.5 10 1.5 5.5C1.5 1.5 6 1 8 4c2-3 6.5-2.5 6.5 1.5C14.5 10 8 14 8 14z" fill="currentColor"/>' },
  archive: { label: 'Archive', kind: 'archive' },
  cloud: { label: 'Cloud', body: '<path d="M4 12h8a3 3 0 0 0 .5-6A4.5 4.5 0 0 0 4 5a3.5 3.5 0 0 0 0 7z" fill="none" stroke="currentColor" stroke-width="1.5"/>' },
};
function folderIcon(icon) {
  const definition = Object.hasOwn(FOLDER_ICONS, icon) ? FOLDER_ICONS[icon] : FOLDER_ICONS.folder;
  return definition.kind ? typeIcon(definition.kind) : `<svg class="type-icon" viewBox="0 0 16 16" width="28" height="28" aria-hidden="true">${definition.body}</svg>`;
}
function entryIcon(entry) {
  if (!entry.dir || !entry.folderAppearance || ['archive-folder', 'trash-folder'].includes(entry.kind)) return typeIcon(entry.kind);
  const { color, icon } = entry.folderAppearance;
  const style = /^#[0-9a-f]{6}$/i.test(color || '') ? ` style="color:${color}"` : '';
  return `<span class="folder-custom-icon"${style}>${folderIcon(icon)}</span>`;
}
function customizableFolder(entry) {
  return entry.dir && !!entry.path && !inTrashPath(entry.path) && !['archive-folder', 'trash-folder'].includes(entry.kind) && !['.ownnas-archive', 'Archive'].includes(entry.path);
}
async function customizeFolders(entries) {
  const folders = entries.length ? entries : [{ path: state.path, name: state.path.split('/').pop(), dir: true, kind: 'folder' }];
  if (!folders.every(customizableFolder)) return;
  const appearance = await api(`/api/folder-appearance?path=${encodeURIComponent(folders[0].path)}`);
  const dialog = $('folder-appearance-dialog');
  let color = appearance?.color || '', icon = appearance?.icon || 'folder';
  $('folder-appearance-title').textContent = folders.length === 1 ? `Customize ${folders[0].name}` : `Customize ${folders.length} folders`;
  $('folder-appearance-icons').innerHTML = Object.entries(FOLDER_ICONS).map(([id, definition]) => `<button type="button" data-folder-icon="${id}" aria-label="${definition.label}" title="${definition.label}">${folderIcon(id)}<span>${definition.label}</span></button>`).join('');
  const colors = ['', '#eab308', '#f97316', '#ef4444', '#ec4899', '#a855f7', '#3b82f6', '#06b6d4', '#22c55e', '#94a3b8'];
  $('folder-appearance-colors').innerHTML = colors.map(value => `<button type="button" data-folder-color="${value}" aria-label="${value || 'Default color'}" title="${value || 'Default color'}"${value ? ` style="background:${value}"` : ''}>${value ? '' : '↺'}</button>`).join('');
  const paint = () => {
    $('folder-appearance-preview').innerHTML = `<span class="folder-custom-icon"${color ? ` style="color:${color}"` : ''}>${folderIcon(icon)}</span><span>${esc(folders.length === 1 ? folders[0].name : `${folders.length} folders`)}</span>`;
    $('folder-appearance-color').value = color || '#3b82f6';
    dialog.querySelectorAll('[data-folder-icon]').forEach(button => button.setAttribute('aria-pressed', String(button.dataset.folderIcon === icon)));
    dialog.querySelectorAll('[data-folder-color]').forEach(button => button.setAttribute('aria-pressed', String(button.dataset.folderColor === color)));
  };
  paint(); dialog.returnValue = ''; dialog.showModal();
  const choice = await new Promise(resolve => {
    const click = event => {
      const iconButton = event.target.closest('[data-folder-icon]');
      const colorButton = event.target.closest('[data-folder-color]');
      if (iconButton) icon = iconButton.dataset.folderIcon;
      if (colorButton) color = colorButton.dataset.folderColor;
      if (event.target.id === 'folder-appearance-reset') { icon = 'folder'; color = ''; }
      paint();
    };
    const input = () => { color = $('folder-appearance-color').value; paint(); };
    dialog.addEventListener('click', click); $('folder-appearance-color').addEventListener('input', input);
    dialog.addEventListener('close', () => {
      dialog.removeEventListener('click', click); $('folder-appearance-color').removeEventListener('input', input);
      resolve(dialog.returnValue === 'save' ? { color, icon } : null);
    }, { once: true });
  });
  if (!choice) return;
  await api('/api/folder-appearance', { method: 'POST', json: { paths: folders.map(folder => folder.path), ...choice } });
  await load(state.path, { keepPreview: true });
  toast('Folder appearance saved');
}

function sortedEntries() {
  const raw = state.filter.trim();
  const lower = raw.toLowerCase();
  const tagQuery = lower.startsWith("tag:") ? lower.slice(4).trim() : "";
  const items = state.entries.filter((entry) => {
    if (!raw) return true;
    if (tagQuery) {
      return (entry.tags || []).some((tag) => tag.toLowerCase() === tagQuery || tag.toLowerCase().includes(tagQuery));
    }
    if (entry.name.toLowerCase().includes(lower)) return true;
    return (entry.tags || []).some((tag) => tag.toLowerCase().includes(lower));
  });
  const factor = state.direction === "desc" ? -1 : 1;
  items.sort((a, b) => {
    if (a.dir !== b.dir) return a.dir ? -1 : 1;
    if (state.sort === "size") return (a.size - b.size) * factor;
    if (state.sort === "modified") return (a.modified - b.modified) * factor;
    if (state.sort === "created") return ((a.created || 0) - (b.created || 0)) * factor;
    if (state.sort === "type") return fileTypeLabel(a).localeCompare(fileTypeLabel(b), undefined, { numeric: true }) * factor || a.name.localeCompare(b.name);
    return a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: "base" }) * factor;
  });
  return items;
}

function fileTypeLabel(entry) {
  if (entry.dir) return "Folder";
  const labels = { image: "image", video: "video", audio: "audio", model3d: "3D model", cad2d: "CAD drawing", pdf: "document", text: "document", spreadsheet: "spreadsheet", archive: "archive", svg: "image" };
  const ext = entry.name.includes(".") ? entry.name.split(".").pop().toUpperCase() : "";
  return ext ? `${ext} ${labels[entry.kind] || "file"}` : "File";
}

function dateGroup(entry, now = new Date()) {
  if (!entry.modified) return "Unknown date";
  const midnight = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const date = new Date(entry.modified * 1000);
  const day = new Date(date.getFullYear(), date.getMonth(), date.getDate());
  const days = Math.round((midnight - day) / 86400000);
  if (days <= 0) return "Today";
  if (days === 1) return "Yesterday";
  if (days < 7) return "Previous 7 days";
  if (days < 30) return "Previous 30 days";
  return "Older";
}

function groupEntries(items) {
  if (state.groupBy === "none") return [{ label: "", items }];
  const groups = new Map();
  for (const entry of items) {
    const labels = state.groupBy === "type" ? [fileTypeLabel(entry)]
      : state.groupBy === "date" ? [dateGroup(entry)]
      : entry.tags?.length ? [...new Set(entry.tags)] : ["Untagged"];
    for (const label of labels) {
      if (!groups.has(label)) groups.set(label, []);
      groups.get(label).push(entry);
    }
  }
  const dates = ["Today", "Yesterday", "Previous 7 days", "Previous 30 days", "Older", "Unknown date"];
  return [...groups].sort(([a], [b]) => state.groupBy === "date" ? dates.indexOf(a) - dates.indexOf(b) : a.localeCompare(b, undefined, { numeric: true }))
    .map(([label, items]) => ({ label, items }));
}

function displayedEntries() {
  const seen = new Set();
  return groupEntries(sortedEntries()).flatMap((group) => state.collapsedGroups.has(`${state.groupBy}:${group.label}`) ? [] : group.items)
    .filter((entry) => !seen.has(entry.path) && seen.add(entry.path));
}

const DETAIL_COLUMNS = [["name", "Name"], ["size", "Size"], ["type", "Type"], ["modified", "Modified"], ["created", "Created"]];

function applyDetailWidths() {
  const [name, ...rest] = state.detailWidths;
  $("files").style.setProperty("--detail-columns", `minmax(${name}px, 1fr) ${rest.map((width) => `${width}px`).join(" ")} 40px`);
  $("files").style.setProperty("--detail-min-width", `${state.detailWidths.reduce((a, b) => a + b, 0) + 40}px`);
}

function detailHeader() {
  return `<div class="details-header" role="row">${DETAIL_COLUMNS.map(([key, label], i) => {
    const sort = state.sort === key ? (state.direction === "asc" ? "ascending" : "descending") : "none";
    return `<div class="detail-heading" role="columnheader" aria-sort="${sort}">
      <button type="button" data-detail-sort="${key}">${label}${sort === "none" ? "" : sort === "ascending" ? " ↑" : " ↓"}</button>
      <span class="detail-resize" data-detail-resize="${i}" role="separator" tabindex="0" aria-label="Resize ${label} column" aria-orientation="vertical" aria-valuemin="${i === 0 ? 180 : 72}" aria-valuemax="800" aria-valuenow="${state.detailWidths[i]}"></span>
    </div>`;
  }).join("")}<span role="columnheader" aria-label="Actions"></span></div>`;
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
  host.className = `files ${state.view}${state.groupBy !== "none" ? " grouped" : ""}`;
  host.setAttribute("role", state.view === "list" ? "table" : "region");
  if (state.view === "columns") {
    $("empty").hidden = true;
    renderColumns();
    updateSelectionSummary();
    return;
  }
  ++columnGeneration;
  applyDetailWidths();
  const items = sortedEntries();
  $("empty").hidden = items.length !== 0;
  $("empty").textContent = state.entries.length === 0 ? "This folder is empty." : "Nothing matches in this folder.";
  const banner = $("banner");
  if (banner) {
    banner.hidden = !state.truncated;
    banner.textContent = state.truncated ? `Showing the first ${state.entries.length} of ${state.total} items.` : "";
  }
  const renderCard = (entry) => {
    const modelThumb = entry.kind === "model3d"
      ? `<span class="model-thumbnail" data-model-path="${esc(entry.path)}" data-model-name="${esc(entry.name)}" data-model-modified="${entry.modified}" data-model-size="${entry.size}"><span class="glyph kind-model3d" aria-hidden="true">${typeIcon("model3d")}</span></span>`
      : null;
    const thumb = modelThumb || (entry.thumb
      ? `<img alt="" loading="lazy" src="/api/thumb?path=${encodeURIComponent(entry.path)}&v=${entry.modified}">`
      : entry.kind === "svg"
        ? `<img alt="" loading="lazy" src="/api/raw?path=${encodeURIComponent(entry.path)}">`
        : `<span class="glyph kind-${esc(entry.kind)}" aria-hidden="true">${entryIcon(entry)}</span>`);
    const selected = state.selected.has(entry.path);
    const cut = state.clipboard && state.clipboard.mode === "cut" && state.clipboard.items.some((item) => item.path === entry.path);
    const archiveFolder = entry.kind === "archive-folder";
    const trashFolder = entry.kind === "trash-folder";
    const mediaCard = ["image", "svg", "video", "model3d"].includes(entry.kind);
    const displayName = archiveFolder ? "Archive" : trashFolder ? "Trash" : entry.name;
    const badge = archiveFolder
      ? `<span class="badge">Archive</span>`
      : trashFolder
        ? `<span class="badge trash">Trash</span>`
        : "";
    const cachedSize = entry.dir
      ? (state.folderSizes.get(entry.path)
        || (entry.measuredSize != null
          ? { bytes: entry.measuredSize, truncated: !!entry.measuredTruncated }
          : null))
      : null;
    const sub = trashFolder
      ? "OwnNAS trash"
      : archiveFolder
        ? "OwnNAS archive"
        : entry.original
          ? `From ${esc(entry.original)}`
          : entry.dir
            ? esc(formatFolderSize(cachedSize))
            : esc(formatSize(entry.size, false));
    const createdLabel = formatDateShort(entry.created || entry.modified);
    const modifiedLabel = formatDateShort(entry.modified);
    const dates = (!trashFolder && !archiveFolder && (createdLabel || modifiedLabel))
      ? `<div class="meta-dates">
          ${createdLabel ? `<span title="Created ${esc(formatDate(entry.created || entry.modified))}">Created ${esc(createdLabel)}</span>` : ""}
          ${modifiedLabel ? `<span title="Modified ${esc(formatDate(entry.modified))}">Modified ${esc(modifiedLabel)}</span>` : ""}
        </div>`
      : "";
    const tags = entry.tags || [];
    const shownTags = tags.slice(0, 3);
    const tagStack = shownTags.length
      ? `<div class="tag-stack">${shownTags.map((tag) => {
          const color = tagColor(tag);
          return `<button type="button" class="tag-badge" data-filter-tag="${esc(tag)}" title="${esc(tag)}" style="--tag-bg:${color}">${esc(tagInitials(tag))}</button>`;
        }).join("")}${tags.length > 3 ? `<span class="tag-badge more-tags" title="${esc(tags.slice(3).join(", "))}">+${tags.length - 3}</span>` : ""}</div>`
      : "";
    if (state.view === "list") {
      return `<article class="card details-row${selected ? " selected" : ""}${cut ? " cut" : ""}" role="row" data-path="${esc(entry.path)}" data-dir="${entry.dir ? "1" : "0"}" aria-selected="${selected ? "true" : "false"}">
        <div class="detail-name" role="cell"><div class="thumb">${thumb}</div><span class="name" title="${esc(entry.path)}">${esc(displayName)}</span></div>
        <div class="sub detail-size" role="cell">${sub}</div>
        <div class="detail-cell" role="cell">${esc(fileTypeLabel(entry))}</div>
        <div class="detail-cell" role="cell" title="${esc(formatDate(entry.modified))}">${esc(modifiedLabel || "—")}</div>
        <div class="detail-cell" role="cell" title="${entry.created ? esc(formatDate(entry.created)) : ""}">${esc(entry.created ? formatDateShort(entry.created) : "—")}</div>
        <div role="cell"><button class="more" type="button" data-menu-for="${esc(entry.path)}" aria-label="Actions for ${esc(displayName)}">···</button></div>
      </article>`;
    }
    return `<article class="card${selected ? " selected" : ""}${cut ? " cut" : ""}${archiveFolder ? " archive-folder" : ""}${trashFolder ? " trash-folder" : ""}${mediaCard ? " media-card" : ""}" data-path="${esc(entry.path)}" data-dir="${entry.dir ? "1" : "0"}" aria-selected="${selected ? "true" : "false"}">
      <div class="thumb">${thumb}${badge}${tagStack}</div>
      <div class="card-body">
        <div class="name" title="${esc(entry.path)}">${esc(displayName)}</div>
        <div class="sub" title="${entry.original ? esc(entry.original) : ""}">${sub}</div>
        ${dates}
      </div>
      <button class="more" type="button" data-menu-for="${esc(entry.path)}" aria-label="Actions for ${esc(displayName)}">···</button>
    </article>`;
  };
  host.innerHTML = (state.view === "list" ? detailHeader() : "") + groupEntries(items).map((group) => {
    if (!group.label) return group.items.map(renderCard).join("");
    const key = `${state.groupBy}:${group.label}`;
    const collapsed = state.collapsedGroups.has(key);
    return `<section class="file-group" ${state.view === "list" ? 'role="rowgroup"' : ""}>
      <button class="file-group-heading" type="button" data-file-group="${esc(key)}" aria-expanded="${!collapsed}"><span aria-hidden="true">${collapsed ? "▸" : "▾"}</span> ${esc(group.label)} <span class="muted">${group.items.length}</span></button>
      <div class="file-group-items" ${collapsed ? "hidden" : ""}>${collapsed ? "" : group.items.map(renderCard).join("")}</div>
    </section>`;
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
  host.querySelectorAll(".card").forEach(card => { card.draggable = !!state.me && !state.me.readonly; });
  observeModelThumbnails(host);
  updateSelectionSummary();
}

async function load(path, options = {}) {
  if (!options.keepPreview && state.editor && state.editor.dirty) {
    if (!window.confirm("You have unsaved edits. Discard them?")) return;
    state.editor.dirty = false;
  }
  const gen = ++loadGen;
  const hidden = state.hidden ? "&hidden=1" : "";
  const data = await api(`/api/list?path=${encodeURIComponent(path)}${hidden}`);
  if (gen !== loadGen) return;
  const changedFolder = state.path !== (data.path || "");
  if (changedFolder || options.resetSearch) resetSearch();
  state.path = data.path || "";
  state.entries = data.entries || [];
  state.truncated = !!data.truncated;
  state.total = data.total || state.entries.length;
  if (data.rootName) state.me.rootName = data.rootName;
  state.selected = new Set();
  state.anchor = "";
  state.focus = "";
  if (!options.keepPreview) closePreview(true);
  renderCrumbs();
  syncUndo();
  renderFiles();
  if (changedFolder) document.querySelector(".explorer-content").scrollTop = 0;
  $("zip-link").href = `/api/zip?path=${encodeURIComponent(state.path)}`;
  syncBookmarkBtn();
  $("empty-trash-btn").hidden = !(state.me && !state.me.readonly && inTrashPath(state.path));
  scheduleFolderSizes();
}

function applyFolderSize(path, info) {
  const card = [...$("files").querySelectorAll('.card[data-dir="1"]')]
    .find((el) => el.dataset.path === path);
  if (!card) return;
  const sub = card.querySelector(".sub");
  if (!sub) return;
  const text = sub.textContent || "";
  if (text === "OwnNAS trash" || text === "OwnNAS archive" || text.startsWith("From ")) return;
  sub.textContent = formatFolderSize(info);
  sub.title = info.truncated ? "Count stopped early" : "";
  const entry = state.entries.find((item) => item.path === path);
  if (entry) {
    entry.size = info.bytes || 0;
    entry.measuredSize = info.bytes || 0;
    entry.measuredTruncated = !!info.truncated;
  }
  if (state.selected.has(path)) updateSelectionSummary();
}

function scheduleFolderSizes() {
  const gen = ++state.folderSizeGen;
  const folders = state.entries.filter((entry) => {
    if (!entry.dir) return false;
    if (entry.kind === "archive-folder" || entry.kind === "trash-folder") return false;
    return true;
  });
  (async () => {
    for (const entry of folders) {
      if (gen !== state.folderSizeGen) return;
      if (entry.measuredSize != null) {
        const info = {
          bytes: entry.measuredSize,
          truncated: !!entry.measuredTruncated,
          files: 0,
        };
        state.folderSizes.set(entry.path, info);
        applyFolderSize(entry.path, info);
        continue;
      }
      // Stamp mismatch or never measured — drop any stale memory cache and scan.
      state.folderSizes.delete(entry.path);
      applyFolderSize(entry.path, null);
      try {
        const usage = await api(`/api/usage?path=${encodeURIComponent(entry.path)}&slow=1`);
        if (gen !== state.folderSizeGen) return;
        const info = {
          bytes: usage.bytes || 0,
          truncated: !!usage.truncated,
          files: usage.files || 0,
        };
        state.folderSizes.set(entry.path, info);
        entry.measuredSize = info.bytes;
        entry.measuredTruncated = info.truncated;
        applyFolderSize(entry.path, info);
        if (!usage.cached) await sleep(150);
      } catch {
        // Leave the placeholder; another pass can retry later.
      }
      if (gen !== state.folderSizeGen) return;
    }
  })();
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
  else await load(path, { resetSearch: true });
}

function findEntry(path) {
  return state.entries.find((entry) => entry.path === path) || columnEntries.get(path);
}

const IMAGE_CONVERT_FORMATS = [
  ["jpeg", "JPEG"],
  ["png", "PNG"],
  ["webp", "WebP"],
  ["gif", "GIF"],
  ["bmp", "BMP"],
  ["tiff", "TIFF"],
];
const COMPRESS_FORMATS = [
  ["zip", "ZIP"],
  ["tar", "TAR"],
  ["tar.gz", "TAR.GZ"],
  ["tar.xz", "TAR.XZ"],
];

function closeMenuSub() {
  const sub = $("menu-sub");
  if (sub) sub.hidden = true;
  state.menuSub = "";
}

function closeMenu() {
  closeMenuSub();
  $("menu").hidden = true;
}

function positionMenuSub(anchorButton) {
  const menu = $("menu");
  const sub = $("menu-sub");
  sub.style.left = "0";
  sub.style.top = "0";
  sub.hidden = false;
  const menuRect = menu.getBoundingClientRect();
  const anchorRect = anchorButton.getBoundingClientRect();
  const subRect = sub.getBoundingClientRect();
  let left = menuRect.right + 6;
  let top = anchorRect.top;
  if (left + subRect.width > window.innerWidth - 8) {
    left = menuRect.left - subRect.width - 6;
  }
  if (top + subRect.height > window.innerHeight - 8) {
    top = Math.max(8, window.innerHeight - subRect.height - 8);
  }
  sub.style.left = `${Math.max(8, left)}px`;
  sub.style.top = `${Math.max(8, top)}px`;
}

function openMenuSub(kind, anchorButton) {
  let buttons = [];
  if (kind === "pdf") {
    buttons = [
      `<div class="menu-label">PDF</div>`,
      `<button type="button" data-menu="pdfExtract">Extract pages…</button>`,
      `<button type="button" data-menu="pdfSplit">Split into pages</button>`,
      `<button type="button" data-menu="pdfRotate">Rotate…</button>`,
    ];
  } else {
    const title = kind === "convert" ? "Convert to" : "Compress to";
    const formats = kind === "convert" ? IMAGE_CONVERT_FORMATS : COMPRESS_FORMATS;
    const action = kind === "convert" ? "convert" : "compress";
    buttons = [`<div class="menu-label">${title}</div>`];
    for (const [format, label] of formats) {
      buttons.push(
        `<button type="button" data-menu="${action}" data-format="${esc(format)}">${esc(label)}</button>`,
      );
    }
  }
  $("menu-sub").innerHTML = buttons.join("");
  state.menuSub = kind;
  positionMenuSub(anchorButton);
}

function parentPath(path) {
  const index = path.lastIndexOf("/");
  return index === -1 ? "" : path.slice(0, index);
}

function selectedEntries() {
  return [...state.selected].map(findEntry).filter(Boolean);
}

function entryKnownBytes(entry) {
  if (!entry) return null;
  if (!entry.dir) return entry.size || 0;
  if (entry.kind === "archive-folder" || entry.kind === "trash-folder") return null;
  const cached = state.folderSizes.get(entry.path);
  if (cached) return cached.bytes || 0;
  if (entry.measuredSize != null) return entry.measuredSize;
  return null;
}

function updateSelectionSummary() {
  const visible = sortedEntries().length;
  const itemCount = `${visible} ${visible === 1 ? "item" : "items"}`;
  const folderTotal = state.truncated ? state.total : state.entries.length;
  $("folder-summary").textContent = visible === folderTotal ? itemCount : `${itemCount} of ${folderTotal}`;
  const el = $("selection-summary");
  if (!el) return;
  const entries = selectedEntries();
  if (!entries.length) {
    el.hidden = true;
    el.textContent = "";
    return;
  }
  let total = 0;
  let unknown = 0;
  for (const entry of entries) {
    const bytes = entryKnownBytes(entry);
    if (bytes == null) unknown += 1;
    else total += bytes;
  }
  const count = entries.length === 1 ? "1 selected" : `${entries.length} selected`;
  let text = `${count} · ${formatSize(total, false)}`;
  if (unknown) {
    text += unknown === entries.length ? " · sizing…" : " · partial";
  }
  el.hidden = false;
  el.textContent = text;
}

function paintSelection() {
  document.querySelectorAll("#files .card, #files [data-column-entry]").forEach((card) => {
    const selected = state.selected.has(card.dataset.path || card.dataset.columnEntry);
    card.classList.toggle("selected", selected);
    card.setAttribute("aria-selected", selected ? "true" : "false");
    const cut = state.clipboard && state.clipboard.mode === "cut" && state.clipboard.items.some((item) => item.path === card.dataset.path);
    card.classList.toggle("cut", !!cut);
  });
  updateSelectionSummary();
}

function selectOnly(path) {
  state.selected = new Set([path]);
  state.anchor = path;
  state.focus = path;
  paintSelection();
}

function toggleSelected(path) {
  if (state.selected.has(path)) state.selected.delete(path);
  else state.selected.add(path);
  state.anchor = path;
  state.focus = path;
  paintSelection();
}

function selectRange(path) {
  const items = displayedEntries();
  const from = items.findIndex((entry) => entry.path === (state.anchor || path));
  const to = items.findIndex((entry) => entry.path === path);
  if (from < 0 || to < 0) {
    selectOnly(path);
    return;
  }
  const [start, end] = from < to ? [from, to] : [to, from];
  state.selected = new Set(items.slice(start, end + 1).map((entry) => entry.path));
  state.focus = path;
  paintSelection();
}

function gridColumnCount() {
  const cards = [...document.querySelectorAll("#files .card")];
  if (cards.length < 2 || state.view === "list") return 1;
  const top = cards[0].getBoundingClientRect().top;
  let cols = 1;
  for (let i = 1; i < cards.length; i += 1) {
    if (Math.abs(cards[i].getBoundingClientRect().top - top) > 2) break;
    cols += 1;
  }
  return Math.max(1, cols);
}

function scrollSelectedIntoView(path) {
  const card = document.querySelector(`#files .card[data-path="${CSS.escape(path)}"]`);
  if (card) card.scrollIntoView({ block: "nearest", inline: "nearest" });
}

function moveSelection(deltaX, deltaY, extend) {
  const items = displayedEntries();
  if (!items.length) return;
  let index = items.findIndex((entry) => entry.path === state.focus);
  if (index < 0) index = items.findIndex((entry) => entry.path === state.anchor);
  if (index < 0) {
    for (let i = items.length - 1; i >= 0; i -= 1) {
      if (state.selected.has(items[i].path)) { index = i; break; }
    }
  }
  let next = index < 0 ? 0 : Math.max(0, Math.min(items.length - 1, index + deltaX + deltaY));
  if (index >= 0 && state.view !== "list") {
    const origin = document.querySelector(`#files .card[data-path="${CSS.escape(items[index].path)}"]`);
    if (origin) {
      const bounds = origin.getBoundingClientRect();
      const x = bounds.left + bounds.width / 2;
      const y = bounds.top + bounds.height / 2;
      let best = Infinity;
      next = index;
      items.forEach((entry, i) => {
        if (i === index) return;
        const card = document.querySelector(`#files .card[data-path="${CSS.escape(entry.path)}"]`);
        if (!card) return;
        const rect = card.getBoundingClientRect();
        const dx = rect.left + rect.width / 2 - x;
        const dy = rect.top + rect.height / 2 - y;
        const forward = deltaX ? dx * deltaX : dy * deltaY;
        if (forward <= 2) return;
        const sideways = Math.abs(deltaX ? dy : dx);
        const score = forward + sideways * 3;
        if (score < best) { best = score; next = i; }
      });
    }
  }
  const entry = items[next];
  if (extend) selectRange(entry.path);
  else selectOnly(entry.path);
  scrollSelectedIntoView(entry.path);
  if (!$("preview").hidden && !entry.dir) openEntry(entry).catch((err) => toast(err.message, true));
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
  // Empty-space menu: folder actions. Selection menu needs at least one item.
  if (!entries.length && !write && !state.me) return;
  state.menuEntries = entries;
  state.pasteInto = pasteInto;
  const one = entries.length === 1 ? entries[0] : null;
  const allInTrash = entries.length > 0 && entries.every((entry) => inTrashPath(entry.path));
  const inTrash = inTrashPath(state.path);
  const bookmarked = state.bookmarks.includes(state.path);
  const buttons = [];
  if (entries.length) {
    buttons.push(`<div class="menu-label">${entries.length === 1 ? esc(entries[0].name) : `${entries.length} items`}</div>`);
  } else {
    buttons.push(`<div class="menu-label">${esc(state.path || "Library")}</div>`);
  }
  if (!entries.length) {
    if (write) {
      buttons.push(`<button type="button" data-menu="mkdir">New folder</button>`);
      buttons.push(`<button type="button" data-menu="newfile">New file…</button>`);
      buttons.push(`<button type="button" data-menu="upload">Upload files…</button>`);
      buttons.push(`<button type="button" data-menu="uploadFolder">Upload folder…</button>`);
    }
    if (write && customizableFolder({ path: state.path, dir: true, kind: 'folder' })) buttons.push('<button type="button" data-menu="folderAppearance">Customize folder…</button>');
    buttons.push(`<button type="button" data-menu="downloadZip">Download folder zip</button>`);
    buttons.push(`<button type="button" data-menu="search">Search…</button>`);
    buttons.push(`<button type="button" data-menu="library">Library</button>`);
    if (write) {
      buttons.push(`<button type="button" data-menu="bookmark">${bookmarked ? "Remove bookmark" : "Bookmark this folder"}</button>`);
    }
    buttons.push(`<button type="button" data-menu="usage">Folder size</button>`);
    buttons.push(`<button type="button" data-menu="duplicates">Find duplicates…</button>`);
    buttons.push(`<button type="button" data-menu="activity">Activity</button>`);
    buttons.push(`<button type="button" data-menu="toggleHidden">${state.hidden ? "Hide dotfiles" : "Show hidden files"}</button>`);
    if (write && inTrash) {
      buttons.push(`<button type="button" data-menu="emptyTrash" class="danger">Empty Trash</button>`);
    }
    if (write && clip) buttons.push(`<button type="button" data-menu="paste">Paste</button>`);
  } else {
    if (one) buttons.push(`<button type="button" data-menu="open">Open</button>`);
    if (write && entries.every(customizableFolder)) buttons.push(`<button type="button" data-menu="folderAppearance">${entries.length === 1 ? 'Customize folder…' : 'Customize folders…'}</button>`);
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
      buttons.push(`<button type="button" data-menu="bulkTag">Tag…</button>`);
    }
    const convertibles = entries.filter((entry) => entry.kind === "image");
    if (write && convertibles.length && convertibles.length === entries.length && !allInTrash) {
      buttons.push(`<button type="button" class="menu-has-sub" data-menu="convertMenu">Convert to…</button>`);
    }
    const pdfs = entries.filter((entry) => entry.kind === "pdf");
    if (write && pdfs.length && pdfs.length === entries.length && !allInTrash) {
      if (pdfs.length === 1) {
        buttons.push(`<button type="button" class="menu-has-sub" data-menu="pdfMenu">PDF…</button>`);
      } else {
        buttons.push(`<button type="button" data-menu="pdfMerge">Join PDFs</button>`);
      }
    }
    if (write && entries.length && !allInTrash) {
      buttons.push(`<button type="button" class="menu-has-sub" data-menu="compressMenu">Compress to…</button>`);
    }
    if (write && entries.length >= 2 && !allInTrash) {
      buttons.push(`<button type="button" data-menu="folderWith">New folder with selection</button>`);
    }
    if (write && entries.length && !allInTrash) {
      buttons.push(`<button type="button" data-menu="rename">Rename</button>`);
    }
    if (write && entries.length) {
      buttons.push(`<button type="button" data-menu="delete" class="danger">${allInTrash ? "Delete forever" : "Move to Trash"}</button>`);
    }
  }
  closeMenuSub();
  const menu = $("menu");
  menu.innerHTML = buttons.join("");
  menu.hidden = false;
  const rect = menu.getBoundingClientRect();
  menu.style.left = `${Math.max(8, Math.min(x, window.innerWidth - rect.width - 8))}px`;
  menu.style.top = `${Math.max(8, Math.min(y, window.innerHeight - rect.height - 8))}px`;
}

function closePreview(force = false) {
  if (!force && state.editor && state.editor.dirty) {
    if (!window.confirm("You have unsaved edits. Discard them?")) return false;
  }
  clearEditor();
  disposeActiveModelViewer();
  setPreviewFullscreen(false);
  $("preview").hidden = true;
  state.current = null;
  $("preview-body").innerHTML = "";
  $("preview-path").hidden = true;
  $("preview-path").textContent = "";
  $("preview-edit-image").hidden = true;
  $("preview-notes").hidden = true;
  $("notes-changed").hidden = true;
  setNotesExpanded(false);
  $("preview-tags").innerHTML = "";
  $("preview-comments").innerHTML = "";
  $("notes-summary-tags").innerHTML = "";
  $("notes-summary-comments").hidden = true;
  $("tag-input").value = "";
  $("comment-input").value = "";
  return true;
}

function clearEditor() {
  state.editor?.destroy?.();
  state.editor = null;
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
  const updates = tab === "updates";
  const users = tab === "users";
  $("settings-tab-theme").setAttribute("aria-selected", theme ? "true" : "false");
  $("settings-tab-updates").setAttribute("aria-selected", updates ? "true" : "false");
  $("settings-tab-users").setAttribute("aria-selected", users ? "true" : "false");
  $("settings-theme").hidden = !theme;
  $("settings-updates").hidden = !updates;
  $("settings-users").hidden = !users;
  if (updates) refreshUpdatePanel().catch((err) => toast(err.message, true));
}

function openSettings(tab) {
  if (!closePreview()) return;
  closeLibrary();
  const admin = !!(state.me && state.me.admin);
  $("settings-tab-users").hidden = !admin;
  if (!admin && tab === "users") tab = "theme";
  if (!tab) tab = "theme";
  setSettingsTab(tab);
  renderThemeGrid();
  $("settings").hidden = false;
  if (admin && tab === "users") loadUsers().catch((err) => toast(err.message, true));
}

async function refreshUpdatePanel() {
  const current = (state.me && state.me.version) || "—";
  $("update-current").textContent = current;
  $("update-target").textContent = (state.me && state.me.updateTarget) || "—";
  $("update-error").hidden = true;
  $("update-error").textContent = "";
  $("update-notes").hidden = true;
  $("update-apply-btn").hidden = true;
  const admin = !!(state.me && state.me.admin);
  $("update-check-btn").disabled = !admin;
  if (!admin) {
    $("update-status").textContent = "Only administrators can check or install updates.";
    return;
  }
  if (!state.me.updatesConfigured) {
    $("update-status").textContent = "Updates are not configured.";
    return;
  }
  try {
    const cached = await api("/api/update/status");
    if (cached.available) {
      $("update-status").textContent = `Version ${cached.latest} is available${cached.signed ? " (signature verified)" : ""}.`;
      $("update-apply-btn").hidden = false;
      $("update-notes").hidden = !cached.notes;
      $("update-notes").textContent = cached.notes || "";
    } else if (cached.error) {
      $("update-status").textContent = "The daily update check failed. You can try again now.";
    } else {
      $("update-status").textContent = cached.checkedAt ? `You’re on the latest version (${current}).` : "The daily update check is running. You can check now.";
    }
  } catch {
    $("update-status").textContent = "The daily update check is running. You can check now.";
  }
}

async function checkForUpdates() {
  $("update-error").hidden = true;
  $("update-check-btn").disabled = true;
  $("update-status").textContent = "Checking…";
  try {
    const data = await api("/api/update/check");
    if (!data.configured) {
      $("update-status").textContent = "Updates are not configured on this server.";
      return;
    }
    $("update-current").textContent = data.current || "—";
    $("update-target").textContent = data.target || "—";
    if (data.notes) {
      $("update-notes").hidden = false;
      $("update-notes").textContent = data.notes;
    } else {
      $("update-notes").hidden = true;
    }
    if (data.available) {
      const signed = data.signed ? " (signature verified)" : "";
      $("update-status").textContent = `Version ${data.latest} is available${signed}.`;
      $("update-apply-btn").hidden = false;
    } else {
      $("update-status").textContent = `You’re on the latest version (${data.current}).`;
      $("update-apply-btn").hidden = true;
    }
  } catch (err) {
    $("update-status").textContent = "Check failed.";
    $("update-error").hidden = false;
    $("update-error").textContent = err.message;
  } finally {
    $("update-check-btn").disabled = false;
  }
}

async function applyUpdate() {
  if (!await askConfirm("Download and install the new OwnNAS binary? The server will restart briefly.", "Install update")) {
    return;
  }
  $("update-apply-btn").disabled = true;
  $("update-check-btn").disabled = true;
  $("update-status").textContent = "Downloading and installing… OwnNAS will restart.";
  try {
    const data = await api("/api/update/apply", { method: "POST", json: {} });
    toast(`Updated to ${data.version || "new version"} — reconnecting…`);
    setTimeout(() => {
      location.reload();
    }, 1500);
  } catch (err) {
    $("update-error").hidden = false;
    $("update-error").textContent = err.message;
    $("update-status").textContent = "Install failed.";
    $("update-apply-btn").disabled = false;
    $("update-check-btn").disabled = false;
  }
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
  if (!closePreview()) return;
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
  if (state.editor && state.editor.dirty && state.current && state.current.path !== entry.path) {
    if (!window.confirm("You have unsaved edits. Discard them?")) return;
    state.editor.dirty = false;
  }
  clearEditor();
  disposeActiveModelViewer();
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
  const canEditImage = write && entry.kind === "image" && !inTrashPath(entry.path);
  $("preview-edit-image").hidden = !canEditImage;
  if (!inTrashPath(entry.path)) {
    api("/api/recent", { method: "POST", json: { path: entry.path } }).then(() => refreshLibrary()).catch(() => {});
  }
  loadAnnotations(entry.path).catch((err) => toast(err.message, true));
  const body = $("preview-body");
  const raw = `/api/raw?path=${encodeURIComponent(entry.path)}`;
  if (entry.kind === "image" || entry.kind === "svg") {
    body.innerHTML = `<img alt="${esc(entry.name)}" src="${raw}&t=${entry.modified || Date.now()}">`;
  } else if (entry.kind === "video") {
    const poster = entry.thumb ? ` poster="/api/thumb?path=${encodeURIComponent(entry.path)}&v=${entry.modified}"` : "";
    body.innerHTML = `<video controls playsinline${poster} src="${raw}"></video>`;
  } else if (entry.kind === "audio") {
    body.innerHTML = `<audio controls src="${raw}"></audio>`;
  } else if (entry.kind === "pdf") {
    body.innerHTML = `<iframe title="${esc(entry.name)}" src="${raw}"></iframe>`;
  } else if (/\.xlsx$/i.test(entry.name)) {
    body.innerHTML = '<p class="muted">Loading spreadsheet…</p>';
    try {
      if (entry.size > 25 * 1024 * 1024) throw new Error('Spreadsheets up to 25 MB are supported.');
      const [module, response] = await Promise.all([import('/assets/xlsx-editor.js'), fetch(`/api/xlsx?path=${encodeURIComponent(entry.path)}`, { credentials: 'same-origin', cache: 'no-store' })]);
      if (!response.ok) throw new Error('Could not load spreadsheet.');
      const bytes = new Uint8Array(await response.arrayBuffer());
      let expectedHash = response.headers.get('x-xlsx-hash');
      if (!expectedHash) throw new Error('Spreadsheet version is missing.');
      if (state.current !== entry) return;
      const editor = { dirty: false, destroy: null };
      const mounted = module.mountXlsxEditor(body, bytes, {
        readonly: !write || inTrashPath(entry.path),
        onChange: dirty => { editor.dirty = dirty; },
        onSave: async data => {
          let binary = '';
          for (let i = 0; i < data.length; i += 8192) binary += String.fromCharCode(...data.subarray(i, i + 8192));
          const result = await api('/api/write-xlsx', { method: 'POST', json: { path: entry.path, data: btoa(binary), expected_hash: expectedHash } });
          expectedHash = result.hash;
          if (state.editor === editor) {
            toast('Spreadsheet saved');
            invalidateFolderSizes();
            load(state.path, { keepPreview: true }).catch(err => toast(err.message, true));
          }
        },
      });
      editor.destroy = () => mounted.destroy(); editor.save = () => mounted.save(); state.editor = editor;
    } catch (err) {
      if (state.current === entry) body.innerHTML = `<p class="muted">${esc(err.message)}</p>`;
    }
  } else if (/\.odt$/i.test(entry.name)) {
    body.innerHTML = '<p class="muted">Loading document…</p>';
    try {
      if (entry.size > 25 * 1024 * 1024) throw new Error('Documents up to 25 MB are supported.');
      const [module, response] = await Promise.all([import('/assets/odt-editor.js'), fetch(`/api/odt?path=${encodeURIComponent(entry.path)}`, { credentials: 'same-origin', cache: 'no-store' })]);
      if (!response.ok) throw new Error('Could not load document.');
      const bytes = new Uint8Array(await response.arrayBuffer());
      let expectedHash = response.headers.get('x-odt-hash');
      if (!expectedHash) throw new Error('Document version is missing.');
      if (state.current !== entry) return;
      const editor = { dirty: false, destroy: null };
      const mounted = module.mountOdtEditor(body, bytes, {
        readonly: !write || inTrashPath(entry.path),
        onChange: dirty => { editor.dirty = dirty; },
        onSave: async data => {
          let binary = '';
          for (let i = 0; i < data.length; i += 8192) binary += String.fromCharCode(...data.subarray(i, i + 8192));
          const result = await api('/api/write-odt', { method: 'POST', json: { path: entry.path, data: btoa(binary), expected_hash: expectedHash } });
          expectedHash = result.hash;
          if (state.editor === editor) {
            toast('Document saved');
            invalidateFolderSizes();
            load(state.path, { keepPreview: true }).catch(err => toast(err.message, true));
          }
        },
      });
      editor.destroy = () => mounted.destroy(); editor.save = () => mounted.save(); state.editor = editor;
    } catch (err) {
      if (state.current === entry) body.innerHTML = `<p class="muted">${esc(err.message)}</p>`;
    }
  } else if (/\.docx$/i.test(entry.name)) {
    body.innerHTML = '<p class="muted">Loading document…</p>';
    try {
      if (entry.size > 25 * 1024 * 1024) throw new Error('Documents up to 25 MB are supported.');
      const [module, response] = await Promise.all([import('/assets/docx-editor.js'), fetch(`/api/docx?path=${encodeURIComponent(entry.path)}`, { credentials: 'same-origin', cache: 'no-store' })]);
      if (!response.ok) throw new Error('Could not load document.');
      const bytes = new Uint8Array(await response.arrayBuffer());
      let expectedHash = response.headers.get('x-docx-hash');
      if (!expectedHash) throw new Error('Document version is missing.');
      if (state.current !== entry) return;
      const editor = { dirty: false, destroy: null };
      const mounted = module.mountDocxEditor(body, bytes, {
        readonly: !write || inTrashPath(entry.path),
        onChange: dirty => { editor.dirty = dirty; },
        onSave: async data => {
          let binary = '';
          for (let i = 0; i < data.length; i += 8192) binary += String.fromCharCode(...data.subarray(i, i + 8192));
          const result = await api('/api/write-docx', { method: 'POST', json: { path: entry.path, data: btoa(binary), expected_hash: expectedHash } });
          expectedHash = result.hash;
          if (state.editor === editor) {
            toast('Document saved');
            invalidateFolderSizes();
            load(state.path, { keepPreview: true }).catch(err => toast(err.message, true));
          }
        },
      });
      editor.destroy = () => mounted.destroy(); editor.save = () => mounted.save(); state.editor = editor;
    } catch (err) {
      if (state.current === entry) body.innerHTML = `<p class="muted">${esc(err.message)}</p>`;
    }
  } else if (entry.kind === "cad2d") {
    openCadPreview(body, entry, raw).catch((err) => {
      if (state.current && state.current.path === entry.path) {
        body.innerHTML = `<p class="muted">${esc(err.message || "Could not open CAD preview")}</p>`;
      }
    });
  } else if (entry.kind === "model3d") {
    openModelPreview(body, entry, raw).catch((err) => {
      if (state.current && state.current.path === entry.path) {
        body.innerHTML = `<p class="muted">${esc(err.message || "Could not open 3D preview")}</p>`;
      }
    });
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

let modelViewerApi = null;
let dxfViewerApi = null;
const MODEL_VIEWER_URL = "/assets/model-viewer.js?v=4";
const DXF_VIEWER_URL = "/assets/dxf-viewer.js?v=1";

async function loadModelViewerApi() {
  if (modelViewerApi) return modelViewerApi;
  modelViewerApi = await import(MODEL_VIEWER_URL);
  return modelViewerApi;
}

async function loadDxfViewerApi() {
  if (dxfViewerApi) return dxfViewerApi;
  dxfViewerApi = await import(DXF_VIEWER_URL);
  return dxfViewerApi;
}

// A serial thumbnail queue keeps CAD conversion and GPU use bounded.
let modelThumbObserver = null;
const modelThumbQueue = [];
const queuedModelThumbs = new WeakSet();
const observedModelThumbs = new Set();
let modelThumbBusy = false;
let modelThumbDb = null;
const modelThumbMemo = new Map();
const modelThumbUrls = new Map();

function openModelThumbDb() {
  if (modelThumbDb) return modelThumbDb;
  modelThumbDb = new Promise((resolve) => {
    try {
      const request = indexedDB.open("ownnas-model-thumbnails", 1);
      request.onupgradeneeded = () => {
        const store = request.result.createObjectStore("thumbnails", { keyPath: "key" });
        store.createIndex("created", "created");
      };
      request.onsuccess = () => resolve(request.result);
      request.onerror = request.onblocked = () => resolve(null);
    } catch { resolve(null); }
  });
  return modelThumbDb;
}

async function cachedModelThumb(key) {
  const db = await openModelThumbDb();
  if (!db) return null;
  return new Promise((resolve) => {
    try {
      const request = db.transaction("thumbnails").objectStore("thumbnails").get(key);
      request.onsuccess = () => resolve(request.result?.blob || null);
      request.onerror = () => resolve(null);
    } catch { resolve(null); }
  });
}

async function storeModelThumb(key, blob) {
  const db = await openModelThumbDb();
  if (!db) return;
  try {
    const tx = db.transaction("thumbnails", "readwrite");
    const store = tx.objectStore("thumbnails");
    store.put({ key, blob, created: Date.now() });
    const count = store.count();
    count.onsuccess = () => {
      let excess = count.result - 256;
      if (excess <= 0) return;
      const cursor = store.index("created").openCursor();
      cursor.onsuccess = () => {
        if (!cursor.result || excess-- <= 0) return;
        cursor.result.delete();
        cursor.result.continue();
      };
    };
    // Storage may be unavailable or full; rendering still works without persistence.
    tx.onerror = () => {};
  } catch { /* Cache is optional. */ }
}

function modelThumbKey(el) {
  return JSON.stringify(["model-v2", state.me?.libraryId || state.me?.rootName, state.me?.username,
    el.dataset.modelPath, el.dataset.modelModified, el.dataset.modelSize]);
}

async function modelThumbBlob(el) {
  const key = modelThumbKey(el);
  if (modelThumbMemo.has(key)) return modelThumbMemo.get(key);
  const promise = (async () => {
    const cached = await cachedModelThumb(key);
    if (cached) return cached;
    const api = await loadModelViewerApi();
    const blob = await api.renderModelThumbnail({
      url: `/api/raw?path=${encodeURIComponent(el.dataset.modelPath)}`,
      name: el.dataset.modelName,
      size: Number(el.dataset.modelSize),
    });
    await storeModelThumb(key, blob);
    return blob;
  })();
  modelThumbMemo.set(key, promise);
  if (modelThumbMemo.size > 64) modelThumbMemo.delete(modelThumbMemo.keys().next().value);
  try { return await promise; }
  catch (err) { modelThumbMemo.delete(key); throw err; }
}

async function pumpModelThumbnails() {
  if (modelThumbBusy) return;
  modelThumbBusy = true;
  try {
    while (modelThumbQueue.length) {
      const el = modelThumbQueue.shift();
      if (!el.isConnected || !state.me || $("app-view").hidden) { queuedModelThumbs.delete(el); continue; }
      el.classList.add("thumbnail-loading");
      el.title = "Generating 3D thumbnail…";
      try {
        const blob = await modelThumbBlob(el);
        if (!el.isConnected) continue;
        const url = URL.createObjectURL(blob);
        modelThumbUrls.set(el, url);
        const image = document.createElement("img");
        image.alt = "";
        image.src = url;
        image.className = "model-thumbnail-image";
        el.replaceChildren(image);
        el.title = "";
      } catch (err) {
        if (el.isConnected) el.title = `3D thumbnail unavailable: ${err.message || "Could not render model"}`;
      } finally {
        el.classList.remove("thumbnail-loading");
      }
    }
  } finally { modelThumbBusy = false; }
}

function observeModelThumbnails(container) {
  for (let i = modelThumbQueue.length - 1; i >= 0; i--) {
    if (!modelThumbQueue[i].isConnected) modelThumbQueue.splice(i, 1);
  }
  for (const el of observedModelThumbs) {
    if (!el.isConnected) { modelThumbObserver?.unobserve(el); observedModelThumbs.delete(el); }
  }
  for (const [el, url] of modelThumbUrls) {
    if (!el.isConnected) { URL.revokeObjectURL(url); modelThumbUrls.delete(el); }
  }
  if (!modelThumbObserver && typeof IntersectionObserver !== "undefined") {
    modelThumbObserver = new IntersectionObserver((entries) => {
      for (const entry of entries) {
        if (!entry.isIntersecting || queuedModelThumbs.has(entry.target)) continue;
        queuedModelThumbs.add(entry.target);
        modelThumbObserver.unobserve(entry.target);
        observedModelThumbs.delete(entry.target);
        modelThumbQueue.push(entry.target);
      }
      pumpModelThumbnails();
    }, { root: document.querySelector(".explorer-content"), rootMargin: "150px" });
  }
  container.querySelectorAll("[data-model-path]").forEach((el) => {
    if (modelThumbObserver) { modelThumbObserver.observe(el); observedModelThumbs.add(el); }
    else { queuedModelThumbs.add(el); modelThumbQueue.push(el); }
  });
  if (!modelThumbObserver) pumpModelThumbnails();
}

function disposeActiveModelViewer() {
  if (modelViewerApi && modelViewerApi.disposeModelViewer) {
    modelViewerApi.disposeModelViewer();
  }
  if (dxfViewerApi && dxfViewerApi.disposeDxfViewer) {
    dxfViewerApi.disposeDxfViewer();
  }
}

function modelExt(name) {
  const lower = String(name || "").toLowerCase();
  const dot = lower.lastIndexOf(".");
  return dot >= 0 ? lower.slice(dot + 1) : "";
}

function canPreviewModel3d(name, apiMod) {
  if (apiMod && typeof apiMod.isViewableModel === "function" && apiMod.isViewableModel(name)) {
    return true;
  }
  return ["obj", "stl", "gltf", "glb", "ply", "3mf", "stp", "step", "iges", "igs"].includes(modelExt(name));
}

async function openModelPreview(body, entry, raw) {
  body.innerHTML = `<p class="muted">Loading 3D viewer…</p>`;
  const apiMod = await loadModelViewerApi();
  if (!state.current || state.current.path !== entry.path) return;
  if (!canPreviewModel3d(entry.name, apiMod)) {
    body.innerHTML = `<p class="muted">This 3D format cannot be previewed in the browser.</p>`;
    return;
  }
  body.innerHTML = "";
  await apiMod.mountModelViewer(body, { url: raw, name: entry.name });
}

async function openCadPreview(body, entry, raw) {
  body.innerHTML = `<p class="muted">Loading CAD viewer…</p>`;
  const apiMod = await loadDxfViewerApi();
  if (!state.current || state.current.path !== entry.path) return;
  body.innerHTML = "";
  await apiMod.mountDxfViewer(body, { url: raw, name: entry.name, size: entry.size });
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
    mountTextEditor(body, entry, meta.text);
    return;
  }
  if (meta.archive) {
    mountArchiveBrowser(body, meta.archive, !!meta.archiveTruncated);
    return;
  }
  body.innerHTML = `<p class="muted">${esc(meta.note || "No inline preview for this file. Download it to open it locally.")}</p>`;
}

function mountArchiveBrowser(body, entries, truncated) {
  const rootNode = { id: 0, name: "Contents", dir: true, children: new Map() };
  const nodes = new Map([[0, rootNode]]);
  let nextId = 1;
  for (const entry of entries || []) {
    const normalized = String(entry.name || "").replace(/\\/g, "/");
    if (!normalized || normalized.startsWith("/") || /^[a-z]:/i.test(normalized)) continue;
    const parts = normalized.split("/").filter(part => part && part !== ".");
    if (!parts.length || parts.some(part => part === "..")) continue;
    let parent = rootNode;
    for (let index = 0; index < parts.length; index++) {
      const part = parts[index];
      let child = parent.children.get(part);
      if (!child) {
        child = { id: nextId++, name: part, dir: index < parts.length - 1 || !!entry.dir, children: new Map(), size: 0 };
        parent.children.set(part, child); nodes.set(child.id, child);
      }
      if (index === parts.length - 1) {
        child.dir = !!entry.dir || child.children.size > 0;
        child.size = Number(entry.size) || 0;
      }
      parent = child;
    }
  }
  const shell = document.createElement("section"); shell.className = "archive-browser";
  const notice = document.createElement("div"); notice.className = "archive-notice";
  notice.textContent = "Archived contents · read-only · files remain compressed";
  const breadcrumbs = document.createElement("nav"); breadcrumbs.className = "archive-breadcrumbs"; breadcrumbs.setAttribute("aria-label", "Archive folder path");
  const list = document.createElement("ul"); list.className = "archive-list";
  const footnote = document.createElement("p"); footnote.className = "muted archive-footnote";
  shell.append(notice, breadcrumbs, list, footnote); body.replaceChildren(shell);
  let current = rootNode;
  function render() {
    breadcrumbs.replaceChildren();
    const path = [];
    let cursor = current;
    while (cursor && cursor !== rootNode) { path.unshift(cursor); cursor = cursor.parent; }
    const chain = [rootNode, ...path];
    chain.forEach((node, index) => {
      if (index) {
        const separator = document.createElement("span"); separator.textContent = "/"; separator.setAttribute("aria-hidden", "true"); breadcrumbs.append(separator);
      }
      const crumb = document.createElement("button"); crumb.type = "button"; crumb.textContent = node.name; crumb.disabled = index === chain.length - 1;
      crumb.addEventListener("click", () => { current = node; render(); }); breadcrumbs.append(crumb);
    });
    list.replaceChildren();
    const children = [...current.children.values()].sort((a, b) => Number(b.dir) - Number(a.dir) || a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: "base" }));
    if (current !== rootNode) {
      const parentRow = document.createElement("li"); parentRow.className = "archive-parent-row";
      const parentButton = document.createElement("button"); parentButton.type = "button"; parentButton.textContent = "↑ Parent folder";
      parentButton.addEventListener("click", () => { current = current.parent || rootNode; render(); });
      parentRow.append(parentButton); list.append(parentRow);
    }
    for (const child of children) {
      const row = document.createElement("li"); row.className = child.dir ? "archive-dir-row" : "archive-file-row";
      const name = document.createElement(child.dir ? "button" : "span");
      if (child.dir) {
        name.type = "button"; name.addEventListener("click", () => { current = child; render(); });
      }
      name.className = "archive-entry-name"; name.textContent = child.name; row.append(name);
      const detail = document.createElement("span"); detail.className = "muted"; detail.textContent = child.dir ? "Archived folder" : formatSize(child.size, false); row.append(detail);
      list.append(row);
    }
    if (!children.length) {
      const empty = document.createElement("li"); empty.className = "muted archive-empty"; empty.textContent = current === rootNode ? "This archive is empty." : "This archived folder is empty."; list.append(empty);
    }
    footnote.textContent = truncated ? "Showing the first entries only. No files have been extracted." : "Select a folder to browse its archived files. No files have been extracted.";
  }
  function linkParents(node, parent) {
    node.parent = parent;
    for (const child of node.children.values()) linkParents(child, node);
  }
  linkParents(rootNode, null); render();
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

function tableSeparator(name) {
  return String(name || "").toLowerCase().endsWith(".tsv") ? "\t" : ",";
}

function isEditableFormat(format) {
  return format === "markdown" || format === "table" || format === "text" || format === "json" || format === "ini" || format === "log";
}

function mountTextEditor(body, entry, text) {
  const format = textFormat(entry.name);
  state.editor = {
    path: entry.path,
    name: entry.name,
    format,
    original: text.content,
    content: text.content,
    truncated: !!text.truncated,
    mode: "view",
    dirty: false,
    sep: tableSeparator(entry.name),
    saving: false,
  };
  paintTextEditor(body);
}

function paintTextEditor(body = $("preview-body")) {
  const ed = state.editor;
  if (!ed || !body) return;
  const write = state.me && !state.me.readonly && !ed.truncated && isEditableFormat(ed.format);
  const editing = write && ed.mode === "edit";
  const actions = [];
  if (write) {
    if (ed.format === "markdown") {
      actions.push(`<button type="button" data-editor="mode" data-mode="view" class="${ed.mode === "view" ? "is-active" : ""}">Preview</button>`);
      actions.push(`<button type="button" data-editor="mode" data-mode="edit" class="${ed.mode === "edit" ? "is-active" : ""}">Edit</button>`);
    } else if (!editing) {
      actions.push(`<button type="button" data-editor="mode" data-mode="edit">Edit</button>`);
    } else {
      actions.push(`<button type="button" data-editor="mode" data-mode="view">Done</button>`);
    }
    if (ed.format === "table" && editing) {
      actions.push(`<button type="button" data-editor="add-row">Add row</button>`);
      actions.push(`<button type="button" data-editor="add-col">Add column</button>`);
      actions.push(`<button type="button" data-editor="del-row">Remove row</button>`);
      actions.push(`<button type="button" data-editor="del-col">Remove column</button>`);
    }
    actions.push(`<button type="button" data-editor="save" ${ed.dirty && !ed.saving ? "" : "disabled"}>Save</button>`);
  }
  let main = "";
  if (editing && ed.format === "table") {
    main = renderCsvEditor(ed.content, ed.sep);
  } else if (editing) {
    main = `<textarea class="text-editor-area" data-editor-input spellcheck="true">${esc(ed.content)}</textarea>`;
  } else if (ed.format === "table") {
    main = renderTable(ed.content, ed.sep);
  } else if (ed.format === "json") {
    let pretty = ed.content;
    try { pretty = JSON.stringify(JSON.parse(pretty), null, 2); } catch { /* keep */ }
    main = `<pre class="text-code">${esc(pretty)}</pre>`;
  } else if (ed.format === "markdown") {
    main = `<div class="md-preview">${renderMarkdown(ed.content)}</div>`;
  } else if (ed.format === "ini") {
    main = `<pre class="text-code ini-preview">${renderIni(ed.content)}</pre>`;
  } else {
    main = renderLinedText(ed.content, ed.format === "log");
  }
  const lines = ed.content ? ed.content.split(/\r?\n/).length : 0;
  body.innerHTML = `<div class="text-viewer format-${ed.format}${editing ? " is-editing" : ""}">
    <div class="text-viewer-bar">
      <div class="text-viewer-meta">
        <span class="pill">${esc(formatLabel(ed.format))}</span>
        <span class="muted">${lines} line${lines === 1 ? "" : "s"}</span>
        ${ed.dirty ? `<span class="editor-dirty">Unsaved</span>` : ""}
      </div>
      <div class="text-viewer-actions">${actions.join("")}</div>
    </div>
    ${main}
    ${ed.truncated ? `<p class="muted text-viewer-note">Preview truncated — editing is disabled for this file.</p>` : ""}
  </div>`;
}

function syncEditorFromDom() {
  const ed = state.editor;
  if (!ed || ed.mode !== "edit") return;
  if (ed.format === "table") {
    const wrap = $("preview-body").querySelector("[data-csv-editor]");
    if (!wrap) return;
    ed.content = serializeCsv(readCsvGrid(wrap), ed.sep);
  } else {
    const area = $("preview-body").querySelector("[data-editor-input]");
    if (!area) return;
    ed.content = area.value;
  }
  ed.dirty = ed.content !== ed.original;
}

function markEditorDirty() {
  const ed = state.editor;
  if (!ed) return;
  syncEditorFromDom();
  const dirty = ed.dirty;
  const saveBtn = $("preview-body").querySelector("[data-editor='save']");
  if (saveBtn) saveBtn.disabled = !dirty || ed.saving;
  const flag = $("preview-body").querySelector(".editor-dirty");
  if (dirty && !flag) {
    const meta = $("preview-body").querySelector(".text-viewer-meta");
    if (meta) meta.insertAdjacentHTML("beforeend", `<span class="editor-dirty">Unsaved</span>`);
  } else if (!dirty && flag) {
    flag.remove();
  }
}

async function saveEditor() {
  const ed = state.editor;
  if (!ed || ed.saving) return;
  if (ed.save) return ed.save();
  syncEditorFromDom();
  if (!ed.dirty) return;
  if (ed.content.length > 1024 * 1024) {
    toast("This file is too large to save in the editor (1 MB max)", true);
    return;
  }
  ed.saving = true;
  paintTextEditor();
  try {
    await api("/api/write", { method: "POST", json: { path: ed.path, content: ed.content } });
    ed.original = ed.content;
    ed.dirty = false;
    ed.saving = false;
    toast("Saved");
    const path = ed.path;
    await load(state.path, { keepPreview: true });
    if (state.current && state.current.path === path) {
      const entry = state.entries.find((item) => item.path === path) || state.current;
      state.current = entry;
      $("preview-meta").textContent = `${formatSize(entry.size, false)} · ${formatDate(entry.modified)}`;
      paintTextEditor();
      loadAnnotations(path).catch(() => {});
    }
  } catch (err) {
    ed.saving = false;
    paintTextEditor();
    toast(err.message, true);
  }
}

function parseCsv(text, sep) {
  const rows = [];
  let row = [];
  let cell = "";
  let i = 0;
  let inQuotes = false;
  const input = String(text || "").replace(/^\uFEFF/, "");
  while (i < input.length) {
    const ch = input[i];
    if (inQuotes) {
      if (ch === '"') {
        if (input[i + 1] === '"') {
          cell += '"';
          i += 2;
          continue;
        }
        inQuotes = false;
        i += 1;
        continue;
      }
      cell += ch;
      i += 1;
      continue;
    }
    if (ch === '"') {
      inQuotes = true;
      i += 1;
      continue;
    }
    if (ch === sep) {
      row.push(cell);
      cell = "";
      i += 1;
      continue;
    }
    if (ch === "\n") {
      row.push(cell);
      rows.push(row);
      row = [];
      cell = "";
      i += 1;
      continue;
    }
    if (ch === "\r") {
      i += 1;
      continue;
    }
    cell += ch;
    i += 1;
  }
  if (cell.length || row.length) {
    row.push(cell);
    rows.push(row);
  }
  if (!rows.length) rows.push([""]);
  const width = Math.max(1, ...rows.map((r) => r.length));
  return rows.map((r) => {
    const next = r.slice();
    while (next.length < width) next.push("");
    return next;
  });
}

function serializeCsv(rows, sep) {
  return rows.map((row) => row.map((value) => {
    const cell = String(value ?? "");
    if (/["\r\n]/.test(cell) || cell.includes(sep)) {
      return `"${cell.replace(/"/g, '""')}"`;
    }
    return cell;
  }).join(sep)).join("\n");
}

function readCsvGrid(wrap) {
  return [...wrap.querySelectorAll("tr")].map((tr) =>
    [...tr.querySelectorAll("input")].map((input) => input.value)
  );
}

function renderCsvEditor(content, sep) {
  const rows = parseCsv(content, sep);
  const body = rows.map((row, r) =>
    `<tr>${row.map((cell, c) =>
      `<td><input type="text" data-r="${r}" data-c="${c}" value="${escAttr(cell)}" spellcheck="false"></td>`
    ).join("")}</tr>`
  ).join("");
  return `<div class="table-wrap csv-editor" data-csv-editor><table><tbody>${body}</tbody></table></div>`;
}

function escAttr(value) {
  return esc(value).replace(/`/g, "&#96;");
}

function mutateCsvGrid(action) {
  const ed = state.editor;
  if (!ed || ed.format !== "table") return;
  syncEditorFromDom();
  let rows = parseCsv(ed.content, ed.sep);
  const width = rows[0] ? rows[0].length : 1;
  if (action === "add-row") {
    rows.push(Array.from({ length: width }, () => ""));
  } else if (action === "add-col") {
    rows = rows.map((row) => row.concat([""]));
  } else if (action === "del-row") {
    if (rows.length > 1) rows.pop();
  } else if (action === "del-col") {
    if (width > 1) rows = rows.map((row) => row.slice(0, -1));
  }
  ed.content = serializeCsv(rows, ed.sep);
  ed.dirty = ed.content !== ed.original;
  paintTextEditor();
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
  const rows = parseCsv(content, separator).slice(0, 200);
  const html = rows.map((row) => `<tr>${row.map((cell) => `<td>${esc(cell)}</td>`).join("")}</tr>`);
  return `<div class="table-wrap"><table>${html.join("")}</table></div>`;
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
  await renameEntries([entry]);
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
  invalidateFolderSizes();
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
  upsertJob("transfer", {
    title,
    detail,
    percent,
    cancelable: true,
    onCancel: () => cancelActiveTransfer(),
  });
}

function hideStatus() {
  removeJob("transfer");
}

const jobState = {
  items: new Map(),
  hidden: false,
  seq: 0,
};

function upsertJob(id, { title, detail = "", percent = null, cancelable = false, onCancel = null }) {
  const existing = jobState.items.get(id) || {};
  jobState.items.set(id, {
    id,
    title,
    detail,
    percent,
    cancelable,
    onCancel: onCancel || existing.onCancel || null,
  });
  if (!jobState.hidden) renderJobsPanel(true);
  else syncJobsButton();
}

function removeJob(id) {
  jobState.items.delete(id);
  renderJobsPanel(!jobState.hidden);
}

function syncJobsButton() {
  const btn = $("jobs-btn");
  const count = jobState.items.size;
  btn.hidden = count === 0;
  const badge = $("jobs-count");
  if (count > 0) {
    badge.hidden = false;
    badge.textContent = String(count);
  } else {
    badge.hidden = true;
  }
}

function renderJobsPanel(show) {
  const panel = $("jobs-panel");
  const list = $("jobs-list");
  const items = [...jobState.items.values()];
  syncJobsButton();
  if (!items.length) {
    panel.hidden = true;
    list.innerHTML = "";
    return;
  }
  list.innerHTML = items.map((job) => {
    const pct = job.percent;
    const barClass = pct == null ? "status-bar indeterminate" : "status-bar";
    const width = pct == null ? "" : ` style="width:${Math.max(0, Math.min(100, Math.round(pct)))}%"`;
    const cancel = job.cancelable
      ? `<button type="button" data-job-cancel="${esc(job.id)}">Cancel</button>`
      : "";
    return `<li class="jobs-item" data-job="${esc(job.id)}">
      <div class="jobs-item-head">
        <span class="jobs-item-title">${esc(job.title)}</span>
        ${cancel}
      </div>
      <span class="jobs-item-detail">${esc(job.detail || "")}</span>
      <div class="status-track"><div class="${barClass}"${width}></div></div>
    </li>`;
  }).join("");
  panel.hidden = !show || jobState.hidden;
}

function openJobsPanel() {
  jobState.hidden = false;
  renderJobsPanel(true);
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
  invalidateFolderSizes();
  await load(state.path);
}

async function emptyTrash() {
  if (!await askConfirm("Empty the Trash? Every item in it will be permanently deleted.", "Empty Trash")) return;
  const result = await api("/api/trash/empty", { method: "POST" });
  toast(result.removed ? `Emptied ${result.removed} item(s)` : "Trash was already empty");
  invalidateFolderSizes();
  await load(state.path);
}

async function duplicateEntries(entries) {
  for (const entry of entries) {
    await api("/api/duplicate", { method: "POST", json: { path: entry.path } });
  }
  toast(entries.length === 1 ? "Duplicated" : `Duplicated ${entries.length} items`);
  invalidateFolderSizes();
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
  invalidateFolderSizes();
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
  invalidateFolderSizes();
  await load(state.path);
}

const NEW_FILE_KINDS = {
  docx: { label: "Word document", ext: "docx", defaultName: "Untitled" },
  odt: { label: "OpenDocument text", ext: "odt", defaultName: "Untitled" },
  md: { label: "Markdown", ext: "md", defaultName: "Untitled" },
  txt: { label: "Text", ext: "txt", defaultName: "Untitled" },
  csv: { label: "CSV", ext: "csv", defaultName: "Untitled" },
  json: { label: "JSON", ext: "json", defaultName: "Untitled" },
  html: { label: "HTML", ext: "html", defaultName: "Untitled" },
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
  invalidateFolderSizes();
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
    invalidateFolderSizes();
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
  invalidateFolderSizes();
  await load(state.path);
  if (error) toast(error.message, true);
  else if (done) toast(clip.mode === "cut" ? "Moved" : "Copied");
  else toast("Already in this folder");
}

function uploadFiles(fileList, names, destination = state.path) {
  const files = [...fileList].map((file, index) => ({
    file,
    name: (names && names[index]) || file.webkitRelativePath || file.name,
  }));
  if (!files.length) return Promise.resolve({ saved: 0, skipped: 0 });
  return sendUploads(files, destination);
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

async function sendUploads(files, destination = state.path) {
  const plan = await api("/api/upload/conflicts", {
    method: "POST",
    json: { path: destination, names: files.map((item) => item.name) },
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
      let posted = await postFile(item, choice, (loaded) => updateProgress(item, index, loaded), destination);
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
        posted = await postFile(item, answer.choice, (loaded) => updateProgress(item, index, loaded), destination);
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

function postFile(item, choice, onProgress, destination = state.path) {
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
    params.set("path", destination);
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

function transferPath(value) {
  const parts = String(value || "").replace(/\\/g, "/").split("/").filter(Boolean);
  if (!parts.length || parts.some((part) => part === "." || part === "..")) {
    throw new Error("The clipboard contains an invalid folder path");
  }
  return parts.join("/");
}

async function readTransfer(dataTransfer) {
  const items = [...(dataTransfer?.items || [])];
  const files = [];
  const directories = new Set();
  const seenFiles = new Set();
  const addDirectory = (name) => directories.add(transferPath(name));
  const addFile = (file, name) => {
    if (!file) return;
    const path = transferPath(name || pasteFileName(file));
    const key = `${path}|${file.size}|${file.lastModified}|${file.type}`;
    if (seenFiles.has(key)) return;
    seenFiles.add(key);
    files.push({ file, name: path });
    const parts = path.split("/");
    for (let index = 1; index < parts.length; index += 1) {
      directories.add(parts.slice(0, index).join("/"));
    }
  };
  const walkHandle = async (handle, prefix = "") => {
    if (handle.kind === "file") {
      addFile(await handle.getFile(), `${prefix}${handle.name}`);
      return;
    }
    const path = `${prefix}${handle.name}`;
    addDirectory(path);
    for await (const [, child] of handle.entries()) await walkHandle(child, `${path}/`);
  };
  const walkEntry = async (entry, prefix = "") => {
    if (entry.isFile) {
      const file = await new Promise((resolve, reject) => entry.file(resolve, reject));
      addFile(file, `${prefix}${entry.name}`);
      return;
    }
    if (!entry.isDirectory) return;
    const path = `${prefix}${entry.name}`;
    addDirectory(path);
    const reader = entry.createReader();
    const children = [];
    while (true) {
      const batch = await new Promise((resolve, reject) => reader.readEntries(resolve, reject));
      if (!batch.length) break;
      children.push(...batch);
    }
    for (const child of children) await walkEntry(child, `${path}/`);
  };

  // Prefer File System Access handles, then the older WebKit entry API. Both
  // preserve directories that contain no files.
  let handledItems = 0;
  for (const item of items) {
    if (item.kind !== "file") continue;
    let handle = null;
    if (typeof item.getAsFileSystemHandle === "function") {
      try {
        handle = await item.getAsFileSystemHandle();
      } catch { /* Try the legacy entry and file APIs. */ }
    }
    if (handle) {
      await walkHandle(handle);
      handledItems += 1;
      continue;
    }
    let entry = null;
    if (typeof item.webkitGetAsEntry === "function") {
      try {
        entry = item.webkitGetAsEntry();
      } catch { /* Try the plain File fallback. */ }
    }
    if (entry) {
      await walkEntry(entry);
      handledItems += 1;
      continue;
    }
    if (typeof item.getAsFile === "function") addFile(item.getAsFile());
  }

  if (!handledItems && !files.length && dataTransfer?.files) {
    for (const file of dataTransfer.files) {
      addFile(file, file.webkitRelativePath || pasteFileName(file));
    }
  }
  return { files, directories: [...directories] };
}

function folderPasteRoots(transfer) {
  const roots = new Set();
  for (const path of transfer.directories) roots.add(path.split("/")[0]);
  for (const item of transfer.files) {
    if (item.name.includes("/")) roots.add(item.name.split("/")[0]);
  }
  return [...roots];
}

async function statusForUploadNames(destination, names) {
  const plan = await api("/api/upload/conflicts", {
    method: "POST",
    json: { path: destination, names },
  });
  return new Map((plan.items || []).map((item) => [item.name, item]));
}

function askFolderPasteConflict(name, existing) {
  $("folder-paste-title").textContent = existing.dir ? "Folder already exists" : "Name already exists";
  $("folder-paste-copy").textContent = existing.dir
    ? `A folder named “${name}” already exists in this location.`
    : `An item named “${name}” already exists in this location.`;
  $("folder-paste-merge").hidden = !existing.dir;
  const dialog = $("folder-paste-dialog");
  dialog.showModal();
  const first = [...dialog.querySelectorAll("button")].find((button) => !button.hidden && button.value !== "cancel");
  first?.focus();
  return new Promise((resolve) => {
    dialog.addEventListener("close", () => {
      const choice = dialog.returnValue;
      resolve(choice === "merge" || choice === "keep" ? choice : null);
    }, { once: true });
  });
}

async function keepBothFolderName(destination, name, reserved) {
  for (let start = 1; start < 10000; start += 100) {
    const candidates = Array.from({ length: 100 }, (_, offset) => `${name} (${start + offset})`)
      .filter((candidate) => !reserved.has(candidate));
    const statuses = await statusForUploadNames(destination, candidates);
    const available = candidates.find((candidate) => !statuses.get(candidate)?.exists);
    if (available) return available;
  }
  throw new Error(`Could not choose a new name for “${name}”`);
}

async function pasteExternalTransfer(transfer, destination) {
  if (!transfer.files.length && !transfer.directories.length) {
    toast("Nothing to upload from the clipboard", true);
    return;
  }
  const roots = folderPasteRoots(transfer);
  const rootNames = new Map();
  // Reserve clipboard roots too, so a generated “(1)” name cannot collide
  // with another folder in the same paste operation.
  const reserved = new Set(roots);
  const statuses = roots.length ? await statusForUploadNames(destination, roots) : new Map();
  for (const name of roots) {
    const existing = statuses.get(name);
    if (!existing?.exists) {
      rootNames.set(name, name);
      reserved.add(name);
      continue;
    }
    const choice = await askFolderPasteConflict(name, existing);
    if (!choice) {
      toast("Upload cancelled");
      return;
    }
    if (choice === "merge") {
      rootNames.set(name, name);
      reserved.add(name);
    } else {
      const kept = await keepBothFolderName(destination, name, reserved);
      rootNames.set(name, kept);
      reserved.add(kept);
    }
  }
  const rewrite = (name) => {
    const path = transferPath(name);
    const slash = path.indexOf("/");
    const root = slash < 0 ? path : path.slice(0, slash);
    const mapped = rootNames.get(root);
    return mapped ? `${mapped}${slash < 0 ? "" : path.slice(slash)}` : path;
  };
  const directories = new Set(roots.map((root) => rootNames.get(root) || root));
  for (const path of transfer.directories) directories.add(rewrite(path));
  const mappedFiles = transfer.files.map((item) => ({ file: item.file, name: rewrite(item.name) }));
  if (directories.size) {
    await api("/api/upload/directories", {
      method: "POST",
      json: { path: destination, names: [...directories].sort((a, b) => a.split("/").length - b.split("/").length) },
    });
  }
  const result = mappedFiles.length
    ? await uploadFiles(mappedFiles.map((item) => item.file), mappedFiles.map((item) => item.name), destination)
    : { saved: 0, skipped: 0 };
  toast(mappedFiles.length ? uploadSummary(result) : `Pasted ${directories.size} folders`);
}

async function readDrop(dataTransfer) {
  return readTransfer(dataTransfer);
}

function syncControls() {
  syncTileSize();
  $("sort").value = state.sort;
  $("group-by").value = state.groupBy;
  $("sort-dir").textContent = state.sort === "name" ? (state.direction === "desc" ? "Z–A" : "A–Z") : (state.direction === "desc" ? "↓" : "↑");
  $("sort-dir").setAttribute("aria-label", state.direction === "desc" ? "Sort descending" : "Sort ascending");
  document.querySelectorAll("[data-view]").forEach((button) => {
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
  if (state.me.admin && state.me.updatesConfigured) {
    pollUpdateNotice();
    if (!updateStatusTimer) updateStatusTimer = setInterval(pollUpdateNotice, 15 * 60 * 1000);
  }
  syncControls();
  try {
    await load(hashToPath());
  } catch (err) {
    toast(err.message, true);
  }
}

async function pollUpdateNotice() {
  if (!state.me?.admin || !state.me.updatesConfigured) return;
  try {
    const update = await api("/api/update/status");
    if (update.available && update.latest !== updateNoticeVersion) {
      updateNoticeVersion = update.latest;
      toast(`OwnNAS ${update.latest} is available. Open Settings → Updates to install it.`);
    }
  } catch {}
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

// Explicitly focus the file pane before marquee selection prevents default focus.
document.querySelector(".explorer-content").addEventListener("pointerdown", (event) => {
  if (event.button !== 0) return;
  if (event.target.closest("#search-panel, .details-header, button, a, input, textarea, select, [contenteditable]:not([contenteditable=\"false\"])")) return;
  const files = $("files");
  files.classList.add("pointer-focused");
  files.focus({ preventScroll: true });
}, true);

document.addEventListener("keydown", () => {
  $("files").classList.remove("pointer-focused");
}, true);

$("files").addEventListener("click", (event) => {
  if (event.target.closest("[data-column-entry]")) return;
  if (skipNextClick) {
    skipNextClick = false;
    return;
  }
  const groupButton = event.target.closest("[data-file-group]");
  if (groupButton) {
    const key = groupButton.dataset.fileGroup;
    if (state.collapsedGroups.has(key)) state.collapsedGroups.delete(key);
    else state.collapsedGroups.add(key);
    const visible = new Set(displayedEntries().map((entry) => entry.path));
    state.selected = new Set([...state.selected].filter((path) => visible.has(path)));
    renderFiles();
    $("files").focus({ preventScroll: true });
    return;
  }
  const sortButton = event.target.closest("[data-detail-sort]");
  if (sortButton) {
    const key = sortButton.dataset.detailSort;
    state.direction = state.sort === key && state.direction === "asc" ? "desc" : "asc";
    state.sort = key;
    localStorage.setItem("ownnas-sort", key);
    localStorage.setItem("ownnas-dir", state.direction);
    syncControls();
    renderFiles();
    return;
  }
  if (event.target.closest(".details-header")) return;
  const tagButton = event.target.closest("[data-filter-tag]");
  if (tagButton) {
    event.stopPropagation();
    const tag = tagButton.dataset.filterTag;
    state.filter = `tag:${tag}`;
    $("search").value = state.filter;
    clearDeepSearch();
    renderFiles();
    $("search").focus();
    runDeepSearch().catch((err) => toast(err.message, true));
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
        state.focus = "";
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
  if (event.target.closest(".more, button, a, input, .details-header, [data-filter-tag], [data-menu-for]")) return;
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
  if (lastHit) {
    state.anchor = lastHit;
    state.focus = lastHit;
  }
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
  state.focus = "";
  paintSelection();
  openSelectionMenu(event.clientX, event.clientY, state.path);
});

function runMenuAction(button, entries, pasteInto) {
  const format = button.dataset.format || "";
  const action = button.getAttribute("data-menu");
  const one = entries.length === 1 ? entries[0] : null;
  if (action === "folderAppearance") customizeFolders(entries).catch(err => toast(err.message, true));
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
  if (action === "rename") renameEntries(entries).catch((err) => toast(err.message, true));
  if (action === "restore") restoreEntries(entries).catch((err) => toast(err.message, true));
  if (action === "delete") deleteEntries(entries).catch((err) => toast(err.message, true));
  if (action === "duplicate") duplicateEntries(entries).catch((err) => toast(err.message, true));
  if (action === "move") moveEntries(entries).catch((err) => toast(err.message, true));
  if (action === "convert") convertImages(entries, format).catch((err) => toast(err.message, true));
  if (action === "compress") compressEntries(entries, format).catch((err) => toast(err.message, true));
  if (action === "pdfExtract") pdfExtractPages(entries).catch((err) => toast(err.message, true));
  if (action === "pdfSplit") pdfSplitPages(entries).catch((err) => toast(err.message, true));
  if (action === "pdfRotate") pdfRotatePages(entries).catch((err) => toast(err.message, true));
  if (action === "pdfMerge") pdfMergeEntries(entries).catch((err) => toast(err.message, true));
  if (action === "upload") $("upload-input").click();
  if (action === "uploadFolder") $("folder-input").click();
  if (action === "downloadZip") $("zip-link").click();
  if (action === "search") focusSearch();
  if (action === "library") $("library-btn").click();
  if (action === "bookmark") $("bookmark-btn").click();
  if (action === "usage") $("usage-btn").click();
  if (action === "duplicates") openDuplicatesFinder().catch((err) => toast(err.message, true));
  if (action === "activity") $("activity-btn").click();
  if (action === "bulkTag") bulkTagEntries(entries).catch((err) => toast(err.message, true));
  if (action === "emptyTrash") $("empty-trash-btn").click();
  if (action === "toggleHidden") {
    const toggle = $("hidden-toggle");
    toggle.checked = !toggle.checked;
    toggle.dispatchEvent(new Event("change", { bubbles: true }));
  }
}

$("menu").addEventListener("click", (event) => {
  const button = event.target.closest("[data-menu]");
  if (!button) return;
  event.preventDefault();
  event.stopPropagation();
  const action = button.getAttribute("data-menu");
  if (action === "convertMenu") {
    openMenuSub("convert", button);
    return;
  }
  if (action === "compressMenu") {
    openMenuSub("compress", button);
    return;
  }
  if (action === "pdfMenu") {
    openMenuSub("pdf", button);
    return;
  }
  const entries = state.menuEntries.slice();
  const pasteInto = state.pasteInto;
  closeMenu();
  runMenuAction(button, entries, pasteInto);
});

$("menu-sub").addEventListener("click", (event) => {
  const button = event.target.closest("[data-menu]");
  if (!button) return;
  event.preventDefault();
  event.stopPropagation();
  const entries = state.menuEntries.slice();
  const pasteInto = state.pasteInto;
  closeMenu();
  runMenuAction(button, entries, pasteInto);
});

// Capture outside clicks before file/menu handlers stop propagation.
document.addEventListener("pointerdown", (event) => {
  if (!$("search-panel").hidden && !event.target.closest("#search-panel, .search-wrap")) {
    clearDeepSearch();
  }
}, true);

document.addEventListener("click", (event) => {
  if (
    !event.target.closest("#menu")
    && !event.target.closest("#menu-sub")
    && !event.target.closest(".more")
    && !event.target.closest("#fab-menu")
  ) closeMenu();
});

$("preview-close").addEventListener("click", () => {
  closePreview();
});

$("preview-expand").addEventListener("click", () => {
  setPreviewFullscreen(!$("preview").classList.contains("is-fullscreen"));
});

$("preview-body").addEventListener("click", (event) => {
  const button = event.target.closest("[data-editor]");
  if (!button || !state.editor) return;
  const action = button.getAttribute("data-editor");
  if (action === "mode") {
    syncEditorFromDom();
    state.editor.mode = button.getAttribute("data-mode") || "view";
    paintTextEditor();
    const focus = $("preview-body").querySelector("[data-editor-input], [data-csv-editor] input");
    if (focus) focus.focus();
    return;
  }
  if (action === "save") {
    saveEditor().catch((err) => toast(err.message, true));
    return;
  }
  if (action === "add-row" || action === "add-col" || action === "del-row" || action === "del-col") {
    mutateCsvGrid(action);
  }
});

$("preview-body").addEventListener("input", (event) => {
  if (!state.editor || state.editor.mode !== "edit") return;
  if (!event.target.closest("[data-editor-input], [data-csv-editor]")) return;
  markEditorDirty();
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

$("search").addEventListener("input", (event) => {
  state.filter = event.target.value;
  if (!state.filter.trim()) clearDeepSearch();
  renderFiles();
});
$("search").addEventListener("keydown", (event) => {
  if (event.key === "Enter") {
    event.preventDefault();
    runDeepSearch().catch((err) => toast(err.message, true));
  }
  if (event.key === "Escape" && state.filter) {
    event.preventDefault();
    event.stopPropagation();
    state.filter = "";
    $("search").value = "";
    clearDeepSearch();
    renderFiles();
  }
});
$("search-go").addEventListener("click", () => {
  runDeepSearch().catch((err) => toast(err.message, true));
});
$("search-panel-close").addEventListener("click", () => {
  resetSearch();
  renderFiles();
});

$("group-by").addEventListener("change", (event) => {
  state.groupBy = normalizeGroup(event.target.value);
  state.collapsedGroups.clear();
  localStorage.setItem("ownnas-group", state.groupBy);
  renderFiles();
});

$("files").addEventListener("pointerdown", (event) => {
  const handle = event.target.closest("[data-detail-resize]");
  if (!handle || event.button !== 0) return;
  event.preventDefault();
  event.stopPropagation();
  const column = Number(handle.dataset.detailResize);
  const start = event.clientX;
  const initial = state.detailWidths[column];
  handle.focus({ preventScroll: true });
  handle.setPointerCapture(event.pointerId);
  const move = (ev) => {
    state.detailWidths[column] = Math.max(column === 0 ? 180 : 72, Math.min(800, initial + ev.clientX - start));
    handle.setAttribute("aria-valuenow", state.detailWidths[column]);
    applyDetailWidths();
  };
  const finish = () => {
    localStorage.setItem("ownnas-detail-widths", JSON.stringify(state.detailWidths));
    handle.removeEventListener("pointermove", move);
    handle.removeEventListener("pointerup", finish);
    handle.removeEventListener("pointercancel", finish);
  };
  handle.addEventListener("pointermove", move);
  handle.addEventListener("pointerup", finish);
  handle.addEventListener("pointercancel", finish);
});
$("files").addEventListener("keydown", (event) => {
  const handle = event.target.closest("[data-detail-resize]");
  if (!handle || !["ArrowLeft", "ArrowRight"].includes(event.key)) return;
  event.preventDefault();
  event.stopPropagation();
  const column = Number(handle.dataset.detailResize);
  state.detailWidths[column] = Math.max(column === 0 ? 180 : 72, Math.min(800, state.detailWidths[column] + (event.key === "ArrowRight" ? 16 : -16)));
  handle.setAttribute("aria-valuenow", state.detailWidths[column]);
  applyDetailWidths();
  localStorage.setItem("ownnas-detail-widths", JSON.stringify(state.detailWidths));
});

$("sort").addEventListener("change", (event) => {
  state.sort = event.target.value;
  localStorage.setItem("ownnas-sort", state.sort);
  syncControls();
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
$("undo-btn").addEventListener("click", undoFileOperation);
$("view-columns").addEventListener("click", () => setView("columns"));
$("files").addEventListener("click", event => {
  const item = event.target.closest("[data-column-entry]");
  if (!item) return;
  const entry = columnEntries.get(item.dataset.columnEntry);
  if (entry?.dir) go(entry.path).catch(err => toast(err.message, true));
  else if (entry) { selectOnly(entry.path); openEntry(entry).catch(err => toast(err.message, true)); }
});
$("view-list").addEventListener("click", () => setView("list"));

$("tile-size").addEventListener("input", (event) => {
  state.tileSize = normalizeTileSize(event.target.value);
  syncTileSize();
});
$("tile-size").addEventListener("change", () => {
  localStorage.setItem("ownnas-tile-size", String(state.tileSize));
});

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
  if (state.me) {
    invalidateFolderSizes();
    load(state.path).catch((err) => toast(err.message, true));
  }
}
$("upload-input").addEventListener("change", () => onPicked($("upload-input")));
$("folder-input").addEventListener("change", () => onPicked($("folder-input")));

window.addEventListener("hashchange", () => {
  if (!state.me) return;
  load(hashToPath()).catch((err) => toast(err.message, true));
});

window.addEventListener("keydown", (event) => {
  if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "s") {
    if (state.editor && state.me && !state.me.readonly && !$("preview").hidden) {
      event.preventDefault();
      saveEditor().catch((err) => toast(err.message, true));
      return;
    }
  }
  if (event.key === "Escape") {
    // Let open dialogs handle Escape themselves (and avoid clearing selection mid-prompt).
    if (document.querySelector("dialog[open]")) return;
    if (!$("search-panel").hidden) {
      event.preventDefault();
      clearDeepSearch();
      return;
    }
    if (!$("menu-sub").hidden) {
      closeMenuSub();
      return;
    }
    closeMenu();
    if ($("preview").classList.contains("is-fullscreen")) {
      setPreviewFullscreen(false);
      return;
    }
    if (!$("preview").hidden && !closePreview()) return;
    if (!$("preview").hidden) return;
    closeLibrary();
    closeSettings();
    if (marquee) endMarquee();
    if (state.selected.size) {
      state.selected = new Set();
      state.anchor = "";
      state.focus = "";
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
  const typing = event.target.closest("input, textarea, select, [contenteditable]:not([contenteditable=\"false\"])");
  if (typing) return;
  if ((event.metaKey || event.ctrlKey) && !event.shiftKey && event.key.toLowerCase() === "z" && !document.querySelector("dialog[open]")) {
    event.preventDefault(); undoFileOperation(); return;
  }
  if (event.key.length === 1 && !event.metaKey && !event.ctrlKey && !event.altKey && !event.isComposing
      && state.me && !$("app-view").hidden && $("preview").hidden && $("settings").hidden && $("library").hidden
      && $("menu").hidden && !event.target.closest("[role=separator]")
      && !(event.key === " " && event.target.closest("button, a"))) {
    event.preventDefault();
    const search = $("search");
    search.focus({ preventScroll: true });
    search.value = state.filter + event.key;
    search.setSelectionRange(search.value.length, search.value.length);
    search.dispatchEvent(new Event("input", { bubbles: true }));
    return;
  }
  if (event.metaKey && !event.ctrlKey && !event.altKey && !event.shiftKey
      && (event.key === "ArrowUp" || event.key === "ArrowDown")) {
    if (!state.me || $("app-view").hidden || !$("settings").hidden || !$("library").hidden) return;
    event.preventDefault();
    if (event.repeat) return;
    closeMenu();
    if (event.key === "ArrowUp") {
      if (state.path) go(parentPath(state.path)).catch((err) => toast(err.message, true));
    } else {
      const entries = selectedEntries();
      if (entries.length === 1) openEntry(entries[0]).catch((err) => toast(err.message, true));
    }
    return;
  }
  const key = event.key.toLowerCase();
  const command = event.metaKey || event.ctrlKey;
  if (!command && event.target.closest("button, a, [role=separator]")
      && ["Enter", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight"].includes(event.key)) return;
  if (command && key === "a") {
    event.preventDefault();
    const items = displayedEntries();
    state.selected = new Set(items.map((entry) => entry.path));
    state.anchor = items.length ? items[0].path : "";
    state.focus = state.anchor;
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

  const editingSensitive = event.target.closest("#login-view, #password-dialog, #text-dialog, #create-file-dialog, #comment-input, #tag-input, #bulk-tag-input, #search, [data-editor-input], [data-csv-editor]");
  const hasFiles = clipboardHasFiles(data);

  // File paste should upload even when the folder filter (or similar) is focused.
  if (hasFiles) {
    if (editingSensitive) return;
    event.preventDefault();
    event.stopPropagation();
    const destination = state.path;
    readTransfer(data)
      .then((transfer) => pasteExternalTransfer(transfer, destination))
      .catch((err) => toast(err.cancel ? "Upload cancelled" : err.message, !err.cancel))
      .finally(() => {
        if (state.me && state.path === destination) load(destination).catch((err) => toast(err.message, true));
      });
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

const INTERNAL_DRAG = "application/x-ownnas-paths";
let internalDragPaths = [];
$("files").addEventListener("dragstart", event => {
  const item = event.target.closest(".card[data-path], [data-column-entry]");
  if (!item || state.me?.readonly) return;
  const path = item.dataset.path || item.dataset.columnEntry;
  internalDragPaths = state.selected.has(path) ? selectedEntries().map(entry => entry.path) : [path];
  event.dataTransfer.setData(INTERNAL_DRAG, JSON.stringify(internalDragPaths));
  event.dataTransfer.effectAllowed = "copyMove";
});
function internalDropTarget(event) {
  const folder = event.target.closest('[data-dir="1"][data-path], [data-dir="1"][data-column-entry]');
  if (folder) return {element: folder, path: folder.dataset.path || folder.dataset.columnEntry};
  const column = event.target.closest("[data-column-path]");
  if (column) return {element: column, path: column.dataset.columnPath};
  const crumb = event.target.closest("[data-go]");
  if (crumb) return {element: crumb, path: crumb.dataset.go};
  if (event.target.closest("#files")) return {element: $("files"), path: state.path};
  return null;
}
window.addEventListener("dragover", event => {
  if (!internalDragPaths.length || ![...event.dataTransfer.types].includes(INTERNAL_DRAG)) return;
  const target = internalDropTarget(event);
  document.querySelectorAll(".folder-drop-target").forEach(el => el.classList.remove("folder-drop-target"));
  if (!target || internalDragPaths.some(p => target.path === p || target.path.startsWith(p + "/") || parentPath(p) === target.path)) { event.dataTransfer.dropEffect = "none"; return; }
  event.preventDefault(); target.element.classList.add("folder-drop-target");
  event.dataTransfer.dropEffect = event.altKey || event.ctrlKey ? "copy" : "move";
});
window.addEventListener("dragend", () => {
  internalDragPaths = [];
  document.querySelectorAll(".folder-drop-target").forEach(el => el.classList.remove("folder-drop-target"));
});
window.addEventListener("drop", async event => {
  if (![...event.dataTransfer.types].includes(INTERNAL_DRAG)) return;
  event.preventDefault();
  const target = internalDropTarget(event);
  const paths = internalDragPaths.slice(); internalDragPaths = [];
  document.querySelectorAll(".folder-drop-target").forEach(el => el.classList.remove("folder-drop-target"));
  if (!target || !paths.length || state.me?.readonly) return;
  const copy = event.altKey || event.ctrlKey;
  try {
    for (const path of paths) {
      if (path === target.path || target.path.startsWith(path + "/") || parentPath(path) === target.path) continue;
      await api(copy ? "/api/copy" : "/api/move", {method: "POST", json: {path, dest: target.path}});
    }
    toast(copy ? "Copied items" : "Moved items");
  } catch (err) { toast(err.message, true); }
  invalidateFolderSizes(); await load(state.path);
});

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
    const destination = state.path;
    const transfer = await readDrop(event.dataTransfer);
    await pasteExternalTransfer(transfer, destination);
  } catch (err) {
    toast(err.cancel ? "Upload cancelled" : err.message, !err.cancel);
  }
  if (state.me && state.path === destination) load(destination).catch((err) => toast(err.message, true));
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
$("tools-more-btn").addEventListener("click", () => {
  const actions = document.querySelector(".toolbar-actions");
  const btn = $("tools-more-btn");
  const open = !actions.classList.contains("tools-expanded");
  actions.classList.toggle("tools-expanded", open);
  btn.setAttribute("aria-expanded", open ? "true" : "false");
  btn.title = open ? "Fewer actions" : "More actions";
});
$("fab-menu").addEventListener("click", (event) => {
  event.stopPropagation();
  const menu = $("menu");
  if (!menu.hidden) {
    closeMenu();
    return;
  }
  const entries = selectedEntries();
  let pasteInto = state.path;
  if (entries.length === 1 && entries[0].dir) pasteInto = entries[0].path;
  openSelectionMenu(8, 8, pasteInto);
  const fab = event.currentTarget.getBoundingClientRect();
  const rect = menu.getBoundingClientRect();
  menu.style.left = `${Math.max(8, Math.min(fab.right - rect.width, window.innerWidth - rect.width - 8))}px`;
  menu.style.top = `${Math.max(8, fab.top - rect.height - 10)}px`;
});
$("settings-btn").addEventListener("click", () => openSettings());
$("settings-close").addEventListener("click", closeSettings);
$("settings-tab-theme").addEventListener("click", () => setSettingsTab("theme"));
$("settings-tab-updates").addEventListener("click", () => setSettingsTab("updates"));
$("settings-tab-users").addEventListener("click", () => {
  setSettingsTab("users");
  loadUsers().catch((err) => toast(err.message, true));
});
$("update-check-btn").addEventListener("click", () => {
  checkForUpdates().catch((err) => toast(err.message, true));
});
$("update-apply-btn").addEventListener("click", () => {
  applyUpdate().catch((err) => toast(err.message, true));
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

$("usage-btn").addEventListener("click", () => {
  measureFolderSize().catch((err) => toast(err.message, true));
});

$("duplicates-btn").addEventListener("click", () => {
  openDuplicatesFinder().catch((err) => toast(err.message, true));
});

$("jobs-btn").addEventListener("click", () => openJobsPanel());
$("jobs-hide").addEventListener("click", () => {
  jobState.hidden = true;
  $("jobs-panel").hidden = true;
});
$("jobs-list").addEventListener("click", (event) => {
  const button = event.target.closest("[data-job-cancel]");
  if (!button) return;
  const job = jobState.items.get(button.dataset.jobCancel);
  if (job && job.onCancel) job.onCancel();
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

async function measureFolderSize() {
  const label = state.path || (state.me && state.me.rootName) || "Library";
  const id = `usage-${++jobState.seq}`;
  const controller = new AbortController();
  upsertJob(id, {
    title: "Folder size",
    detail: `Counting ${label}…`,
    percent: null,
    cancelable: true,
    onCancel: () => controller.abort(),
  });
  try {
    const usage = await api(`/api/usage?path=${encodeURIComponent(state.path)}&slow=1`, {
      signal: controller.signal,
    });
    const extra = usage.truncated ? " (count stopped early)" : "";
    const detail = `${formatSize(usage.bytes, false)} in ${usage.files} files${extra}`;
    upsertJob(id, { title: "Folder size", detail, percent: 100, cancelable: false });
    toast(detail);
    setTimeout(() => removeJob(id), 5000);
  } catch (err) {
    removeJob(id);
    if (isCancelError(err)) {
      toast("Folder size cancelled");
      return;
    }
    throw err;
  }
}

async function compressEntries(entries, format) {
  const items = (entries || []).filter((entry) => entry && entry.path);
  if (!items.length || !format) return;
  const label = String(format).toUpperCase();
  const id = `compress-${++jobState.seq}`;
  const controller = new AbortController();
  upsertJob(id, {
    title: `Compress to ${label}`,
    detail: items.length === 1 ? items[0].name : `${items.length} items`,
    percent: null,
    cancelable: true,
    onCancel: () => controller.abort(),
  });
  try {
    const data = await api("/api/compress", {
      method: "POST",
      json: {
        paths: items.map((entry) => entry.path),
        format,
      },
      signal: controller.signal,
    });
    invalidateFolderSizes();
    await load(state.path);
    const name = (data.path || "").split("/").pop() || label;
    toast(`Created ${name}`);
  } catch (err) {
    if (isCancelError(err)) {
      toast("Compression cancelled");
      return;
    }
    throw err;
  } finally {
    removeJob(id);
  }
}

async function pdfExtractPages(entries) {
  const entry = (entries || []).find((item) => item && item.kind === "pdf");
  if (!entry) return;
  let pageCount = 0;
  try {
    const info = await api(`/api/pdf/info?path=${encodeURIComponent(entry.path)}`);
    pageCount = info.pages || 0;
  } catch (err) {
    toast(err.message, true);
    return;
  }
  $("pdf-extract-meta").textContent = pageCount
    ? `“${entry.name}” has ${pageCount} page${pageCount === 1 ? "" : "s"}.`
    : `“${entry.name}”`;
  $("pdf-extract-pages").value = pageCount > 1 ? "1" : "1";
  $("pdf-extract-mode-combined").checked = true;
  const dialog = $("pdf-extract-dialog");
  dialog.returnValue = "";
  dialog.showModal();
  $("pdf-extract-pages").focus();
  $("pdf-extract-pages").select();
  const ok = await new Promise((resolve) => {
    dialog.addEventListener("close", () => resolve(dialog.returnValue === "ok"), { once: true });
  });
  if (!ok) return;
  const pages = $("pdf-extract-pages").value.trim();
  const mode = $("pdf-extract-mode-separate").checked ? "separate" : "combined";
  if (!pages) {
    toast("Enter at least one page number", true);
    return;
  }
  const id = `pdf-extract-${++jobState.seq}`;
  upsertJob(id, {
    title: "Extract PDF pages",
    detail: entry.name,
    percent: null,
  });
  try {
    const data = await api("/api/pdf/extract", {
      method: "POST",
      json: { path: entry.path, pages, mode },
    });
    invalidateFolderSizes();
    await load(state.path);
    const count = (data.paths || []).length;
    toast(count === 1 ? "Created 1 PDF" : `Created ${count} PDFs`);
  } finally {
    removeJob(id);
  }
}

async function pdfSplitPages(entries) {
  const entry = (entries || []).find((item) => item && item.kind === "pdf");
  if (!entry) return;
  let pageCount = "?";
  try {
    const info = await api(`/api/pdf/info?path=${encodeURIComponent(entry.path)}`);
    pageCount = info.pages || "?";
  } catch (_) {
    /* confirm without count */
  }
  if (!await askConfirm(
    `Split “${entry.name}” into individual page PDFs${pageCount !== "?" ? ` (${pageCount} files)` : ""}?`,
    "Split",
  )) return;
  const id = `pdf-split-${++jobState.seq}`;
  upsertJob(id, {
    title: "Split PDF",
    detail: entry.name,
    percent: null,
  });
  try {
    const data = await api("/api/pdf/split", {
      method: "POST",
      json: { path: entry.path },
    });
    invalidateFolderSizes();
    await load(state.path);
    const count = (data.paths || []).length;
    toast(`Created ${count} page PDF${count === 1 ? "" : "s"}`);
  } finally {
    removeJob(id);
  }
}

async function pdfRotatePages(entries) {
  const entry = (entries || []).find((item) => item && item.kind === "pdf");
  if (!entry) return;
  let pageCount = 0;
  try {
    const info = await api(`/api/pdf/info?path=${encodeURIComponent(entry.path)}`);
    pageCount = info.pages || 0;
  } catch (err) {
    toast(err.message, true);
    return;
  }
  $("pdf-rotate-meta").textContent = pageCount
    ? `“${entry.name}” has ${pageCount} page${pageCount === 1 ? "" : "s"}. Leave pages blank to rotate all.`
    : `“${entry.name}”. Leave pages blank to rotate all.`;
  $("pdf-rotate-pages").value = "";
  $("pdf-rotate-degrees").value = "90";
  const dialog = $("pdf-rotate-dialog");
  dialog.returnValue = "";
  dialog.showModal();
  $("pdf-rotate-pages").focus();
  const ok = await new Promise((resolve) => {
    dialog.addEventListener("close", () => resolve(dialog.returnValue === "ok"), { once: true });
  });
  if (!ok) return;
  const pages = $("pdf-rotate-pages").value.trim();
  const degrees = Number($("pdf-rotate-degrees").value) || 90;
  const id = `pdf-rotate-${++jobState.seq}`;
  upsertJob(id, {
    title: "Rotate PDF",
    detail: entry.name,
    percent: null,
  });
  try {
    const data = await api("/api/pdf/rotate", {
      method: "POST",
      json: { path: entry.path, pages, degrees },
    });
    invalidateFolderSizes();
    await load(state.path);
    const name = (data.path || "").split("/").pop() || "rotated.pdf";
    toast(`Created ${name}`);
  } finally {
    removeJob(id);
  }
}

async function pdfMergeEntries(entries) {
  const items = (entries || []).filter((entry) => entry && entry.kind === "pdf");
  if (items.length < 2) {
    toast("Select at least two PDFs to join", true);
    return;
  }
  if (!await askConfirm(
    `Join ${items.length} PDFs into one file (written beside them, originals kept)?`,
    "Join",
  )) return;
  const id = `pdf-merge-${++jobState.seq}`;
  upsertJob(id, {
    title: "Join PDFs",
    detail: `${items.length} files`,
    percent: null,
  });
  try {
    const data = await api("/api/pdf/merge", {
      method: "POST",
      json: { paths: items.map((entry) => entry.path) },
    });
    invalidateFolderSizes();
    await load(state.path);
    const name = (data.path || "").split("/").pop() || "joined.pdf";
    toast(`Created ${name}`);
  } finally {
    removeJob(id);
  }
}

async function convertImages(entries, format) {
  const images = (entries || []).filter((entry) => entry && entry.kind === "image");
  if (!images.length || !format) return;
  const label = format === "jpeg" ? "JPEG" : String(format).toUpperCase();
  const id = `convert-${++jobState.seq}`;
  const controller = new AbortController();
  upsertJob(id, {
    title: `Convert to ${label}`,
    detail: images.length === 1 ? images[0].name : `${images.length} images`,
    percent: 0,
    cancelable: true,
    onCancel: () => controller.abort(),
  });
  let done = 0;
  let failed = 0;
  try {
    for (const entry of images) {
      if (controller.signal.aborted) {
        const err = new Error("Cancelled");
        err.name = "AbortError";
        err.cancel = true;
        throw err;
      }
      upsertJob(id, {
        title: `Convert to ${label}`,
        detail: `${entry.name} (${done + 1}/${images.length})`,
        percent: (done / images.length) * 100,
        cancelable: true,
        onCancel: () => controller.abort(),
      });
      try {
        await api("/api/convert-image", {
          method: "POST",
          json: { path: entry.path, format },
          signal: controller.signal,
        });
        done += 1;
      } catch (err) {
        if (isCancelError(err)) throw err;
        failed += 1;
        if (images.length === 1) throw err;
      }
    }
    invalidateFolderSizes();
    await load(state.path);
    if (failed && !done) throw new Error("Could not convert the selected images");
    if (failed) toast(`Converted ${done}, skipped ${failed}`, true);
    else toast(done === 1 ? `Saved as ${label}` : `Converted ${done} images to ${label}`);
  } catch (err) {
    if (isCancelError(err)) {
      toast(done ? `Converted ${done}, then cancelled` : "Conversion cancelled");
      if (done) {
        invalidateFolderSizes();
        await load(state.path);
      }
      return;
    }
    throw err;
  } finally {
    removeJob(id);
  }
}

async function bulkTagEntries(entries) {
  const items = (entries || []).filter((entry) => entry && entry.path);
  if (!items.length) return;

  const existing = [];
  const seen = new Set();
  for (const entry of items) {
    for (const tag of entry.tags || []) {
      const key = String(tag).toLowerCase();
      if (seen.has(key)) continue;
      seen.add(key);
      existing.push(String(tag));
    }
  }
  existing.sort((a, b) => a.localeCompare(b, undefined, { sensitivity: "base" }));

  const draft = {
    add: [],
    remove: new Set(),
  };

  const renderExisting = () => {
    const wrap = $("bulk-tag-existing-wrap");
    const box = $("bulk-tag-existing");
    if (!existing.length) {
      wrap.hidden = true;
      box.innerHTML = "";
      return;
    }
    wrap.hidden = false;
    box.innerHTML = existing.map((tag) => {
      const marked = draft.remove.has(tag.toLowerCase());
      return `<span class="tag-chip${marked ? " is-remove" : ""}">
        <span>${esc(tag)}</span>
        <button type="button" data-bulk-existing="${esc(tag)}" aria-label="${marked ? "Keep" : "Remove"} tag ${esc(tag)}">×</button>
      </span>`;
    }).join("");
  };

  const renderPending = () => {
    $("bulk-tag-pending").innerHTML = draft.add.map((tag) => (
      `<span class="tag-chip">
        <span>${esc(tag)}</span>
        <button type="button" data-bulk-pending="${esc(tag)}" aria-label="Remove tag ${esc(tag)}">×</button>
      </span>`
    )).join("");
  };

  const addPending = (raw) => {
    const parts = String(raw || "")
      .split(/[,;\n]+/)
      .map((part) => part.trim())
      .filter(Boolean);
    for (const tag of parts) {
      if (tag.length > 48) {
        toast("Tags must be 48 characters or fewer", true);
        continue;
      }
      const key = tag.toLowerCase();
      if (draft.add.some((item) => item.toLowerCase() === key)) continue;
      // If it was marked for removal, undo that instead of duplicating as add.
      if (draft.remove.has(key)) {
        draft.remove.delete(key);
        continue;
      }
      if (existing.some((item) => item.toLowerCase() === key)) {
        toast(`“${tag}” is already on the selection`);
        continue;
      }
      draft.add.push(tag);
    }
    renderPending();
    renderExisting();
  };

  $("bulk-tag-summary").textContent = `Apply to ${items.length} selected item${items.length === 1 ? "" : "s"}.`;
  $("bulk-tag-input").value = "";
  renderExisting();
  renderPending();

  const dialog = $("bulk-tag-dialog");
  const form = $("bulk-tag-form");
  const input = $("bulk-tag-input");
  dialog.showModal();
  input.focus();

  const onExistingClick = (event) => {
    const button = event.target.closest("[data-bulk-existing]");
    if (!button) return;
    const tag = button.dataset.bulkExisting;
    const key = tag.toLowerCase();
    if (draft.remove.has(key)) draft.remove.delete(key);
    else draft.remove.add(key);
    renderExisting();
  };
  const onPendingClick = (event) => {
    const button = event.target.closest("[data-bulk-pending]");
    if (!button) return;
    const tag = button.dataset.bulkPending;
    draft.add = draft.add.filter((item) => item.toLowerCase() !== tag.toLowerCase());
    renderPending();
  };
  const onInputKeydown = (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      const value = input.value.trim();
      if (!value) return;
      addPending(value);
      input.value = "";
    }
    if (event.key === "Backspace" && !input.value && draft.add.length) {
      draft.add.pop();
      renderPending();
    }
  };
  const onCancel = () => dialog.close();

  $("bulk-tag-existing").addEventListener("click", onExistingClick);
  $("bulk-tag-pending").addEventListener("click", onPendingClick);
  input.addEventListener("keydown", onInputKeydown);
  $("bulk-tag-cancel").addEventListener("click", onCancel);

  const applied = await new Promise((resolve) => {
    const onSubmit = (event) => {
      event.preventDefault();
      const typed = input.value.trim();
      if (typed) {
        addPending(typed);
        input.value = "";
      }
      if (!draft.add.length && !draft.remove.size) {
        toast("Add a tag or mark one to remove", true);
        return;
      }
      resolve({
        add: draft.add.slice(),
        remove: Array.from(draft.remove).map((key) => {
          const found = existing.find((tag) => tag.toLowerCase() === key);
          return found || key;
        }),
      });
      dialog.close();
    };
    const onClose = () => {
      form.removeEventListener("submit", onSubmit);
      $("bulk-tag-existing").removeEventListener("click", onExistingClick);
      $("bulk-tag-pending").removeEventListener("click", onPendingClick);
      input.removeEventListener("keydown", onInputKeydown);
      $("bulk-tag-cancel").removeEventListener("click", onCancel);
      // Cancel / Escape with no resolve from submit → null
      resolve(null);
    };
    form.addEventListener("submit", onSubmit);
    dialog.addEventListener("close", onClose, { once: true });
  });

  if (!applied) return;

  const paths = items.map((entry) => entry.path);
  for (const tag of applied.remove) {
    await api("/api/tags/bulk", {
      method: "POST",
      json: { paths, tag, action: "remove" },
    });
  }
  for (const tag of applied.add) {
    await api("/api/tags/bulk", {
      method: "POST",
      json: { paths, tag, action: "add" },
    });
  }
  const parts = [];
  if (applied.add.length) parts.push(`added ${applied.add.length}`);
  if (applied.remove.length) parts.push(`removed ${applied.remove.length}`);
  toast(`Tags ${parts.join(", ")}`);
  await load(state.path);
  if (state.current) loadAnnotations(state.current.path).catch(() => {});
}

function fileStem(name) {
  const base = String(name || "");
  const index = base.lastIndexOf(".");
  if (index <= 0) return base;
  return base.slice(0, index);
}

function applyRenamePattern(pattern, entry, index, total) {
  const ext = fileExtension(entry.name);
  const extPart = ext ? `.${ext}` : "";
  const parent = entry.path.includes("/")
    ? entry.path.slice(0, entry.path.lastIndexOf("/")).split("/").pop()
    : (state.me && state.me.rootName) || "Library";
  const n = index + 1;
  return String(pattern)
    .replaceAll("{name}", fileStem(entry.name))
    .replaceAll("{ext}", extPart)
    .replaceAll("{parent}", parent || "Library")
    .replaceAll("{nnn}", String(n).padStart(3, "0"))
    .replaceAll("{nn}", String(n).padStart(2, "0"))
    .replaceAll("{n}", String(n))
    .replaceAll("{total}", String(total));
}

function renderRenamePreview(entries, pattern) {
  const box = $("rename-preview");
  box.innerHTML = entries.map((entry, index) => {
    const next = applyRenamePattern(pattern, entry, index, entries.length);
    const same = next === entry.name;
    return `<div class="row"><span title="${esc(entry.name)}">${esc(entry.name)}</span><span class="arrow">→</span><span title="${esc(next)}"${same ? ' class="muted"' : ""}>${esc(next)}</span></div>`;
  }).join("");
}

async function renameEntries(entries) {
  const items = (entries || []).filter((entry) => entry && entry.path);
  if (!items.length) return;
  const dialog = $("rename-dialog");
  const input = $("rename-pattern");
  const hint = $("rename-hint");
  // Single item: start with the current name so it feels like a normal rename.
  // Tokens are still optional — type {name}/{ext}/{n} etc. when needed.
  input.value = items.length === 1 ? items[0].name : "{name}-{nn}{ext}";
  hint.hidden = items.length === 1;
  const refresh = () => renderRenamePreview(items, input.value);
  refresh();
  dialog.returnValue = "";
  dialog.showModal();
  input.focus();
  input.select();
  input.addEventListener("input", refresh);
  const ok = await new Promise((resolve) => {
    dialog.addEventListener("close", () => {
      input.removeEventListener("input", refresh);
      resolve(dialog.returnValue === "ok");
    }, { once: true });
  });
  if (!ok) return;
  const pattern = input.value.trim();
  if (!pattern) return;
  const plan = items.map((entry, index) => ({
    entry,
    name: applyRenamePattern(pattern, entry, index, items.length).trim(),
  })).filter((item) => item.name && item.name !== item.entry.name);
  if (!plan.length) {
    toast("Nothing to rename");
    return;
  }
  const names = new Set();
  for (const item of plan) {
    if (names.has(item.name.toLowerCase())) {
      throw new Error(`Pattern produces duplicate name “${item.name}”`);
    }
    names.add(item.name.toLowerCase());
  }
  for (const item of plan) {
    if (!item.entry.dir) {
      const fromExt = fileExtension(item.entry.name);
      const toExt = fileExtension(item.name);
      if (fromExt !== toExt) {
        const confirmed = await askConfirm(
          plan.length === 1
            ? `You are changing the file extension from ${extensionLabel(fromExt)} to ${extensionLabel(toExt)}.\n\n“${item.entry.name}” → “${item.name}”\n\nPrograms may no longer open this file correctly.`
            : `“${item.entry.name}” → “${item.name}” changes the extension. Continue with remaining renames?`,
          "Change extension",
        );
        if (!confirmed) return;
      }
    }
    await api("/api/rename", { method: "POST", json: { path: item.entry.path, name: item.name } });
  }
  toast(plan.length === 1 ? "Renamed" : `Renamed ${plan.length} items`);
  invalidateFolderSizes();
  await load(state.path);
}

async function openDuplicatesFinder() {
  const dialog = $("duplicates-dialog");
  $("duplicates-status").textContent = "Scanning this folder and subfolders…";
  $("duplicates-results").innerHTML = "";
  dialog.showModal();
  const id = `dupes-${++jobState.seq}`;
  const controller = new AbortController();
  upsertJob(id, {
    title: "Duplicate scan",
    detail: "Comparing sizes and hashes…",
    percent: null,
    cancelable: true,
    onCancel: () => controller.abort(),
  });
  try {
    const data = await api(`/api/duplicates?path=${encodeURIComponent(state.path)}`, {
      signal: controller.signal,
    });
    const groups = data.groups || [];
    removeJob(id);
    if (!groups.length) {
      $("duplicates-status").textContent = "No duplicates found under this folder.";
      return;
    }
    $("duplicates-status").textContent = `${groups.length} duplicate group${groups.length === 1 ? "" : "s"} (same size + SHA-256).`;
    $("duplicates-results").innerHTML = groups.map((group, groupIndex) => {
      const paths = group.paths || [];
      return `<section class="dup-group" data-group="${groupIndex}">
        <div class="dup-group-meta">${formatSize(group.size || 0, false)} · ${esc((group.sha256 || "").slice(0, 12))}… · ${paths.length} copies</div>
        <ul>${paths.map((path) => `<li>
          <button type="button" data-dup-open="${esc(path)}">${esc(path)}</button>
          <button type="button" class="danger" data-dup-trash="${esc(path)}">Trash</button>
        </li>`).join("")}</ul>
      </section>`;
    }).join("");
  } catch (err) {
    removeJob(id);
    if (isCancelError(err)) {
      $("duplicates-status").textContent = "Scan cancelled.";
      toast("Duplicate scan cancelled");
      return;
    }
    $("duplicates-status").textContent = err.message || "Scan failed";
    throw err;
  }
}

$("duplicates-results").addEventListener("click", async (event) => {
  const openBtn = event.target.closest("[data-dup-open]");
  const trashBtn = event.target.closest("[data-dup-trash]");
  if (openBtn) {
    const path = openBtn.dataset.dupOpen;
    const parent = path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : "";
    $("duplicates-dialog").close();
    try {
      if (state.path !== parent) await go(parent);
      const entry = findEntry(path);
      if (entry) await openEntry(entry);
      else await openEntry({ path, name: path.split("/").pop(), dir: false, size: 0, modified: 0, kind: "file" });
    } catch (err) {
      toast(err.message, true);
    }
    return;
  }
  if (!trashBtn) return;
  const path = trashBtn.dataset.dupTrash;
  const name = path.split("/").pop();
  try {
    await deleteEntries([{ path, name, dir: false }]);
    const li = trashBtn.closest("li");
    if (li) li.remove();
    toast("Moved to Trash");
  } catch (err) {
    toast(err.message, true);
  }
});

function focusSearch() {
  const input = $("search");
  input.focus();
  input.select();
}

function resetSearch() {
  state.filter = "";
  $("search").value = "";
  clearDeepSearch();
}

function clearDeepSearch() {
  state.searchHits = [];
  const panel = $("search-panel");
  if (panel) panel.hidden = true;
  const box = $("search-results");
  if (box) { box.innerHTML = ""; observeModelThumbnails(box); }
  if (state.searchController) {
    state.searchController.abort();
    state.searchController = null;
  }
}

function syncSearchHint() {
  const hint = $("search-hint");
  if (!hint) return;
  const ocr = !!(state.me && state.me.ocr);
  hint.innerHTML = ocr
    ? `Searching this folder and below for partial file and folder names, tags, and text in documents (including PDFs and OCR’d images). Press Enter. Use <code>tag:family</code> for an exact tag.`
    : `Searching this folder and below for partial file and folder names, tags, and text in documents (including PDFs). Install <code>tesseract</code> for image OCR. Press Enter. Use <code>tag:family</code> for an exact tag.`;
}

let searchAbort = null;

function openTagSearch(tag) {
  state.filter = `tag:${tag}`;
  $("search").value = state.filter;
  renderFiles();
  focusSearch();
  runDeepSearch().catch((err) => toast(err.message, true));
}

async function runDeepSearch() {
  const q = ($("search").value || "").trim();
  state.filter = q;
  if (!q) {
    clearDeepSearch();
    toast("Type something to search", true);
    return;
  }
  if (state.searchController) state.searchController.abort();
  const controller = new AbortController();
  state.searchController = controller;
  const panel = $("search-panel");
  const box = $("search-results");
  panel.hidden = false;
  syncSearchHint();
  box.innerHTML = `<span class="search-message muted">Searching…</span>`;
  try {
    const data = await api(
      `/api/search?path=${encodeURIComponent(state.path)}&q=${encodeURIComponent(q)}`,
      { signal: controller.signal },
    );
    if (state.searchController !== controller) return;
    const hits = data.hits || [];
    state.searchHits = hits;
    box.innerHTML = hits.length
      ? hits.map((hit) => {
        const kind = hit.dir ? "folder" : (hit.kind || "file");
        const name = hit.name || hit.path.split("/").pop();
        const folder = parentPath(hit.path);
        const extras = [];
        if (hit.matchedTag) extras.push(`<span class="tag-chip mini">#${esc(hit.matchedTag)}</span>`);
        if (hit.matchedContent) extras.push(`<span class="search-hit-snippet" title="${esc(hit.matchedContent)}">${esc(hit.matchedContent)}</span>`);
        const model = kind === "model3d" ? `<span class="model-thumbnail" data-model-path="${esc(hit.path)}" data-model-name="${esc(name)}" data-model-modified="${hit.modified || 0}" data-model-size="${hit.size || 0}"><span class="glyph kind-model3d" aria-hidden="true">${typeIcon("model3d")}</span></span>` : "";
        const image = !hit.dir && (kind === "image" || kind === "video" || kind === "svg")
          ? `<img alt="" loading="lazy" src="${kind === "svg" ? "/api/raw" : "/api/thumb"}?path=${encodeURIComponent(hit.path)}">`
          : "";
        return `<button class="search-hit" type="button" data-hit="${esc(hit.path)}" data-dir="${hit.dir ? "1" : "0"}" title="${esc(hit.path)}">
          <span class="search-hit-thumb"><span class="glyph kind-${esc(kind)}" aria-hidden="true">${entryIcon({ ...hit, kind })}</span>${image}${model}</span>
          <span class="search-hit-details">
            <span class="search-hit-name">${esc(name)}</span>
            <span class="search-hit-path">${esc(folder || (state.me && state.me.rootName) || "Library")}</span>
            ${extras.join("")}
          </span>
        </button>`;
      }).join("")
      : `<span class="search-message muted">No matches under this folder</span>`;
    observeModelThumbnails(box);
    box.querySelectorAll("img").forEach((img) => {
      img.addEventListener("load", () => img.classList.add("loaded"));
      img.addEventListener("error", () => img.remove());
      if (img.complete && img.naturalWidth) img.classList.add("loaded");
    });
  } catch (err) {
    if (state.searchController !== controller) return;
    if (isCancelError(err)) {
      box.innerHTML = `<span class="search-message muted">Search cancelled</span>`;
      return;
    }
    box.innerHTML = `<span class="search-message muted">${esc(err.message || "Search failed")}</span>`;
    throw err;
  } finally {
    if (state.searchController === controller) state.searchController = null;
  }
}

$("search-results").addEventListener("click", (event) => {
  const button = event.target.closest("[data-hit]");
  if (!button) return;
  const hit = state.searchHits.find((hit) => hit.path === button.dataset.hit);
  if (!hit) return;
  resetSearch();
  renderFiles();
  if (hit.dir) go(hit.path).catch((err) => toast(err.message, true));
  else openEntry({ ...hit, size: hit.size || 0, modified: hit.modified || 0 }).catch((err) => toast(err.message, true));
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

function blobToBase64(blob) {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const result = String(reader.result || "");
      const comma = result.indexOf(",");
      resolve(comma >= 0 ? result.slice(comma + 1) : result);
    };
    reader.onerror = () => reject(new Error("Could not read image data"));
    reader.readAsDataURL(blob);
  });
}

function loadImageElement(url) {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => resolve(img);
    img.onerror = () => reject(new Error("Could not load image"));
    img.src = url;
  });
}

async function openImageEditor(entry) {
  if (!entry || entry.kind !== "image") return;
  const body = $("preview-body");
  const sourceUrl = `/api/raw?path=${encodeURIComponent(entry.path)}&t=${Date.now()}`;
  body.innerHTML = `<div class="image-editor"><p class="muted">Loading editor…</p></div>`;
  const img = await loadImageElement(sourceUrl);
  const editor = {
    rotation: 0,
    crop: null,
    dragging: false,
    start: null,
    source: img,
  };
  body.innerHTML = `
    <div class="image-editor" data-image-editor="1">
      <div class="image-editor-toolbar">
        <button type="button" data-img="rotate">Rotate 90°</button>
        <button type="button" data-img="reset-crop">Clear crop</button>
        <button type="button" data-img="cancel">Cancel</button>
        <button type="button" data-img="save">Save</button>
      </div>
      <p class="muted">Drag on the image to set a crop. Save overwrites the file.</p>
      <div class="image-editor-stage">
        <canvas></canvas>
        <div class="crop-rect" hidden></div>
      </div>
    </div>`;
  const canvas = body.querySelector("canvas");
  const stage = body.querySelector(".image-editor-stage");
  const cropEl = body.querySelector(".crop-rect");
  const ctx = canvas.getContext("2d");

  function draw() {
    const rad = (editor.rotation % 360) * Math.PI / 180;
    const swapped = editor.rotation % 180 !== 0;
    const w = swapped ? img.naturalHeight : img.naturalWidth;
    const h = swapped ? img.naturalWidth : img.naturalHeight;
    canvas.width = w;
    canvas.height = h;
    ctx.save();
    ctx.translate(w / 2, h / 2);
    ctx.rotate(rad);
    ctx.drawImage(img, -img.naturalWidth / 2, -img.naturalHeight / 2);
    ctx.restore();
    updateCropOverlay();
  }

  function canvasPoint(event) {
    const rect = canvas.getBoundingClientRect();
    const x = ((event.clientX - rect.left) / rect.width) * canvas.width;
    const y = ((event.clientY - rect.top) / rect.height) * canvas.height;
    return {
      x: Math.max(0, Math.min(canvas.width, x)),
      y: Math.max(0, Math.min(canvas.height, y)),
    };
  }

  function updateCropOverlay() {
    if (!editor.crop) {
      cropEl.hidden = true;
      return;
    }
    const rect = canvas.getBoundingClientRect();
    const stageRect = stage.getBoundingClientRect();
    const scaleX = rect.width / canvas.width;
    const scaleY = rect.height / canvas.height;
    const left = rect.left - stageRect.left + editor.crop.x * scaleX;
    const top = rect.top - stageRect.top + editor.crop.y * scaleY;
    cropEl.hidden = false;
    cropEl.style.left = `${left}px`;
    cropEl.style.top = `${top}px`;
    cropEl.style.width = `${Math.max(1, editor.crop.w) * scaleX}px`;
    cropEl.style.height = `${Math.max(1, editor.crop.h) * scaleY}px`;
  }

  draw();

  stage.addEventListener("pointerdown", (event) => {
    if (event.target !== canvas && event.target !== stage) return;
    editor.dragging = true;
    editor.start = canvasPoint(event);
    editor.crop = { x: editor.start.x, y: editor.start.y, w: 0, h: 0 };
    stage.setPointerCapture(event.pointerId);
    updateCropOverlay();
  });
  stage.addEventListener("pointermove", (event) => {
    if (!editor.dragging || !editor.start) return;
    const point = canvasPoint(event);
    const x = Math.min(editor.start.x, point.x);
    const y = Math.min(editor.start.y, point.y);
    const w = Math.abs(point.x - editor.start.x);
    const h = Math.abs(point.y - editor.start.y);
    editor.crop = { x, y, w, h };
    updateCropOverlay();
  });
  stage.addEventListener("pointerup", () => {
    editor.dragging = false;
    if (editor.crop && (editor.crop.w < 2 || editor.crop.h < 2)) editor.crop = null;
    updateCropOverlay();
  });
  window.addEventListener("resize", updateCropOverlay);

  body.querySelector("[data-img='rotate']").addEventListener("click", () => {
    editor.rotation = (editor.rotation + 90) % 360;
    editor.crop = null;
    draw();
  });
  body.querySelector("[data-img='reset-crop']").addEventListener("click", () => {
    editor.crop = null;
    updateCropOverlay();
  });
  body.querySelector("[data-img='cancel']").addEventListener("click", () => {
    window.removeEventListener("resize", updateCropOverlay);
    openEntry(entry).catch((err) => toast(err.message, true));
  });
  body.querySelector("[data-img='save']").addEventListener("click", async () => {
    try {
      const out = document.createElement("canvas");
      const crop = editor.crop && editor.crop.w >= 2 && editor.crop.h >= 2
        ? editor.crop
        : { x: 0, y: 0, w: canvas.width, h: canvas.height };
      out.width = Math.max(1, Math.round(crop.w));
      out.height = Math.max(1, Math.round(crop.h));
      out.getContext("2d").drawImage(
        canvas,
        crop.x, crop.y, crop.w, crop.h,
        0, 0, out.width, out.height,
      );
      const ext = fileExtension(entry.name);
      const mime = ext === "jpg" || ext === "jpeg" ? "image/jpeg"
        : ext === "webp" ? "image/webp"
        : "image/png";
      const quality = mime === "image/jpeg" ? 0.92 : undefined;
      const blob = await new Promise((resolve, reject) => {
        out.toBlob((value) => (value ? resolve(value) : reject(new Error("Could not encode image"))), mime, quality);
      });
      const data = await blobToBase64(blob);
      await api("/api/write-image", { method: "POST", json: { path: entry.path, data } });
      toast("Image saved");
      window.removeEventListener("resize", updateCropOverlay);
      invalidateFolderSizes();
      await load(state.path);
      const refreshed = findEntry(entry.path) || { ...entry, modified: Date.now() / 1000 };
      await openEntry(refreshed);
    } catch (err) {
      toast(err.message, true);
    }
  });
}

$("preview-prev").addEventListener("click", () => imageStep(-1));
$("preview-next").addEventListener("click", () => imageStep(1));
$("preview-edit-image").addEventListener("click", () => {
  if (state.current) openImageEditor(state.current).catch((err) => toast(err.message, true));
});
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

window.addEventListener("beforeunload", event => { if (state.editor?.dirty) { event.preventDefault(); event.returnValue = ""; } });
