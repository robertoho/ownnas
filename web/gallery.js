// The Gallery: every photo and video by the day it was taken, nested virtual albums, and tags.
// Albums only list library paths; photos stay where they are and are never changed.
// Loaded on demand by app.js.

const PAGE = 200;

function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (value === false || value === undefined || value === null) continue;
    if (key === "class") node.className = value;
    // Through the CSSOM: the page's security policy blocks style attributes.
    else if (key === "style") node.style.cssText = value;
    else if (key.startsWith("on")) node.addEventListener(key.slice(2), value);
    else node.setAttribute(key, value === true ? "" : value);
  }
  for (const child of children.flat(Infinity)) {
    if (child === null || child === undefined || child === false) continue;
    node.append(child instanceof Node ? child : String(child));
  }
  return node;
}

const number = (n) => Number(n).toLocaleString();
const nameOf = (path) => path.split("/").pop();
const plural = (n, one, many = `${one}s`) => `${number(n)} ${n === 1 ? one : many}`;

/// Camera dates have no time zone (stored as UTC); file dates are real instants.
function dateParts(item) {
  if (item.takenAt === null || item.takenAt === undefined) return null;
  const date = new Date(item.takenAt * 1000);
  const timeZone = item.dateSource === "exif" ? "UTC" : undefined;
  const pick = (options) => new Intl.DateTimeFormat(undefined, { ...options, timeZone }).format(date);
  const key = (options) => new Intl.DateTimeFormat("en-CA", { ...options, timeZone }).format(date);
  return {
    day: key({ year: "numeric", month: "2-digit", day: "2-digit" }),
    month: key({ year: "numeric", month: "2-digit" }),
    dayLabel: pick({ weekday: "short", day: "numeric", month: "short", year: "numeric" }),
    monthLabel: pick({ month: "long", year: "numeric" }),
    full: pick({ dateStyle: "medium", timeStyle: "short" }),
  };
}

function thumbUrl(item) {
  return `/api/thumb?path=${encodeURIComponent(item.path)}&v=${item.takenAt || 0}-${item.size || 0}`;
}

