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
/// Photos the server can make smaller (not GIFs, HEIC, or videos).
const resizable = (path) => /\.(jpe?g|png|webp|bmp|tiff?)$/i.test(path);
const nameOf = (path) => path.split("/").pop();
const plural = (n, one, many = `${one}s`) => `${number(n)} ${n === 1 ? one : many}`;

/// Camera dates have no time zone (stored as UTC); file dates are real instants.
function dateParts(item) {
  if (item.takenAt === null || item.takenAt === undefined) return null;
  const date = new Date(item.takenAt * 1000);
  // "exif": a camera's local time with no zone, kept as UTC. "utc" and "file": real instants.
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
const VIEW_KEY = "ownnas-gallery-view";
const GAP = 4; // between photos, as in the CSS

/// How photos are laid out: square crops, rows that keep each photo's shape, columns of
/// different heights, or a list with the details.
const VIEWS = [
  { id: "grid", label: "Squares", icon: '<rect x="1" y="1" width="6" height="6" rx="1"/><rect x="9" y="1" width="6" height="6" rx="1"/><rect x="1" y="9" width="6" height="6" rx="1"/><rect x="9" y="9" width="6" height="6" rx="1"/>' },
  { id: "justified", label: "Rows", icon: '<rect x="1" y="1" width="8" height="6" rx="1"/><rect x="11" y="1" width="4" height="6" rx="1"/><rect x="1" y="9" width="4" height="6" rx="1"/><rect x="7" y="9" width="8" height="6" rx="1"/>' },
  { id: "masonry", label: "Masonry", icon: '<rect x="1" y="1" width="6" height="8" rx="1"/><rect x="9" y="1" width="6" height="4" rx="1"/><rect x="9" y="7" width="6" height="8" rx="1"/><rect x="1" y="11" width="6" height="4" rx="1"/>' },
  { id: "list", label: "Details", icon: '<rect x="1" y="1.5" width="4" height="3.5" rx="0.8"/><rect x="1" y="6.25" width="4" height="3.5" rx="0.8"/><rect x="1" y="11" width="4" height="3.5" rx="0.8"/><path d="M7.5 3.25h7M7.5 8h7M7.5 12.75h7" stroke-linecap="round"/>' },
];

function storedView() {
  try {
    const value = localStorage.getItem(VIEW_KEY);
    if (VIEWS.some((view) => view.id === value)) return value;
  } catch {}
  return "grid";
}

/// Width over height, as the photo shows; until it is known, a guess (thumbnails correct it).
function aspectOf(item) {
  if (item.aspect) return item.aspect;
  if (item.width && item.height) return Math.min(4, Math.max(0.25, item.width / item.height));
  return item.kind === "video" ? 16 / 9 : 1;
}

function sizeText(bytes) {
  if (!Number.isFinite(bytes)) return "";
  const units = ["bytes", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000;
    unit++;
  }
  return `${unit ? value.toFixed(value < 10 ? 1 : 0) : value} ${units[unit]}`;
}

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
  let focusPath = ""; // the photo the arrow keys move from
  let hiddenCount = 0; // photos left out of the timeline
  let statusTimer = 0;
  let generation = 0;
  let current = null; // the album shown, if any
  let cardAlbums = new Map(); // id -> the albums shown as cards
  let menu = null;
  let placesMap = null; // the Places map, while it shows

  const tabs = el("nav", { class: "gallery-tabs", "aria-label": "Gallery sections" });
  for (const [tab, label] of [["photos", "Photos"], ["albums", "Albums"], ["people", "People"], ["places", "Places"], ["tags", "Tags"]]) {
    tabs.append(el("a", { href: `#gallery${tab === "photos" ? "" : `/${tab}`}`, "data-tab": tab, hidden: tab === "people" }, label));
  }
  const searchBox = el("input", { type: "search", class: "gallery-search", placeholder: "Search photos: “dog on the beach”…", "aria-label": "Search photos by what they show", hidden: true, title: "Search in words, or drop a picture here to find photos like it" });
  let smart = null; // what /api/ml says
  let sample = null; // a picture dropped in, to find photos like it
  let people = [];
  let currentPerson = null;
  const status = el("span", { class: "gallery-status" });
  const actions = el("div", { class: "gallery-actions" });
  const size = el("input", { type: "range", class: "gallery-size", min: "80", max: "320", step: "10", "aria-label": "Photo size", title: "Photo size" });
  const sizeControl = el("label", { class: "tile-size-control gallery-size-control", title: "Photo size" }, el("span", {}, "Size"), size);
  let view = storedView();
  const viewSwitch = el("div", { class: "view-switch gallery-views", role: "group", "aria-label": "Layout" });
  for (const option of VIEWS) {
    const button = el("button", { type: "button", "data-view": option.id, title: option.label, "aria-label": `${option.label} layout` });
    button.innerHTML = `<svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.5" aria-hidden="true">${option.icon}</svg>`;
    button.addEventListener("click", () => setView(option.id));
    viewSwitch.append(button);
  }
  const head = el("div", { class: "gallery-head" }, tabs, searchBox, actions);
  // At the bottom, as in the file browser: what is shown on the left, how it is shown on the right.
  const footer = el("footer", { class: "explorer-status-bar gallery-status-bar", "aria-label": "Gallery status and layout" },
    el("div", { class: "status-summary", role: "status", "aria-live": "polite" }, status),
    el("div", { class: "gallery-status-tools" }, viewSwitch, el("span", { class: "gallery-status-divider", "aria-hidden": "true" }), sizeControl));
  const selectionBar = el("div", { class: "gallery-selection", hidden: true });
  const body = el("div", { class: "gallery-body" });
  const scroller = el("div", { class: "gallery-scroller" }, body);
  const scrubberThumb = el("span", { class: "gallery-scrubber-thumb", "aria-hidden": "true" });
  const scrubberBubble = el("span", { class: "gallery-scrubber-bubble", hidden: true });
  const scrubber = el("div", { class: "gallery-scrubber", hidden: true, title: "Drag to jump to a date" });
  // The selection bar floats over the photos, so showing it never moves them under the pointer.
  const viewport = el("div", { class: "gallery-viewport" }, scroller, scrubber, selectionBar);
  root.replaceChildren(head, viewport, footer);
  size.value = String(storedSize());
  size.addEventListener("input", () => applySize(Number(size.value)));

  function stale(mine) {
    return mine !== generation || root.hidden;
  }

  // ---- Tiles and selection ----

  function tile(item) {
    byPath.set(item.path, item);
    const node = el("div", { class: "gallery-tile", "data-path": item.path, title: item.missing ? `${item.path} is missing` : nameOf(item.path), draggable: item.missing ? false : "true" });
    if (item.missing) {
      node.classList.add("is-missing");
      node.append(el("span", { class: "gallery-missing" }, "Missing", el("small", {}, item.path)));
      node.append(el("button", { type: "button", class: "gallery-check", "aria-label": "Select", tabindex: "-1" }));
    } else {
      const img = el("img", { src: thumbUrl(item), alt: nameOf(item.path), loading: "lazy", decoding: "async", draggable: "false" });
      img.addEventListener("error", () => node.classList.add("no-thumb"), { once: true });
      // The thumbnail shows the photo's real shape: when the index had it wrong (or not at
      // all, as for videos), lay its grid out again.
      img.addEventListener("load", () => {
        if (!img.naturalWidth || !img.naturalHeight) return;
        const real = img.naturalWidth / img.naturalHeight;
        if (Math.abs(real - aspectOf(item)) / real < 0.03) return;
        item.aspect = real;
        const owner = node.closest(".gallery-grid");
        if (owner && (view === "justified" || view === "masonry")) scheduleLayout(owner);
      }, { once: true });
      const thumb = el("div", { class: "gallery-thumb" }, img, el("span", { class: "gallery-fallback" }, nameOf(item.path)));
      if (item.kind === "video") thumb.append(el("span", { class: "gallery-badge", "aria-label": "Video" }, "▶"));
      if (item.hidden && route.tab !== "hidden") {
        node.classList.add("is-hidden");
        thumb.append(el("span", { class: "gallery-hidden-badge", title: "Hidden from Photos: it still shows here and in its folder" }, "Hidden"));
      }
      thumb.append(el("button", { type: "button", class: "gallery-check", "aria-label": "Select", tabindex: "-1" }));
      const when = dateParts(item);
      const folder = item.path.includes("/") ? item.path.slice(0, item.path.lastIndexOf("/")) : "";
      node.append(thumb, el("span", { class: "gallery-info" },
        el("span", { class: "gallery-info-name" }, nameOf(item.path)),
        el("span", { class: "gallery-info-date" }, when ? when.full : ""),
        el("span", { class: "gallery-info-dims" }, item.width && item.height ? `${item.width} × ${item.height}` : ""),
        el("span", { class: "gallery-info-size" }, sizeText(item.size)),
        el("span", { class: "gallery-info-folder", title: folder }, folder)));
    }
    if (selected.has(item.path)) node.classList.add("is-selected");
    if (focusPath === item.path) node.classList.add("is-focused");
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
    focusPath = item.path;
    paintSelection();
  });

  // Photos dragged out (into a terminal, which types their paths) carry their library paths, as
  // files dragged from the file list do. A selected photo brings the whole selection along.
  body.addEventListener("dragstart", (event) => {
    const node = event.target.closest(".gallery-tile");
    const item = node && byPath.get(node.dataset.path);
    if (!item || item.missing) {
      event.preventDefault();
      return;
    }
    const paths = selected.has(item.path) ? selectedItems().filter((entry) => !entry.missing).map((entry) => entry.path) : [item.path];
    event.dataTransfer.setData("application/x-ownnas-paths", JSON.stringify(paths));
    event.dataTransfer.setData("text/plain", paths.join("\n"));
    event.dataTransfer.effectAllowed = "copy";
  });

  function paintSelection() {
    for (const node of body.querySelectorAll(".gallery-tile")) {
      node.classList.toggle("is-selected", selected.has(node.dataset.path));
      node.classList.toggle("is-focused", node.dataset.path === focusPath);
    }
    root.classList.toggle("is-selecting", selected.size > 0);
    renderSelectionBar();
  }

  function clearSelection() {
    selected = new Set();
    anchor = "";
    focusPath = "";
    paintSelection();
  }

  // ---- Rubber-band selection, as in the file browser ----

  // Kept in the scroller's content coordinates, so the band grows as the page scrolls under it.
  let band = null;
  let bandFrame = 0;
  // The file browser's band; the two never draw at once.
  const bandBox = document.getElementById("marquee") || document.body.appendChild(el("div", { class: "marquee", hidden: true }));

  function contentPoint(x, y) {
    const rect = scroller.getBoundingClientRect();
    return { x: x - rect.left + scroller.scrollLeft, y: y - rect.top + scroller.scrollTop };
  }

  scroller.addEventListener("mousedown", (event) => {
    root.classList.remove("is-keyboard");
    if (event.button !== 0) return;
    if (event.target.closest(".gallery-tile, .gallery-album, .gallery-person, button, a, input, textarea, select, label")) return;
    // Not on the scrollbar.
    const rect = scroller.getBoundingClientRect();
    if (event.clientX >= rect.left + scroller.clientWidth || event.clientY >= rect.top + scroller.clientHeight) return;
    event.preventDefault();
    closeMenu();
    const additive = event.ctrlKey || event.metaKey;
    band = { start: contentPoint(event.clientX, event.clientY), x: event.clientX, y: event.clientY, additive, origin: additive ? new Set(selected) : new Set(), moved: false };
    window.addEventListener("mousemove", onBandMove);
    window.addEventListener("mouseup", endBand);
  });

  function onBandMove(event) {
    if (!band) return;
    band.x = event.clientX;
    band.y = event.clientY;
    if (!band.moved) {
      const now = contentPoint(band.x, band.y);
      if (Math.hypot(now.x - band.start.x, now.y - band.start.y) < 3) return;
      band.moved = true;
      root.classList.add("is-banding");
      bandFrame = requestAnimationFrame(bandTick);
    }
  }

  /// Each frame: scroll when the pointer is near the top or bottom edge, then draw and select.
  function bandTick() {
    if (!band) return;
    const rect = scroller.getBoundingClientRect();
    const edge = 40;
    if (band.y < rect.top + edge) scroller.scrollTop -= Math.ceil((rect.top + edge - band.y) / 3);
    else if (band.y > rect.bottom - edge) scroller.scrollTop += Math.ceil((band.y - (rect.bottom - edge)) / 3);
    drawBand(rect);
    bandFrame = requestAnimationFrame(bandTick);
  }

  function drawBand(rect) {
    const now = contentPoint(band.x, band.y);
    const area = {
      left: Math.min(band.start.x, now.x),
      right: Math.max(band.start.x, now.x),
      top: Math.min(band.start.y, now.y),
      bottom: Math.max(band.start.y, now.y),
    };
    // On screen, the band is cut to the visible part of the scroller.
    const offsetX = rect.left - scroller.scrollLeft;
    const offsetY = rect.top - scroller.scrollTop;
    const left = Math.max(area.left + offsetX, rect.left);
    const top = Math.max(area.top + offsetY, rect.top);
    const right = Math.min(area.right + offsetX, rect.left + scroller.clientWidth);
    const bottom = Math.min(area.bottom + offsetY, rect.top + scroller.clientHeight);
    bandBox.hidden = false;
    bandBox.style.left = `${left}px`;
    bandBox.style.top = `${top}px`;
    bandBox.style.width = `${Math.max(0, right - left)}px`;
    bandBox.style.height = `${Math.max(0, bottom - top)}px`;

    const next = new Set(band.origin);
    let last = "";
    for (const node of body.querySelectorAll(".gallery-tile")) {
      const box = node.getBoundingClientRect();
      const hit = !(box.right - offsetX < area.left || box.left - offsetX > area.right || box.bottom - offsetY < area.top || box.top - offsetY > area.bottom);
      if (!hit) continue;
      next.add(node.dataset.path);
      last = node.dataset.path;
    }
    const changed = next.size !== selected.size || [...next].some((path) => !selected.has(path));
    selected = next;
    if (last) {
      anchor = last;
      focusPath = last;
    }
    if (changed) paintSelection();
  }

  function endBand() {
    window.removeEventListener("mousemove", onBandMove);
    window.removeEventListener("mouseup", endBand);
    cancelAnimationFrame(bandFrame);
    bandBox.hidden = true;
    root.classList.remove("is-banding");
    // A plain click on empty space clears the selection.
    if (band && !band.moved && !band.additive && selected.size) clearSelection();
    band = null;
  }

  // ---- Arrow keys: move between photos by where they sit on screen ----

  function neighbour(nodes, from, key) {
    const index = nodes.indexOf(from);
    if (key === "ArrowLeft") return nodes[Math.max(0, index - 1)];
    if (key === "ArrowRight") return nodes[Math.min(nodes.length - 1, index + 1)];
    const here = from.getBoundingClientRect();
    const down = key === "ArrowDown";
    const x = here.left + here.width / 2;
    const y = here.top + here.height / 2;
    // The photo below (or above) that is nearest, favouring the same column.
    let best = null;
    let bestScore = Infinity;
    for (const node of nodes) {
      if (node === from) continue;
      const box = node.getBoundingClientRect();
      const dy = down ? box.top - (here.bottom - 2) : (here.top + 2) - box.bottom;
      if (dy < 0) continue;
      const score = dy + 2 * Math.abs(box.left + box.width / 2 - x) + Math.abs(box.top + box.height / 2 - y) * 0.01;
      if (score < bestScore) {
        bestScore = score;
        best = node;
      }
    }
    return best || from;
  }

  function moveFocus(key, extend) {
    const nodes = [...body.querySelectorAll(".gallery-tile")];
    if (!nodes.length) return;
    const from = nodes.find((node) => node.dataset.path === focusPath);
    let to;
    if (from) {
      to = neighbour(nodes, from, key);
    } else {
      // Start from the first photo in view.
      const top = scroller.getBoundingClientRect().top;
      to = nodes.find((node) => node.getBoundingClientRect().bottom > top) || nodes[0];
    }
    const path = to.dataset.path;
    if (extend && anchor) {
      const list = nodes.map((node) => node.dataset.path);
      const a = list.indexOf(anchor);
      const b = list.indexOf(path);
      selected = new Set(a < 0 ? [path] : list.slice(Math.min(a, b), Math.max(a, b) + 1));
    } else {
      selected = new Set([path]);
      anchor = path;
    }
    focusPath = path;
    root.classList.add("is-keyboard");
    paintSelection();
    to.scrollIntoView({ block: "nearest" });
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
      write && present.length && present.every(resizable) ? button("Resize…", () => ctx.resize(present)) : null,
      write && ctx.canConvertVideos() && present.length && picked.every((item) => item.kind === "video") ? button("Convert video…", () => ctx.convertVideos(present, "format")) : null,
      present.length ? button("Download", () => ctx.download(present)) : null,
      present.length === 1 ? button("Show in folder", () => ctx.showInFolder(present[0])) : null,
      write && picked.some((item) => !item.hidden) && route.tab !== "hidden" ? button("Hide from Photos", () => setHidden(picked.filter((item) => !item.hidden).map((item) => item.path), true)) : null,
      write && picked.some((item) => item.hidden) ? button("Show in Photos", () => setHidden(picked.filter((item) => item.hidden).map((item) => item.path), false)) : null,
      write && current && picked.length === 1 && !picked[0].missing ? button("Use as cover", () => setCover(picked[0].path)) : null,
      write && current ? button("Remove from album", () => removeFromAlbum(picked.map((item) => item.path)), { class: "danger" }) : null,
      write && present.length ? button("Delete, never back up again…", () => deleteAndBlock(present), { class: "danger" }) : null,
      write && route.tab === "person" && picked.every((item) => item.faceId)
        ? button(`Not ${currentPerson && currentPerson.name ? currentPerson.name : "this person"}`, () => moveFaces(picked.map((item) => item.faceId), null), { class: "danger" })
        : null,
      button("Clear", clearSelection, { class: "gallery-clear" }),
    ].filter(Boolean));
  }

  // ---- Layouts: rows and masonry are placed here; squares and the list are plain CSS ----

  const dirty = new Set();
  let layoutFrame = 0;

  function scheduleLayout(gridNode) {
    if (gridNode) dirty.add(gridNode);
    else for (const node of body.querySelectorAll(".gallery-grid:not(.is-placeholder)")) dirty.add(node);
    cancelAnimationFrame(layoutFrame);
    layoutFrame = requestAnimationFrame(() => {
      const grids = [...dirty];
      dirty.clear();
      for (const node of grids) if (node.isConnected) layoutGrid(node);
    });
  }

  function layoutGrid(gridNode) {
    if (gridNode.classList.contains("is-placeholder")) return;
    const tiles = [...gridNode.children].filter((node) => node.classList.contains("gallery-tile"));
    if (view !== "justified" && view !== "masonry") {
      gridNode.style.height = "";
      for (const node of tiles) node.style.cssText = "";
      return;
    }
    const width = gridNode.clientWidth;
    if (!width) return;
    const target = Number(size.value) || 150;
    const aspect = (node) => {
      const item = byPath.get(node.dataset.path);
      return item && !item.missing ? aspectOf(item) : 1;
    };
    if (view === "justified") {
      // Rows as tall as the size slider, then stretched (or shrunk) to fill the width exactly.
      gridNode.style.height = "";
      let row = [];
      let sum = 0;
      const place = (height, last) => {
        row.forEach((node, index) => {
          node.style.cssText = `width:${Math.floor(aspect(node) * height * 100) / 100}px;height:${Math.round(height)}px`;
          node.classList.toggle("is-row-end", !last && index === row.length - 1);
        });
        row = [];
        sum = 0;
      };
      for (const node of tiles) {
        row.push(node);
        sum += aspect(node);
        const fill = (width - GAP * (row.length - 1)) / sum;
        if (fill <= target) place(fill, false);
      }
      if (row.length) place(target, true);
      return;
    }
    // Masonry: columns about as wide as the slider; each photo goes under the shortest column.
    const columns = Math.max(1, Math.round((width + GAP) / (target + GAP)));
    const columnWidth = (width - GAP * (columns - 1)) / columns;
    const heights = new Array(columns).fill(0);
    for (const node of tiles) {
      let column = 0;
      for (let i = 1; i < columns; i++) if (heights[i] < heights[column] - 1) column = i;
      const height = columnWidth / aspect(node);
      node.style.cssText = `left:${column * (columnWidth + GAP)}px;top:${heights[column]}px;width:${columnWidth}px;height:${height}px`;
      heights[column] += height + GAP;
    }
    gridNode.style.height = `${Math.max(0, Math.max(...heights) - GAP)}px`;
  }

  // New tiles (a month of the timeline coming in, an album opening) are laid out as they arrive.
  new MutationObserver((records) => {
    if (view !== "justified" && view !== "masonry") return;
    for (const record of records) {
      const owner = record.target.closest?.(".gallery-grid");
      if (owner) scheduleLayout(owner);
      for (const added of record.addedNodes) {
        if (added.nodeType !== 1) continue;
        if (added.classList.contains("gallery-grid")) scheduleLayout(added);
        for (const inner of added.querySelectorAll(".gallery-grid")) scheduleLayout(inner);
      }
    }
  }).observe(body, { childList: true, subtree: true });

  function setView(id) {
    view = id;
    try {
      localStorage.setItem(VIEW_KEY, id);
    } catch {}
    // Stay with the photos that were on screen: the first one in view keeps its place.
    const top = scroller.getBoundingClientRect().top;
    const keep = scroller.scrollTop > 0 ? [...body.querySelectorAll(".gallery-tile")].find((node) => node.getBoundingClientRect().bottom > top) : null;
    const before = keep ? keep.getBoundingClientRect().top - top : 0;
    paintView();
    scheduleLayout();
    sizePlaceholders();
    requestAnimationFrame(() => requestAnimationFrame(() => {
      if (keep?.isConnected) scroller.scrollTop += keep.getBoundingClientRect().top - top - before;
      scheduleScrubber();
    }));
  }

  function paintView() {
    body.dataset.view = view;
    // The list has no photo size; the slider stays in place so nothing moves.
    size.disabled = view === "list";
    sizeControl.classList.toggle("is-disabled", view === "list");
    for (const button of viewSwitch.children) button.setAttribute("aria-pressed", String(button.dataset.view === view));
  }
  paintView();

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
    scheduleLayout();
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
    showMemories(mine);
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
  /// Deletes photos on the server for good, so phones don't back them up again.
  async function deleteAndBlock(paths) {
    try {
      const done = await ctx.deleteAndBlock(paths);
      if (!done) return;
      for (const path of paths) selected.delete(path);
      if (route.tab === "photos") {
        removeFromTimeline(paths);
        paintSelection();
      } else {
        show(location.hash);
      }
    } catch (err) {
      ctx.toast(err.message, true);
    }
  }

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
  /// Months not loaded yet get about the height they will have, so the scrollbar and the
  /// scrubber are right from the start.
  function sizePlaceholders() {
    const sample = body.querySelector(".gallery-grid");
    if (!sample) return;
    const width = sample.clientWidth || body.clientWidth;
    const target = Number(size.value) || 150;
    let height;
    if (view === "list") {
      const row = 56 + 2;
      height = (count) => count * row - 2;
    } else if (view === "justified") {
      // Photos are mostly 4:3 or 3:4; about 1.2 wide on average.
      const perRow = Math.max(1, width / (target * 1.2 + GAP));
      height = (count) => Math.ceil(count / perRow) * (target + GAP) - GAP;
    } else if (view === "masonry") {
      const columns = Math.max(1, Math.round((width + GAP) / (target + GAP)));
      const columnWidth = (width - GAP * (columns - 1)) / columns;
      height = (count) => Math.ceil(count / columns) * (columnWidth / 1.1 + GAP) - GAP;
    } else {
      const style = getComputedStyle(sample);
      const tracks = style.gridTemplateColumns.split(" ").filter(Boolean);
      const columns = Math.max(1, tracks.length);
      const tileSize = parseFloat(tracks[0]) || target;
      height = (count) => {
        const rows = Math.ceil(count / columns);
        return rows * tileSize + Math.max(0, rows - 1) * GAP;
      };
    }
    for (const placeholder of body.querySelectorAll(".gallery-grid.is-placeholder")) {
      placeholder.style.height = `${Math.max(0, height(Number(placeholder.dataset.count)))}px`;
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
  let lastWidth = 0;
  new ResizeObserver(() => {
    sizePlaceholders();
    // Rows and columns depend on the width only.
    if (scroller.clientWidth !== lastWidth) {
      lastWidth = scroller.clientWidth;
      scheduleLayout();
    }
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
      write && paths.length && paths.every(resizable) && { label: "Resize…", icon: "resize", run: () => ctx.resize(paths) },
      write && ctx.canConvertVideos() && present.length && present.every((entry) => entry.kind === "video") && { label: "Convert video…", icon: "convertVideo", run: () => ctx.convertVideos(paths, "format") },
      paths.length && { label: "Download", icon: "download", run: () => ctx.download(paths) },
      one && !one.missing && { label: "Show in folder", icon: "folder", run: () => ctx.showInFolder(one.path) },
      one && !one.missing && smart && smart.available && { label: "Find similar photos", icon: "findSimilar", run: () => ctx.findSimilar(one.path) },
      write && route.tab === "person" && picked.every((entry) => entry.faceId) && {
        label: `Not ${currentPerson && currentPerson.name ? currentPerson.name : "this person"}`,
        icon: "removeFromAlbum",
        danger: true,
        run: () => moveFaces(picked.map((entry) => entry.faceId), null),
      },
      write && route.tab === "person" && picked.every((entry) => entry.faceId) && {
        label: "This is someone else…",
        icon: "move",
        run: async () => {
          const to = await choosePerson("Who is it?", currentPerson && currentPerson.id);
          if (to) moveFaces(picked.map((entry) => entry.faceId), to);
        },
      },
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
      write && paths.length && { label: "Delete, never back up again…", icon: "deleteBlock", danger: true, run: () => deleteAndBlock(paths) },
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
    const personCard = event.target.closest(".gallery-person");
    if (personCard) {
      const person = people.find((entry) => entry.id === Number(personCard.dataset.person));
      if (!person) return;
      event.preventDefault();
      const write = ctx.canWrite();
      openMenu(event.clientX, event.clientY, person.name || "Unnamed person", [
        { label: "Open", icon: "open", run: () => { location.hash = `#gallery/people/${person.id}`; } },
        write && { label: person.name ? "Rename…" : "Add a name…", icon: "rename", run: () => renamePerson(person) },
        write && { label: "Same person as…", icon: "move", run: () => mergePerson(person) },
        write && { label: person.hidden ? "Show in People" : "Hide from People", icon: "hide", run: () => hidePerson(person, !person.hidden) },
      ]);
      return;
    }
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

  // ---- Smart search: by words, or by a photo ----

  /// What the smart features can do right now (fetched again on each visit).
  async function loadSmart() {
    try {
      smart = await ctx.api("/api/ml");
    } catch {
      smart = { available: false };
    }
    const searchOn = !!(smart.available && smart.search.enabled && smart.search.ready);
    const facesOn = !!(smart.available && smart.faces.enabled && smart.faces.ready);
    searchBox.hidden = !searchOn;
    tabs.querySelector('[data-tab="people"]').hidden = !facesOn;
  }

  const searchOn = () => !!(smart && smart.available && smart.search.enabled && smart.search.ready);

  searchBox.addEventListener("keydown", (event) => {
    if (event.key !== "Enter") return;
    const query = searchBox.value.trim();
    if (query) location.hash = `#gallery/search/${encodeURIComponent(query)}`;
  });
  // Drop or paste a picture onto the search box (or anywhere in the Gallery) to find photos like it.
  const pictureOf = (transfer) => [...((transfer && transfer.files) || [])].find((file) => file.type.startsWith("image/") || /\.(heic|heif|avif)$/i.test(file.name));
  searchBox.addEventListener("paste", (event) => {
    const picture = pictureOf(event.clipboardData);
    if (!picture) return;
    event.preventDefault();
    findLike(picture);
  });
  root.addEventListener("dragover", (event) => {
    if (searchOn() && [...(event.dataTransfer?.types || [])].includes("Files")) {
      event.preventDefault();
      root.classList.add("is-drop-target");
    }
  });
  root.addEventListener("dragleave", (event) => {
    if (!root.contains(event.relatedTarget)) root.classList.remove("is-drop-target");
  });
  root.addEventListener("drop", (event) => {
    root.classList.remove("is-drop-target");
    const picture = pictureOf(event.dataTransfer);
    if (!picture || !searchOn()) return;
    event.preventDefault();
    findLike(picture);
  });

  function findLike(picture) {
    sample = picture;
    if (location.hash === "#gallery/similar") show(location.hash);
    else location.hash = "#gallery/similar";
  }

  async function showSearch(query) {
    const mine = ++generation;
    searchBox.value = query;
    actions.replaceChildren();
    body.replaceChildren(el("p", { class: "muted gallery-message" }, "Searching…"));
    try {
      const data = await ctx.api(`/api/gallery/search?q=${encodeURIComponent(query)}`);
      if (stale(mine)) return;
      items = data.items;
      status.textContent = items.length ? `${plural(items.length, "photo")}, best matches first` : "";
      body.replaceChildren(
        el("nav", { class: "gallery-crumbs" }, el("a", { href: "#gallery" }, "Photos"), el("span", { "aria-hidden": "true" }, "›"), el("strong", {}, `“${query}”`)),
        items.length ? grid(items) : el("p", { class: "muted gallery-message" }, "Nothing matches. Try other words, in English: “red car”, “people at a table”, “snowy mountains”."),
      );
    } catch (err) {
      if (!stale(mine)) body.replaceChildren(el("p", { class: "muted gallery-message" }, err.message));
    }
  }

  async function showSimilar(path) {
    const mine = ++generation;
    actions.replaceChildren();
    body.replaceChildren(el("p", { class: "muted gallery-message" }, "Looking for similar photos…"));
    try {
      let data;
      let preview;
      if (path) {
        data = await ctx.api(`/api/gallery/similar?path=${encodeURIComponent(path)}`);
        preview = el("img", { src: thumbUrl({ path }), alt: nameOf(path) });
      } else if (sample) {
        const response = await fetch("/api/gallery/similar", { method: "POST", body: sample, credentials: "same-origin", headers: { "X-OwnNAS": "1" } });
        data = await response.json().catch(() => ({}));
        if (!response.ok) throw new Error(data.error || "Could not search");
        preview = el("img", { src: URL.createObjectURL(sample), alt: sample.name || "Sample picture" });
      } else {
        body.replaceChildren(el("p", { class: "muted gallery-message" }, "Drop or paste a picture onto the search box to find photos like it."));
        return;
      }
      if (stale(mine)) return;
      items = data.items;
      status.textContent = plural(items.length, "similar photo");
      body.replaceChildren(
        el("div", { class: "gallery-sample" }, el("span", { class: "gallery-sample-picture" }, preview), el("div", {},
          el("strong", {}, "Photos like this one"),
          el("p", { class: "muted" }, path ? path : "From the picture you dropped in. It was not saved."))),
        items.length ? grid(items) : el("p", { class: "muted gallery-message" }, "No photo looks much like this one."),
      );
    } catch (err) {
      if (!stale(mine)) body.replaceChildren(el("p", { class: "muted gallery-message" }, err.message));
    }
  }

  // ---- People ----

  function faceImage(faceId, name) {
    return faceId
      ? el("img", { src: `/api/faces/crop?id=${faceId}`, alt: name || "", loading: "lazy" })
      : el("span", { class: "gallery-person-empty", "aria-hidden": "true" }, "?");
  }

  async function showPeople(hiddenOnes) {
    const mine = ++generation;
    actions.replaceChildren();
    body.replaceChildren(el("p", { class: "muted gallery-message" }, "Loading people…"));
    try {
      const data = await ctx.api(`/api/people${hiddenOnes ? "?hidden=1" : ""}`);
      if (stale(mine)) return;
      people = data.people;
      items = [];
      status.textContent = plural(people.length, hiddenOnes ? "hidden person" : "person", hiddenOnes ? "hidden people" : "people");
      if (!hiddenOnes && data.hiddenCount) actions.append(el("a", { class: "button", href: "#gallery/people/hidden" }, `Hidden · ${number(data.hiddenCount)}`));
      const scanning = smart && smart.scanned < smart.images;
      body.replaceChildren(...[
        hiddenOnes ? el("nav", { class: "gallery-crumbs" }, el("a", { href: "#gallery/people" }, "People"), el("span", { "aria-hidden": "true" }, "›"), el("strong", {}, "Hidden")) : null,
        people.length
          ? el("div", { class: "gallery-people" }, people.map((person) =>
            el("a", { class: "gallery-person", href: `#gallery/people/${person.id}`, "data-person": String(person.id) },
              el("span", { class: "gallery-person-face" }, faceImage(person.coverFace, person.name)),
              el("strong", { class: person.name ? null : "muted" }, person.name || "Add a name"),
              el("span", { class: "muted" }, plural(person.photos, "photo")))))
          : el("p", { class: "muted gallery-message" }, hiddenOnes ? "Nobody is hidden."
            : scanning ? `Looking for faces… ${number(smart.scanned)} of ${number(smart.images)} photos so far. People show up here once someone is in two photos.`
              : "No people yet. Someone shows up here once they are in two photos."),
      ].filter(Boolean));
    } catch (err) {
      if (!stale(mine)) body.replaceChildren(el("p", { class: "muted gallery-message" }, err.message));
    }
  }

  async function showPerson(id) {
    const mine = ++generation;
    body.replaceChildren(el("p", { class: "muted gallery-message" }, "Loading…"));
    try {
      const data = await ctx.api(`/api/people/photos?id=${id}`);
      if (stale(mine)) return;
      currentPerson = data.person;
      items = data.items;
      const write = ctx.canWrite();
      status.textContent = plural(items.length, "photo");
      actions.replaceChildren(...[
        write && el("button", { type: "button", onclick: () => renamePerson(currentPerson) }, currentPerson.name ? "Rename" : "Add a name"),
        write && el("button", { type: "button", onclick: () => mergePerson(currentPerson) }, "Same person as…"),
        write && el("button", { type: "button", onclick: () => hidePerson(currentPerson, !currentPerson.hidden) }, currentPerson.hidden ? "Show in People" : "Hide"),
      ].filter(Boolean));
      body.replaceChildren(
        el("nav", { class: "gallery-crumbs" }, el("a", { href: "#gallery/people" }, "People"), el("span", { "aria-hidden": "true" }, "›"), el("strong", {}, currentPerson.name || "Unnamed person")),
        items.length ? grid(items) : el("p", { class: "muted gallery-message" }, "No photos left for this person."),
      );
    } catch (err) {
      if (!stale(mine)) body.replaceChildren(el("p", { class: "muted gallery-message" }, err.message));
    }
  }

  async function renamePerson(person) {
    const name = await ctx.askText(person.name ? "Rename" : "Who is this?", "Name", person.name || "", "Save");
    if (name === null) return;
    try {
      await ctx.api("/api/people", { method: "POST", json: { id: person.id, name } });
      show(location.hash);
    } catch (err) {
      ctx.toast(err.message, true);
    }
  }

  async function hidePerson(person, hidden) {
    try {
      await ctx.api("/api/people", { method: "POST", json: { id: person.id, hidden } });
      ctx.toast(hidden ? `${person.name || "This person"} is hidden from People` : `${person.name || "This person"} is back in People`);
      if (route.tab === "person") location.hash = hidden ? "#gallery/people" : `#gallery/people/${person.id}`;
      else show(location.hash);
    } catch (err) {
      ctx.toast(err.message, true);
    }
  }

  async function mergePerson(person) {
    const into = await choosePerson(`${person.name || "This person"} is the same as…`, person.id);
    if (!into) return;
    try {
      await ctx.api("/api/people", { method: "POST", json: { id: person.id, into } });
      ctx.toast("Put together");
      location.hash = `#gallery/people/${into}`;
    } catch (err) {
      ctx.toast(err.message, true);
    }
  }

  /// "This is not them": the faces move out to a person of their own (or to `to`).
  async function moveFaces(faceIds, to) {
    try {
      await ctx.api("/api/faces/move", { method: "POST", json: { faces: faceIds, to } });
      ctx.toast(to ? "Moved" : `Taken out of ${currentPerson && currentPerson.name ? currentPerson.name : "this person"}`);
      selected = new Set();
      show(location.hash);
    } catch (err) {
      ctx.toast(err.message, true);
    }
  }

  /// Resolves with the id of the person chosen, or null.
  function choosePerson(title, exclude) {
    return new Promise(async (resolve) => {
      let list = [];
      try {
        list = (await ctx.api("/api/people")).people.filter((person) => person.id !== exclude);
      } catch (err) {
        ctx.toast(err.message, true);
        resolve(null);
        return;
      }
      let result = null;
      const filter = el("input", { type: "search", placeholder: "Find someone", "aria-label": "Find someone" });
      const grid_ = el("div", { class: "person-picker-list" });
      const dialog = el("dialog", { class: "person-picker" }, el("h3", {}, title), filter, grid_,
        el("div", { class: "dialog-actions" }, el("span", { class: "spacer" }), el("button", { type: "button", onclick: () => dialog.close() }, "Cancel")));
      const render = () => {
        const query = filter.value.trim().toLowerCase();
        const shown = list.filter((person) => !query || (person.name || "").toLowerCase().includes(query));
        grid_.replaceChildren(...(shown.length ? shown.map((person) => el("button", {
          type: "button",
          class: "person-picker-row",
          onclick: () => { result = person.id; dialog.close(); },
        }, el("span", { class: "gallery-person-face" }, faceImage(person.coverFace, person.name)), el("span", {}, person.name || `Unnamed (${plural(person.photos, "photo")})`))) : [el("p", { class: "muted" }, "Nobody else yet.")]));
      };
      filter.addEventListener("input", render);
      dialog.addEventListener("close", () => {
        dialog.remove();
        resolve(result);
      });
      document.body.append(dialog);
      render();
      dialog.showModal();
    });
  }

  // ---- Routing ----

  // ---- On this day: photos taken on today's date in earlier years ----

  /// Today as the viewer sees it.
  function today() {
    const now = new Date();
    return { year: now.getFullYear(), month: now.getMonth() + 1, day: now.getDate() };
  }

  async function memories() {
    const { year, month, day } = today();
    return ctx.api(`/api/gallery/memories?month=${month}&day=${day}&year=${year}`);
  }

  /// A strip of years above the timeline, when there are any.
  async function showMemories(mine) {
    let data;
    try {
      data = await memories();
    } catch {
      return;
    }
    if (stale(mine) || !data.years.length) return;
    const { year: now } = today();
    const cards = data.years.map((entry) => {
      const ago = now - entry.year;
      const cover = entry.items[0];
      return el("a", { class: "gallery-memory", href: `#gallery/memories/${entry.year}`, title: `Photos from this day in ${entry.year}` },
        el("img", { src: thumbUrl(cover), alt: "", loading: "lazy", draggable: "false" }),
        el("span", { class: "gallery-memory-label" }, el("strong", {}, ago === 1 ? "1 year ago" : `${ago} years ago`), el("span", {}, `${entry.year} · ${plural(entry.count, "photo")}`)));
    });
    body.prepend(el("section", { class: "gallery-memories", "aria-label": "On this day" }, el("h2", { class: "gallery-month" }, "On this day"), el("div", { class: "gallery-memory-row" }, cards)));
  }

  async function showMemoryYear(year) {
    const mine = ++generation;
    actions.replaceChildren();
    body.replaceChildren(el("p", { class: "muted gallery-message" }, "Loading…"));
    try {
      const data = await memories();
      if (stale(mine)) return;
      const entry = data.years.find((found) => found.year === year);
      items = entry ? entry.items : [];
      const { year: now } = today();
      const label = new Intl.DateTimeFormat(undefined, { day: "numeric", month: "long", year: "numeric" }).format(new Date(year, new Date().getMonth(), new Date().getDate()));
      status.textContent = entry && entry.count > items.length ? `${number(items.length)} of ${plural(entry.count, "photo")}` : plural(items.length, "photo");
      body.replaceChildren(
        el("nav", { class: "gallery-crumbs" }, el("a", { href: "#gallery" }, "Photos"), el("span", { "aria-hidden": "true" }, "›"), el("strong", {}, `On this day, ${now - year === 1 ? "1 year" : `${now - year} years`} ago (${label})`)),
        items.length ? grid(items) : el("p", { class: "muted gallery-message" }, "No photos from this day in that year."),
      );
    } catch (err) {
      if (!stale(mine)) body.replaceChildren(el("p", { class: "muted gallery-message" }, err.message));
    }
  }

  // ---- Places: photos with a location, on a map ----

  async function showPlaces() {
    const mine = ++generation;
    actions.replaceChildren();
    body.replaceChildren(el("p", { class: "muted gallery-message" }, "Loading the map…"));
    let viewers;
    let data;
    try {
      [viewers, data] = await Promise.all([import("/assets/viewers.js"), ctx.api("/api/gallery/places?zoom=2")]);
    } catch (err) {
      if (!stale(mine)) body.replaceChildren(el("p", { class: "muted gallery-message" }, err.message));
      return;
    }
    if (stale(mine)) return;
    if (!data.extent) {
      status.textContent = "";
      body.replaceChildren(el("p", { class: "muted gallery-message" }, "No photos with a location yet. Photos and videos taken with location on (most phones do) show up here on a map."));
      return;
    }
    const byZoom = new Map([[2, data.places]]);
    let places = data.places;
    let shownZoom = 2;
    const here = el("p", { class: "muted gallery-places-count" });
    const list = el("div", { class: "gallery-places-list" });
    const map = viewers.createMap({
      maxZoom: 18,
      paint({ layer, at, width, height }) {
        // The biggest groups last, so they show on top of smaller ones nearby.
        for (const place of [...places].reverse()) {
          const [x, y] = at(place.lat, place.lon);
          if (x < -40 || y < -40 || x > width + 40 || y > height + 40) continue;
          const marker = el("button", { type: "button", class: "map-marker", title: place.count === 1 ? nameOf(place.newest.path) : `${plural(place.count, "photo")} here` },
            el("img", { src: thumbUrl(place.newest), alt: "", draggable: "false" }),
            place.count > 1 ? el("span", { class: "map-marker-count" }, number(place.count)) : null);
          marker.style.transform = `translate(${Math.round(x)}px, ${Math.round(y)}px)`;
          marker.addEventListener("click", () => {
            if (place.count > 1 && map.zoom < 16) map.show(place.lat, place.lon, map.zoom + 2);
            else ctx.openPreview(place.newest, [place.newest]);
          });
          layer.append(marker);
        }
      },
      async moved(view) {
        if (stale(mine)) return;
        // Places group differently at each zoom: fetch them once per zoom.
        if (view.zoom !== shownZoom) {
          shownZoom = view.zoom;
          if (!byZoom.has(view.zoom)) {
            try {
              byZoom.set(view.zoom, (await ctx.api(`/api/gallery/places?zoom=${view.zoom}`)).places);
            } catch {
              return;
            }
          }
          if (stale(mine) || shownZoom !== view.zoom) return;
          places = byZoom.get(view.zoom);
          map.redraw();
        }
        listHere(view);
      },
    });
    placesMap = map;
    let listed = "";
    async function listHere(view) {
      const query = `west=${view.west}&south=${view.south}&east=${view.east}&north=${view.north}`;
      if (query === listed) return;
      listed = query;
      try {
        const found = await ctx.api(`/api/gallery/places/items?${query}&limit=300`);
        if (stale(mine) || listed !== query) return;
        items = found.items;
        here.textContent = found.total ? `${plural(found.total, "photo")} on this part of the map${found.total > items.length ? ` (the newest ${number(items.length)} below)` : ""}` : "No photos on this part of the map.";
        status.textContent = plural(data.extent.count, "photo") + " with a location";
        list.replaceChildren(items.length ? grid(items) : "");
      } catch {}
    }
    body.replaceChildren(el("div", { class: "gallery-places" }, map.element, here, list));
    const e = data.extent;
    map.fit({ north: e.north, south: e.south, west: e.west, east: e.east });
  }

  function parse(hash) {
    const parts = hash.replace(/^#?gallery\/?/, "").split("/").filter(Boolean).map(decodeURIComponent);
    if (parts[0] === "albums") return { tab: "albums", id: parts[1] ? Number(parts[1]) : null };
    if (parts[0] === "tags") return { tab: "tags", tag: parts[1] || "" };
    if (parts[0] === "hidden") return { tab: "hidden" };
    if (parts[0] === "places") return { tab: "places" };
    if (parts[0] === "memories" && parts[1]) return { tab: "memories", year: Number(parts[1]) };
    if (parts[0] === "search") return { tab: "search", query: parts.slice(1).join("/") };
    if (parts[0] === "similar") return { tab: "similar", path: parts.slice(1).join("/") };
    if (parts[0] === "people" && parts[1] === "hidden") return { tab: "people", hidden: true };
    if (parts[0] === "people" && parts[1]) return { tab: "person", id: Number(parts[1]) };
    if (parts[0] === "people") return { tab: "people" };
    return { tab: "photos" };
  }

  function show(hash) {
    root.hidden = false;
    route = parse(hash);
    current = null;
    placesMap?.destroy();
    placesMap = null;
    selected = new Set();
    anchor = "";
    focusPath = "";
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
    const tab = { hidden: "photos", search: "photos", similar: "photos", memories: "photos", person: "people" }[route.tab] || route.tab;
    if (route.tab !== "search") searchBox.value = "";
    for (const link of tabs.children) link.setAttribute("aria-current", link.dataset.tab === tab ? "page" : "false");
    scroller.scrollTop = 0;
    if (route.tab === "albums") showAlbums(route.id);
    else if (route.tab === "tags") showTags(route.tag);
    else if (route.tab === "hidden") showHidden();
    else if (route.tab === "search") showSearch(route.query);
    else if (route.tab === "similar") showSimilar(route.path);
    else if (route.tab === "people") showPeople(!!route.hidden);
    else if (route.tab === "person") showPerson(route.id);
    else if (route.tab === "places") showPlaces();
    else if (route.tab === "memories") showMemoryYear(route.year);
    else showPhotos();
    loadSmart();
  }

  function hide() {
    closeMenu();
    placesMap?.destroy();
    placesMap = null;
    root.hidden = true;
    generation++;
    monthObserver?.disconnect();
    clearTimeout(statusTimer);
  }

  /// Keys for the gallery (app.js passes them on while it shows): arrows move and select (Shift
  /// extends), Enter opens, Esc clears the selection and ⌘A selects everything shown. Returns
  /// whether the key was used.
  function handleKey(event) {
    const plain = !event.metaKey && !event.ctrlKey && !event.altKey;
    if (plain && ["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"].includes(event.key) && body.querySelector(".gallery-tile")) {
      event.preventDefault();
      moveFocus(event.key, event.shiftKey);
      return true;
    }
    if (plain && !event.shiftKey && event.key === "Enter") {
      const item = byPath.get(focusPath) || (selected.size === 1 ? byPath.get([...selected][0]) : null);
      if (!item || item.missing) return false;
      event.preventDefault();
      ctx.openPreview(item, ordered().filter((entry) => !entry.missing));
      return true;
    }
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
