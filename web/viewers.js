// Previews for file formats the browser does not show on its own, loaded when first needed.
// Each `mount…` fills the preview body and returns { destroy } to stop what it started.
// The page's security policy allows no inline styles: sizes and colours come from classes.

const enc = encodeURIComponent;

/// An element with a class and text or children.
function h(tag, className, ...children) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  for (const child of children.flat(Infinity)) {
    if (child == null || child === false) continue;
    node.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
  return node;
}

async function getJson(url) {
  const response = await fetch(url, { credentials: "same-origin" });
  let data = null;
  try { data = await response.json(); } catch { /* not JSON */ }
  if (!response.ok) throw new Error((data && data.error) || "Could not load the preview");
  return data;
}

function clock(seconds) {
  if (!Number.isFinite(seconds)) return "";
  const total = Math.max(0, Math.round(seconds));
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const secs = String(total % 60).padStart(2, "0");
  return hours ? `${hours}:${String(minutes).padStart(2, "0")}:${secs}` : `${minutes}:${secs}`;
}

function cssColor(name, fallback) {
  const value = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  return value || fallback;
}

// ---- Audio: cover, tags, and a waveform to see and seek in ----

/// The loudest sample in each of `buckets` slices of the song, from 0 to 1.
function peaksOf(buffer, buckets) {
  const channels = Array.from({ length: buffer.numberOfChannels }, (_, i) => buffer.getChannelData(i));
  const size = Math.max(1, Math.floor(buffer.length / buckets));
  const peaks = new Float32Array(buckets);
  let loudest = 0;
  for (let bucket = 0; bucket < buckets; bucket += 1) {
    let peak = 0;
    const start = bucket * size;
    // Every few samples is enough to see the shape, and quick for a long song.
    const step = Math.max(1, Math.floor(size / 400));
    for (let i = start; i < Math.min(start + size, buffer.length); i += step) {
      for (const data of channels) {
        const value = Math.abs(data[i]);
        if (value > peak) peak = value;
      }
    }
    peaks[bucket] = peak;
    if (peak > loudest) loudest = peak;
  }
  if (loudest > 0) for (let i = 0; i < buckets; i += 1) peaks[i] /= loudest;
  return peaks;
}

export function mountAudio(body, entry) {
  const raw = `/api/raw?path=${enc(entry.path)}`;
  const audio = h("audio", "audio-player-element");
  audio.controls = true;
  audio.preload = "metadata";
  audio.src = raw;
  const cover = h("div", "audio-cover", h("span", "audio-cover-empty", "♪"));
  const title = h("h3", "audio-title", entry.name.replace(/\.[^.]+$/, ""));
  const artist = h("p", "audio-artist");
  const album = h("p", "audio-album muted");
  const details = h("p", "audio-details muted");
  const canvas = h("canvas", "audio-wave");
  const waveNote = h("p", "audio-wave-note muted", "Drawing the waveform…");
  const wave = h("div", "audio-wave-box", canvas, waveNote);
  body.replaceChildren(h("div", "audio-view", h("div", "audio-head", cover, h("div", "audio-text", title, artist, album, details)), wave, audio));

  let peaks = null;
  let stopped = false;
  const draw = () => {
    if (!peaks || stopped) return;
    const ratio = window.devicePixelRatio || 1;
    const width = canvas.clientWidth;
    const height = canvas.clientHeight;
    if (!width || !height) return;
    canvas.width = Math.round(width * ratio);
    canvas.height = Math.round(height * ratio);
    const ctx = canvas.getContext("2d");
    ctx.scale(ratio, ratio);
    const played = audio.duration ? audio.currentTime / audio.duration : 0;
    const bars = Math.max(1, Math.floor(width / 3));
    const accent = cssColor("--accent", "#7aa2f7");
    const rest = cssColor("--muted", "#888");
    for (let bar = 0; bar < bars; bar += 1) {
      const peak = peaks[Math.min(peaks.length - 1, Math.floor((bar / bars) * peaks.length))];
      const barHeight = Math.max(1.5, peak * (height - 4));
      ctx.fillStyle = bar / bars < played ? accent : rest;
      ctx.globalAlpha = bar / bars < played ? 1 : 0.45;
      ctx.fillRect(bar * 3, (height - barHeight) / 2, 2, barHeight);
    }
  };
  const seek = (event) => {
    if (!audio.duration) return;
    const box = canvas.getBoundingClientRect();
    audio.currentTime = ((event.clientX - box.left) / box.width) * audio.duration;
    draw();
  };
  canvas.addEventListener("click", seek);
  audio.addEventListener("timeupdate", draw);
  audio.addEventListener("seeked", draw);
  const resize = new ResizeObserver(draw);
  resize.observe(canvas);

  getJson(`/api/audio/info?path=${enc(entry.path)}`).then((info) => {
    if (stopped) return;
    if (info.title) title.textContent = info.title;
    artist.textContent = info.artist || info.albumArtist || "";
    const albumLine = [info.album, info.year].filter(Boolean).join(" · ");
    const trackLine = info.track ? `Track ${info.track}${info.trackTotal ? ` of ${info.trackTotal}` : ""}${info.disc ? `, disc ${info.disc}` : ""}` : "";
    album.textContent = [albumLine, trackLine].filter(Boolean).join(" — ");
    details.textContent = [
      info.duration ? clock(info.duration) : "",
      info.genre,
      info.bitrate ? `${info.bitrate} kbps` : "",
      info.sampleRate ? `${(info.sampleRate / 1000).toLocaleString()} kHz` : "",
      info.bitDepth ? `${info.bitDepth}-bit` : "",
      info.channels === 1 ? "Mono" : info.channels === 2 ? "Stereo" : info.channels ? `${info.channels} channels` : "",
    ].filter(Boolean).join(" · ");
    if (info.hasCover) {
      const img = h("img", "audio-cover-img");
      img.alt = "";
      img.src = `/api/audio/cover?path=${enc(entry.path)}&v=${entry.modified || 0}`;
      img.addEventListener("load", () => cover.replaceChildren(img), { once: true });
    }
  }).catch(() => {});

  // The waveform needs the whole song decoded: only for files of a reasonable size.
  if (entry.size > 80 * 1024 * 1024 || !window.AudioContext) {
    waveNote.textContent = "This song is too long to draw its waveform.";
    canvas.hidden = true;
  } else {
    fetch(raw, { credentials: "same-origin" })
      .then((response) => {
        if (!response.ok) throw new Error();
        return response.arrayBuffer();
      })
      .then((bytes) => {
        const context = new AudioContext();
        return context.decodeAudioData(bytes).finally(() => context.close());
      })
      .then((buffer) => {
        if (stopped) return;
        peaks = peaksOf(buffer, 1200);
        waveNote.remove();
        draw();
      })
      .catch(() => {
        waveNote.textContent = "The waveform could not be drawn for this file.";
        canvas.hidden = true;
      });
  }
  return {
    destroy() {
      stopped = true;
      resize.disconnect();
      audio.pause();
      audio.removeAttribute("src");
      audio.load();
    },
  };
}