export function mountGallery(root, ctx) {
  let route = { tab: "photos" };
  let items = []; // what is shown, in order: for selection ranges and stepping in the preview
  let selected = new Set();
  let anchor = -1;
  let timeline = { more: false, loading: false, lastDay: "", lastMonth: "" };
  let observer = null;
  let statusTimer = 0;
  let generation = 0;
  let current = null; // the album shown, if any

  const tabs = el("nav", { class: "gallery-tabs", "aria-label": "Gallery sections" });
  for (const [tab, label] of [["photos", "Photos"], ["albums", "Albums"], ["tags", "Tags"]]) {
    tabs.append(el("a", { href: `#gallery${tab === "photos" ? "" : `/${tab}`}`, "data-tab": tab }, label));
  }
  const status = el("span", { class: "gallery-status", role: "status" });
  const actions = el("div", { class: "gallery-actions" });
  const head = el("div", { class: "gallery-head" }, tabs, status, actions);
  const selectionBar = el("div", { class: "gallery-selection", hidden: true });
  const body = el("div", { class: "gallery-body" });
  const sentinel = el("div", { class: "gallery-sentinel" });
  root.replaceChildren(head, selectionBar, body);

  function stale(mine) {
    return mine !== generation || root.hidden;
  }

  // ---- Tiles and selection ----

  function tile(item, index) {
    const node = el("div", { class: "gallery-tile", "data-index": String(index), title: item.missing ? `${item.path} is missing` : nameOf(item.path) });
    if (item.missing) {
      node.classList.add("is-missing");
      node.append(el("span", { class: "gallery-missing" }, "Missing", el("small", {}, item.path)));
    } else {
      const img = el("img", { src: thumbUrl(item), alt: nameOf(item.path), loading: "lazy", decoding: "async", draggable: "false" });
      img.addEventListener("error", () => node.classList.add("no-thumb"), { once: true });
      node.append(img, el("span", { class: "gallery-fallback" }, nameOf(item.path)));
      if (item.kind === "video") node.append(el("span", { class: "gallery-badge", "aria-label": "Video" }, "▶"));
    }
    node.append(el("button", { type: "button", class: "gallery-check", "aria-label": "Select", tabindex: "-1" }));
    if (selected.has(item.path)) node.classList.add("is-selected");
    return node;
  }

  function grid(list, offset = 0) {
    return el("div", { class: "gallery-grid" }, list.map((item, i) => tile(item, offset + i)));
  }

  body.addEventListener("click", (event) => {
    const node = event.target.closest(".gallery-tile");
    if (!node) return;
    const index = Number(node.dataset.index);
    const item = items[index];
    if (!item) return;
    const picking = event.target.closest(".gallery-check") || selected.size > 0 || event.metaKey || event.ctrlKey || event.shiftKey;
    if (!picking) {
      if (item.missing) return;
      ctx.openPreview(item, items.filter((entry) => !entry.missing));
      return;
    }
    if (event.shiftKey && anchor >= 0) {
      const [from, to] = anchor < index ? [anchor, index] : [index, anchor];
      for (let i = from; i <= to; i++) selected.add(items[i].path);
    } else if (selected.has(item.path)) {
      selected.delete(item.path);
    } else {
      selected.add(item.path);
    }
    anchor = index;
    paintSelection();
  });

  function paintSelection() {
    for (const node of body.querySelectorAll(".gallery-tile")) {
      const item = items[Number(node.dataset.index)];
      node.classList.toggle("is-selected", !!item && selected.has(item.path));
    }
    root.classList.toggle("is-selecting", selected.size > 0);
    renderSelectionBar();
  }

  function clearSelection() {
    selected = new Set();
    anchor = -1;
    paintSelection();
  }

  function selectedItems() {
    return items.filter((item) => selected.has(item.path));
  }

  function renderSelectionBar() {
    selectionBar.hidden = selected.size === 0;
    if (!selected.size) return;
    const picked = selectedItems();
    const write = ctx.canWrite();
    const present = picked.filter((item) => !item.missing).map((item) => item.path);
    const button = (label, onclick, extra = {}) => el("button", { type: "button", onclick, ...extra }, label);
    selectionBar.replaceChildren(...[
      el("strong", {}, `${number(selected.size)} selected`),
      write && present.length ? button("Add to album…", () => pickAlbum(present)) : null,
      write && present.length ? button("Tag…", () => ctx.tag(present)) : null,
      present.length ? button("Download", () => ctx.download(present)) : null,
      present.length === 1 ? button("Show in folder", () => ctx.showInFolder(present[0])) : null,
      write && current && picked.length === 1 && !picked[0].missing ? button("Use as cover", () => setCover(picked[0].path)) : null,
      write && current ? button("Remove from album", () => removeFromAlbum(picked.map((item) => item.path)), { class: "danger" }) : null,
      button("Clear", clearSelection, { class: "gallery-clear" }),
    ].filter(Boolean));
  }

  // ---- Photos: the timeline ----

  async function showPhotos() {
    const mine = ++generation;
    items = [];
    timeline = { more: false, loading: false, lastDay: "", lastMonth: "" };
    actions.replaceChildren(el("button", { type: "button", onclick: rescan, title: "Look through the library again for new or changed photos" }, "Rescan"));
    body.replaceChildren(el("p", { class: "muted gallery-message" }, "Loading photos…"));
    await loadPage(mine, true);
  }

  async function loadPage(mine, first = false) {
    if (timeline.loading) return;
    timeline.loading = true;
    const last = items[items.length - 1];
    const params = new URLSearchParams({ limit: String(PAGE) });
    if (!first && last) {
      params.set("before", String(last.takenAt));
      params.set("beforePath", last.path);
    }
    try {
      const data = await ctx.api(`/api/gallery/timeline?${params}`);
      if (stale(mine)) return;
      if (first) body.replaceChildren();
      showStatus(data.status);
      appendDays(data.items);
      timeline.more = data.more;
      if (first && !data.items.length) {
        body.replaceChildren(el("p", { class: "muted gallery-message" }, data.status.scanning
          ? "Looking for photos in the library…"
          : "No photos or videos yet. Photos anywhere in the library show up here, newest first."));
      }
      sentinel.remove();
      if (timeline.more) {
        body.append(sentinel);
        observe();
      }
    } catch (err) {
      if (!stale(mine)) body.replaceChildren(el("p", { class: "muted gallery-message" }, err.message));
    } finally {
      timeline.loading = false;
    }
  }

  /// Adds photos under day headings (and a month heading when the month changes).
  function appendDays(list) {
    let section = body.lastElementChild && body.lastElementChild.classList.contains("gallery-day") ? body.lastElementChild : null;
    for (const item of list) {
      const index = items.push(item) - 1;
      const parts = dateParts(item) || { day: "?", month: "?", dayLabel: "Unknown date", monthLabel: "Unknown date" };
      if (parts.month !== timeline.lastMonth) {
        timeline.lastMonth = parts.month;
        body.append(el("h2", { class: "gallery-month" }, parts.monthLabel));
        section = null;
      }
      if (parts.day !== timeline.lastDay || !section) {
        timeline.lastDay = parts.day;
        section = el("section", { class: "gallery-day" }, el("h3", {}, parts.dayLabel), el("div", { class: "gallery-grid" }));
        body.append(section);
      }
      section.lastElementChild.append(tile(item, index));
    }
  }

  function observe() {
    observer?.disconnect();
    const mine = generation;
    observer = new IntersectionObserver((entries) => {
      if (entries.some((entry) => entry.isIntersecting) && timeline.more && !stale(mine)) loadPage(mine);
    }, { root, rootMargin: "800px" });
    observer.observe(sentinel);
  }

  function showStatus(info) {
    clearTimeout(statusTimer);
    if (!info) return;
    if (info.scanning) {
      status.textContent = `Looking for photos… ${number(info.seen)} found`;
      const mine = generation;
      statusTimer = setTimeout(async () => {
        try {
          const next = await ctx.api("/api/gallery/status");
          if (stale(mine)) return;
          // When a scan ends, show what it found.
          if (!next.scanning && route.tab === "photos") showPhotos();
          else showStatus(next);
        } catch {}
      }, 2000);
    } else {
      status.textContent = info.error ? `Could not finish looking for photos: ${info.error}` : plural(info.total, "photo or video", "photos and videos");
    }
  }

  async function rescan() {
    try {
      showStatus(await ctx.api("/api/gallery/rescan", { method: "POST" }));
      ctx.toast("Looking through the library for photos");
    } catch (err) {
      ctx.toast(err.message, true);
    }
  }

  // ---- Albums ----

  async function showAlbums(id) {
    const mine = ++generation;
    body.replaceChildren(el("p", { class: "muted gallery-message" }, "Loading albums…"));
    try {
      const data = await ctx.api(`/api/albums${id ? `?id=${id}` : ""}`);
      if (stale(mine)) return;
      current = data.album;
      items = data.items;
      status.textContent = current ? `${plural(current.count, "photo")}${current.albums ? ` · ${plural(current.albums, "album")}` : ""}` : plural(data.albums.length, "album");
      renderAlbumActions();
      const crumbs = el("nav", { class: "gallery-crumbs", "aria-label": "Album path" },
        el("a", { href: "#gallery/albums" }, "Albums"),
        data.breadcrumb.map((crumb, i) => [
          el("span", { "aria-hidden": "true" }, "›"),
          i === data.breadcrumb.length - 1 ? el("strong", {}, crumb.name) : el("a", { href: `#gallery/albums/${crumb.id}` }, crumb.name),
        ]));
      const cards = data.albums.length ? el("div", { class: "gallery-albums" }, data.albums.map(albumCard)) : null;
      let content;
      if (current) {
        content = items.length ? grid(items) : el("p", { class: "muted gallery-message" },
          ctx.canWrite() ? "No photos in this album yet. Select photos in Photos, or right-click them in the files, and choose Add to album." : "No photos in this album yet.");
      } else if (!cards) {
        content = el("p", { class: "muted gallery-message" },
          ctx.canWrite() ? "No albums yet. Albums gather photos from anywhere in the library without moving them; albums can hold other albums, like folders." : "No albums yet.");
      }
      body.replaceChildren(...[crumbs, cards, current && cards && items.length ? el("h3", { class: "gallery-subhead" }, "Photos") : null, content].filter(Boolean));
    } catch (err) {
      if (!stale(mine)) body.replaceChildren(el("p", { class: "muted gallery-message" }, err.message));
    }
  }

  function albumCard(album) {
    const cover = album.cover
      ? el("img", { src: thumbUrl({ path: album.cover }), alt: "", loading: "lazy" })
      : el("span", { class: "gallery-album-empty", "aria-hidden": "true" }, "▢");
    const detail = [album.count ? plural(album.count, "photo") : "", album.albums ? plural(album.albums, "album") : ""].filter(Boolean).join(" · ") || "Empty";
    return el("a", { class: "gallery-album", href: `#gallery/albums/${album.id}` },
      el("span", { class: "gallery-album-cover" }, cover),
      el("strong", {}, album.name),
      el("span", { class: "muted" }, detail));
  }

  function renderAlbumActions() {
    actions.replaceChildren();
    if (!ctx.canWrite()) return;
    actions.append(el("button", { type: "button", onclick: () => newAlbum(current ? current.id : null) }, current ? "New album inside" : "New album"));
    if (!current) return;
    actions.append(
      el("button", { type: "button", onclick: renameAlbum }, "Rename"),
      el("button", { type: "button", onclick: moveAlbum }, "Move…"),
      el("button", { type: "button", class: "danger", onclick: deleteAlbum }, "Delete album"),
    );
  }

  async function newAlbum(parentId, { open = true } = {}) {
    const name = await ctx.askText(parentId ? "New album inside this one" : "New album", "Name", "", "Create");
    if (name === null || !name.trim()) return null;
    try {
      const data = await ctx.api("/api/albums", { method: "POST", json: { name: name.trim(), parentId } });
      if (open) location.hash = `#gallery/albums/${data.album.id}`;
      return data.album;
    } catch (err) {
      ctx.toast(err.message, true);
      return null;
    }
  }

  async function renameAlbum() {
    const name = await ctx.askText("Rename album", "Name", current.name, "Rename");
    if (name === null || !name.trim() || name.trim() === current.name) return;
    try {
      await ctx.api("/api/albums", { method: "PATCH", json: { id: current.id, name: name.trim() } });
      showAlbums(current.id);
    } catch (err) {
      ctx.toast(err.message, true);
    }
  }

  async function moveAlbum() {
    const album = current;
    const target = await chooseAlbum({ title: `Move “${album.name}” into…`, ok: "Move here", top: true, exclude: album.id });
    if (target === undefined) return;
    try {
      await ctx.api("/api/albums", { method: "PATCH", json: { id: album.id, move: true, parentId: target } });
      ctx.toast(target === null ? `Moved “${album.name}” to the top` : "Album moved");
      showAlbums(album.id);
    } catch (err) {
      ctx.toast(err.message, true);
    }
  }

  async function deleteAlbum() {
    const album = current;
    const inside = album.albums ? ` and the ${plural(album.albums, "album")} inside it` : "";
    if (!(await ctx.askConfirm(`Delete the album “${album.name}”${inside}? The photos stay where they are; only the album goes.`, "Delete album"))) return;
    try {
      await ctx.api(`/api/albums?id=${album.id}`, { method: "DELETE" });
      ctx.toast(`Deleted the album “${album.name}”`);
      location.hash = album.parentId ? `#gallery/albums/${album.parentId}` : "#gallery/albums";
    } catch (err) {
      ctx.toast(err.message, true);
    }
  }

  async function removeFromAlbum(paths) {
    try {
      await ctx.api("/api/albums/items", { method: "POST", json: { albumId: current.id, paths, action: "remove" } });
      ctx.toast(`Removed ${plural(paths.length, "photo")} from “${current.name}”`);
      selected = new Set();
      showAlbums(current.id);
    } catch (err) {
      ctx.toast(err.message, true);
    }
  }

  async function setCover(path) {
    try {
      await ctx.api("/api/albums", { method: "PATCH", json: { id: current.id, setCover: true, cover: path } });
      ctx.toast("Cover changed");
      clearSelection();
    } catch (err) {
      ctx.toast(err.message, true);
    }
  }

  // ---- Choosing an album (to add photos to, or to move an album into) ----

  /// Resolves with the album id chosen, null for "top level" (when `top`), or undefined if cancelled.
  function chooseAlbum({ title, ok, top = false, exclude = null }) {
    return new Promise((resolve) => {
      let albums = [];
      let choice;
      const filter = el("input", { type: "search", placeholder: "Find an album", "aria-label": "Find an album" });
      const list = el("div", { class: "album-picker-list", role: "listbox", "aria-label": "Albums" });
      const confirm = el("button", { type: "button", class: "primary", disabled: true }, ok);
      const create = el("button", { type: "button" }, "New album");
      const dialog = el("dialog", { class: "album-picker" },
        el("h3", {}, title), filter, list,
        el("div", { class: "dialog-actions" }, create, el("span", { class: "spacer" }), el("button", { type: "button", onclick: () => dialog.close() }, "Cancel"), confirm));
      document.body.append(dialog);
      let result;
      dialog.addEventListener("close", () => {
        dialog.remove();
        resolve(result);
      });
      confirm.addEventListener("click", () => {
        result = choice;
        dialog.close();
      });

      const excluded = new Set();
      function rows() {
        const byParent = new Map();
        for (const album of albums) {
          const key = album.parentId ?? null;
          if (!byParent.has(key)) byParent.set(key, []);
          byParent.get(key).push(album);
        }
        excluded.clear();
        if (exclude !== null) {
          const stack = [exclude];
          while (stack.length) {
            const id = stack.pop();
            excluded.add(id);
            for (const child of byParent.get(id) || []) stack.push(child.id);
          }
        }
        const out = [];
        const walk = (parent, depth, path) => {
          for (const album of byParent.get(parent) || []) {
            const full = path ? `${path} / ${album.name}` : album.name;
            out.push({ album, depth, full });
            walk(album.id, depth + 1, full);
          }
        };
        walk(null, 0, "");
        return out;
      }

      function render() {
        const query = filter.value.trim().toLowerCase();
        const options = [];
        if (top && !query) options.push({ id: null, label: "Top level (not inside an album)", depth: 0 });
        for (const row of rows()) {
          if (excluded.has(row.album.id)) continue;
          if (query && !row.full.toLowerCase().includes(query)) continue;
          options.push({ id: row.album.id, label: query ? row.full : row.album.name, depth: query ? 0 : row.depth, count: row.album.count });
        }
        list.replaceChildren(...options.map((option) => {
          const button = el("button", {
            type: "button",
            role: "option",
            class: "album-picker-row",
            "aria-selected": option.id === choice ? "true" : "false",
            style: `--depth:${option.depth}`,
          }, el("span", {}, option.label), option.count ? el("span", { class: "muted" }, number(option.count)) : null);
          button.addEventListener("click", () => {
            choice = option.id;
            confirm.disabled = false;
            render();
          });
          button.addEventListener("dblclick", () => {
            result = option.id;
            dialog.close();
          });
          return button;
        }));
        if (!options.length) list.append(el("p", { class: "muted" }, albums.length ? "No album matches." : "No albums yet. Create one with New album."));
      }

      async function load() {
        try {
          albums = (await ctx.api("/api/albums/tree")).albums;
          render();
        } catch (err) {
          list.replaceChildren(el("p", { class: "muted" }, err.message));
        }
      }

      filter.addEventListener("input", render);
      create.addEventListener("click", async () => {
        const parent = choice === undefined ? null : choice;
        const album = await newAlbum(parent, { open: false });
        if (!album) return;
        choice = album.id;
        confirm.disabled = false;
        filter.value = "";
        await load();
      });
      dialog.showModal();
      load();
    });
  }

  async function pickAlbum(paths) {
    if (!paths.length) return;
    const id = await chooseAlbum({ title: `Add ${plural(paths.length, "photo")} to…`, ok: "Add" });
    if (id === undefined || id === null) return;
    try {
      const data = await ctx.api("/api/albums/items", { method: "POST", json: { albumId: id, paths } });
      const already = paths.length - data.changed;
      ctx.toast(`Added ${plural(data.changed, "photo")} to “${data.album.name}”${already ? ` (${number(already)} already there)` : ""}`);
      clearSelection();
      if (route.tab === "albums") showAlbums(route.id);
    } catch (err) {
      ctx.toast(err.message, true);
    }
  }

  // ---- Tags ----

  async function showTags(tag) {
    const mine = ++generation;
    body.replaceChildren(el("p", { class: "muted gallery-message" }, "Loading tags…"));
    actions.replaceChildren();
    try {
      if (!tag) {
        items = [];
        const data = await ctx.api("/api/tags/all");
        if (stale(mine)) return;
        status.textContent = plural(data.tags.length, "tag");
        body.replaceChildren(data.tags.length
          ? el("div", { class: "gallery-tags" }, data.tags.map((entry) =>
            el("a", { class: "gallery-tag", href: `#gallery/tags/${encodeURIComponent(entry.tag)}` }, entry.tag, el("span", {}, number(entry.count)))))
          : el("p", { class: "muted gallery-message" }, "No tags yet. Tag photos from the preview or with Tag… on a selection."));
        return;
      }
      const data = await ctx.api(`/api/gallery/tag?tag=${encodeURIComponent(tag)}`);
      if (stale(mine)) return;
      items = data.items;
      status.textContent = plural(items.length, "photo");
      body.replaceChildren(
        el("nav", { class: "gallery-crumbs" }, el("a", { href: "#gallery/tags" }, "Tags"), el("span", { "aria-hidden": "true" }, "›"), el("strong", {}, tag)),
        items.length ? grid(items) : el("p", { class: "muted gallery-message" }, "No photos or videos have this tag."),
      );
    } catch (err) {
      if (!stale(mine)) body.replaceChildren(el("p", { class: "muted gallery-message" }, err.message));
    }
  }

  // ---- Routing ----

  function parse(hash) {
    const parts = hash.replace(/^#?gallery\/?/, "").split("/").filter(Boolean).map(decodeURIComponent);
    if (parts[0] === "albums") return { tab: "albums", id: parts[1] ? Number(parts[1]) : null };
    if (parts[0] === "tags") return { tab: "tags", tag: parts[1] || "" };
    return { tab: "photos" };
  }

  function show(hash) {
    root.hidden = false;
    route = parse(hash);
    current = null;
    selected = new Set();
    anchor = -1;
    root.classList.remove("is-selecting");
    selectionBar.hidden = true;
    observer?.disconnect();
    clearTimeout(statusTimer);
    status.textContent = "";
    for (const link of tabs.children) link.setAttribute("aria-current", link.dataset.tab === route.tab ? "page" : "false");
    root.scrollTop = 0;
    if (route.tab === "albums") showAlbums(route.id);
    else if (route.tab === "tags") showTags(route.tag);
    else showPhotos();
  }

  function hide() {
    root.hidden = true;
    generation++;
    observer?.disconnect();
    clearTimeout(statusTimer);
  }

  /// Keys for the gallery (app.js passes them on while it shows): Esc clears the selection and
  /// ⌘A selects everything shown. Returns whether the key was used.
  function handleKey(event) {
    if (event.key === "Escape" && selected.size) {
      event.preventDefault();
      clearSelection();
      return true;
    }
    if ((event.metaKey || event.ctrlKey) && !event.shiftKey && event.key.toLowerCase() === "a" && items.length) {
      event.preventDefault();
      selected = new Set(items.map((item) => item.path));
      paintSelection();
      return true;
    }
    return false;
  }

  return {
    show,
    hide,
    pickAlbum,
    handleKey,
    /// The album the page shows, so changes elsewhere can refresh it.
    refresh() {
      if (!root.hidden) show(location.hash);
    },
  };
}
