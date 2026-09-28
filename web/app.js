const state = {
  me: null,
  path: "",
  entries: [],
  truncated: false,
  total: 0,
  view: localStorage.getItem("ownnas-view") || "grid",
  sort: localStorage.getItem("ownnas-sort") || "name",
  direction: localStorage.getItem("ownnas-dir") || "asc",
  hidden: localStorage.getItem("ownnas-hidden") === "1",
  filter: "",
  current: null,
  menuPath: "",
};

const $ = (id) => document.getElementById(id);
let loadGen = 0;
let toastTimer = 0;

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
  $("login-view").hidden = false;
  $("login-error").textContent = message || "";
  state.me = null;
}

function showApp() {
  $("boot").hidden = true;
  $("login-view").hidden = true;
  $("app-view").hidden = false;
  $("who").textContent = state.me.username;
  const write = !state.me.readonly;
  $("mkdir-btn").hidden = !write;
  $("upload-btn").hidden = !write;
  $("folder-btn").hidden = !write;
  $("preview-rename").hidden = !write;
  $("preview-delete").hidden = !write;
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

function glyph(kind) {
  return {
    folder: "DIR",
    image: "IMG",
    svg: "SVG",
    video: "VID",
    audio: "AUD",
    pdf: "PDF",
    text: "TXT",
    archive: "ZIP",
    file: "FILE",
  }[kind] || "FILE";
}

function sortedEntries() {
  const query = state.filter.trim().toLowerCase();
  const items = state.entries.filter((entry) => entry.name.toLowerCase().includes(query));
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
    html += `<span class="muted">/</span><button type="button" data-go="${esc(walked)}"${current}>${esc(part)}</button>`;
  });
  nav.innerHTML = html;
}