// ---- Video: the browser plays it, or the server converts it while it plays ----

export function mountVideo(body, entry, { poster = "" } = {}) {
  const raw = `/api/raw?path=${enc(entry.path)}`;
  const video = h("video", "");
  video.controls = true;
  video.playsInline = true;
  if (poster) video.poster = poster;
  video.src = raw;
  const note = h("p", "video-note muted");
  note.hidden = true;
  const seekBar = h("input", "video-seek");
  seekBar.type = "range";
  seekBar.min = "0";
  seekBar.step = "1";
  const seekTime = h("span", "video-seek-time muted");
  const seekRow = h("div", "video-seek-row", seekBar, seekTime);
  seekRow.hidden = true;
  body.replaceChildren(h("div", "video-view", video, seekRow, note));

  let stopped = false;
  let info = null;
  let streaming = false;
  let offset = 0;
  const infoReady = getJson(`/api/video/probe?path=${enc(entry.path)}`).then((data) => { info = data; return data; }).catch(() => null);

  const setTracks = (shift) => {
    // Switched off first, or the browser may keep showing a line from the old one.
    for (const track of [...video.textTracks]) track.mode = "disabled";
    for (const track of [...video.querySelectorAll("track")]) track.remove();
    if (!info || !info.subtitles) return;
    info.subtitles.forEach((subtitle, index) => {
      const track = document.createElement("track");
      track.kind = "subtitles";
      track.label = subtitle.label;
      if (subtitle.language) track.srclang = subtitle.language;
      track.src = `/api/subtitles?path=${enc(subtitle.path)}${shift ? `&shift=${shift}` : ""}`;
      if (index === 0) track.default = true;
      video.append(track);
    });
  };
  infoReady.then(() => { if (!stopped && !streaming) setTracks(0); });

  const streamFrom = (start, play) => {
    offset = Math.max(0, start);
    video.src = `/api/video/stream?path=${enc(entry.path)}&start=${offset.toFixed(2)}`;
    setTracks(offset);
    if (play) video.play().catch(() => {});
  };
  const showTime = () => {
    if (!streaming) return;
    const now = offset + (video.currentTime || 0);
    if (document.activeElement !== seekBar) seekBar.value = String(Math.floor(now));
    seekTime.textContent = `${clock(now)} / ${clock(info.streams.duration)}`;
  };
  const switchToStream = async () => {
    if (streaming || stopped) return;
    const data = await infoReady;
    if (stopped) return;
    note.hidden = false;
    if (!data || !data.ffmpeg) {
      note.textContent = "This browser cannot play this video. Install ffmpeg on the server to have OwnNAS convert it while it plays, or download it.";
      return;
    }
    if (!data.streams || !data.streams.video) {
      note.textContent = "This video could not be read.";
      return;
    }
    streaming = true;
    const copied = data.streams.video === "h264";
    note.textContent = copied
      ? "Played through OwnNAS: this browser cannot open the file as it is."
      : "Converted by OwnNAS while it plays: this browser cannot play its format. Use the bar below to jump.";
    if (data.streams.duration) {
      seekBar.max = String(Math.floor(data.streams.duration));
      seekRow.hidden = false;
    }
    streamFrom(0, false);
  };
  video.addEventListener("error", () => {
    if (!streaming) switchToStream();
  });
  // A video whose picture the browser cannot decode may still play its sound.
  video.addEventListener("loadedmetadata", () => {
    if (!streaming && video.videoWidth === 0) switchToStream();
  });
  video.addEventListener("timeupdate", showTime);
  seekBar.addEventListener("input", () => { seekTime.textContent = `${clock(Number(seekBar.value))} / ${clock(info.streams.duration)}`; });
  seekBar.addEventListener("change", () => streamFrom(Number(seekBar.value), true));

  return {
    destroy() {
      stopped = true;
      video.pause();
      video.removeAttribute("src");
      video.load();
    },
  };
}

// ---- Subtitles on their own: the lines and when they show ----

export async function mountSubtitles(body, entry) {
  body.replaceChildren(h("p", "muted", "Loading subtitles…"));
  const response = await fetch(`/api/subtitles?path=${enc(entry.path)}`, { credentials: "same-origin" });
  if (!response.ok) throw new Error("Could not read the subtitles");
  const text = await response.text();
  const cues = text.split(/\n\n+/).map((block) => {
    const lines = block.split("\n");
    const timing = lines.findIndex((line) => line.includes("-->"));
    if (timing < 0) return null;
    const [from, to] = lines[timing].split("-->").map((part) => part.trim().split(/\s+/)[0]);
    return { from: from.replace(/\.\d+$/, ""), to: to.replace(/\.\d+$/, ""), text: lines.slice(timing + 1).join("\n").replace(/<[^>]+>/g, "") };
  }).filter(Boolean);
  const list = h("ol", "subtitle-list", cues.map((cue) => h("li", "", h("span", "subtitle-time muted", `${cue.from} → ${cue.to}`), h("span", "subtitle-text", cue.text))));
  body.replaceChildren(h("div", "subtitle-view", h("p", "muted", `${cues.length.toLocaleString()} lines${cues.length ? `, ending at ${cues[cues.length - 1].to}` : ""}`), list));
  return { destroy() {} };
}

// ---- Documents, presentations, and spreadsheets read by the server (see src/documents.rs) ----

function partUrl(entry, part) {
  return `/api/archive/entry?path=${enc(entry.path)}&entry=${enc(part)}`;
}

function renderBlocks(entry, blocks) {
  return blocks.map((block) => {
    if (block.type === "heading") return h(`h${Math.min(6, Math.max(2, block.level + 1))}`, "doc-heading", block.text);
    if (block.type === "paragraph") return h("p", `doc-paragraph${block.level ? ` doc-level-${Math.min(block.level, 4)}` : ""}`, block.text);
    if (block.type === "table") {
      return h("div", "doc-table-wrap", h("table", "doc-table", h("tbody", "", block.rows.map((row) => h("tr", "", row.map((cell) => h("td", "", cell)))))));
    }
    if (block.type === "image") {
      const img = h("img", "doc-image");
      img.alt = "";
      img.loading = "lazy";
      img.src = partUrl(entry, block.entry);
      img.addEventListener("error", () => img.remove(), { once: true });
      return img;
    }
    return null;
  });
}

/// "A", "B", … "Z", "AA": a spreadsheet's column names.
function columnName(index) {
  let name = "";
  for (let n = index + 1; n > 0; n = Math.floor((n - 1) / 26)) name = String.fromCharCode(65 + ((n - 1) % 26)) + name;
  return name;
}

