// Remotes: the servers and clouds set up in rclone on the server (SFTP machines, OneDrive,
// Nextcloud, S3 buckets…), browsed like folders. Files open in a preview or download, streamed
// through the server's rclone. Administrators choose who else sees each remote, and where
// rclone and its config file are. Loaded on demand by app.js, which calls show() and hide().
//
// Files and folders have the library's context menu and clipboard (app.js's): copied or cut
// here, they paste into another remote's folder or the library's, and the library's paste here.
// The server's rclone does the copying, followed in the jobs panel.

const TEXT_EXTENSIONS = new Set(["txt", "md", "markdown", "log", "csv", "tsv", "json", "xml", "yml", "yaml", "toml", "ini", "conf", "cfg", "sh", "bash", "zsh", "py", "js", "mjs", "ts", "rs", "go", "java", "c", "h", "cpp", "hpp", "cs", "php", "rb", "pl", "sql", "html", "htm", "css", "env", "properties", "gitignore", "dockerfile", "service", "srt", "vtt"]);
const IMAGE_EXTENSIONS = new Set(["jpg", "jpeg", "png", "gif", "webp", "bmp", "svg", "avif", "ico"]);
const VIDEO_EXTENSIONS = new Set(["mp4", "m4v", "webm", "mov", "ogv"]);
const AUDIO_EXTENSIONS = new Set(["mp3", "m4a", "aac", "ogg", "oga", "opus", "wav", "flac"]);
/// The first part of a text file shown in its preview.
const TEXT_PREVIEW_BYTES = 256 * 1024;

const TYPE_LABELS = {
  sftp: "SFTP", ftp: "FTP", s3: "S3", b2: "Backblaze B2", onedrive: "OneDrive", drive: "Google Drive", dropbox: "Dropbox", webdav: "WebDAV",
  iclouddrive: "iCloud Drive", box: "Box", pcloud: "pCloud", mega: "MEGA", smb: "SMB", local: "Local", crypt: "Encrypted", alias: "Alias",
  union: "Union", combine: "Combine", azureblob: "Azure Blob", azurefiles: "Azure Files", swift: "Swift", http: "HTTP", protondrive: "Proton Drive",
  googlephotos: "Google Photos", sharefile: "ShareFile", seafile: "Seafile", yandex: "Yandex Disk", koofr: "Koofr", jottacloud: "Jottacloud",
};
const TYPE_ICONS = { sftp: "🖥", ftp: "🖥", smb: "🖥", local: "🖥", s3: "🪣", b2: "🪣", azureblob: "🪣", swift: "🪣", crypt: "🔒" };

function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (value === false || value === undefined || value === null) continue;
    if (key === "class") node.className = value;
    else if (key.startsWith("on")) node.addEventListener(key.slice(2), value);
    else node.setAttribute(key, value === true ? "" : value);
  }
  for (const child of children.flat(Infinity)) {
    if (child === null || child === undefined || child === false) continue;
    node.append(child instanceof Node ? child : String(child));
  }
  return node;
}

const esc = (text) => String(text).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);

const IS_MAC = /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent || "");

const extensionOf = (name) => (name.includes(".") ? name.split(".").pop().toLowerCase() : "");

function previewKind(entry) {
  const ext = extensionOf(entry.name);
  const mime = entry.mime || "";
  if (IMAGE_EXTENSIONS.has(ext)) return "image";
  if (VIDEO_EXTENSIONS.has(ext)) return "video";
  if (AUDIO_EXTENSIONS.has(ext)) return "audio";
  if (ext === "pdf") return "pdf";
  if (TEXT_EXTENSIONS.has(ext) || mime.startsWith("text/") || (!ext && entry.size < 512 * 1024)) return "text";
  return "";
}

function formatSize(bytes) {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1000 && unit < units.length - 1) { value /= 1000; unit += 1; }
  return `${value >= 10 || unit === 0 ? Math.round(value) : value.toFixed(1)} ${units[unit]}`;
}

function formatDate(seconds) {
  if (!seconds) return "";
  const when = new Date(seconds * 1000);
  const sameYear = when.getFullYear() === new Date().getFullYear();
  return when.toLocaleString([], { day: "numeric", month: "short", year: sameYear ? undefined : "numeric", hour: "2-digit", minute: "2-digit" });
}

