// Remotes: the servers and clouds set up in rclone on the server (SFTP machines, OneDrive,
// Nextcloud, S3 buckets…), browsed like folders. Files open in a preview or download, streamed
// through the server's rclone. Administrators choose who else sees each remote, and where
// rclone and its config file are. Loaded on demand by app.js, which calls show() and hide().

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
      el("p", { class: "muted" }, "Administrators can use every remote. They see its files, and can open and download them."),
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
    const list = el("div", { class: "remotes-list", role: "list" });
    const paintList = () => {
      const shown = entries.filter((entry) => !filter || entry.name.toLowerCase().includes(filter.toLowerCase()));
      list.replaceChildren(...shown.map((entry) => {
        const row = el("div", { class: `remote-row${entry.dir ? " is-dir" : ""}`, role: "listitem", tabindex: "0" },
          el("span", { class: "remote-row-icon", "aria-hidden": "true" }, entry.dir ? "📁" : iconFor(entry)),
          el("span", { class: "remote-row-name", title: entry.name }, entry.name),
          el("span", { class: "remote-row-size muted" }, entry.dir ? "" : formatSize(entry.size)),
          el("span", { class: "remote-row-date muted" }, formatDate(entry.modified)),
          entry.dir ? el("span", {}) : el("a", { class: "remote-row-download", href: fileUrl(entry, true), title: "Download", "aria-label": `Download ${entry.name}`, onclick: (event) => event.stopPropagation() }, "⬇"));
        const open = () => (entry.dir ? (location.hash = hashFor(remote, fullPath(entry))) : preview(entry, shown.filter((other) => !other.dir)));
        row.addEventListener("click", open);
        row.addEventListener("keydown", (event) => { if (event.key === "Enter") open(); });
        return row;
      }));
      count.textContent = loading ? "" : `${shown.length === entries.length ? entries.length : `${shown.length} of ${entries.length}`} item${entries.length === 1 ? "" : "s"}`;
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

  function iconFor(entry) {
    const kind = previewKind(entry);
    return { image: "🖼", video: "🎞", audio: "🎵", pdf: "📕", text: "📄" }[kind] || "📄";
  }

  async function openFolder(name, folder) {
    const ticket = ++asked;
    if (name !== remote) filter = "";
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
  };
}