function renderSheets(sheets) {
  if (!sheets.length) return h("p", "muted", "This spreadsheet has no sheets.");
  const tabs = h("div", "sheet-tabs");
  const holder = h("div", "sheet-holder");
  const show = (index) => {
    const sheet = sheets[index];
    for (const [i, tab] of [...tabs.children].entries()) tab.setAttribute("aria-selected", i === index ? "true" : "false");
    const columns = Math.max(0, ...sheet.rows.map((row) => row.length));
    const head = h("tr", "", h("th", "sheet-corner", ""), Array.from({ length: columns }, (_, i) => h("th", "", columnName(i))));
    const rows = sheet.rows.map((row, r) => h("tr", "", h("th", "", String(r + 1)), Array.from({ length: columns }, (_, c) => {
      const value = row[c] ?? "";
      return h("td", /^-?[\d.,]+%?$/.test(value) ? "sheet-number" : "", value);
    })));
    const cut = sheet.totalRows > sheet.rows.length || sheet.totalColumns > columns;
    holder.replaceChildren(...[
      sheet.rows.length ? h("div", "sheet-scroll", h("table", "sheet-table", h("thead", "", head), h("tbody", "", rows))) : h("p", "muted", "This sheet is empty."),
      cut ? h("p", "muted sheet-note", `Showing ${sheet.rows.length.toLocaleString()} of ${sheet.totalRows.toLocaleString()} rows and ${columns} of ${sheet.totalColumns} columns. Download the file to see all of it.`) : null,
    ].filter(Boolean));
  };
  sheets.forEach((sheet, index) => {
    const tab = h("button", "sheet-tab", sheet.name);
    tab.type = "button";
    tab.addEventListener("click", () => show(index));
    tabs.append(tab);
  });
  show(0);
  return h("div", "sheet-view", sheets.length > 1 ? tabs : null, holder);
}

export async function mountDocument(body, entry) {
  body.replaceChildren(h("p", "muted", "Reading the document…"));
  const view = await getJson(`/api/document?path=${enc(entry.path)}`);
  let content;
  if (view.kind === "document") {
    content = h("article", "doc-view", view.note ? h("p", "doc-note muted", view.note) : null, view.blocks.length ? renderBlocks(entry, view.blocks) : h("p", "muted", "This document has no text."));
  } else if (view.kind === "slides") {
    const slides = view.slides.map((slide, index) => h("section", "slide-card",
      h("header", "slide-head", h("span", "slide-number", String(index + 1)), h("h3", "slide-title", slide.title || "")),
      h("div", "slide-body", renderBlocks(entry, slide.blocks)),
      slide.notes ? h("details", "slide-notes", h("summary", "", "Speaker notes"), h("p", "", slide.notes)) : null,
    ));
    content = h("div", "slides-view",
      h("p", "muted", `${view.slides.length} slide${view.slides.length === 1 ? "" : "s"}${view.note ? `. ${view.note}` : ""}`),
      slides);
  } else if (view.kind === "sheets") {
    content = renderSheets(view.sheets);
  } else if (view.kind === "picture") {
    const url = partUrl(entry, view.entry);
    let shown;
    if (view.pdf) {
      shown = h("iframe", "doc-pdf");
      shown.title = entry.name;
      shown.src = url;
    } else {
      shown = h("img", "doc-picture");
      shown.alt = entry.name;
      shown.src = url;
    }
    content = h("div", "doc-picture-view", shown, h("p", "muted", view.note));
  } else {
    throw new Error("This document could not be shown");
  }
  body.replaceChildren(content);
  return { destroy() {} };
}

/// Remembered per viewer (the place in a book, the page of a comic); never required.
function remember(key, value) {
  try {
    if (value === undefined) return localStorage.getItem(key);
    localStorage.setItem(key, String(value));
  } catch { /* private window, or storage off */ }
  return null;
}

/// Arrow keys (and Page Up/Down) for a viewer while it shows, before the file browser sees them.
function captureArrows(container, onStep) {
  const handler = (event) => {
    if (!container.isConnected || event.metaKey || event.ctrlKey || event.altKey) return;
    if (event.target.closest && event.target.closest("input, textarea, select, [contenteditable]")) return;
    const step = { ArrowLeft: -1, ArrowRight: 1, PageUp: -1, PageDown: 1 }[event.key];
    if (!step) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    onStep(step);
  };
  window.addEventListener("keydown", handler, true);
  return () => window.removeEventListener("keydown", handler, true);
}

// ---- E-books (EPUB): chapters in a sandboxed frame, with the table of contents ----

function bookPartUrl(entry, part) {
  const [path, fragment] = part.split("#");
  return `/api/book/${enc(entry.path)}/${path.split("/").map(enc).join("/")}${fragment ? `#${fragment}` : ""}`;
}

export async function mountBook(body, entry) {
  body.replaceChildren(h("p", "muted", "Opening the book…"));
  const book = await getJson(`/api/book/info?path=${enc(entry.path)}`);
  const frame = h("iframe", "book-frame");
  frame.title = book.title || entry.name;
  // No scripts: pages are read, not run.
  frame.setAttribute("sandbox", "allow-same-origin");
  const prev = h("button", "book-step", "‹ Previous");
  const next = h("button", "book-step", "Next ›");
  prev.type = next.type = "button";
  const contents = h("select", "book-contents");
  contents.setAttribute("aria-label", "Contents");
  const position = h("span", "book-position muted");
  const head = h("div", "book-head",
    h("div", "book-title", h("strong", "", book.title || entry.name), book.author ? h("span", "muted", ` · ${book.author}`) : null),
    h("div", "book-tools", prev, contents, next, position));
  body.replaceChildren(h("div", "book-view", head, frame));

  // The table of contents, or the chapters by number when the book has none.
  const items = book.toc.length ? book.toc : book.chapters.map((chapter, index) => ({ title: `Part ${index + 1}`, entry: chapter, level: 0 }));
  for (const item of items) {
    const option = h("option", "", `${"  ".repeat(Math.min(item.level, 4))}${item.title}`);
    option.value = item.entry;
    contents.append(option);
  }
  const key = `ownnas-book:${entry.path}`;
  let chapter = Math.min(Math.max(0, Number(remember(key)) || 0), book.chapters.length - 1);
  const show = (index, part = null) => {
    chapter = Math.min(Math.max(0, index), book.chapters.length - 1);
    frame.src = bookPartUrl(entry, part || book.chapters[chapter]);
    prev.disabled = chapter === 0;
    next.disabled = chapter === book.chapters.length - 1;
    position.textContent = `${chapter + 1} / ${book.chapters.length}`;
    // The contents entry for this chapter, if it has one.
    const match = items.findIndex((item) => item.entry.split("#")[0] === book.chapters[chapter]);
    if (match >= 0 && (!part || part.split("#")[0] !== contents.value.split("#")[0])) contents.selectedIndex = match;
    remember(key, chapter);
  };
  prev.addEventListener("click", () => show(chapter - 1));
  next.addEventListener("click", () => show(chapter + 1));
  contents.addEventListener("change", () => {
    const target = contents.value;
    const index = book.chapters.indexOf(target.split("#")[0]);
    show(index >= 0 ? index : chapter, target);
  });
  const release = captureArrows(body, (step) => show(chapter + step));
  show(chapter);
  return { destroy: release };
}