/// "#remotes/odroid/home/ann" as ["odroid", "home/ann"].
function parseHash(hash) {
  const rest = hash.replace(/^#remotes\/?/, "");
  if (!rest) return { remote: "", path: "" };
  const parts = rest.split("/").map((part) => { try { return decodeURIComponent(part); } catch { return part; } });
  return { remote: parts[0], path: parts.slice(1).filter(Boolean).join("/") };
}

export function hashFor(remote, path = "") {
  return `#remotes/${[remote, ...path.split("/").filter(Boolean)].map(encodeURIComponent).join("/")}`;
}

export function mountRemotes(host, ctx) {
  let overview = null;
  let remote = "";
  let path = "";
  let entries = [];
  let filter = "";
  let asked = 0;
  /// The full paths chosen in the folder shown, and where a Shift+click range starts.
  let selected = new Set();
  let anchor = "";
  /// The entries shown (the filter applied), in order.
  let shown = [];
  const coarse = window.matchMedia("(pointer: coarse)");

  // rclone gives each entry's path from the remote's top.
  const fullPath = (entry) => entry.path || (path ? `${path}/${entry.name}` : entry.name);
  const fileUrl = (entry, download = false) => `/api/remotes/file?remote=${encodeURIComponent(remote)}&path=${encodeURIComponent(fullPath(entry))}${download ? "&download=1" : ""}`;

  async function loadOverview(force = false) {
    if (!overview || force) overview = await ctx.api("/api/remotes");
    return overview;
  }

  // ---- The remotes ----

  function settingsBox() {
    const data = overview;
    const program = el("input", { type: "text", spellcheck: "false", autocomplete: "off", value: data.settings?.program || "", placeholder: data.program || "/usr/bin/rclone" });
    const config = el("input", { type: "text", spellcheck: "false", autocomplete: "off", value: data.settings?.config || "", placeholder: data.config || "~/.config/rclone/rclone.conf" });
    const error = el("p", { class: "form-error", role: "alert" });
    const form = el("form", { class: "remotes-settings-form", hidden: true },
      el("label", {}, "rclone program", program, el("span", { class: "muted" }, "Leave empty to find it on the PATH.")),
      el("label", {}, "Config file", config, el("span", { class: "muted" }, "Leave empty for rclone's own, of the account OwnNAS runs as.")),
      el("div", { class: "dialog-actions" }, el("button", { type: "submit", class: "primary" }, "Save")),
      error);
    form.addEventListener("submit", async (event) => {
      event.preventDefault();
      error.textContent = "";
      try {
        await ctx.api("/api/remotes/settings", { method: "POST", json: { program: program.value, config: config.value } });
        ctx.toast("Saved");
        await loadOverview(true);
        paintHome();
      } catch (err) {
        error.textContent = err.message;
      }
    });
    const toggle = el("button", { type: "button", class: "link-button", onclick: () => { form.hidden = !form.hidden; } }, "Change");
    return el("div", { class: "remotes-settings" },
      el("p", { class: "muted" },
        data.available ? `${data.version || "rclone"} · ${data.program}` : "rclone is not installed on the server.",
        data.config ? el("span", {}, " · config ", el("code", {}, data.config)) : null,
        " ", toggle),
      form);
  }

  function accessDialog(item) {
    const users = overview.users || [];
    const dialog = el("dialog", { class: "remotes-access-dialog" });
    const boxes = users.map((user) => {
      const box = el("input", { type: "checkbox", value: String(user.id) });
      box.checked = (item.users || []).includes(user.id);
      return el("label", { class: "check" }, box, ` ${user.username}`);
    });
    const form = el("form", { method: "dialog" },
      el("h3", {}, `Who can use “${item.name}”`),
      el("p", { class: "muted" }, "Administrators can use every remote. They see its files, open and download them, and can copy, move, rename and delete them (unless the server is read-only)."),
      users.length ? el("div", { class: "remotes-access-users" }, boxes) : el("p", { class: "muted" }, "There are no other accounts yet. Add them in Settings > Users."),
      el("div", { class: "dialog-actions" },
        el("span", { class: "spacer" }),
        el("button", { type: "submit", value: "cancel" }, "Cancel"),
        users.length ? el("button", { type: "submit", value: "ok", class: "primary" }, "Save") : null));
    dialog.append(form);
    document.body.append(dialog);
    dialog.addEventListener("close", async () => {
      dialog.remove();
      if (dialog.returnValue !== "ok") return;
      const chosen = boxes.map((label) => label.querySelector("input")).filter((box) => box.checked).map((box) => Number(box.value));
      try {
        await ctx.api("/api/remotes/access", { method: "POST", json: { remote: item.name, users: chosen } });
        item.users = chosen;
        paintHome();
      } catch (err) {
        ctx.toast(err.message, true);
      }
    });
    dialog.showModal();
  }

  function paintHome() {
    const data = overview;
    const admin = ctx.isAdmin();
    const parts = [el("h2", { class: "remotes-title" }, "Remotes")];
    if (admin) parts.push(settingsBox());
    if (!data.available) {
      parts.push(el("p", { class: "remotes-empty" }, admin
        ? "Install rclone on the server (rclone.org/install), set up your servers and clouds with “rclone config”, and they show here."
        : "No remotes are shared with you."));
    } else if (data.error) {
      parts.push(el("p", { class: "remotes-empty form-error" }, data.error));
    } else if (!data.remotes.length) {
      parts.push(el("p", { class: "remotes-empty" }, admin ? "rclone has no remotes yet. Set them up on the server with “rclone config”." : "No remotes are shared with you."));
    } else {
      parts.push(el("div", { class: "remotes-grid" }, data.remotes.map((item) => {
        const shared = (item.users || []).length;
        return el("div", { class: "remote-card" },
          el("a", { class: "remote-open", href: hashFor(item.name) },
            el("span", { class: "remote-icon", "aria-hidden": "true" }, TYPE_ICONS[item.type] || "☁️"),
            el("strong", {}, item.name),
            el("span", { class: "muted" }, TYPE_LABELS[item.type] || item.type || "Remote")),
          admin ? el("button", { type: "button", class: "remote-access", title: "Choose who else can use it", onclick: () => accessDialog(item) },
            shared ? `Shared with ${shared}` : "Only administrators") : null);
      })));
    }
    host.replaceChildren(...parts);
  }

  // ---- A folder of a remote ----

  function crumbs() {
    const nav = el("nav", { class: "remotes-crumbs", "aria-label": "Folder on the remote" }, el("a", { href: "#remotes" }, "Remotes"), el("span", { class: "muted" }, "/"), el("a", { href: hashFor(remote) }, `${remote}:`));
    let walked = "";
    for (const part of path.split("/").filter(Boolean)) {
      walked = walked ? `${walked}/${part}` : part;
      nav.append(el("span", { class: "muted" }, "/"), el("a", { href: hashFor(remote, walked) }, part));
    }
    return nav;
  }

  function paintFolder(loading = false, error = "") {
    const search = el("input", { type: "search", class: "remotes-filter", placeholder: "Filter this folder", "aria-label": "Filter this folder", value: filter });
    const refresh = el("button", { type: "button", title: "Look again" }, "Refresh");
    refresh.addEventListener("click", () => openFolder(remote, path));
    const list = el("div", { class: "remotes-list", role: "listbox", "aria-multiselectable": "true" });
    const paintList = () => {
      shown = entries.filter((entry) => !filter || entry.name.toLowerCase().includes(filter.toLowerCase()));
      list.replaceChildren(...shown.map((entry) => {
        const row = el("div", { class: `remote-row${entry.dir ? " is-dir" : ""}`, role: "option", tabindex: "0", "data-path": fullPath(entry) },
          el("span", { class: "remote-row-icon", "aria-hidden": "true" }, entry.dir ? "📁" : iconFor(entry)),
          el("span", { class: "remote-row-name", title: entry.name }, entry.name),
          el("span", { class: "remote-row-size muted" }, entry.dir ? "" : formatSize(entry.size)),
          el("span", { class: "remote-row-date muted" }, formatDate(entry.modified)),
          entry.dir ? el("span", {}) : el("a", { class: "remote-row-download", href: fileUrl(entry, true), title: "Download", "aria-label": `Download ${entry.name}`, onclick: (event) => event.stopPropagation() }, "⬇"));
        row.addEventListener("click", (event) => {
          // On touch screens a tap opens, as before, unless items are being chosen.
          if (coarse.matches && !selected.size && !event.metaKey && !event.ctrlKey && !event.shiftKey) {
            openEntry(entry);
            return;
          }
          choose(entry, event);
        });
        row.addEventListener("dblclick", () => openEntry(entry));
        return row;
      }));
      count.textContent = loading ? "" : `${shown.length === entries.length ? entries.length : `${shown.length} of ${entries.length}`} item${entries.length === 1 ? "" : "s"}`;
      paintSelection();
    };
    const count = el("span", { class: "muted remotes-count" });
    search.addEventListener("input", () => { filter = search.value; paintList(); });
    const parts = [el("div", { class: "remotes-head" }, crumbs(), el("span", { class: "spacer" }), count, search, refresh)];
    if (loading) parts.push(el("p", { class: "remotes-empty" }, el("span", { class: "spinner", "aria-hidden": "true" }), " Asking the remote…"));
    else if (error) parts.push(el("p", { class: "remotes-empty form-error" }, error));
    else if (!entries.length) parts.push(el("p", { class: "remotes-empty muted" }, "This folder is empty."));
    else parts.push(list);
    host.replaceChildren(...parts);
    if (!loading && !error) paintList();
  }

  function openEntry(entry) {
    if (entry.dir) location.hash = hashFor(remote, fullPath(entry));
    else preview(entry, shown.filter((other) => !other.dir));
  }

  // ---- Choosing items, as in the library ----

  const selectedEntries = () => shown.filter((entry) => selected.has(fullPath(entry)));

  function choose(entry, event) {
    const key = fullPath(entry);
    if (event.shiftKey && anchor) {
      const keys = shown.map(fullPath);
      const [from, to] = [keys.indexOf(anchor), keys.indexOf(key)].sort((a, b) => a - b);
      if (from >= 0) {
        selected = new Set(keys.slice(from, to + 1));
        paintSelection();
        return;
      }
    }
    if (event.metaKey || event.ctrlKey) {
      if (selected.has(key)) selected.delete(key);
      else selected.add(key);
    } else {
      selected = new Set([key]);
    }
    anchor = key;
    paintSelection();
  }

  /// Marks the chosen rows, and the ones cut to the clipboard.
  function paintSelection() {
    const clip = ctx.clipboard();
    const cut = new Set(clip && clip.mode === "cut" ? clip.items.filter((item) => item.remote === remote).map((item) => item.path) : []);
    host.querySelectorAll(".remote-row[data-path]").forEach((row) => {
      const on = selected.has(row.dataset.path);
      row.classList.toggle("selected", on);
      row.setAttribute("aria-selected", on ? "true" : "false");
      row.classList.toggle("cut", cut.has(row.dataset.path));
    });
  }

  // ---- The context menu ----

  const menuButton = (action, label, danger = false) => `<button type="button" data-menu="${action}"${danger ? ' class="danger"' : ""}>${esc(label)}</button>`;

  /// The menu for the items chosen, or (none chosen) for the folder shown.
  function openMenu(x, y) {
    const write = ctx.canWrite();
    const items = selectedEntries();
    const clip = ctx.clipboard();
    const pasting = write && clip && clip.items.length;
    const one = items.length === 1 ? items[0] : null;
    const buttons = [];
    if (items.length) {
      buttons.push(`<div class="menu-label">${esc(one ? one.name : `${items.length} items`)}</div>`);
      if (one) buttons.push(menuButton("open", "Open"));
      if (items.every((entry) => !entry.dir)) buttons.push(menuButton("download", "Download"));
      buttons.push(menuButton("copy", "Copy"));
      if (write) buttons.push(menuButton("cut", "Cut"));
      if (pasting) buttons.push(menuButton("paste", one && one.dir ? "Paste into folder" : "Paste"));
      if (write && one) buttons.push(menuButton("rename", "Rename"));
      if (write) buttons.push(menuButton("delete", "Delete", true));
    } else {
      buttons.push(`<div class="menu-label">${esc(path ? path.split("/").pop() : `${remote}:`)}</div>`);
      if (write) buttons.push(menuButton("mkdir", "New folder"));
      if (pasting) buttons.push(menuButton("paste", "Paste"));
      if (shown.length) buttons.push(menuButton("selectAll", "Select all"));
      buttons.push(menuButton("refresh", "Refresh"));
    }
    const into = one && one.dir ? fullPath(one) : path;
    ctx.openMenu(x, y, buttons.join(""), (action) => run(action, items, into));
  }

  function run(action, items, into) {
    const fail = (err) => ctx.toast(err.message, true);
    const one = items.length === 1 ? items[0] : null;
    if (action === "open" && one) openEntry(one);
    if (action === "download") download(items);
    if (action === "copy" || action === "cut") toClipboard(action, items);
    if (action === "paste") paste(into);
    if (action === "rename" && one) rename(one).catch(fail);
    if (action === "delete") remove(items).catch(fail);
    if (action === "mkdir") makeFolder().catch(fail);
    if (action === "selectAll") selectAll();
    if (action === "refresh") openFolder(remote, path);
  }

  function selectAll() {
    selected = new Set(shown.map(fullPath));
    anchor = shown.length ? fullPath(shown[0]) : "";
    paintSelection();
  }

  function toClipboard(mode, items) {
    if (!items.length) return;
    ctx.setClipboard(mode, items.map((entry) => ({ remote, path: fullPath(entry), name: entry.name, dir: entry.dir })));
    paintSelection();
  }

  function paste(into) {
    const clip = ctx.clipboard();
    if (!clip || !clip.items.length || !ctx.canWrite()) return;
    ctx.paste({ remote, path: into }).catch((err) => ctx.toast(err.message, true));
  }

  /// Files download one after another (a remote's folder cannot be zipped on the way).
  function download(items) {
    const files = items.filter((entry) => !entry.dir);
    files.forEach((entry, index) => {
      setTimeout(() => {
        const link = el("a", { href: fileUrl(entry, true), download: entry.name, hidden: true });
        document.body.append(link);
        link.click();
        link.remove();
      }, index * 400);
    });
  }

  async function rename(entry) {
    const name = ((await ctx.askText("Rename", "New name", entry.name, "Rename", { watchExtensionFrom: entry.dir ? "" : entry.name })) || "").trim();
    if (!name || name === entry.name) return;
    const answer = await ctx.api("/api/remotes/rename", { method: "POST", json: { remote, path: fullPath(entry), name } });
    ctx.toast("Renamed");
    selected = new Set([answer.path]);
    await openFolder(remote, path, true);
  }

  async function remove(items) {
    if (!items.length) return;
    const what = items.length === 1 ? `“${items[0].name}”` : `${items.length} items`;
    const folders = items.some((entry) => entry.dir) ? " Folders go with everything in them." : "";
    if (!await ctx.askConfirm(`Delete ${what} from ${remote}?${folders} This cannot be undone here.`, "Delete")) return;
    try {
      await ctx.api("/api/remotes/delete", { method: "POST", json: { remote, paths: items.map(fullPath) } });
      ctx.toast(items.length === 1 ? "Deleted" : `Deleted ${items.length} items`);
    } finally {
      selected = new Set();
      await openFolder(remote, path, true);
    }
  }

  async function makeFolder() {
    const name = ((await ctx.askText("New folder", "Folder name", "New folder", "Create")) || "").trim();
    if (!name) return;
    const answer = await ctx.api("/api/remotes/mkdir", { method: "POST", json: { remote, path, name } });
    ctx.toast("Folder created");
    selected = new Set([answer.path]);
    await openFolder(remote, path, true);
  }

  host.addEventListener("contextmenu", (event) => {
    if (!remote || host.hidden || event.target.closest("input, textarea, a.remote-row-download")) return;
    const row = event.target.closest(".remote-row[data-path]");
    event.preventDefault();
    // Ctrl+click chooses more items (as in the library), it does not open the menu.
    if (event.ctrlKey) {
      if (!row) return;
      if (selected.has(row.dataset.path)) selected.delete(row.dataset.path);
      else selected.add(row.dataset.path);
      paintSelection();
      return;
    }
    if (row) {
      if (!selected.has(row.dataset.path)) {
        selected = new Set([row.dataset.path]);
        anchor = row.dataset.path;
      }
    } else {
      selected = new Set();
    }
    paintSelection();
    openMenu(event.clientX, event.clientY);
  });

  // A click on empty space lets go of what was chosen.
  host.addEventListener("click", (event) => {
    if (!remote || event.target.closest(".remote-row, button, a, input")) return;
    if (!selected.size) return;
    selected = new Set();
    paintSelection();
  });

  /// The keys of the library, for a remote's folder.
  function keydown(event) {
    if (!remote) return;
    const key = event.key.toLowerCase();
    const command = event.metaKey || event.ctrlKey;
    const write = ctx.canWrite();
    const items = selectedEntries();
    if (command && key === "a") {
      event.preventDefault();
      selectAll();
    } else if (command && (key === "c" || (key === "x" && write)) && items.length) {
      event.preventDefault();
      toClipboard(key === "c" ? "copy" : "cut", items);
    } else if (write && items.length && (event.key === "Delete" || (IS_MAC && event.metaKey && event.key === "Backspace"))) {
      event.preventDefault();
      remove(items).catch((err) => ctx.toast(err.message, true));
    } else if (write && items.length === 1 && event.key === "F2") {
      event.preventDefault();
      rename(items[0]).catch((err) => ctx.toast(err.message, true));
    } else if (event.key === "Enter" && items.length === 1 && !event.target.closest("button, a, input")) {
      event.preventDefault();
      openEntry(items[0]);
    } else if (event.key === "Escape" && selected.size) {
      selected = new Set();
      paintSelection();
    } else if ((event.key === "ArrowDown" || event.key === "ArrowUp") && shown.length && !command) {
      event.preventDefault();
      const keys = shown.map(fullPath);
      const at = keys.indexOf(anchor);
      const next = Math.max(0, Math.min(keys.length - 1, at < 0 ? 0 : at + (event.key === "ArrowDown" ? 1 : -1)));
      if (event.shiftKey && anchor) selected.add(keys[next]);
      else selected = new Set([keys[next]]);
      anchor = keys[next];
      paintSelection();
      host.querySelector(`.remote-row[data-path="${CSS.escape(keys[next])}"]`)?.scrollIntoView({ block: "nearest" });
    } else if (event.key === "Backspace" && !command && path) {
      event.preventDefault();
      location.hash = hashFor(remote, path.split("/").slice(0, -1).join("/"));
    }
  }

  function iconFor(entry) {
    const kind = previewKind(entry);
    return { image: "🖼", video: "🎞", audio: "🎵", pdf: "📕", text: "📄" }[kind] || "📄";
  }

  /// Shows a folder of a remote; `keep`: the same folder again, keeping what is chosen.
  async function openFolder(name, folder, keep = false) {
    const ticket = ++asked;
    if (name !== remote) filter = "";
    if (!keep && (name !== remote || folder !== path)) {
      selected = new Set();
      anchor = "";
    }
    remote = name;
    path = folder;
    entries = [];
    paintFolder(true);
    try {
      const data = await ctx.api(`/api/remotes/list?remote=${encodeURIComponent(remote)}&path=${encodeURIComponent(path)}`);
      if (ticket !== asked) return;
      entries = data.entries;
      paintFolder();
    } catch (err) {
      if (ticket !== asked) return;
      paintFolder(false, err.message);
    }
  }

  // ---- Previews ----

  function preview(entry, siblings) {
    const dialog = el("dialog", { class: "remote-preview" });
    const title = el("h3", { class: "remote-preview-title" });
    const body = el("div", { class: "remote-preview-body" });
    const download = el("a", { class: "button", download: "" }, "Download");
    const prev = el("button", { type: "button", "aria-label": "Previous file" }, "‹");
    const next = el("button", { type: "button", "aria-label": "Next file" }, "›");
    const close = el("button", { type: "button", "aria-label": "Close" }, "✕");
    let index = Math.max(0, siblings.indexOf(entry));
    const show = async (at) => {
      index = (at + siblings.length) % siblings.length;
      const item = siblings[index];
      title.textContent = item.name;
      download.href = fileUrl(item, true);
      prev.hidden = next.hidden = siblings.length < 2;
      const url = fileUrl(item);
      const kind = previewKind(item);
      if (kind === "image") body.replaceChildren(el("img", { src: url, alt: item.name }));
      else if (kind === "video") body.replaceChildren(el("video", { src: url, controls: true, autoplay: true, playsinline: true }));
      else if (kind === "audio") body.replaceChildren(el("audio", { src: url, controls: true, autoplay: true }));
      else if (kind === "pdf") body.replaceChildren(el("iframe", { src: url, title: item.name }));
      else if (kind === "text") {
        body.replaceChildren(el("p", { class: "muted" }, "Loading…"));
        try {
          const response = await fetch(url, { headers: { Range: `bytes=0-${TEXT_PREVIEW_BYTES - 1}` }, credentials: "same-origin" });
          if (!response.ok) throw new Error((await response.json().catch(() => ({}))).error || "Could not read the file");
          const text = await response.text();
          if (siblings[index] !== item) return;
          body.replaceChildren(el("pre", { class: "remote-preview-text" }, text), item.size > TEXT_PREVIEW_BYTES ? el("p", { class: "muted" }, `The first ${formatSize(TEXT_PREVIEW_BYTES)} of ${formatSize(item.size)}. Download it to see the rest.`) : null);
        } catch (err) {
          body.replaceChildren(el("p", { class: "form-error" }, err.message));
        }
      } else {
        body.replaceChildren(el("div", { class: "remote-preview-none" }, el("span", { "aria-hidden": "true" }, iconFor(item)), el("p", {}, `${formatSize(item.size)}${item.modified ? ` · ${formatDate(item.modified)}` : ""}`), el("p", { class: "muted" }, "There is no preview for this kind of file.")));
      }
    };
    prev.addEventListener("click", () => show(index - 1));
    next.addEventListener("click", () => show(index + 1));
    close.addEventListener("click", () => dialog.close());
    dialog.addEventListener("keydown", (event) => {
      if (event.target.closest("video, audio")) return;
      if (event.key === "ArrowLeft") show(index - 1);
      if (event.key === "ArrowRight") show(index + 1);
    });
    dialog.addEventListener("close", () => {
      body.querySelectorAll("video, audio").forEach((media) => { media.pause(); media.removeAttribute("src"); media.load(); });
      dialog.remove();
    });
    dialog.append(el("div", { class: "remote-preview-bar" }, title, el("span", { class: "spacer" }), prev, next, download, close), body);
    document.body.append(dialog);
    dialog.showModal();
    show(index);
  }

  return {
    async show(hash) {
      host.hidden = false;
      const { remote: name, path: folder } = parseHash(hash);
      if (!name) {
        remote = "";
        path = "";
        selected = new Set();
        host.replaceChildren(el("p", { class: "remotes-empty" }, el("span", { class: "spinner", "aria-hidden": "true" }), " Asking rclone…"));
        try {
          await loadOverview(true);
          paintHome();
        } catch (err) {
          host.replaceChildren(el("p", { class: "remotes-empty form-error" }, err.message));
        }
        return;
      }
      await openFolder(name, folder);
    },
    hide() {
      host.hidden = true;
      asked += 1;
    },
    keydown,
    /// Pastes the clipboard into the folder shown.
    pasteHere() {
      if (!remote) {
        ctx.toast("Open a folder of a remote to paste into", true);
        return;
      }
      paste(path);
    },
    /// The menu, from the touch screens' button; false on the list of remotes.
    openMenuAt(x, y) {
      if (!remote) return false;
      openMenu(x, y);
      return true;
    },
    /// The clipboard changed.
    repaint() {
      if (remote) paintSelection();
    },
    /// Something was copied or moved: shows the folder again.
    refresh() {
      if (remote && !host.hidden) openFolder(remote, path, true);
    },
  };
}