function renderFiles() {
  const host = $("files");
  host.className = `files ${state.view}`;
  const items = sortedEntries();
  $("empty").hidden = items.length !== 0;
  $("empty").textContent = state.entries.length === 0 ? "This folder is empty." : "Nothing matches that filter.";
  $("banner").hidden = !state.truncated;
  $("banner").textContent = state.truncated ? `Showing the first ${state.entries.length} of ${state.total} items.` : "";
  host.innerHTML = items.map((entry) => {
    const thumb = entry.thumb
      ? `<img alt="" loading="lazy" src="/api/thumb?path=${encodeURIComponent(entry.path)}&v=${entry.modified}">`
      : entry.kind === "svg"
        ? `<img alt="" loading="lazy" src="/api/raw?path=${encodeURIComponent(entry.path)}">`
        : `<span class="glyph">${glyph(entry.kind)}</span>`;
    return `<article class="card" data-path="${esc(entry.path)}" data-dir="${entry.dir ? "1" : "0"}">
      <div class="thumb">${thumb}</div>
      <div class="card-row">
        <div class="name" title="${esc(entry.name)}">${esc(entry.name)}</div>
        <button class="more" type="button" data-menu-for="${esc(entry.path)}" aria-label="Actions for ${esc(entry.name)}">···</button>
      </div>
      <div class="sub">${esc(formatSize(entry.size, entry.dir))}</div>
    </article>`;
  }).join("");
  host.querySelectorAll("img").forEach((img) => {
    img.addEventListener("error", () => {
      img.replaceWith(Object.assign(document.createElement("span"), { className: "glyph", textContent: "FILE" }));
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
  closePreview();
  renderCrumbs();
  renderFiles();
}

async function go(path) {
  const next = pathToHash(path);
  if (location.hash !== next) location.hash = next;
  else await load(path);
}

function findEntry(path) {
  return state.entries.find((entry) => entry.path === path);
}

function closeMenu() {
  $("menu").hidden = true;
  state.menuPath = "";
}

function openMenu(path, anchor) {
  const entry = findEntry(path);
  if (!entry) return;
  state.menuPath = path;
  const write = state.me && !state.me.readonly;
  const buttons = [`<button type="button" data-menu="open">Open</button>`];
  if (!entry.dir) buttons.push(`<button type="button" data-menu="download">Download</button>`);
  if (write) {
    buttons.push(`<button type="button" data-menu="rename">Rename</button>`);
    buttons.push(`<button type="button" data-menu="delete">Delete</button>`);
  }
  const menu = $("menu");
  menu.innerHTML = buttons.join("");
  menu.hidden = false;
  const rect = anchor.getBoundingClientRect();
  menu.style.top = `${Math.min(rect.bottom + 4, window.innerHeight - 160)}px`;
  menu.style.left = `${Math.min(rect.left, window.innerWidth - 180)}px`;
}

function closePreview() {
  $("preview").hidden = true;
  state.current = null;
  $("preview-body").innerHTML = "";
}

async function openEntry(entry) {
  if (!entry) return;
  if (entry.dir) {
    await go(entry.path);
    return;
  }
  state.current = entry;
  $("preview").hidden = false;
  $("preview-title").textContent = entry.name;
  $("preview-meta").textContent = `${formatSize(entry.size, false)} · ${formatDate(entry.modified)}`;
  $("preview-download").href = `/api/raw?download=1&path=${encodeURIComponent(entry.path)}`;
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
      if (state.current !== entry) return;
      renderMeta(body, entry, meta);
    } catch (err) {
      if (state.current === entry) body.innerHTML = `<p class="muted">${esc(err.message)}</p>`;
    }
  }
}

function renderMeta(body, entry, meta) {
  if (meta.text && !meta.text.binary) {
    if (entry.name.toLowerCase().endsWith(".csv") || entry.name.toLowerCase().endsWith(".tsv")) {
      body.innerHTML = renderTable(meta.text.content, entry.name.toLowerCase().endsWith(".tsv") ? "\t" : ",");
    } else if (entry.name.toLowerCase().endsWith(".json")) {
      let pretty = meta.text.content;
      try { pretty = JSON.stringify(JSON.parse(pretty), null, 2); } catch { /* keep source */ }
      body.innerHTML = `<pre class="text-preview">${esc(pretty)}</pre>`;
    } else {
      body.innerHTML = `<pre class="text-preview">${esc(meta.text.content)}</pre>`;
    }
    if (meta.text.truncated) body.insertAdjacentHTML("beforeend", `<p class="muted">Preview truncated.</p>`);
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

function renderTable(content, separator) {
  const lines = content.split(/\r?\n/).filter((line) => line.length).slice(0, 200);
  const rows = lines.map((line) => `<tr>${line.split(separator).map((cell) => `<td>${esc(cell)}</td>`).join("")}</tr>`);
  return `<div class="table-wrap"><table>${rows.join("")}</table></div>`;
}

function askText(title, label, value, okLabel) {
  $("text-title").textContent = title;
  $("text-label-copy").textContent = label;
  $("text-input").value = value || "";
  $("text-ok").textContent = okLabel || "Save";
  const dialog = $("text-dialog");
  dialog.showModal();
  $("text-input").focus();
  return new Promise((resolve) => {
    dialog.addEventListener("close", () => {
      resolve(dialog.returnValue === "ok" ? $("text-input").value : null);
    }, { once: true });
  });
}

function askConfirm(message) {
  $("confirm-copy").textContent = message;
  const dialog = $("confirm-dialog");
  dialog.showModal();
  return new Promise((resolve) => {
    dialog.addEventListener("close", () => resolve(dialog.returnValue === "ok"), { once: true });
  });
}

async function renameEntry(entry) {
  const name = await askText("Rename", "New name", entry.name, "Rename");
  if (!name || name === entry.name) return;
  await api("/api/rename", { method: "POST", json: { path: entry.path, name } });
  toast("Renamed");
  await load(state.path);
}

async function deleteEntry(entry) {
  const message = entry.dir
    ? `Delete folder “${entry.name}” and everything inside it?`
    : `Delete “${entry.name}”?`;
  if (!await askConfirm(message)) return;
  await api(`/api/entry?path=${encodeURIComponent(entry.path)}`, { method: "DELETE" });
  toast("Deleted");
  await load(state.path);
}

function uploadFiles(fileList, names) {
  const files = [...fileList];
  if (!files.length) return Promise.resolve();
  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    xhr.open("POST", `/api/upload?path=${encodeURIComponent(state.path)}`);
    xhr.setRequestHeader("X-OwnNAS", "1");
    xhr.upload.onprogress = (event) => {
      if (!event.lengthComputable) return;
      const bar = $("progress");
      bar.hidden = false;
      bar.style.width = `${Math.round((event.loaded / event.total) * 100)}%`;
    };
    xhr.onload = () => {
      $("progress").hidden = true;
      $("progress").style.width = "0";
      if (xhr.status >= 200 && xhr.status < 300) resolve();
      else {
        let message = "Upload failed";
        try { message = JSON.parse(xhr.responseText).error || message; } catch { /* keep */ }
        reject(new Error(message));
      }
    };
    xhr.onerror = () => {
      $("progress").hidden = true;
      reject(new Error("Upload failed"));
    };
    const body = new FormData();
    files.forEach((file, index) => {
      body.append("file", file, (names && names[index]) || file.webkitRelativePath || file.name);
    });
    xhr.send(body);
  });
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
  $("view-toggle").textContent = state.view === "grid" ? "List" : "Grid";
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

$("files").addEventListener("click", (event) => {
  const menuButton = event.target.closest("[data-menu-for]");
  if (menuButton) {
    event.stopPropagation();
    openMenu(menuButton.dataset.menuFor, menuButton);
    return;
  }
  const card = event.target.closest("[data-path]");
  if (!card) return;
  openEntry(findEntry(card.dataset.path)).catch((err) => toast(err.message, true));
});

$("menu").addEventListener("click", (event) => {
  const button = event.target.closest("[data-menu]");
  if (!button) return;
  const entry = findEntry(state.menuPath);
  closeMenu();
  if (!entry) return;
  const action = button.dataset.menu;
  if (action === "open") openEntry(entry).catch((err) => toast(err.message, true));
  if (action === "download") location.href = `/api/raw?download=1&path=${encodeURIComponent(entry.path)}`;
  if (action === "rename") renameEntry(entry).catch((err) => toast(err.message, true));
  if (action === "delete") deleteEntry(entry).catch((err) => toast(err.message, true));
});

document.addEventListener("click", (event) => {
  if (!event.target.closest("#menu") && !event.target.closest(".more")) closeMenu();
});

$("preview-close").addEventListener("click", closePreview);
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

$("view-toggle").addEventListener("click", () => {
  state.view = state.view === "grid" ? "list" : "grid";
  localStorage.setItem("ownnas-view", state.view);
  syncControls();
  renderFiles();
});

$("hidden-toggle").addEventListener("change", (event) => {
  state.hidden = event.target.checked;
  localStorage.setItem("ownnas-hidden", state.hidden ? "1" : "0");
  load(state.path).catch((err) => toast(err.message, true));
});

$("mkdir-btn").addEventListener("click", async () => {
  const name = await askText("New folder", "Folder name", "", "Create");
  if (!name) return;
  try {
    await api("/api/mkdir", { method: "POST", json: { path: state.path, name } });
    await load(state.path);
  } catch (err) {
    toast(err.message, true);
  }
});

$("upload-btn").addEventListener("click", () => $("upload-input").click());
$("folder-btn").addEventListener("click", () => $("folder-input").click());

async function onPicked(input) {
  try {
    await uploadFiles(input.files);
    toast("Upload finished");
    await load(state.path);
  } catch (err) {
    toast(err.message, true);
  }
  input.value = "";
}
$("upload-input").addEventListener("change", () => onPicked($("upload-input")));
$("folder-input").addEventListener("change", () => onPicked($("folder-input")));

window.addEventListener("hashchange", () => {
  if (!state.me) return;
  load(hashToPath()).catch((err) => toast(err.message, true));
});

window.addEventListener("keydown", (event) => {
  if (event.key === "Escape") {
    closeMenu();
    closePreview();
  }
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
    const items = await readDrop(event.dataTransfer);
    await uploadFiles(items.map((item) => item.file), items.map((item) => item.name));
    toast("Upload finished");
    await load(state.path);
  } catch (err) {
    toast(err.message, true);
  }
});

boot();