// ---- Comics (CBZ, CBR, CB7): their pages, one at a time ----

const PAGE_PICTURE = /\.(jpe?g|png|gif|webp|avif|bmp)$/i;

export async function mountComic(body, entry) {
  body.replaceChildren(h("p", "muted", "Opening the comic…"));
  const listing = await getJson(`/api/archive/list?path=${enc(entry.path)}`);
  const pages = listing.entries
    .filter((item) => !item.dir && PAGE_PICTURE.test(item.name) && !/(^|\/)(__MACOSX|\.)/.test(item.name))
    .map((item) => item.name)
    .sort((a, b) => a.localeCompare(b, undefined, { numeric: true, sensitivity: "base" }));
  if (!pages.length) throw new Error("This comic has no pages inside");
  const img = h("img", "comic-page");
  img.alt = "";
  const prev = h("button", "comic-step", "‹");
  const next = h("button", "comic-step", "›");
  prev.type = next.type = "button";
  prev.setAttribute("aria-label", "Previous page");
  next.setAttribute("aria-label", "Next page");
  const slider = h("input", "comic-slider");
  slider.type = "range";
  slider.min = "1";
  slider.max = String(pages.length);
  const counter = h("span", "comic-counter muted");
  const stage = h("div", "comic-stage", img);
  body.replaceChildren(h("div", "comic-view", stage, h("div", "comic-tools", prev, slider, next, counter)));

  const key = `ownnas-comic:${entry.path}`;
  let page = Math.min(Math.max(0, Number(remember(key)) || 0), pages.length - 1);
  const url = (index) => `/api/archive/entry?path=${enc(entry.path)}&entry=${enc(pages[index])}`;
  const show = (index) => {
    page = Math.min(Math.max(0, index), pages.length - 1);
    img.src = url(page);
    slider.value = String(page + 1);
    counter.textContent = `${page + 1} / ${pages.length}`;
    prev.disabled = page === 0;
    next.disabled = page === pages.length - 1;
    remember(key, page);
    // The next page loads while this one is read.
    if (page + 1 < pages.length) new Image().src = url(page + 1);
  };
  prev.addEventListener("click", () => show(page - 1));
  next.addEventListener("click", () => show(page + 1));
  slider.addEventListener("input", () => show(Number(slider.value) - 1));
  // A click on the left of the page goes back, on the right forward.
  stage.addEventListener("click", (event) => {
    const box = stage.getBoundingClientRect();
    show(page + (event.clientX - box.left < box.width / 3 ? -1 : 1));
  });
  const release = captureArrows(body, (step) => show(page + step));
  show(page);
  return { destroy: release };
}

// ---- E-mail (.eml, .msg) ----

function formatWhen(seconds) {
  if (!seconds) return "";
  return new Date(seconds * 1000).toLocaleString(undefined, { dateStyle: "full", timeStyle: "short" });
}

function sizeLabel(bytes) {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) { value /= 1024; unit += 1; }
  return `${value.toFixed(value >= 10 ? 0 : 1)} ${units[unit]}`;
}

export async function mountMail(body, entry) {
  body.replaceChildren(h("p", "muted", "Opening the mail…"));
  const mail = await getJson(`/api/mail?path=${enc(entry.path)}`);
  const row = (label, value) => value ? h("div", "mail-row", h("span", "mail-label muted", label), h("span", "mail-value", value)) : null;
  const head = h("header", "mail-head",
    h("h3", "mail-subject", mail.subject || "(no subject)"),
    row("From", mail.from), row("To", mail.to), row("Cc", mail.cc), row("Date", formatWhen(mail.date)));
  const attachmentUrl = (index) => `/api/mail/attachment?path=${enc(entry.path)}&index=${index}`;
  const attachments = mail.attachments.length ? h("ul", "mail-attachments", mail.attachments.map((item) => {
    const open = h("a", "mail-attachment-name", item.name);
    open.href = attachmentUrl(item.index);
    open.target = "_blank";
    open.rel = "noopener";
    const save = h("a", "mail-attachment-save muted", "Download");
    save.href = `${attachmentUrl(item.index)}&download=1`;
    return h("li", "", open, h("span", "muted", sizeLabel(item.size)), save);
  })) : null;
  let content;
  const textView = () => h("pre", "mail-text", mail.text || "This mail has no text.");
  if (mail.hasHtml) {
    const frame = h("iframe", "mail-frame");
    frame.title = mail.subject || "Mail";
    // No scripts; links open in a new tab.
    frame.setAttribute("sandbox", "allow-same-origin allow-popups allow-popups-to-escape-sandbox");
    frame.src = `/api/mail/html?path=${enc(entry.path)}`;
    const holder = h("div", "mail-body", frame);
    const toggle = h("button", "mail-toggle", "Show as plain text");
    toggle.type = "button";
    let plain = false;
    toggle.addEventListener("click", () => {
      plain = !plain;
      holder.replaceChildren(plain ? textView() : frame);
      toggle.textContent = plain ? "Show as sent" : "Show as plain text";
    });
    content = [h("div", "mail-tools", toggle, h("span", "muted", "Pictures from the internet are not loaded.")), holder];
  } else {
    content = [h("div", "mail-body", textView())];
  }
  body.replaceChildren(h("article", "mail-view", head, attachments, ...content));
  return { destroy() {} };
}

// ---- iCalendar (.ics) and vCard (.vcf), read in the browser ----

/// The lines of an iCalendar or vCard file, folded lines joined, as { name, params, value }.
function contentLines(text) {
  const unfolded = text.replace(/\r\n/g, "\n").replace(/\n[ \t]/g, "");
  return unfolded.split("\n").filter(Boolean).map((line) => {
    // The name and parameters end at the first colon outside quotes.
    let quoted = false;
    let split = -1;
    for (let i = 0; i < line.length; i += 1) {
      if (line[i] === '"') quoted = !quoted;
      else if (line[i] === ":" && !quoted) { split = i; break; }
    }
    if (split < 0) return null;
    const [name, ...rawParams] = line.slice(0, split).split(";");
    const params = {};
    for (const param of rawParams) {
      const [key, value = ""] = param.split("=");
      params[key.toUpperCase()] = value.replace(/^"|"$/g, "");
    }
    return { name: name.toUpperCase().replace(/^ITEM\d+\./, ""), params, value: line.slice(split + 1) };
  }).filter(Boolean);
}

function unescapeValue(value) {
  return value.replace(/\\n/gi, "\n").replace(/\\([,;\\])/g, "$1");
}

