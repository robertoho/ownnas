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

function utc(ms, options) {
  return new Intl.DateTimeFormat(undefined, { ...options, timeZone: "UTC" }).format(new Date(ms));
}

/// "2024-07-15" as "Mon, 15 Jul 2024".
function dayLabel(day) {
  return utc(Date.parse(`${day}T00:00:00Z`), { weekday: "short", day: "numeric", month: "short", year: "numeric" });
}

const SIZE_KEY = "ownnas-gallery-size";

function thumbUrl(item) {
  return `/api/thumb?path=${encodeURIComponent(item.path)}&v=${item.takenAt || 0}-${item.size || 0}`;
}

export function mountGallery(root, ctx) {
  let route = { tab: "photos" };
  let items = []; // the album's or tag's photos (the timeline loads month by month)
  let byPath = new Map(); // path -> item, for every tile shown
  let months = []; // the timeline's months, newest first
  let monthObserver = null;
  let selected = new Set();
  let anchor = ""; // the path a Shift-click selects from
  let hiddenCount = 0; // photos left out of the timeline
  let statusTimer = 0;
  let generation = 0;
  let current = null; // the album shown, if any
  let cardAlbums = new Map(); // id -> the albums shown as cards
  let menu = null;

  const tabs = el("nav", { class: "gallery-tabs", "aria-label": "Gallery sections" });
  for (const [tab, label] of [["photos", "Photos"], ["albums", "Albums"], ["tags", "Tags"]]) {
    tabs.append(el("a", { href: `#gallery${tab === "photos" ? "" : `/${tab}`}`, "data-tab": tab }, label));
  }
  const status = el("span", { class: "gallery-status", role: "status" });
  const actions = el("div", { class: "gallery-actions" });
  const size = el("input", { type: "range", class: "gallery-size", min: "80", max: "320", step: "10", "aria-label": "Photo size", title: "Photo size" });
  const sizeControl = el("label", { class: "gallery-size-control", title: "Photo size" },
    el("span", { class: "gallery-size-small", "aria-hidden": "true" }), size, el("span", { class: "gallery-size-large", "aria-hidden": "true" }));
  const head = el("div", { class: "gallery-head" }, tabs, status, actions, sizeControl);
  const selectionBar = el("div", { class: "gallery-selection", hidden: true });
  const body = el("div", { class: "gallery-body" });
  const scroller = el("div", { class: "gallery-scroller" }, body);
  const scrubberThumb = el("span", { class: "gallery-scrubber-thumb", "aria-hidden": "true" });
  const scrubberBubble = el("span", { class: "gallery-scrubber-bubble", hidden: true });
  const scrubber = el("div", { class: "gallery-scrubber", hidden: true, title: "Drag to jump to a date" });
  const viewport = el("div", { class: "gallery-viewport" }, scroller, scrubber);
  root.replaceChildren(head, selectionBar, viewport);
  size.value = String(storedSize());
  size.addEventListener("input", () => applySize(Number(size.value)));

  function stale(mine) {
    return mine !== generation || root.hidden;
  }

  // ---- Tiles and selection ----

  function tile(item) {
    byPath.set(item.path, item);
    const node = el("div", { class: "gallery-tile", "data-path": item.path, title: item.missing ? `${item.path} is missing` : nameOf(item.path) });
    if (item.missing) {
      node.classList.add("is-missing");
      node.append(el("span", { class: "gallery-missing" }, "Missing", el("small", {}, item.path)));
    } else {
      const img = el("img", { src: thumbUrl(item), alt: nameOf(item.path), loading: "lazy", decoding: "async", draggable: "false" });
      img.addEventListener("error", () => node.classList.add("no-thumb"), { once: true });
      node.append(img, el("span", { class: "gallery-fallback" }, nameOf(item.path)));
      if (item.kind === "video") node.append(el("span", { class: "gallery-badge", "aria-label": "Video" }, "▶"));
      if (item.hidden && route.tab !== "hidden") {
        node.classList.add("is-hidden");
        node.append(el("span", { class: "gallery-hidden-badge", title: "Hidden from Photos: it still shows here and in its folder" }, "Hidden"));
      }
    }
    node.append(el("button", { type: "button", class: "gallery-check", "aria-label": "Select", tabindex: "-1" }));
    if (selected.has(item.path)) node.classList.add("is-selected");
    return node;
  }

  function grid(list) {
    return el("div", { class: "gallery-grid" }, list.map(tile));
  }

  /// The photos shown, in order (in the timeline, only the months loaded so far).
  function ordered() {
    return [...body.querySelectorAll(".gallery-tile")].map((node) => byPath.get(node.dataset.path)).filter(Boolean);
  }

  body.addEventListener("click", (event) => {
    const node = event.target.closest(".gallery-tile");
    if (!node) return;
    const item = byPath.get(node.dataset.path);
    if (!item) return;
    const picking = event.target.closest(".gallery-check") || selected.size > 0 || event.metaKey || event.ctrlKey || event.shiftKey;
    if (!picking) {
      if (item.missing) return;
      ctx.openPreview(item, ordered().filter((entry) => !entry.missing));
      return;
    }
    const list = ordered();
    const from = list.findIndex((entry) => entry.path === anchor);
    const to = list.indexOf(item);
    if (event.shiftKey && from >= 0) {
      for (let i = Math.min(from, to); i <= Math.max(from, to); i++) selected.add(list[i].path);
    } else if (selected.has(item.path)) {
      selected.delete(item.path);
    } else {
      selected.add(item.path);
    }
    anchor = item.path;
    paintSelection();
  });

  function paintSelection() {
    for (const node of body.querySelectorAll(".gallery-tile")) node.classList.toggle("is-selected", selected.has(node.dataset.path));
    root.classList.toggle("is-selecting", selected.size > 0);
    renderSelectionBar();
  }

  function clearSelection() {
    selected = new Set();
    anchor = "";
    paintSelection();
  }

  function selectedItems() {
    return ordered().filter((item) => selected.has(item.path));
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
      write && picked.some((item) => !item.hidden) && route.tab !== "hidden" ? button("Hide from Photos", () => setHidden(picked.filter((item) => !item.hidden).map((item) => item.path), true)) : null,
      write && picked.some((item) => item.hidden) ? button("Show in Photos", () => setHidden(picked.filter((item) => item.hidden).map((item) => item.path), false)) : null,
      write && current && picked.length === 1 && !picked[0].missing ? button("Use as cover", () => setCover(picked[0].path)) : null,
      write && current ? button("Remove from album", () => removeFromAlbum(picked.map((item) => item.path)), { class: "danger" }) : null,
      button("Clear", clearSelection, { class: "gallery-clear" }),
    ].filter(Boolean));
  }

  // ---- Tile size ----

  function storedSize() {
    try {
      const value = Number(localStorage.getItem(SIZE_KEY));
      if (value >= 80 && value <= 320) return value;
    } catch {}
    return 150;
  }

  function applySize(value) {
    root.style.setProperty("--gallery-tile", `${value}px`);
    try {
      localStorage.setItem(SIZE_KEY, String(value));
    } catch {}
    sizePlaceholders();
    scheduleScrubber();
  }

  // ---- Photos: the whole timeline, laid out from the number of photos each day and
  // filled in a month at a time as it comes into view ----

  async function showPhotos() {
    const mine = ++generation;
    months = [];
    actions.replaceChildren(el("button", { type: "button", onclick: rescan, title: "Look through the library again for new or changed photos" }, "Rescan"));
    body.replaceChildren(el("p", { class: "muted gallery-message" }, "Loading photos…"));
    let data;
    try {
      data = await ctx.api("/api/gallery/days");
    } catch (err) {
      if (!stale(mine)) body.replaceChildren(el("p", { class: "muted gallery-message" }, err.message));
      return;
    }
    if (stale(mine)) return;
    showStatus(data.status);
    hiddenCount = data.hidden || 0;
    renderHiddenButton();
    if (!data.days.length) {
      body.replaceChildren(el("p", { class: "muted gallery-message" }, data.status.scanning
        ? "Looking for photos in the library…"
        : "No photos or videos yet. Photos anywhere in the library show up here, newest first."));
      scheduleScrubber();
      return;
    }
    const byMonth = new Map();
    for (const { day, count } of data.days) {
      const key = day.slice(0, 7);
      if (!byMonth.has(key)) byMonth.set(key, { key, days: [], count: 0 });
      const month = byMonth.get(key);
      month.days.push({ day, count });
      month.count += count;
    }
    months = [...byMonth.values()];
    const sections = [];
    for (const month of months) {
      const [year, number_] = month.key.split("-").map(Number);
      Object.assign(month, {
        year,
        from: Date.UTC(year, number_ - 1, 1) / 1000,
        to: Date.UTC(year, number_, 1) / 1000,
        label: utc(Date.UTC(year, number_ - 1, 1), { month: "long", year: "numeric" }),
        short: utc(Date.UTC(year, number_ - 1, 1), { month: "short" }),
      });
      month.section = el("section", { class: "gallery-month-section", "data-month": month.key }, el("h2", { class: "gallery-month" }, month.label));
      for (const day of month.days) {
        day.grid = el("div", { class: "gallery-grid is-placeholder", "data-count": String(day.count) });
        month.section.append(el("section", { class: "gallery-day", "data-day": day.day }, el("h3", {}, dayLabel(day.day)), day.grid));
      }
      sections.push(month.section);
    }
    body.replaceChildren(...sections);
    sizePlaceholders();
    monthObserver?.disconnect();
    monthObserver = new IntersectionObserver((entries) => {
      for (const entry of entries) {
        if (entry.isIntersecting) loadMonth(months.find((month) => month.section === entry.target), mine);
      }
    }, { root: scroller, rootMargin: "1500px 0px" });
    for (const month of months) monthObserver.observe(month.section);
    scheduleScrubber();
  }

  function renderHiddenButton() {
    actions.querySelector(".gallery-hidden-link")?.remove();
    if (!hiddenCount) return;
    actions.prepend(el("a", { class: "button gallery-hidden-link", href: "#gallery/hidden", title: "Photos left out of the timeline" }, `Hidden · ${number(hiddenCount)}`));
  }

  /// Hides photos from the timeline (or shows them again). The files are not touched.
  async function setHidden(paths, hidden) {
    if (!paths.length) return;
    try {
      const data = await ctx.api("/api/gallery/hidden", { method: "POST", json: { paths, hidden } });
      ctx.toast(hidden
        ? `Hid ${plural(data.changed, "photo")} from Photos. ${data.changed === 1 ? "It is" : "They are"} still in ${data.changed === 1 ? "its folder" : "their folders"} and albums; see Hidden.`
        : `${plural(data.changed, "photo")} back in Photos`);
      for (const path of paths) selected.delete(path);
      if (route.tab === "photos" && hidden) {
        removeFromTimeline(paths);
        hiddenCount += data.changed;
        renderHiddenButton();
        paintSelection();
      } else {
        show(location.hash);
      }
    } catch (err) {
      ctx.toast(err.message, true);
    }
  }

  /// Takes photos out of the timeline in place, without reloading it.
  function removeFromTimeline(paths) {
    for (const path of paths) {
      body.querySelector(`.gallery-tile[data-path="${CSS.escape(path)}"]`)?.remove();
      byPath.delete(path);
    }
    for (const day of body.querySelectorAll(".gallery-day")) {
      const grid_ = day.querySelector(".gallery-grid");
      if (grid_ && !grid_.classList.contains("is-placeholder") && !grid_.children.length) day.remove();
    }
    months = months.filter((month) => {
      if (month.section.querySelector(".gallery-day")) return true;
      month.section.remove();
      return false;
    });
    scheduleScrubber();
  }

  // ---- Hidden: photos left out of the timeline ----

  async function showHidden() {
    const mine = ++generation;
    actions.replaceChildren();
    body.replaceChildren(el("p", { class: "muted gallery-message" }, "Loading…"));
    try {
      const data = await ctx.api("/api/gallery/hidden");
      if (stale(mine)) return;
      items = data.items;
      status.textContent = plural(items.length, "hidden photo");
      body.replaceChildren(
        el("nav", { class: "gallery-crumbs" }, el("a", { href: "#gallery" }, "Photos"), el("span", { "aria-hidden": "true" }, "›"), el("strong", {}, "Hidden")),
        el("p", { class: "muted gallery-note" }, "These are left out of the Photos timeline. They are not moved or changed, and they still show in their folders, albums, and tags. Select some and choose Show in Photos to bring them back."),
        items.length ? grid(items) : el("p", { class: "muted gallery-message" }, "Nothing is hidden."),
      );
    } catch (err) {
      if (!stale(mine)) body.replaceChildren(el("p", { class: "muted gallery-message" }, err.message));
    }
  }

  async function loadMonth(month, mine) {
    if (!month || month.loaded || month.loading) return;
    month.loading = true;
    try {
      const data = await ctx.api(`/api/gallery/timeline?from=${month.from}&to=${month.to}&limit=50000`);
      if (stale(mine)) return;
      const byDay = new Map();
      for (const item of data.items) {
        const day = new Date(item.takenAt * 1000).toISOString().slice(0, 10);
        if (!byDay.has(day)) byDay.set(day, []);
        byDay.get(day).push(item);
      }
      for (const day of month.days) {
        day.grid.classList.remove("is-placeholder");
        day.grid.style.height = "";
        day.grid.replaceChildren(...(byDay.get(day.day) || []).map(tile));
        byDay.delete(day.day);
      }
      // Days the layout did not know about yet (photos indexed since it was made).
      for (const [day, list] of byDay) month.section.append(el("section", { class: "gallery-day" }, el("h3", {}, dayLabel(day)), grid(list)));
      month.loaded = true;
      monthObserver?.unobserve(month.section);
      scheduleScrubber();
    } catch (err) {
      if (!stale(mine)) ctx.toast(err.message, true);
    } finally {
      month.loading = false;
    }
  }

  /// Gives days not loaded yet the height their photos will take, so the page (and the
  /// scrubber) has its real length from the start.
  function sizePlaceholders() {
    const sample = body.querySelector(".gallery-grid");
    if (!sample) return;
    const style = getComputedStyle(sample);
    const tracks = style.gridTemplateColumns.split(" ").filter(Boolean);
    const columns = Math.max(1, tracks.length);
    const size = parseFloat(tracks[0]) || 150;
    const gap = parseFloat(style.rowGap) || 4;
    for (const placeholder of body.querySelectorAll(".gallery-grid.is-placeholder")) {
      const rows = Math.ceil(Number(placeholder.dataset.count) / columns);
      placeholder.style.height = `${rows * size + Math.max(0, rows - 1) * gap}px`;
    }
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
    } else if (info.thumbnailing && info.thumbsDone < info.thumbsTotal) {
      status.textContent = `${plural(info.total, "photo or video", "photos and videos")} · making thumbnails ${number(info.thumbsDone)} of ${number(info.thumbsTotal)}`;
      const mine = generation;
      statusTimer = setTimeout(async () => {
        try {
          const next = await ctx.api("/api/gallery/status");
          if (!stale(mine)) showStatus(next);
        } catch {}
      }, 3000);
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

  // ---- The scrubber: years and months down the side, to jump through the timeline ----

  let scrubberFrame = 0;
  function scheduleScrubber() {
    cancelAnimationFrame(scrubberFrame);
    scrubberFrame = requestAnimationFrame(buildScrubber);
  }

  function buildScrubber() {
    const show = route.tab === "photos" && months.length > 1 && scroller.scrollHeight > scroller.clientHeight * 1.5;
    scrubber.hidden = !show;
    if (!show) return;
    const scrollable = Math.max(1, scroller.scrollHeight - scroller.clientHeight);
    const height = scrubber.clientHeight;
    const marks = [];
    let lastYear = null;
    let lastLabel = -100;
    let lastDot = -100;
    for (const month of months) {
      month.fraction = Math.min(1, month.section.offsetTop / scrollable);
      const y = month.fraction * height;
      if (month.year !== lastYear && y - lastLabel >= 18) {
        marks.push(el("span", { class: "gallery-scrubber-year", style: `top:${y}px` }, String(month.year)));
        lastLabel = y;
        lastDot = y;
      } else if (y - lastDot >= 6 && y - lastLabel >= 12) {
        marks.push(el("span", { class: "gallery-scrubber-dot", style: `top:${y}px`, title: month.label }));
        lastDot = y;
      }
      lastYear = month.year;
    }
    scrubber.replaceChildren(...marks, scrubberThumb, scrubberBubble);
    moveThumb();
  }

  function moveThumb() {
    if (scrubber.hidden) return;
    const scrollable = Math.max(1, scroller.scrollHeight - scroller.clientHeight);
    scrubberThumb.style.top = `${(scroller.scrollTop / scrollable) * scrubber.clientHeight}px`;
  }

  function monthAt(fraction) {
    let found = months[0];
    for (const month of months) {
      if (month.fraction <= fraction + 1e-6) found = month;
      else break;
    }
    return found;
  }

  function scrubTo(event, jump) {
    const rect = scrubber.getBoundingClientRect();
    const fraction = Math.min(1, Math.max(0, (event.clientY - rect.top) / rect.height));
    const month = monthAt(fraction);
    if (!month) return;
    scrubberBubble.hidden = false;
    scrubberBubble.textContent = month.label;
    scrubberBubble.style.top = `${event.clientY - rect.top}px`;
    if (jump) scroller.scrollTop = month.section.offsetTop;
  }

  let scrubbing = false;
  scrubber.addEventListener("pointerdown", (event) => {
    event.preventDefault();
    scrubbing = true;
    scrubber.setPointerCapture(event.pointerId);
    scrubber.classList.add("is-active");
    scrubTo(event, true);
  });
  scrubber.addEventListener("pointermove", (event) => scrubTo(event, scrubbing));
  const stopScrub = () => {
    scrubbing = false;
    scrubber.classList.remove("is-active");
    scrubberBubble.hidden = true;
  };
  scrubber.addEventListener("pointerup", stopScrub);
  scrubber.addEventListener("pointercancel", stopScrub);
  scrubber.addEventListener("pointerleave", () => {
    if (!scrubbing) scrubberBubble.hidden = true;
  });
  scroller.addEventListener("scroll", moveThumb, { passive: true });
  new ResizeObserver(() => {
    sizePlaceholders();
    scheduleScrubber();
  }).observe(scroller);

  // ---- Albums ----

  async function showAlbums(id) {
    const mine = ++generation;
    body.replaceChildren(el("p", { class: "muted gallery-message" }, "Loading albums…"));
    try {
      const data = await ctx.api(`/api/albums${id ? `?id=${id}` : ""}`);
      if (stale(mine)) return;
      current = data.album;
      items = data.items;
      cardAlbums = new Map(data.albums.map((album) => [album.id, album]));
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
    return el("a", { class: "gallery-album", href: `#gallery/albums/${album.id}`, "data-album": String(album.id) },
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
      el("button", { type: "button", onclick: () => renameAlbum() }, "Rename"),
      el("button", { type: "button", onclick: () => moveAlbum() }, "Move…"),
      el("button", { type: "button", class: "danger", onclick: () => deleteAlbum() }, "Delete album"),
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

  async function renameAlbum(album = current) {
    const name = await ctx.askText("Rename album", "Name", album.name, "Rename");
    if (name === null || !name.trim() || name.trim() === album.name) return;
    try {
      await ctx.api("/api/albums", { method: "PATCH", json: { id: album.id, name: name.trim() } });
      showAlbums(route.id);
    } catch (err) {
      ctx.toast(err.message, true);
    }
  }

  async function moveAlbum(album = current) {
    const target = await chooseAlbum({ title: `Move “${album.name}” into…`, ok: "Move here", top: true, exclude: album.id });
    if (target === undefined) return;
    try {
      await ctx.api("/api/albums", { method: "PATCH", json: { id: album.id, move: true, parentId: target } });
      ctx.toast(target === null ? `Moved “${album.name}” to the top` : "Album moved");
      showAlbums(route.id);
    } catch (err) {
      ctx.toast(err.message, true);
    }
  }

  async function deleteAlbum(album = current) {
    const inside = album.albums ? ` and the ${plural(album.albums, "album")} inside it` : "";
    if (!(await ctx.askConfirm(`Delete the album “${album.name}”${inside}? The photos stay where they are; only the album goes.`, "Delete album"))) return;
    try {
      await ctx.api(`/api/albums?id=${album.id}`, { method: "DELETE" });
      ctx.toast(`Deleted the album “${album.name}”`);
      if (current && album.id === current.id) location.hash = album.parentId ? `#gallery/albums/${album.parentId}` : "#gallery/albums";
      else showAlbums(route.id);
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

  // ---- Context menus ----

  function closeMenu() {
    if (!menu) return;
    menu.remove();
    menu = null;
    document.removeEventListener("pointerdown", onOutside, true);
    document.removeEventListener("keydown", onMenuKey, true);
    window.removeEventListener("blur", closeMenu);
    scroller.removeEventListener("scroll", closeMenu);
  }

  function onOutside(event) {
    if (menu && !menu.contains(event.target)) closeMenu();
  }

  function onMenuKey(event) {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      closeMenu();
    }
  }

  /// A menu like the file browser's: `entries` are {label, icon, run, danger}; falsy ones are skipped.
  function openMenu(x, y, label, entries) {
    closeMenu();
    menu = el("div", { class: "menu gallery-menu", role: "menu" }, label ? el("div", { class: "menu-label" }, label) : null);
    for (const entry of entries.filter(Boolean)) {
      const button = el("button", { type: "button", role: "menuitem", "data-menu": entry.icon, class: entry.danger ? "danger" : null }, entry.label);
      button.addEventListener("click", () => {
        closeMenu();
        entry.run();
      });
      menu.append(button);
    }
    document.body.append(menu);
    ctx.decorateMenu(menu);
    const rect = menu.getBoundingClientRect();
    menu.style.left = `${Math.max(8, Math.min(x, window.innerWidth - rect.width - 8))}px`;
    menu.style.top = `${Math.max(8, Math.min(y, window.innerHeight - rect.height - 8))}px`;
    document.addEventListener("pointerdown", onOutside, true);
    document.addEventListener("keydown", onMenuKey, true);
    window.addEventListener("blur", closeMenu);
    scroller.addEventListener("scroll", closeMenu, { once: true });
    menu.querySelector("button")?.focus();
  }

  function photoMenu(event, item) {
    // A right-click on a selected photo acts on the whole selection; otherwise on that photo.
    const picked = selected.has(item.path) ? selectedItems() : [item];
    const present = picked.filter((entry) => !entry.missing);
    const paths = present.map((entry) => entry.path);
    const one = picked.length === 1 ? picked[0] : null;
    const write = ctx.canWrite();
    openMenu(event.clientX, event.clientY, one ? nameOf(one.path) : `${number(picked.length)} photos`, [
      one && !one.missing && { label: "Open", icon: "open", run: () => ctx.openPreview(one, ordered().filter((entry) => !entry.missing)) },
      write && paths.length && { label: "Add to album…", icon: "addToAlbum", run: () => pickAlbum(paths) },
      write && paths.length && { label: "Tag…", icon: "bulkTag", run: () => ctx.tag(paths) },
      paths.length && { label: "Download", icon: "download", run: () => ctx.download(paths) },
      one && !one.missing && { label: "Show in folder", icon: "folder", run: () => ctx.showInFolder(one.path) },
      write && route.tab !== "hidden" && picked.some((entry) => !entry.hidden) && {
        label: picked.length === 1 ? "Hide from Photos" : `Hide ${number(picked.length)} from Photos`,
        icon: "hide",
        run: () => setHidden(picked.filter((entry) => !entry.hidden).map((entry) => entry.path), true),
      },
      write && picked.some((entry) => entry.hidden) && {
        label: "Show in Photos",
        icon: "toggleHidden",
        run: () => setHidden(picked.filter((entry) => entry.hidden).map((entry) => entry.path), false),
      },
      !one || selected.has(one.path) ? { label: "Clear selection", icon: "close", run: clearSelection } : { label: "Select", icon: "check", run: () => { selected.add(one.path); paintSelection(); } },
      write && current && one && !one.missing && { label: "Use as album cover", icon: "cover", run: () => setCover(one.path) },
      write && current && { label: picked.length === 1 ? "Remove from album" : `Remove ${number(picked.length)} from album`, icon: "removeFromAlbum", danger: true, run: () => removeFromAlbum(picked.map((entry) => entry.path)) },
    ]);
  }

  function albumMenu(event, album) {
    const write = ctx.canWrite();
    openMenu(event.clientX, event.clientY, album.name, [
      { label: "Open", icon: "open", run: () => { location.hash = `#gallery/albums/${album.id}`; } },
      write && { label: "New album inside…", icon: "newAlbum", run: () => newAlbum(album.id) },
      write && { label: "Rename…", icon: "rename", run: () => renameAlbum(album) },
      write && { label: "Move…", icon: "move", run: () => moveAlbum(album) },
      write && { label: "Delete album", icon: "delete", danger: true, run: () => deleteAlbum(album) },
    ]);
  }

  function spaceMenu(event) {
    const write = ctx.canWrite();
    openMenu(event.clientX, event.clientY, current ? current.name : "Albums", [
      write && { label: current ? "New album inside…" : "New album…", icon: "newAlbum", run: () => newAlbum(current ? current.id : null) },
      ordered().length && { label: "Select all", icon: "check", run: () => { selected = new Set(ordered().map((item) => item.path)); paintSelection(); } },
      write && current && { label: "Rename…", icon: "rename", run: () => renameAlbum() },
      write && current && { label: "Move…", icon: "move", run: () => moveAlbum() },
      current && current.parentId !== null && { label: "Open the album above", icon: "open", run: () => { location.hash = `#gallery/albums/${current.parentId}`; } },
      write && current && { label: "Delete album", icon: "delete", danger: true, run: () => deleteAlbum() },
    ]);
  }

  /// A day in the timeline: everything taken that day at once.
  function dayMenu(event, section) {
    const write = ctx.canWrite();
    const list = [...section.querySelectorAll(".gallery-tile")].map((node) => byPath.get(node.dataset.path)).filter((item) => item && !item.missing);
    if (!list.length) return false;
    const paths = list.map((item) => item.path);
    const label = section.querySelector("h3")?.textContent || "This day";
    openMenu(event.clientX, event.clientY, `${label} · ${plural(list.length, "photo")}`, [
      { label: "Select this day", icon: "check", run: () => { for (const path of paths) selected.add(path); paintSelection(); } },
      write && { label: "Add this day to album…", icon: "addToAlbum", run: () => pickAlbum(paths) },
      write && { label: "Tag this day…", icon: "bulkTag", run: () => ctx.tag(paths) },
      { label: "Download this day", icon: "download", run: () => ctx.download(paths) },
      write && { label: "Hide this day from Photos", icon: "hide", run: () => setHidden(paths, true) },
    ]);
    return true;
  }

  function photosMenu(event) {
    const shown = ordered();
    openMenu(event.clientX, event.clientY, "Photos", [
      shown.length && { label: "Select all shown", icon: "check", run: () => { selected = new Set(shown.map((item) => item.path)); paintSelection(); } },
      selected.size && { label: "Clear selection", icon: "close", run: clearSelection },
      { label: "Rescan the library", icon: "restore", run: rescan },
      hiddenCount && { label: `Hidden photos (${number(hiddenCount)})`, icon: "hide", run: () => { location.hash = "#gallery/hidden"; } },
    ]);
  }

  root.addEventListener("contextmenu", (event) => {
    if (event.target.closest(".gallery-head, .gallery-selection, input, textarea")) return;
    const tileNode = event.target.closest(".gallery-tile");
    const card = event.target.closest(".gallery-album");
    if (tileNode) {
      const item = byPath.get(tileNode.dataset.path);
      if (!item) return;
      event.preventDefault();
      photoMenu(event, item);
    } else if (card) {
      const album = cardAlbums.get(Number(card.dataset.album));
      if (!album) return;
      event.preventDefault();
      albumMenu(event, album);
    } else if (route.tab === "albums") {
      event.preventDefault();
      spaceMenu(event);
    } else if (route.tab === "photos") {
      const day = event.target.closest(".gallery-day");
      event.preventDefault();
      if (!day || !dayMenu(event, day)) photosMenu(event);
    }
  });

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
    if (parts[0] === "hidden") return { tab: "hidden" };
    return { tab: "photos" };
  }

  function show(hash) {
    root.hidden = false;
    route = parse(hash);
    current = null;
    selected = new Set();
    anchor = "";
    byPath = new Map();
    months = [];
    closeMenu();
    root.classList.remove("is-selecting");
    selectionBar.hidden = true;
    monthObserver?.disconnect();
    clearTimeout(statusTimer);
    status.textContent = "";
    scrubber.hidden = true;
    applySize(Number(size.value));
    const tab = route.tab === "hidden" ? "photos" : route.tab;
    for (const link of tabs.children) link.setAttribute("aria-current", link.dataset.tab === tab ? "page" : "false");
    scroller.scrollTop = 0;
    if (route.tab === "albums") showAlbums(route.id);
    else if (route.tab === "tags") showTags(route.tag);
    else if (route.tab === "hidden") showHidden();
    else showPhotos();
  }

  function hide() {
    closeMenu();
    root.hidden = true;
    generation++;
    monthObserver?.disconnect();
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
    if ((event.metaKey || event.ctrlKey) && !event.shiftKey && event.key.toLowerCase() === "a" && ordered().length) {
      event.preventDefault();
      selected = new Set(ordered().map((item) => item.path));
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
