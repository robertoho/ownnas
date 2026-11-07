// Notes: quick notes and checklists as cards (Google Keep's way), kept as Markdown files in one
// library folder, so they open in Obsidian or any editor and sync like everything else. Colour
// and pinning live in each file's front matter, labels are tags and #hashtags, and [[links]]
// between notes show as backlinks. A note opens in the Markdown editor. Loaded on demand by
// app.js, which calls show(), hide() and refresh().

const COLOR_NAMES = { red: "Red", orange: "Orange", yellow: "Yellow", green: "Green", teal: "Teal", blue: "Blue", purple: "Purple", pink: "Pink", brown: "Brown", gray: "Grey" };
const MOST_ITEMS = 8;

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

/// Markdown as plain words for a card: no heading marks, emphasis or link syntax.
function plain(text) {
  return text
    .replace(/^#{1,6}\s+/gm, "")
    .replace(/\[\[([^\]|#]+)(?:#[^\]|]*)?(?:\|([^\]]+))?\]\]/g, (_, target, alias) => alias || target)
    .replace(/!\[[^\]]*\]\([^)]*\)/g, "")
    .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1")
    .replace(/(\*\*|__)(.+?)\1/g, "$2")
    .replace(/`([^`]+)`/g, "$1")
    .replace(/^>\s?/gm, "")
    .trim();
}

const stemOf = (path) => path.split("/").pop().replace(/\.(md|markdown)$/i, "");

export function mountNotes(host, ctx) {
  let data = null;
  let query = "";
  let label = "";
  let archived = false;
  let listMode = false;
  let color = "";
  let loading = null;

  const title = el("input", { type: "text", class: "notes-capture-title", placeholder: "Title", "aria-label": "Title", hidden: true });
  const body = el("textarea", { class: "notes-capture-body", rows: "1", placeholder: "Take a note…", "aria-label": "Take a note" });
  const listToggle = el("button", { type: "button", class: "notes-icon-btn", title: "Checklist: each line is an item", "aria-pressed": "false" }, "☑ List");
  const palette = el("div", { class: "notes-palette", role: "group", "aria-label": "Colour" });
  const add = el("button", { type: "button", class: "primary" }, "Add");
  const close = el("button", { type: "button" }, "Close");
  const captureTools = el("div", { class: "notes-capture-tools", hidden: true }, listToggle, palette, el("span", { class: "spacer" }), close, add);
  const capture = el("div", { class: "notes-capture" }, title, body, captureTools);

  const search = el("input", { type: "search", class: "notes-search", placeholder: "Search notes", "aria-label": "Search notes" });
  const today = el("button", { type: "button", title: "Open today's daily note" }, "Today");
  const archiveToggle = el("button", { type: "button", "aria-pressed": "false" }, "Archive");
  const folderButton = el("button", { type: "button", class: "notes-folder-btn", title: "Where the notes are kept" });
  const labels = el("div", { class: "notes-labels" });
  const grid = el("div", { class: "notes-body" });
  host.replaceChildren(
    el("div", { class: "notes-head" }, capture, el("div", { class: "notes-tools" }, search, today, archiveToggle, folderButton)),
    labels,
    grid,
  );

  const paintPalette = () => {
    palette.replaceChildren(...["", ...Object.keys(COLOR_NAMES)].map((name) => el("button", {
      type: "button",
      class: `notes-swatch${name ? ` note-color-${name}` : " notes-swatch-none"}${color === name ? " is-chosen" : ""}`,
      title: name ? COLOR_NAMES[name] : "No colour",
      "aria-label": name ? COLOR_NAMES[name] : "No colour",
      onclick: () => { color = name; paintPalette(); capture.className = `notes-capture${name ? ` note-color-${name}` : ""}`; },
    })));
  };

  const openCapture = () => {
    title.hidden = false;
    captureTools.hidden = false;
    capture.classList.add("is-open");
    paintPalette();
  };
  const resetCapture = () => {
    title.value = "";
    body.value = "";
    body.rows = 1;
    color = "";
    listMode = false;
    listToggle.setAttribute("aria-pressed", "false");
    body.placeholder = "Take a note…";
    capture.className = "notes-capture";
    title.hidden = true;
    captureTools.hidden = true;
  };
  body.addEventListener("focus", openCapture);
  body.addEventListener("input", () => { body.rows = Math.min(14, Math.max(2, body.value.split("\n").length)); });
  listToggle.addEventListener("click", () => {
    listMode = !listMode;
    listToggle.setAttribute("aria-pressed", String(listMode));
    body.placeholder = listMode ? "One item a line" : "Take a note…";
    body.focus();
  });
  close.addEventListener("click", () => {
    if (title.value.trim() || body.value.trim()) save().catch((err) => ctx.toast(err.message, true));
    else resetCapture();
  });
  add.addEventListener("click", () => save().catch((err) => ctx.toast(err.message, true)));
  for (const field of [title, body]) {
    field.addEventListener("keydown", (event) => {
      if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
        event.preventDefault();
        save().catch((err) => ctx.toast(err.message, true));
      }
      if (event.key === "Escape") {
        event.preventDefault();
        resetCapture();
        body.blur();
      }
    });
  }

  async function save() {
    const heading = title.value.trim();
    let text = body.value.replace(/\s+$/, "");
    if (listMode) text = text.split("\n").filter((line) => line.trim()).map((line) => (/^\s*[-*] \[[ xX]\]/.test(line) ? line : `- [ ] ${line.trim()}`)).join("\n");
    if (!heading && !text.trim()) return;
    const full = heading ? `# ${heading}\n\n${text}\n` : `${text}\n`;
    add.disabled = true;
    try {
      await ctx.api("/api/notes", { method: "POST", json: { text: full, color: color || undefined } });
      resetCapture();
      await load();
      body.focus();
    } finally {
      add.disabled = false;
    }
  }

  search.addEventListener("input", () => { query = search.value.trim().toLowerCase(); paint(); });
  archiveToggle.addEventListener("click", () => { location.hash = archived ? "#notes" : "#notes/archive"; });
  today.addEventListener("click", async () => {
    try {
      const { path } = await ctx.api("/api/notes/daily", { method: "POST" });
      await ctx.openNote(path, true);
    } catch (err) {
      ctx.toast(err.message, true);
    }
  });
  folderButton.addEventListener("click", async () => {
    if (!ctx.isAdmin()) {
      ctx.openFolder(data?.folder || "");
      return;
    }
    const chosen = await ctx.pickFolder({ title: "Keep the notes in", ok: "Use this folder" });
    if (chosen === null || chosen === undefined || chosen === "") return;
    try {
      await ctx.api("/api/notes/folder", { method: "POST", json: { folder: chosen } });
      await load();
    } catch (err) {
      ctx.toast(err.message, true);
    }
  });

  async function load() {
    const request = ctx.api("/api/notes");
    loading = request;
    const answer = await request;
    if (loading !== request) return;
    data = answer;
    paint();
  }

  /// Notes that link to each note, by the names they use ([[Title]] or [[file name]]).
  function backlinks() {
    const byName = new Map();
    for (const note of data.notes) {
      for (const name of [note.title, stemOf(note.path)]) {
        const key = name.toLowerCase();
        if (!byName.has(key)) byName.set(key, note.path);
      }
    }
    const linked = new Map();
    for (const note of data.notes) {
      for (const link of note.links) {
        const target = byName.get(link.toLowerCase());
        if (!target || target === note.path) continue;
        if (!linked.has(target)) linked.set(target, []);
        linked.get(target).push(note);
      }
    }
    return linked;
  }

  const labelsOf = (note) => [...new Set([...note.tags, ...note.hashtags].map((tag) => tag.toLowerCase()))];

  function matches(note) {
    if (note.archived !== archived) return false;
    if (label && !labelsOf(note).includes(label)) return false;
    if (!query) return true;
    const words = [note.title, note.excerpt, ...note.items.map((item) => item.text), ...labelsOf(note), note.path].join("\n").toLowerCase();
    return query.split(/\s+/).every((word) => words.includes(word));
  }

  function paint() {
    if (!data) return;
    archiveToggle.textContent = archived ? "Back to notes" : "Archive";
    archiveToggle.setAttribute("aria-pressed", String(archived));
    folderButton.textContent = `📁 ${data.folder}`;
    const allLabels = [...new Set(data.notes.filter((note) => note.archived === archived).flatMap(labelsOf))].sort();
    if (label && !allLabels.includes(label)) label = "";
    labels.replaceChildren(...(allLabels.length ? [
      el("button", { type: "button", class: `notes-label${label ? "" : " is-chosen"}`, onclick: () => { label = ""; paint(); } }, "All"),
      ...allLabels.map((name) => el("button", { type: "button", class: `notes-label${label === name ? " is-chosen" : ""}`, onclick: () => { label = label === name ? "" : name; paint(); } }, `#${name}`)),
    ] : []));
    const shown = data.notes.filter(matches);
    const linked = backlinks();
    if (!data.notes.length) {
      grid.replaceChildren(el("div", { class: "notes-empty" },
        el("p", {}, "No notes yet. Write one above."),
        el("p", { class: "muted" }, `Notes are Markdown files in “${data.folder}”, so they open in Obsidian or any editor, and sync and keep their history like your other files. Tick checklists on the cards, colour and pin them, label them with #hashtags, and link them with [[Note name]].`)));
      return;
    }
    if (!shown.length) {
      grid.replaceChildren(el("p", { class: "notes-empty muted" }, archived ? "Nothing archived here." : "No notes match."));
      return;
    }
    const pinned = archived ? [] : shown.filter((note) => note.pinned);
    const others = archived ? shown : shown.filter((note) => !note.pinned);
    const section = (heading, notes) => notes.length ? [
      heading ? el("h3", { class: "notes-section" }, heading) : null,
      el("div", { class: "notes-grid" }, notes.map((note) => card(note, linked.get(note.path) || []))),
    ] : [];
    grid.replaceChildren(...section(pinned.length ? "Pinned" : "", pinned), ...section(pinned.length ? "Others" : "", others));
  }

  async function change(url, json, done) {
    try {
      await ctx.api(url, { method: "POST", json });
      if (done) ctx.toast(done);
      await load();
    } catch (err) {
      ctx.toast(err.message, true);
      await load().catch(() => {});
    }
  }

  function colorMenu(note, anchor) {
    document.querySelector(".notes-color-menu")?.remove();
    const menu = el("div", { class: "notes-color-menu notes-palette", role: "menu" },
      ["", ...Object.keys(COLOR_NAMES)].map((name) => el("button", {
        type: "button",
        role: "menuitem",
        class: `notes-swatch${name ? ` note-color-${name}` : " notes-swatch-none"}${(note.color || "") === name ? " is-chosen" : ""}`,
        title: name ? COLOR_NAMES[name] : "No colour",
        "aria-label": name ? COLOR_NAMES[name] : "No colour",
        onclick: (event) => { event.stopPropagation(); menu.remove(); change("/api/notes/meta", { path: note.path, color: name }); },
      })));
    anchor.closest(".note-card").append(menu);
    const away = (event) => {
      if (!menu.contains(event.target)) {
        menu.remove();
        document.removeEventListener("pointerdown", away, true);
      }
    };
    document.addEventListener("pointerdown", away, true);
  }

  function card(note, linkedFrom) {
    const write = ctx.canWrite();
    const items = note.items.slice(0, MOST_ITEMS).map((item) => {
      const box = el("input", { type: "checkbox", checked: item.done, disabled: !write, "aria-label": item.text });
      box.addEventListener("click", (event) => event.stopPropagation());
      box.addEventListener("change", () => change("/api/notes/tick", { path: note.path, line: item.line, text: item.text, done: box.checked }));
      return el("li", { class: item.done ? "is-done" : "" }, el("label", { onclick: (event) => event.stopPropagation() }, box, el("span", {}, plain(item.text))));
    });
    const more = note.itemCount - items.length;
    const action = (label, symbol, run, extra = "") => el("button", {
      type: "button", class: `notes-icon-btn ${extra}`, title: label, "aria-label": label,
      onclick: (event) => { event.stopPropagation(); run(event); },
    }, symbol);
    const node = el("article", { class: `note-card${note.color ? ` note-color-${note.color}` : ""}${note.pinned ? " is-pinned" : ""}`, tabindex: "0", "data-path": note.path, title: note.path },
      write && !note.archived ? action(note.pinned ? "Unpin" : "Pin", "📌", () => change("/api/notes/meta", { path: note.path, pinned: !note.pinned }), "note-pin") : null,
      el("h4", { class: "note-title" }, note.title),
      note.excerpt ? el("p", { class: "note-excerpt" }, plain(note.excerpt)) : null,
      items.length ? el("ul", { class: "note-items" }, items) : null,
      more > 0 ? el("p", { class: "muted note-more" }, `+ ${more} more item${more === 1 ? "" : "s"}`) : null,
      note.itemCount ? el("p", { class: "muted note-more" }, `${note.doneCount} of ${note.itemCount} done`) : null,
      labelsOf(note).length ? el("div", { class: "note-labels" }, labelsOf(note).map((name) => el("span", { class: "tag-chip mini" }, `#${name}`))) : null,
      linkedFrom.length ? el("p", { class: "muted note-backlinks", title: linkedFrom.map((from) => from.title).join(", ") }, `↩ Linked from ${linkedFrom.map((from) => from.title).slice(0, 3).join(", ")}${linkedFrom.length > 3 ? ` and ${linkedFrom.length - 3} more` : ""}`) : null,
      write ? el("div", { class: "note-actions" },
        action("Colour", "🎨", (event) => colorMenu(note, event.currentTarget)),
        action("Labels", "🏷", () => ctx.tag([note.path]).then(() => load())),
        action(note.archived ? "Unarchive" : "Archive", note.archived ? "⤴" : "🗄", () => change("/api/notes/archive", { path: note.path, archived: !note.archived }, note.archived ? "Back in your notes" : "Archived")),
        action("Show in folder", "📁", () => ctx.showInFolder(note.path)),
        action("Move to the Trash", "🗑", async () => {
          try {
            await ctx.api(`/api/entry?path=${encodeURIComponent(note.path)}`, { method: "DELETE" });
            ctx.toast("Moved to the Trash");
            await load();
          } catch (err) {
            ctx.toast(err.message, true);
          }
        }, "danger")) : null);
    const open = () => ctx.openNote(note.path, false);
    node.addEventListener("click", open);
    node.addEventListener("keydown", (event) => {
      if (event.key === "Enter" && event.target === node) open();
    });
    return node;
  }

  window.addEventListener("focus", () => {
    if (!host.hidden) load().catch(() => {});
  });

  return {
    show(hash) {
      archived = hash.startsWith("#notes/archive");
      host.hidden = false;
      paint();
      load().catch((err) => ctx.toast(err.message, true));
    },
    hide() {
      host.hidden = true;
    },
    refresh() {
      if (!host.hidden) load().catch(() => {});
    },
  };
}