/// "20240715T183000Z", "20240715T183000", or "20240715" as a Date, and whether it has a time.
function icsDate(value) {
  const match = /^(\d{4})(\d{2})(\d{2})(?:T(\d{2})(\d{2})(\d{2})?(Z)?)?/.exec(value || "");
  if (!match) return null;
  const [, y, mo, d, hh, mm, ss, utc] = match;
  if (!hh) return { date: new Date(Number(y), Number(mo) - 1, Number(d)), allDay: true };
  const parts = [Number(y), Number(mo) - 1, Number(d), Number(hh), Number(mm), Number(ss || 0)];
  return { date: utc ? new Date(Date.UTC(...parts)) : new Date(...parts), allDay: false };
}

export async function mountCalendar(body, entry) {
  body.replaceChildren(h("p", "muted", "Reading the calendar…"));
  const response = await fetch(`/api/raw?path=${enc(entry.path)}`, { credentials: "same-origin" });
  if (!response.ok) throw new Error("Could not read the calendar");
  const lines = contentLines(await response.text());
  const events = [];
  let current = null;
  let calendarName = "";
  for (const line of lines) {
    if (line.name === "BEGIN" && (line.value === "VEVENT" || line.value === "VTODO")) current = { kind: line.value };
    else if (line.name === "END" && current && (line.value === "VEVENT" || line.value === "VTODO")) { events.push(current); current = null; }
    else if (!current && line.name === "X-WR-CALNAME") calendarName = unescapeValue(line.value);
    else if (current) {
      if (line.name === "SUMMARY") current.summary = unescapeValue(line.value);
      if (line.name === "LOCATION") current.location = unescapeValue(line.value);
      if (line.name === "DESCRIPTION") current.description = unescapeValue(line.value);
      if (line.name === "DTSTART" || line.name === "DUE") current.start = icsDate(line.value);
      if (line.name === "DTEND") current.end = icsDate(line.value);
      if (line.name === "RRULE") current.repeats = line.value;
      if (line.name === "STATUS") current.status = line.value;
    }
  }
  events.sort((a, b) => (a.start?.date ?? 0) - (b.start?.date ?? 0));
  const when = (event) => {
    if (!event.start) return "No date";
    const day = { dateStyle: "medium" };
    const time = { timeStyle: "short" };
    const start = event.start.allDay ? event.start.date.toLocaleDateString(undefined, day) : event.start.date.toLocaleString(undefined, { ...day, ...time });
    if (!event.end) return start;
    const sameDay = event.end.date.toDateString() === event.start.date.toDateString();
    if (event.start.allDay) return start;
    return `${start} – ${sameDay ? event.end.date.toLocaleTimeString(undefined, time) : event.end.date.toLocaleString(undefined, { ...day, ...time })}`;
  };
  const repeatLabel = (rule) => {
    const freq = /FREQ=(\w+)/.exec(rule)?.[1];
    return { DAILY: "Every day", WEEKLY: "Every week", MONTHLY: "Every month", YEARLY: "Every year" }[freq] || "Repeats";
  };
  let lastMonth = "";
  const items = [];
  for (const event of events) {
    const month = event.start ? event.start.date.toLocaleDateString(undefined, { month: "long", year: "numeric" }) : "";
    if (month !== lastMonth) { items.push(h("h4", "cal-month", month || "Undated")); lastMonth = month; }
    items.push(h("div", `cal-event${event.status === "CANCELLED" ? " cal-cancelled" : ""}`,
      h("div", "cal-when muted", when(event), event.repeats ? ` · ${repeatLabel(event.repeats)}` : "", event.kind === "VTODO" ? " · To do" : ""),
      h("div", "cal-summary", event.summary || "(no title)"),
      event.location ? h("div", "cal-location muted", event.location) : null,
      event.description ? h("details", "cal-description", h("summary", "", "Details"), h("p", "", event.description)) : null));
  }
  body.replaceChildren(h("div", "cal-view",
    h("p", "muted", `${calendarName ? `${calendarName} · ` : ""}${events.length.toLocaleString()} event${events.length === 1 ? "" : "s"}`),
    items.length ? items : h("p", "muted", "This calendar has no events.")));
  return { destroy() {} };
}

export async function mountContacts(body, entry) {
  body.replaceChildren(h("p", "muted", "Reading the contacts…"));
  const response = await fetch(`/api/raw?path=${enc(entry.path)}`, { credentials: "same-origin" });
  if (!response.ok) throw new Error("Could not read the contacts");
  const cards = [];
  let card = null;
  for (const line of contentLines(await response.text())) {
    if (line.name === "BEGIN" && line.value.toUpperCase() === "VCARD") card = { phones: [], emails: [], addresses: [], urls: [] };
    else if (line.name === "END" && card) { cards.push(card); card = null; }
    else if (card) {
      const value = unescapeValue(line.value);
      const type = (line.params.TYPE || "").toLowerCase().split(",").filter((t) => !["pref", "voice", "internet"].includes(t)).join(", ");
      if (line.name === "FN") card.name = value;
      if (line.name === "N" && !card.name) card.name = value.split(";").filter(Boolean).reverse().join(" ");
      if (line.name === "ORG") card.org = value.split(";").filter(Boolean).join(", ");
      if (line.name === "TITLE") card.title = value;
      if (line.name === "TEL") card.phones.push({ value, type });
      if (line.name === "EMAIL") card.emails.push({ value, type });
      if (line.name === "ADR") card.addresses.push({ value: value.split(";").filter(Boolean).join(", "), type });
      if (line.name === "URL") card.urls.push({ value, type });
      if (line.name === "BDAY") card.birthday = value;
      if (line.name === "NOTE") card.note = value;
    }
  }
  cards.sort((a, b) => (a.name || "").localeCompare(b.name || "", undefined, { sensitivity: "base" }));
  const search = h("input", "contacts-search");
  search.type = "search";
  search.placeholder = `Search ${cards.length.toLocaleString()} contact${cards.length === 1 ? "" : "s"}`;
  const list = h("div", "contacts-list");
  const line = (label, items) => items.map((item) => h("div", "contact-line", h("span", "muted", item.type || label), " ", item.value));
  const render = () => {
    const query = search.value.trim().toLowerCase();
    const shown = cards.filter((c) => !query || JSON.stringify(c).toLowerCase().includes(query));
    list.replaceChildren(...shown.slice(0, 500).map((c) => h("div", "contact-card",
      h("div", "contact-avatar", (c.name || "?").trim().charAt(0).toUpperCase()),
      h("div", "contact-text",
        h("strong", "", c.name || "(no name)"),
        c.org || c.title ? h("div", "muted", [c.title, c.org].filter(Boolean).join(", ")) : null,
        line("phone", c.phones), line("email", c.emails), line("address", c.addresses), line("web", c.urls),
        c.birthday ? h("div", "contact-line", h("span", "muted", "birthday"), " ", c.birthday) : null,
        c.note ? h("div", "contact-line muted", c.note) : null))));
    if (shown.length > 500) list.append(h("p", "muted", `Showing 500 of ${shown.length.toLocaleString()}. Search to find the others.`));
    if (!shown.length) list.append(h("p", "muted", "No contacts match."));
  };
  search.addEventListener("input", render);
  render();
  body.replaceChildren(h("div", "contacts-view", cards.length > 6 ? search : h("p", "muted", `${cards.length} contact${cards.length === 1 ? "" : "s"}`), list));
  return { destroy() {} };
}

// ---- Jupyter notebooks (.ipynb) ----

/// Terminal colour codes left in error tracebacks.
const ANSI = /\u001b\[[0-9;]*[A-Za-z]/g;

export async function mountNotebook(body, entry, { renderMarkdown } = {}) {
  body.replaceChildren(h("p", "muted", "Opening the notebook…"));
  if (entry.size > 50 * 1024 * 1024) throw new Error("This notebook is too large to preview");
  const response = await fetch(`/api/raw?path=${enc(entry.path)}`, { credentials: "same-origin" });
  if (!response.ok) throw new Error("Could not read the notebook");
  let notebook;
  try { notebook = JSON.parse(await response.text()); } catch { throw new Error("This notebook is not valid JSON"); }
  const urls = [];
  const text = (value) => Array.isArray(value) ? value.join("") : String(value ?? "");
  const language = notebook.metadata?.kernelspec?.display_name || notebook.metadata?.language_info?.name || "";
  const picture = (data, type) => {
    const bytes = Uint8Array.from(atob(text(data).replace(/\s/g, "")), (c) => c.charCodeAt(0));
    const url = URL.createObjectURL(new Blob([bytes], { type }));
    urls.push(url);
    const img = h("img", "nb-image");
    img.alt = "";
    img.src = url;
    return img;
  };
  const output = (out) => {
    if (out.output_type === "stream") return h("pre", `nb-output${out.name === "stderr" ? " nb-stderr" : ""}`, text(out.text));
    if (out.output_type === "error") return h("pre", "nb-output nb-stderr", text(out.traceback).replace(ANSI, "") || `${out.ename}: ${out.evalue}`);
    const data = out.data || {};
    if (data["image/png"]) return picture(data["image/png"], "image/png");
    if (data["image/jpeg"]) return picture(data["image/jpeg"], "image/jpeg");
    if (data["text/markdown"] && renderMarkdown) {
      const div = h("div", "nb-markdown");
      div.innerHTML = renderMarkdown(text(data["text/markdown"]));
      return div;
    }
    if (data["text/plain"]) return h("pre", "nb-output", text(data["text/plain"]));
    if (data["text/html"]) return h("pre", "nb-output muted", "(HTML output, not shown)");
    return null;
  };
  const cells = (notebook.cells || []).map((cell) => {
    const source = text(cell.source);
    if (cell.cell_type === "markdown") {
      const div = h("div", "nb-cell nb-markdown");
      if (renderMarkdown) div.innerHTML = renderMarkdown(source);
      else div.textContent = source;
      return div;
    }
    if (cell.cell_type === "code") {
      const count = cell.execution_count != null ? `[${cell.execution_count}]` : "[ ]";
      return h("div", "nb-cell nb-code",
        h("div", "nb-prompt muted", count),
        h("div", "nb-io", h("pre", "nb-source", source), (cell.outputs || []).map(output)));
    }
    return h("pre", "nb-cell nb-raw", source);
  });
  body.replaceChildren(h("div", "nb-view",
    h("p", "muted", `${(notebook.cells || []).length} cells${language ? ` · ${language}` : ""}`),
    cells));
  return { destroy() { for (const url of urls) URL.revokeObjectURL(url); } };
}

// ---- Fonts (.ttf, .otf, .woff, .woff2) ----

let fontCount = 0;

export async function mountFont(body, entry) {
  body.replaceChildren(h("p", "muted", "Loading the font…"));
  fontCount += 1;
  const family = `ownnas-preview-font-${fontCount}`;
  const face = new FontFace(family, `url(/api/raw?path=${enc(entry.path)}&v=${entry.modified || 0})`);
  await face.load().catch(() => { throw new Error("This font could not be loaded by the browser"); });
  document.fonts.add(face);
  const info = /\.(ttf|otf)$/i.test(entry.name) ? await getJson(`/api/font/info?path=${enc(entry.path)}`).catch(() => null) : null;
  const sample = h("input", "font-sample-input");
  sample.type = "text";
  sample.value = "The quick brown fox jumps over the lazy dog";
  sample.setAttribute("aria-label", "Sample text");
  const lines = h("div", "font-lines");
  const paint = () => {
    lines.replaceChildren(...[64, 40, 28, 20, 15, 12].map((size) => {
      const line = h("div", "font-line", h("span", "font-size muted", `${size}`), h("span", "font-text", sample.value || " "));
      const shown = line.querySelector(".font-text");
      shown.style.fontFamily = `"${family}"`;
      shown.style.fontSize = `${size}px`;
      return line;
    }));
  };
  sample.addEventListener("input", paint);
  paint();
  const charset = h("div", "font-charset", "ABCDEFGHIJKLMNOPQRSTUVWXYZ\nabcdefghijklmnopqrstuvwxyz\n0123456789 .,;:!?¿¡ & @ # $ € £ % ( ) [ ] { } “ ” ‘ ’ – —\nÁÉÍÓÚ áéíóú Ññ Çç Ää Öö Üü ß Ąą Ęę Łł Śś Żż");
  charset.style.fontFamily = `"${family}"`;
  const facts = info ? h("dl", "font-facts", [
    ["Family", info.family], ["Style", info.style], ["Version", info.version], ["Designer", info.designer],
    ["Characters", info.glyphs ? `${info.glyphs.toLocaleString()} glyphs` : ""],
    ["Kind", [info.monospaced ? "Monospaced" : "", info.variable ? "Variable" : ""].filter(Boolean).join(", ")],
    ["Copyright", info.copyright],
  ].filter(([, value]) => value).map(([label, value]) => [h("dt", "muted", label), h("dd", "", value)])) : null;
  body.replaceChildren(h("div", "font-view", h("h3", "font-title", info?.fullName || info?.family || entry.name), sample, lines, charset, facts));
  return { destroy() { document.fonts.delete(face); } };
}

// ---- GPS tracks (GPX, KML, KMZ, GeoJSON) on a map ----

/// Lines (lists of [lat, lon, elevation?, time?]) and named points from a track file.
function parseTrack(name, text) {
  const lines = [];
  const points = [];
  const lower = name.toLowerCase();
  if (lower.endsWith(".geojson") || lower.endsWith(".json")) {
    const data = JSON.parse(text);
    const geometry = (g, props = {}) => {
      if (!g) return;
      const ll = (c) => [c[1], c[0], c[2] ?? null, null];
      if (g.type === "LineString") lines.push(g.coordinates.map(ll));
      if (g.type === "MultiLineString") for (const part of g.coordinates) lines.push(part.map(ll));
      if (g.type === "Polygon") lines.push(g.coordinates[0].map(ll));
      if (g.type === "MultiPolygon") for (const poly of g.coordinates) lines.push(poly[0].map(ll));
      if (g.type === "Point") points.push({ lat: g.coordinates[1], lon: g.coordinates[0], name: props.name || props.title || "" });
      if (g.type === "MultiPoint") for (const c of g.coordinates) points.push({ lat: c[1], lon: c[0], name: props.name || "" });
      if (g.type === "GeometryCollection") for (const part of g.geometries) geometry(part, props);
    };
    const features = data.type === "FeatureCollection" ? data.features : data.type === "Feature" ? [data] : [{ geometry: data }];
    for (const feature of features) geometry(feature.geometry, feature.properties || {});
    return { lines, points };
  }
  const xml = new DOMParser().parseFromString(text, "application/xml");
  if (xml.querySelector("parsererror")) throw new Error("This track file could not be read");
  const local = (node, tag) => [...node.getElementsByTagName("*")].filter((n) => n.localName === tag);
  const childText = (node, tag) => [...node.children].find((n) => n.localName === tag)?.textContent?.trim() || "";
  if (lower.endsWith(".gpx")) {
    for (const segment of [...local(xml, "trkseg"), ...local(xml, "rte")]) {
      const pts = [...segment.children].filter((n) => n.localName === "trkpt" || n.localName === "rtept");
      lines.push(pts.map((p) => [Number(p.getAttribute("lat")), Number(p.getAttribute("lon")), childText(p, "ele") ? Number(childText(p, "ele")) : null, childText(p, "time") ? Date.parse(childText(p, "time")) : null]));
    }
    for (const w of local(xml, "wpt")) points.push({ lat: Number(w.getAttribute("lat")), lon: Number(w.getAttribute("lon")), name: childText(w, "name") });
    return { lines, points };
  }
  // KML: lines, points, and Google's timed tracks.
  const coords = (text) => text.trim().split(/\s+/).map((triple) => triple.split(",").map(Number)).filter((c) => c.length >= 2 && !c.some(Number.isNaN)).map((c) => [c[1], c[0], c[2] ?? null, null]);
  for (const placemark of local(xml, "Placemark")) {
    const label = childText(placemark, "name");
    for (const line of [...local(placemark, "LineString"), ...local(placemark, "LinearRing")]) lines.push(coords(local(line, "coordinates")[0]?.textContent || ""));
    for (const point of local(placemark, "Point")) {
      const [c] = coords(local(point, "coordinates")[0]?.textContent || "");
      if (c) points.push({ lat: c[0], lon: c[1], name: label });
    }
    for (const track of local(placemark, "Track")) {
      const when = local(track, "when").map((n) => Date.parse(n.textContent));
      lines.push(local(track, "coord").map((n, i) => {
        const [lon, lat, ele] = n.textContent.trim().split(/\s+/).map(Number);
        return [lat, lon, ele ?? null, when[i] ?? null];
      }));
    }
  }
  return { lines: lines.filter((l) => l.length), points };
}

function haversine(a, b) {
  const rad = Math.PI / 180;
  const dLat = (b[0] - a[0]) * rad;
  const dLon = (b[1] - a[1]) * rad;
  const s = Math.sin(dLat / 2) ** 2 + Math.cos(a[0] * rad) * Math.cos(b[0] * rad) * Math.sin(dLon / 2) ** 2;
  return 2 * 6371000 * Math.asin(Math.min(1, Math.sqrt(s)));
}

function trackStats(lines) {
  let distance = 0;
  let climb = 0;
  let descent = 0;
  let low = Infinity;
  let high = -Infinity;
  let first = null;
  let last = null;
  const profile = [];
  for (const line of lines) {
    for (let i = 0; i < line.length; i += 1) {
      const p = line[i];
      if (i > 0) {
        distance += haversine(line[i - 1], p);
        if (p[2] != null && line[i - 1][2] != null) {
          const step = p[2] - line[i - 1][2];
          if (step > 0) climb += step; else descent -= step;
        }
      }
      if (p[2] != null) {
        low = Math.min(low, p[2]);
        high = Math.max(high, p[2]);
        profile.push([distance, p[2]]);
      }
      if (p[3] != null && !Number.isNaN(p[3])) {
        if (first == null || p[3] < first) first = p[3];
        if (last == null || p[3] > last) last = p[3];
      }
    }
  }
  return { distance, climb, descent, low, high, first, last, profile };
}

const TILE = 256;
const project = (lat, lon, zoom) => {
  const scale = TILE * 2 ** zoom;
  const sin = Math.sin((Math.max(-85, Math.min(85, lat)) * Math.PI) / 180);
  return [((lon + 180) / 360) * scale, (0.5 - Math.log((1 + sin) / (1 - sin)) / (4 * Math.PI)) * scale];
};

export async function mountTrack(body, entry, { api } = {}) {
  body.replaceChildren(h("p", "muted", "Reading the track…"));
  let text;
  if (/\.kmz$/i.test(entry.name)) {
    const listing = await getJson(`/api/archive/list?path=${enc(entry.path)}`);
    const kml = listing.entries.find((e) => /\.kml$/i.test(e.name));
    if (!kml) throw new Error("This KMZ file has no map inside");
    const response = await fetch(`/api/archive/entry?path=${enc(entry.path)}&entry=${enc(kml.name)}`, { credentials: "same-origin" });
    text = await response.text();
  } else {
    if (entry.size > 60 * 1024 * 1024) throw new Error("This track is too large to draw");
    const response = await fetch(`/api/raw?path=${enc(entry.path)}`, { credentials: "same-origin" });
    if (!response.ok) throw new Error("Could not read the track");
    text = await response.text();
  }
  const { lines, points } = parseTrack(entry.name.replace(/\.kmz$/i, ".kml"), text);
  const all = [...lines.flat(), ...points.map((p) => [p.lat, p.lon])].filter((p) => Number.isFinite(p[0]) && Number.isFinite(p[1]));
  if (!all.length) throw new Error("This file has no places to show");

  const tiles = h("div", "map-tiles");
  const svgNs = "http://www.w3.org/2000/svg";
  const overlay = document.createElementNS(svgNs, "svg");
  overlay.setAttribute("class", "map-overlay");
  const zoomIn = h("button", "map-zoom", "+");
  const zoomOut = h("button", "map-zoom", "−");
  zoomIn.type = zoomOut.type = "button";
  const credit = h("div", "map-credit", "© OpenStreetMap contributors");
  const map = h("div", "map-box", tiles, overlay, h("div", "map-zooms", zoomIn, zoomOut), credit);

  const stats = trackStats(lines);
  const km = (m) => (m >= 1000 ? `${(m / 1000).toFixed(m >= 100000 ? 0 : 2)} km` : `${Math.round(m)} m`);
  const facts = [];
  if (stats.distance) facts.push(["Distance", km(stats.distance)]);
  if (stats.first != null && stats.last > stats.first) {
    const seconds = (stats.last - stats.first) / 1000;
    facts.push(["Time", clock(seconds)]);
    if (stats.distance) facts.push(["Average speed", `${((stats.distance / seconds) * 3.6).toFixed(1)} km/h`]);
    facts.push(["Started", new Date(stats.first).toLocaleString()]);
  }
  if (Number.isFinite(stats.low) && stats.high > stats.low) {
    facts.push(["Climb", `${Math.round(stats.climb)} m up, ${Math.round(stats.descent)} m down`]);
    facts.push(["Elevation", `${Math.round(stats.low)}–${Math.round(stats.high)} m`]);
  }
  if (points.length) facts.push(["Places", String(points.length)]);
  const factList = h("dl", "track-facts", facts.map(([k, v]) => [h("dt", "muted", k), h("dd", "", v)]));

  // The elevation along the way.
  let chart = null;
  if (stats.profile.length > 1 && stats.high > stats.low) {
    chart = document.createElementNS(svgNs, "svg");
    chart.setAttribute("class", "track-profile");
    chart.setAttribute("viewBox", "0 0 1000 120");
    chart.setAttribute("preserveAspectRatio", "none");
    const total = stats.profile[stats.profile.length - 1][0] || 1;
    const d = stats.profile.map(([x, e], i) => `${i ? "L" : "M"}${((x / total) * 1000).toFixed(1)},${(115 - ((e - stats.low) / (stats.high - stats.low)) * 105).toFixed(1)}`).join("");
    const area = document.createElementNS(svgNs, "path");
    area.setAttribute("d", `${d}L1000,120L0,120Z`);
    area.setAttribute("class", "track-profile-area");
    const line = document.createElementNS(svgNs, "path");
    line.setAttribute("d", d);
    line.setAttribute("class", "track-profile-line");
    chart.append(area, line);
  }
  body.replaceChildren(h("div", "track-view", map, factList, chart));

  // Fit the whole track in view.
  const lats = all.map((p) => p[0]);
  const lons = all.map((p) => p[1]);
  const box = () => map.getBoundingClientRect();
  let zoom = 16;
  let center = [0, 0];
  const fit = () => {
    const { width, height } = box();
    for (zoom = 17; zoom > 1; zoom -= 1) {
      const [x1, y1] = project(Math.max(...lats), Math.min(...lons), zoom);
      const [x2, y2] = project(Math.min(...lats), Math.max(...lons), zoom);
      if (x2 - x1 < width * 0.85 && y2 - y1 < height * 0.85) break;
    }
    const [x1, y1] = project(Math.max(...lats), Math.min(...lons), zoom);
    const [x2, y2] = project(Math.min(...lats), Math.max(...lons), zoom);
    center = [(x1 + x2) / 2, (y1 + y2) / 2];
  };
  const draw = () => {
    const { width, height } = box();
    if (!width) return;
    const left = center[0] - width / 2;
    const top = center[1] - height / 2;
    const count = 2 ** zoom;
    const wanted = new Map();
    for (let tx = Math.floor(left / TILE); tx <= Math.floor((left + width) / TILE); tx += 1) {
      for (let ty = Math.floor(top / TILE); ty <= Math.floor((top + height) / TILE); ty += 1) {
        if (ty < 0 || ty >= count) continue;
        const wrapped = ((tx % count) + count) % count;
        wanted.set(`${zoom}/${tx}/${ty}`, { x: tx * TILE - left, y: ty * TILE - top, src: `/api/map/tile/${zoom}/${wrapped}/${ty}.png` });
      }
    }
    for (const img of [...tiles.children]) if (!wanted.has(img.dataset.key)) img.remove();
    const have = new Set([...tiles.children].map((img) => img.dataset.key));
    for (const [key, tile] of wanted) {
      let img = tiles.querySelector(`[data-key="${key}"]`);
      if (!have.has(key)) {
        img = h("img", "map-tile");
        img.alt = "";
        img.dataset.key = key;
        img.src = tile.src;
        img.addEventListener("error", () => img.classList.add("map-tile-missing"), { once: true });
        tiles.append(img);
      }
      img.style.transform = `translate(${tile.x}px, ${tile.y}px)`;
    }
    overlay.setAttribute("viewBox", `0 0 ${width} ${height}`);
    overlay.replaceChildren();
    for (const line of lines) {
      const path = document.createElementNS(svgNs, "path");
      path.setAttribute("class", "map-line");
      path.setAttribute("d", line.map((p, i) => {
        const [x, y] = project(p[0], p[1], zoom);
        return `${i ? "L" : "M"}${(x - left).toFixed(1)},${(y - top).toFixed(1)}`;
      }).join(""));
      overlay.append(path);
    }
    const dot = (p, cls, label) => {
      const [x, y] = project(p[0], p[1], zoom);
      const circle = document.createElementNS(svgNs, "circle");
      circle.setAttribute("cx", (x - left).toFixed(1));
      circle.setAttribute("cy", (y - top).toFixed(1));
      circle.setAttribute("r", "6");
      circle.setAttribute("class", cls);
      if (label) {
        const title = document.createElementNS(svgNs, "title");
        title.textContent = label;
        circle.append(title);
      }
      overlay.append(circle);
    };
    if (lines.length) {
      dot(lines[0][0], "map-start", "Start");
      const lastLine = lines[lines.length - 1];
      dot(lastLine[lastLine.length - 1], "map-end", "End");
    }
    for (const p of points) dot([p.lat, p.lon], "map-point", p.name);
  };
  const zoomBy = (step, at = null) => {
    const next = Math.max(1, Math.min(19, zoom + step));
    if (next === zoom) return;
    const { width, height, left, top } = box();
    // Keep the point under the pointer (or the middle) where it is.
    const anchor = at ? [at[0] - left - width / 2, at[1] - top - height / 2] : [0, 0];
    const factor = 2 ** (next - zoom);
    center = [(center[0] + anchor[0]) * factor - anchor[0], (center[1] + anchor[1]) * factor - anchor[1]];
    zoom = next;
    draw();
  };
  zoomIn.addEventListener("click", () => zoomBy(1));
  zoomOut.addEventListener("click", () => zoomBy(-1));
  map.addEventListener("wheel", (event) => {
    event.preventDefault();
    zoomBy(event.deltaY < 0 ? 1 : -1, [event.clientX, event.clientY]);
  }, { passive: false });
  let drag = null;
  map.addEventListener("pointerdown", (event) => {
    if (event.target.closest("button")) return;
    drag = { x: event.clientX, y: event.clientY, center: [...center] };
    map.setPointerCapture(event.pointerId);
  });
  map.addEventListener("pointermove", (event) => {
    if (!drag) return;
    center = [drag.center[0] - (event.clientX - drag.x), drag.center[1] - (event.clientY - drag.y)];
    draw();
  });
  map.addEventListener("pointerup", () => { drag = null; });
  map.addEventListener("dblclick", (event) => zoomBy(1, [event.clientX, event.clientY]));
  const resize = new ResizeObserver(() => draw());
  resize.observe(map);
  fit();
  draw();
  return { destroy() { resize.disconnect(); } };
}
