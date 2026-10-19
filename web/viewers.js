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
  const lyrics = h("div", "audio-lyrics");
  lyrics.hidden = true;
  body.replaceChildren(h("div", "audio-view", h("div", "audio-head", cover, h("div", "audio-text", title, artist, album, details)), wave, audio, lyrics));
  let timedLines = null;
  const followLyrics = () => {
    if (!timedLines) return;
    const now = audio.currentTime;
    let current = -1;
    timedLines.forEach((line, i) => { if (line.time <= now + 0.15) current = i; });
    timedLines.forEach((line, i) => line.node.classList.toggle("is-now", i === current));
    const node = timedLines[current]?.node;
    if (node && node.dataset.shown !== "1") {
      for (const line of timedLines) line.node.dataset.shown = "";
      node.dataset.shown = "1";
      node.scrollIntoView({ block: "center", behavior: "smooth" });
    }
  };
  audio.addEventListener("timeupdate", followLyrics);

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
    if (info.lrc) {
      fetch(`/api/raw?path=${enc(info.lrc)}`, { credentials: "same-origin" }).then((r) => (r.ok ? r.text() : "")).then((text) => {
        const lines = parseLrc(text);
        if (!lines.length || stopped) return;
        timedLines = lines.map((line) => {
          const node = h("p", "lyric-line", line.text || "♪");
          node.addEventListener("click", () => { audio.currentTime = line.time; audio.play().catch(() => {}); });
          return { time: line.time, node };
        });
        lyrics.replaceChildren(h("h4", "muted", "Lyrics"), ...timedLines.map((l) => l.node));
        lyrics.hidden = false;
      }).catch(() => {});
    } else if (info.lyrics) {
      lyrics.replaceChildren(h("h4", "muted", "Lyrics"), h("p", "lyric-text", info.lyrics));
      lyrics.hidden = false;
    }
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
    if (!info) return;
    // Subtitle files beside the video, then the tracks inside it.
    const sources = [
      ...(info.subtitles || []).map((s) => ({ label: s.label, language: s.language, url: `/api/subtitles?path=${enc(s.path)}` })),
      ...((info.streams && info.streams.subtitles) || []).map((s) => ({ label: s.label, language: s.language, url: `/api/subtitles?path=${enc(entry.path)}&track=${s.track}` })),
    ];
    sources.forEach((source, index) => {
      const track = document.createElement("track");
      track.kind = "subtitles";
      track.label = source.label;
      if (source.language && /^[a-z]{2,3}$/i.test(source.language)) track.srclang = source.language;
      track.src = `${source.url}${shift ? `&shift=${shift}` : ""}`;
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
  // Pictures found inside older Word and PowerPoint files are numbered.
  if (part.startsWith("#picture/")) return `/api/document/picture?path=${enc(entry.path)}&index=${part.slice(9)}`;
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
    // Data files name their columns in the first row; spreadsheets get A, B, C.
    const names = sheet.header ? sheet.rows[0] || [] : Array.from({ length: columns }, (_, i) => columnName(i));
    const body = sheet.header ? sheet.rows.slice(1) : sheet.rows;
    const head = h("tr", "", h("th", "sheet-corner", ""), Array.from({ length: columns }, (_, i) => h("th", "", names[i] ?? "")));
    const rows = body.map((row, r) => h("tr", "", h("th", "", String(r + 1)), Array.from({ length: columns }, (_, c) => {
      const value = row[c] ?? "";
      return h("td", /^-?[\d.,]+%?$/.test(value) ? "sheet-number" : "", value);
    })));
    const cut = sheet.totalRows > sheet.rows.length || sheet.totalColumns > columns;
    const shownRows = body.length;
    const allRows = sheet.totalRows - (sheet.header ? 1 : 0);
    holder.replaceChildren(...[
      body.length || sheet.header ? h("div", "sheet-scroll", h("table", "sheet-table", h("thead", "", head), h("tbody", "", rows))) : h("p", "muted", "This sheet is empty."),
      cut ? h("p", "muted sheet-note", `Showing ${shownRows.toLocaleString()} of ${allRows.toLocaleString()} rows and ${columns} of ${sheet.totalColumns} columns. Download the file to see all of it.`) : null,
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
  return mountPager(body, `ownnas-comic:${entry.path}`, pages.length, (index) => `/api/archive/entry?path=${enc(entry.path)}&entry=${enc(pages[index])}`);
}

/// DjVu books: each page drawn by the server, at the width it shows.
export async function mountDjvu(body, entry) {
  body.replaceChildren(h("p", "muted", "Opening the book…"));
  const info = await getJson(`/api/djvu/info?path=${enc(entry.path)}`);
  if (!info.pages) throw new Error("This book has no pages");
  // Sharp enough for the expanded preview too.
  const width = Math.min(2000, Math.max(1200, Math.round((body.clientWidth || 800) * (window.devicePixelRatio || 1) * 1.5)));
  return mountPager(body, `ownnas-djvu:${entry.path}`, info.pages, (index) => `/api/djvu/page?path=${enc(entry.path)}&page=${index}&width=${width}`);
}

/// Pages one at a time: a slider, the arrow keys, clicks on either side, and the next one loaded ahead.
function mountPager(body, key, count, url) {
  const pages = { length: count };
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

  let page = Math.min(Math.max(0, Number(remember(key)) || 0), pages.length - 1);
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

export async function mountMail(body, entry, { message = null, onBack = null } = {}) {
  body.replaceChildren(h("p", "muted", "Opening the mail…"));
  // A message of a mailbox is named by its number.
  const which = message == null ? "" : `&message=${message}`;
  const mail = await getJson(`/api/mail?path=${enc(entry.path)}${which}`);
  const row = (label, value) => value ? h("div", "mail-row", h("span", "mail-label muted", label), h("span", "mail-value", value)) : null;
  const head = h("header", "mail-head",
    h("h3", "mail-subject", mail.subject || "(no subject)"),
    row("From", mail.from), row("To", mail.to), row("Cc", mail.cc), row("Date", formatWhen(mail.date)));
  const attachmentUrl = (index) => `/api/mail/attachment?path=${enc(entry.path)}&index=${index}${which}`;
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
    frame.src = `/api/mail/html?path=${enc(entry.path)}${which}`;
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
  let back = null;
  if (onBack) {
    back = h("button", "mail-back", "‹ Mailbox");
    back.type = "button";
    back.addEventListener("click", onBack);
  }
  body.replaceChildren(h("article", "mail-view", back, head, attachments, ...content));
  return { destroy() {} };
}

// ---- Mailboxes (.mbox): the messages, newest last as stored, each opening as a mail ----

export async function mountMailbox(body, entry) {
  body.replaceChildren(h("p", "muted", "Reading the mailbox (a large one takes a moment)…"));
  const loaded = [];
  let total = 0;
  const page = async () => {
    const data = await getJson(`/api/mbox?path=${enc(entry.path)}&from=${loaded.length}&count=200`);
    total = data.total;
    loaded.push(...data.messages);
  };
  await page();
  const search = h("input", "mailbox-search");
  search.type = "search";
  search.placeholder = "Search loaded messages";
  const list = h("ul", "mailbox-list");
  const more = h("button", "mailbox-more", "Load more");
  more.type = "button";
  const count = h("p", "muted mailbox-count");
  const view = h("div", "mailbox-view", count, search, list, more);
  let scroll = 0;
  const showList = () => {
    body.replaceChildren(view);
    view.scrollTop = scroll;
  };
  const render = () => {
    const query = search.value.trim().toLowerCase();
    const shown = loaded.filter((m) => !query || `${m.from || ""} ${m.subject || ""}`.toLowerCase().includes(query));
    list.replaceChildren(...shown.map((m) => {
      const item = h("li", "mailbox-item",
        h("div", "mailbox-line", h("strong", "", m.from || "(unknown sender)"), h("span", "muted", m.date ? new Date(m.date * 1000).toLocaleDateString() : "")),
        h("div", "mailbox-subject", m.subject || "(no subject)", m.attachments ? h("span", "muted", " · attachments") : null));
      item.tabIndex = 0;
      const open = () => {
        scroll = view.scrollTop;
        mountMail(body, entry, { message: m.index, onBack: showList }).catch((err) => body.replaceChildren(h("p", "muted", err.message)));
      };
      item.addEventListener("click", open);
      item.addEventListener("keydown", (event) => { if (event.key === "Enter") open(); });
      return item;
    }));
    count.textContent = `${total.toLocaleString()} message${total === 1 ? "" : "s"}${loaded.length < total ? `, ${loaded.length.toLocaleString()} loaded` : ""}`;
    more.hidden = loaded.length >= total;
  };
  search.addEventListener("input", render);
  more.addEventListener("click", async () => {
    more.disabled = true;
    try { await page(); render(); } finally { more.disabled = false; }
  });
  render();
  showList();
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

  const svgNs = "http://www.w3.org/2000/svg";
  const dot = (overlay, [x, y], cls, label) => {
    const circle = document.createElementNS(svgNs, "circle");
    circle.setAttribute("cx", x.toFixed(1));
    circle.setAttribute("cy", y.toFixed(1));
    circle.setAttribute("r", "6");
    circle.setAttribute("class", cls);
    if (label) {
      const title = document.createElementNS(svgNs, "title");
      title.textContent = label;
      circle.append(title);
    }
    overlay.append(circle);
  };
  const view = createMap({
    paint({ overlay, at }) {
      for (const line of lines) {
        const path = document.createElementNS(svgNs, "path");
        path.setAttribute("class", "map-line");
        path.setAttribute("d", line.map((p, i) => {
          const [x, y] = at(p[0], p[1]);
          return `${i ? "L" : "M"}${x.toFixed(1)},${y.toFixed(1)}`;
        }).join(""));
        overlay.append(path);
      }
      if (lines.length) {
        dot(overlay, at(...lines[0][0]), "map-start", "Start");
        const lastLine = lines[lines.length - 1];
        dot(overlay, at(...lastLine[lastLine.length - 1]), "map-end", "End");
      }
      for (const p of points) dot(overlay, at(p.lat, p.lon), "map-point", p.name);
    },
  });
  const map = view.element;

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
  view.fit({ north: Math.max(...lats), south: Math.min(...lats), west: Math.min(...lons), east: Math.max(...lons) });
  return { destroy() { view.destroy(); } };
}

/// A pannable, zoomable OpenStreetMap, its tiles fetched through the server. After each move,
/// `paint` draws over it: an SVG `overlay` and a `layer` for HTML markers, with `at(lat, lon)`
/// giving a place's position on screen. `moved` hears where the map settled, after a pause.
export function createMap({ paint = () => {}, moved = null, maxZoom = 19 } = {}) {
  const svgNs = "http://www.w3.org/2000/svg";
  const tiles = h("div", "map-tiles");
  const overlay = document.createElementNS(svgNs, "svg");
  overlay.setAttribute("class", "map-overlay");
  const layer = h("div", "map-markers");
  const zoomIn = h("button", "map-zoom", "+");
  const zoomOut = h("button", "map-zoom", "−");
  zoomIn.type = zoomOut.type = "button";
  zoomIn.setAttribute("aria-label", "Zoom in");
  zoomOut.setAttribute("aria-label", "Zoom out");
  const credit = h("div", "map-credit", "© OpenStreetMap contributors");
  const map = h("div", "map-box", tiles, overlay, layer, h("div", "map-zooms", zoomIn, zoomOut), credit);
  const box = () => map.getBoundingClientRect();
  let zoom = 2;
  let center = project(20, 0, zoom);
  let pending = null; // a fit asked for before the map had a size
  let settle = 0;
  const unproject = (x, y, z) => {
    const scale = TILE * 2 ** z;
    const lon = (x / scale) * 360 - 180;
    const n = Math.PI - (2 * Math.PI * y) / scale;
    return [(180 / Math.PI) * Math.atan(Math.sinh(n)), lon];
  };
  /// What the map shows: its zoom and box.
  const current = () => {
    const { width, height } = box();
    const [north, west] = unproject(center[0] - width / 2, center[1] - height / 2, zoom);
    const [south, east] = unproject(center[0] + width / 2, center[1] + height / 2, zoom);
    return { zoom, north: Math.min(85, north), south: Math.max(-85, south), west: width >= TILE * 2 ** zoom ? -180 : ((west + 540) % 360) - 180, east: width >= TILE * 2 ** zoom ? 180 : ((east + 540) % 360) - 180 };
  };
  const draw = () => {
    const { width, height } = box();
    if (!width) return;
    if (pending) {
      const bounds = pending;
      pending = null;
      fit(bounds);
      return;
    }
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
    layer.replaceChildren();
    // The copy of a place nearest the middle, so markers follow the map round the world.
    const world = TILE * count;
    const at = (lat, lon) => {
      const [x, y] = project(lat, lon, zoom);
      const shifted = x + Math.round((center[0] - x) / world) * world;
      return [shifted - left, y - top];
    };
    paint({ overlay, layer, at, zoom, width, height });
    if (moved) {
      clearTimeout(settle);
      settle = setTimeout(() => moved(current()), 250);
    }
  };
  /// Shows a box (north, south, west, east) as close up as it fits.
  const fit = (bounds) => {
    const { width, height } = box();
    if (!width) {
      pending = bounds;
      return;
    }
    for (zoom = Math.min(17, maxZoom); zoom > 1; zoom -= 1) {
      const [x1, y1] = project(bounds.north, bounds.west, zoom);
      const [x2, y2] = project(bounds.south, bounds.east, zoom);
      if (x2 - x1 < width * 0.85 && y2 - y1 < height * 0.85) break;
    }
    const [x1, y1] = project(bounds.north, bounds.west, zoom);
    const [x2, y2] = project(bounds.south, bounds.east, zoom);
    center = [(x1 + x2) / 2, (y1 + y2) / 2];
    draw();
  };
  const zoomBy = (step, at = null) => {
    const next = Math.max(1, Math.min(maxZoom, zoom + step));
    if (next === zoom) return;
    const { width, height, left, top } = box();
    // Keep the point under the pointer (or the middle) where it is.
    const anchor = at ? [at[0] - left - width / 2, at[1] - top - height / 2] : [0, 0];
    const factor = 2 ** (next - zoom);
    center = [(center[0] + anchor[0]) * factor - anchor[0], (center[1] + anchor[1]) * factor - anchor[1]];
    zoom = next;
    draw();
  };
  /// Centres a place at a zoom.
  const show = (lat, lon, z) => {
    zoom = Math.max(1, Math.min(maxZoom, z));
    center = project(lat, lon, zoom);
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
    if (event.target.closest("button, a, .map-marker")) return;
    drag = { x: event.clientX, y: event.clientY, center: [...center] };
    map.setPointerCapture(event.pointerId);
  });
  map.addEventListener("pointermove", (event) => {
    if (!drag) return;
    center = [drag.center[0] - (event.clientX - drag.x), drag.center[1] - (event.clientY - drag.y)];
    draw();
  });
  map.addEventListener("pointerup", () => { drag = null; });
  map.addEventListener("dblclick", (event) => {
    if (!event.target.closest(".map-marker")) zoomBy(1, [event.clientX, event.clientY]);
  });
  const resize = new ResizeObserver(() => draw());
  resize.observe(map);
  return {
    element: map,
    fit,
    show,
    redraw: draw,
    view: current,
    get zoom() { return zoom; },
    destroy() {
      resize.disconnect();
      clearTimeout(settle);
    },
  };
}


// ---- Lyrics (.lrc), playlists (.m3u, .pls, .xspf), and cue sheets (.cue) ----

/// Timed lyrics: "[01:02.50]words" lines (one line may carry several times), in order.
function parseLrc(text) {
  const lines = [];
  let offset = 0;
  for (const raw of text.replace(/\r/g, "").split("\n")) {
    const tag = /^\[offset:\s*([+-]?\d+)\]/i.exec(raw);
    if (tag) offset = Number(tag[1]) / 1000;
    const times = [...raw.matchAll(/\[(\d+):(\d+(?:[.:]\d+)?)\]/g)];
    if (!times.length) continue;
    const words = raw.replace(/\[[^\]]*\]/g, "").trim();
    for (const t of times) lines.push({ time: Number(t[1]) * 60 + Number(t[2].replace(":", ".")) - offset, text: words });
  }
  return lines.sort((a, b) => a.time - b.time);
}

async function fetchText(entry) {
  const response = await fetch(`/api/raw?path=${enc(entry.path)}`, { credentials: "same-origin" });
  if (!response.ok) throw new Error("Could not read the file");
  return response.text();
}

export async function mountLyrics(body, entry) {
  const lines = parseLrc(await fetchText(entry));
  body.replaceChildren(h("div", "subtitle-view", h("p", "muted", `${lines.length} timed lines`),
    h("ol", "subtitle-list", lines.map((l) => h("li", "", h("span", "subtitle-time muted", clock(l.time)), h("span", "subtitle-text", l.text || "♪"))))));
  return { destroy() {} };
}

/// A path in a playlist as a library path, when it is relative to the playlist's folder.
function libraryTarget(entry, location) {
  if (/^[a-z][a-z0-9+.-]*:\/\//i.test(location)) return { url: location };
  const clean = location.replace(/\\/g, "/");
  if (/^([a-z]:)?\//i.test(clean)) return { foreign: clean };
  const parts = entry.path.split("/").slice(0, -1);
  for (const piece of decodeURIComponent(clean).split("/")) {
    if (piece === "..") parts.pop();
    else if (piece && piece !== ".") parts.push(piece);
  }
  return { path: parts.join("/") };
}

function parsePlaylist(name, text) {
  const items = [];
  if (/\.pls$/i.test(name)) {
    const files = {};
    const titles = {};
    for (const line of text.split(/\r?\n/)) {
      const m = /^(File|Title)(\d+)=(.*)$/i.exec(line.trim());
      if (m) (m[1].toLowerCase() === "file" ? files : titles)[m[2]] = m[3];
    }
    for (const key of Object.keys(files).sort((a, b) => a - b)) items.push({ location: files[key], title: titles[key] || "" });
  } else if (/\.xspf$/i.test(name)) {
    const xml = new DOMParser().parseFromString(text, "application/xml");
    for (const track of [...xml.getElementsByTagName("track")]) {
      const get = (tag) => track.getElementsByTagName(tag)[0]?.textContent?.trim() || "";
      items.push({ location: get("location").replace(/^file:\/\//, ""), title: [get("creator"), get("title")].filter(Boolean).join(" – ") });
    }
  } else {
    let title = "";
    for (const line of text.split(/\r?\n/)) {
      const trimmed = line.trim();
      if (!trimmed) continue;
      if (trimmed.startsWith("#EXTINF:")) title = trimmed.split(",").slice(1).join(",").trim();
      else if (!trimmed.startsWith("#")) { items.push({ location: trimmed, title }); title = ""; }
    }
  }
  return items;
}

export async function mountPlaylist(body, entry, { openPath } = {}) {
  const items = parsePlaylist(entry.name, await fetchText(entry)).map((item) => ({ ...item, ...libraryTarget(entry, item.location) }));
  const audio = h("audio", "audio-player-element");
  audio.controls = true;
  const playable = items.filter((item) => item.path || item.url);
  let current = -1;
  const rows = items.map((item, index) => {
    const name = item.title || item.location.split(/[\\/]/).pop();
    const row = h("li", `playlist-item${item.foreign ? " muted" : ""}`, h("span", "playlist-number muted", String(index + 1)), h("span", "playlist-name", name));
    if (item.path && openPath) {
      const show = h("button", "playlist-open", "Open");
      show.type = "button";
      show.addEventListener("click", (event) => { event.stopPropagation(); openPath(item.path); });
      row.append(show);
    }
    if (item.foreign) row.title = `Not in the library: ${item.foreign}`;
    else row.addEventListener("click", () => play(playable.indexOf(item)));
    item.row = row;
    return row;
  });
  const play = (index) => {
    if (index < 0 || index >= playable.length) return;
    current = index;
    const item = playable[index];
    audio.src = item.path ? `/api/raw?path=${enc(item.path)}` : item.url;
    audio.play().catch(() => {});
    for (const i of items) i.row.classList.toggle("is-now", i === item);
  };
  audio.addEventListener("ended", () => play(current + 1));
  audio.addEventListener("error", () => { if (current >= 0) play(current + 1); });
  const playAll = h("button", "playlist-play", "▶ Play all");
  playAll.type = "button";
  playAll.disabled = !playable.length;
  playAll.addEventListener("click", () => play(0));
  const foreign = items.filter((i) => i.foreign).length;
  body.replaceChildren(h("div", "subtitle-view",
    h("p", "muted", `${items.length} item${items.length === 1 ? "" : "s"}${foreign ? `, ${foreign} outside the library` : ""}`),
    h("div", "playlist-tools", playAll), audio, h("ol", "playlist-list", rows)));
  return { destroy() { audio.pause(); audio.removeAttribute("src"); audio.load(); } };
}

/// A cue sheet: the tracks of a disc ripped into one file, which plays when it is beside it.
export async function mountCue(body, entry) {
  const text = await fetchText(entry);
  let file = "";
  let album = "";
  let performer = "";
  const tracks = [];
  for (const line of text.split(/\r?\n/)) {
    const t = line.trim();
    const quoted = (s) => (/"([^"]*)"/.exec(s) || [, s.split(/\s+/).slice(1).join(" ")])[1];
    if (/^FILE /i.test(t)) file = quoted(t.slice(5).replace(/\s+\w+$/, ""));
    else if (/^TRACK /i.test(t)) tracks.push({ number: Number(t.split(/\s+/)[1]), title: "", performer: "", start: 0 });
    else if (/^TITLE /i.test(t)) { if (tracks.length) tracks[tracks.length - 1].title = quoted(t); else album = quoted(t); }
    else if (/^PERFORMER /i.test(t)) { if (tracks.length) tracks[tracks.length - 1].performer = quoted(t); else performer = quoted(t); }
    else if (/^INDEX 01 /i.test(t) && tracks.length) {
      const [m, s, f] = t.split(/\s+/)[2].split(":").map(Number);
      tracks[tracks.length - 1].start = m * 60 + s + f / 75;
    }
  }
  const target = file ? libraryTarget(entry, file) : {};
  const audio = h("audio", "audio-player-element");
  audio.controls = true;
  if (target.path) audio.src = `/api/raw?path=${enc(target.path)}`;
  const rows = tracks.map((track) => {
    const row = h("li", "playlist-item", h("span", "playlist-number muted", String(track.number)),
      h("span", "playlist-name", [track.title || `Track ${track.number}`, track.performer && track.performer !== performer ? ` – ${track.performer}` : ""].join("")),
      h("span", "muted", clock(track.start)));
    row.addEventListener("click", () => { audio.currentTime = track.start; audio.play().catch(() => {}); });
    track.row = row;
    return row;
  });
  audio.addEventListener("timeupdate", () => {
    let now = null;
    for (const track of tracks) if (track.start <= audio.currentTime + 0.2) now = track;
    for (const track of tracks) track.row.classList.toggle("is-now", track === now);
  });
  body.replaceChildren(h("div", "subtitle-view",
    h("h3", "audio-title", album || entry.name), performer ? h("p", "audio-artist", performer) : null,
    target.path ? audio : h("p", "muted", file ? `The audio file “${file}” is not beside this cue sheet.` : "This cue sheet names no audio file."),
    h("ol", "playlist-list", rows)));
  return { destroy() { audio.pause(); audio.removeAttribute("src"); audio.load(); } };
}

// ---- Torrents, property lists, and shortcuts ----

/// Bencode (torrent files): returns the value and, for the "info" dictionary, its raw bytes.
function bdecode(bytes) {
  let at = 0;
  let infoSpan = null;
  const text = (start, end) => new TextDecoder().decode(bytes.subarray(start, end));
  const value = () => {
    const c = bytes[at];
    if (c === 0x69) { const end = bytes.indexOf(0x65, at); const n = Number(text(at + 1, end)); at = end + 1; return n; }
    if (c === 0x6c) { at += 1; const list = []; while (bytes[at] !== 0x65) list.push(value()); at += 1; return list; }
    if (c === 0x64) {
      at += 1;
      const dict = {};
      while (bytes[at] !== 0x65) {
        const key = new TextDecoder().decode(value());
        const start = at;
        dict[key] = value();
        if (key === "info" && infoSpan === null) infoSpan = [start, at];
      }
      at += 1;
      return dict;
    }
    const colon = bytes.indexOf(0x3a, at);
    const length = Number(text(at, colon));
    if (!Number.isFinite(length)) throw new Error("This torrent file is damaged");
    const raw = bytes.subarray(colon + 1, colon + 1 + length);
    at = colon + 1 + length;
    return raw;
  };
  return { value: value(), info: infoSpan && bytes.subarray(infoSpan[0], infoSpan[1]) };
}

export async function mountTorrent(body, entry) {
  const response = await fetch(`/api/raw?path=${enc(entry.path)}`, { credentials: "same-origin" });
  const bytes = new Uint8Array(await response.arrayBuffer());
  const { value, info: infoBytes } = bdecode(bytes);
  const str = (b) => (b instanceof Uint8Array ? new TextDecoder().decode(b) : b == null ? "" : String(b));
  const info = value.info || {};
  const files = info.files
    ? info.files.map((f) => ({ name: (f["path.utf-8"] || f.path).map(str).join("/"), size: f.length }))
    : [{ name: str(info["name.utf-8"] || info.name), size: info.length }];
  const total = files.reduce((sum, f) => sum + (f.size || 0), 0);
  let hash = "";
  if (infoBytes && crypto.subtle) {
    const digest = await crypto.subtle.digest("SHA-1", infoBytes);
    hash = [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
  }
  const trackers = [...new Set([str(value.announce), ...((value["announce-list"] || []).flat().map(str))].filter(Boolean))];
  const magnet = hash ? `magnet:?xt=urn:btih:${hash}&dn=${encodeURIComponent(str(info.name))}${trackers.slice(0, 5).map((t) => `&tr=${encodeURIComponent(t)}`).join("")}` : "";
  const facts = [
    ["Name", str(info.name)], ["Size", `${sizeLabel(total)} in ${files.length} file${files.length === 1 ? "" : "s"}`],
    ["Pieces", info["piece length"] ? `${sizeLabel(info["piece length"])} each` : ""],
    ["Created", value["creation date"] ? new Date(value["creation date"] * 1000).toLocaleString() : ""],
    ["Created by", str(value["created by"])], ["Comment", str(value.comment)], ["Private", info.private ? "Yes" : ""],
    ["Info hash", hash],
  ].filter(([, v]) => v);
  const copy = h("button", "", "Copy magnet link");
  copy.type = "button";
  copy.hidden = !magnet;
  copy.addEventListener("click", () => navigator.clipboard?.writeText(magnet).then(() => { copy.textContent = "Copied"; }).catch(() => {}));
  body.replaceChildren(h("div", "subtitle-view",
    h("dl", "font-facts", facts.map(([k, v]) => [h("dt", "muted", k), h("dd", "", v)])), copy,
    trackers.length ? h("details", "", h("summary", "", `${trackers.length} tracker${trackers.length === 1 ? "" : "s"}`), h("ul", "", trackers.map((t) => h("li", "", t)))) : null,
    h("ol", "subtitle-list", files.slice(0, 2000).map((f) => h("li", "", h("span", "subtitle-time muted", sizeLabel(f.size || 0)), h("span", "subtitle-text", f.name))))));
  return { destroy() {} };
}

/// A JSON-like value as a tree that opens level by level.
function tree(value, key = null, depth = 0) {
  const label = key == null ? null : h("span", "tree-key", `${key}: `);
  if (value && typeof value === "object") {
    const entries = Array.isArray(value) ? value.map((v, i) => [i, v]) : Object.entries(value);
    const details = h("details", "tree-node", h("summary", "", label, h("span", "muted", Array.isArray(value) ? `[${entries.length}]` : `{${entries.length}}`)),
      entries.map(([k, v]) => tree(v, k, depth + 1)));
    if (depth < 2) details.open = true;
    return details;
  }
  return h("div", "tree-leaf", label, h("span", typeof value === "string" ? "tree-string" : "tree-value", JSON.stringify(value)));
}

export async function mountPlist(body, entry) {
  const value = await getJson(`/api/plist?path=${enc(entry.path)}`);
  body.replaceChildren(h("div", "subtitle-view tree-view", tree(value)));
  return { destroy() {} };
}

/// Internet shortcuts (.url, .webloc) and desktop entries (.desktop): where they lead.
export async function mountShortcut(body, entry) {
  let target = "";
  let name = entry.name.replace(/\.[^.]+$/, "");
  let comment = "";
  if (/\.webloc$/i.test(entry.name)) {
    target = (await getJson(`/api/plist?path=${enc(entry.path)}`)).URL || "";
  } else {
    for (const line of (await fetchText(entry)).split(/\r?\n/)) {
      const [k, ...rest] = line.split("=");
      const v = rest.join("=").trim();
      if (/^url$/i.test(k.trim())) target = v;
      if (k.trim() === "Exec" && !target) target = v;
      if (k.trim() === "Name") name = v;
      if (k.trim() === "Comment") comment = v;
    }
  }
  const web = /^https?:\/\//i.test(target);
  const link = web ? h("a", "shortcut-link", target) : h("code", "shortcut-link", target || "(no target)");
  if (web) { link.href = target; link.target = "_blank"; link.rel = "noopener noreferrer"; }
  body.replaceChildren(h("div", "subtitle-view shortcut-view", h("h3", "audio-title", name), comment ? h("p", "muted", comment) : null,
    h("p", "muted", web ? "This shortcut opens:" : "This shortcut runs or opens:"), link));
  return { destroy() {} };
}

// ---- MIDI: read the notes and play them on a small synthesizer (WebAudio) ----

function parseMidi(bytes) {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const text = (at, n) => String.fromCharCode(...bytes.subarray(at, at + n));
  if (text(0, 4) !== "MThd") throw new Error("This is not a MIDI file");
  const tracksCount = view.getUint16(10);
  const division = view.getUint16(12);
  if (division & 0x8000) throw new Error("MIDI files timed in SMPTE frames are not played here");
  let at = 8 + view.getUint32(4);
  const events = [];
  const names = [];
  for (let track = 0; track < tracksCount && at + 8 <= bytes.length; track += 1) {
    if (text(at, 4) !== "MTrk") break;
    const end = at + 8 + view.getUint32(at + 4);
    let p = at + 8;
    let tick = 0;
    let status = 0;
    const varLen = () => { let n = 0; let b; do { b = bytes[p++]; n = (n << 7) | (b & 0x7f); } while (b & 0x80 && p < end); return n; };
    while (p < end) {
      tick += varLen();
      let byte = bytes[p];
      if (byte & 0x80) { status = byte; p += 1; } else if (!status) break;
      const kind = status & 0xf0;
      const channel = status & 0x0f;
      if (status === 0xff) {
        const type = bytes[p++];
        const length = varLen();
        if (type === 0x51) events.push({ tick, tempo: (bytes[p] << 16) | (bytes[p + 1] << 8) | bytes[p + 2] });
        if (type === 0x03 && length) names[track] = new TextDecoder().decode(bytes.subarray(p, p + length));
        p += length;
      } else if (status === 0xf0 || status === 0xf7) {
        p += varLen();
      } else if (kind === 0x90 || kind === 0x80) {
        const note = bytes[p];
        const velocity = bytes[p + 1];
        p += 2;
        events.push({ tick, on: kind === 0x90 && velocity > 0, note, velocity, channel, track });
      } else if (kind === 0xc0) {
        events.push({ tick, program: bytes[p], channel });
        p += 1;
      } else if (kind === 0xd0) {
        p += 1;
      } else {
        p += 2;
      }
    }
    at = end;
  }
  events.sort((a, b) => a.tick - b.tick);
  // Ticks to seconds, through the tempo changes.
  let tempo = 500000;
  let lastTick = 0;
  let seconds = 0;
  const notes = [];
  const open = new Map();
  const programs = new Array(16).fill(0);
  for (const event of events) {
    seconds += ((event.tick - lastTick) * tempo) / division / 1e6;
    lastTick = event.tick;
    if (event.tempo) tempo = event.tempo;
    else if (event.program !== undefined) programs[event.channel] = event.program;
    else if (event.note !== undefined) {
      const key = `${event.channel}:${event.note}`;
      if (event.on) {
        open.set(key, { start: seconds, note: event.note, velocity: event.velocity, channel: event.channel, program: programs[event.channel] });
      } else if (open.has(key)) {
        const started = open.get(key);
        open.delete(key);
        notes.push({ ...started, end: Math.max(seconds, started.start + 0.05) });
      }
    }
  }
  for (const started of open.values()) notes.push({ ...started, end: seconds + 0.5 });
  notes.sort((a, b) => a.start - b.start);
  return { notes, duration: notes.reduce((m, n) => Math.max(m, n.end), 0), names: names.filter(Boolean), channels: [...new Set(notes.map((n) => n.channel))] };
}

/// General MIDI instrument families, by program number / 8.
const GM_FAMILIES = ["Piano", "Chromatic percussion", "Organ", "Guitar", "Bass", "Strings", "Ensemble", "Brass", "Reed", "Pipe", "Synth lead", "Synth pad", "Synth effects", "Ethnic", "Percussive", "Sound effects"];

export async function mountMidi(body, entry) {
  const response = await fetch(`/api/raw?path=${enc(entry.path)}`, { credentials: "same-origin" });
  const song = parseMidi(new Uint8Array(await response.arrayBuffer()));
  const play = h("button", "midi-play", "▶ Play");
  play.type = "button";
  const seek = h("input", "video-seek");
  seek.type = "range";
  seek.min = "0";
  seek.max = String(Math.ceil(song.duration));
  seek.step = "0.1";
  seek.value = "0";
  const time = h("span", "video-seek-time muted", `0:00 / ${clock(song.duration)}`);
  const roll = h("canvas", "midi-roll");
  const instruments = [...new Set(song.notes.map((n) => (n.channel === 9 ? "Drums" : GM_FAMILIES[Math.floor(n.program / 8)])))];
  body.replaceChildren(h("div", "audio-view",
    h("h3", "audio-title", entry.name.replace(/\.[^.]+$/, "")),
    h("p", "audio-details muted", [clock(song.duration), `${song.notes.length.toLocaleString()} notes`, instruments.join(", ")].filter(Boolean).join(" · ")),
    song.names.length ? h("p", "muted", song.names.join(" · ")) : null,
    roll, h("div", "video-seek-row", play, seek, time),
    h("p", "muted midi-note", "Played on a simple built-in synthesizer, so instruments sound alike.")));

  let context = null;
  let startedAt = 0;
  let offset = 0;
  let next = 0;
  let timer = 0;
  let voices = [];
  const now = () => (context && timer ? context.currentTime - startedAt + offset : offset);
  const frequency = (note) => 440 * 2 ** ((note - 69) / 12);
  const voice = (n, when, length) => {
    const gain = context.createGain();
    const level = (n.velocity / 127) * 0.18;
    gain.gain.setValueAtTime(0, when);
    gain.gain.linearRampToValueAtTime(level, when + 0.01);
    if (n.channel === 9) {
      // Drums: a short burst of noise.
      const noise = context.createBufferSource();
      const buffer = context.createBuffer(1, context.sampleRate * 0.2, context.sampleRate);
      const data = buffer.getChannelData(0);
      for (let i = 0; i < data.length; i += 1) data[i] = (Math.random() * 2 - 1) * (1 - i / data.length);
      noise.buffer = buffer;
      gain.gain.exponentialRampToValueAtTime(0.001, when + 0.18);
      noise.connect(gain).connect(context.destination);
      noise.start(when);
      voices.push(noise);
      return;
    }
    const osc = context.createOscillator();
    const family = Math.floor(n.program / 8);
    osc.type = [0, 1, 3, 13].includes(family) ? "triangle" : [4, 10].includes(family) ? "sawtooth" : [2, 8, 9].includes(family) ? "square" : "sine";
    osc.frequency.setValueAtTime(frequency(n.note), when);
    gain.gain.setTargetAtTime(level * 0.6, when + 0.02, 0.15);
    gain.gain.setTargetAtTime(0, when + length, 0.08);
    osc.connect(gain).connect(context.destination);
    osc.start(when);
    osc.stop(when + length + 0.5);
    voices.push(osc);
  };
  // Notes are scheduled a little ahead of time, as they come.
  const schedule = () => {
    const horizon = now() + 0.25;
    while (next < song.notes.length && song.notes[next].start < horizon) {
      const n = song.notes[next];
      if (n.start >= offset - 0.01 && voices.length < 4000) voice(n, startedAt + n.start - offset, Math.max(0.05, n.end - n.start));
      next += 1;
    }
    voices = voices.slice(-2000);
    const t = now();
    if (document.activeElement !== seek) seek.value = String(t);
    time.textContent = `${clock(t)} / ${clock(song.duration)}`;
    drawRoll(t);
    if (t > song.duration + 0.5) stop(true);
  };
  const silence = () => { for (const v of voices) { try { v.stop(); } catch { /* already stopped */ } } voices = []; };
  const start = () => {
    context = context || new AudioContext();
    context.resume();
    startedAt = context.currentTime + 0.05;
    next = song.notes.findIndex((n) => n.start >= offset);
    if (next < 0) next = song.notes.length;
    timer = setInterval(schedule, 40);
    play.textContent = "❚❚ Pause";
  };
  const stop = (ended = false) => {
    if (timer) offset = ended ? 0 : now();
    clearInterval(timer);
    timer = 0;
    silence();
    play.textContent = "▶ Play";
    drawRoll(offset);
  };
  play.addEventListener("click", () => (timer ? stop() : start()));
  seek.addEventListener("change", () => {
    const playing = !!timer;
    stop();
    offset = Number(seek.value);
    if (playing) start();
    drawRoll(offset);
  });
  // A piano roll: the notes around the playing point.
  const low = Math.min(...song.notes.map((n) => n.note), 60);
  const high = Math.max(...song.notes.map((n) => n.note), 72);
  function drawRoll(t) {
    const ratio = window.devicePixelRatio || 1;
    const width = roll.clientWidth;
    const height = roll.clientHeight;
    if (!width) return;
    roll.width = width * ratio;
    roll.height = height * ratio;
    const ctx = roll.getContext("2d");
    ctx.scale(ratio, ratio);
    const span = 8;
    const from = t - 2;
    const accent = cssColor("--accent", "#7aa2f7");
    const rest = cssColor("--muted", "#888");
    for (const n of song.notes) {
      if (n.end < from || n.start > from + span) continue;
      const x = ((n.start - from) / span) * width;
      const w = Math.max(2, ((n.end - n.start) / span) * width);
      const y = height - ((n.note - low + 1) / (high - low + 2)) * height;
      ctx.fillStyle = n.start <= t && n.end >= t ? accent : rest;
      ctx.globalAlpha = n.channel === 9 ? 0.35 : 0.8;
      ctx.fillRect(x, y, w, Math.max(2, height / (high - low + 2) - 1));
    }
    ctx.globalAlpha = 1;
    ctx.fillStyle = accent;
    ctx.fillRect((2 / span) * width, 0, 1, height);
  }
  requestAnimationFrame(() => drawRoll(0));
  return { destroy() { stop(); if (context) context.close(); } };
}

// ---- G-code (3D printing): the printed path, layer by layer ----

export async function mountGcode(body, entry) {
  if (entry.size > 150 * 1024 * 1024) throw new Error("This G-code file is too large to draw");
  body.replaceChildren(h("p", "muted", "Reading the toolpath…"));
  const [THREE, { OrbitControls }, text] = await Promise.all([
    import("/assets/vendor/three/three.module.min.js"),
    import("/assets/vendor/three/addons/controls/OrbitControls.js"),
    fetchText(entry),
  ]);
  // Moves, split into printing (the extruder pushes) and travel.
  const print = [];
  const travel = [];
  const layerZ = [];
  const layerStart = [];
  let x = 0, y = 0, z = 0, e = 0;
  let relative = false;
  let relativeE = false;
  let filament = 0;
  let seconds = 0;
  let feed = 1500;
  const comments = {};
  for (const raw of text.split("\n")) {
    const semicolon = raw.indexOf(";");
    if (semicolon >= 0) {
      const comment = raw.slice(semicolon + 1).trim();
      const m = /^(TIME|PRINT_TIME|estimated printing time.*?|filament used.*?|Filament used|LAYER_COUNT|generated by|FLAVOR)\s*[:=]\s*(.+)$/i.exec(comment);
      if (m && !comments[m[1].toLowerCase()]) comments[m[1].toLowerCase()] = m[2].trim();
    }
    const line = (semicolon >= 0 ? raw.slice(0, semicolon) : raw).trim().toUpperCase();
    if (!line) continue;
    const code = line.split(/\s+/)[0];
    const arg = (letter) => { const m = new RegExp(`${letter}(-?[\\d.]+)`).exec(line); return m ? Number(m[1]) : null; };
    if (code === "G90") relative = false;
    else if (code === "G91") relative = true;
    else if (code === "M82") relativeE = false;
    else if (code === "M83") relativeE = true;
    else if (code === "G92") { if (arg("E") !== null) e = arg("E"); if (arg("X") !== null) x = arg("X"); if (arg("Y") !== null) y = arg("Y"); if (arg("Z") !== null) z = arg("Z"); }
    else if (code === "G0" || code === "G1" || code === "G00" || code === "G01") {
      const nx = arg("X") === null ? x : relative ? x + arg("X") : arg("X");
      const ny = arg("Y") === null ? y : relative ? y + arg("Y") : arg("Y");
      const nz = arg("Z") === null ? z : relative ? z + arg("Z") : arg("Z");
      const ne = arg("E") === null ? e : relative || relativeE ? e + arg("E") : arg("E");
      if (arg("F") !== null) feed = arg("F");
      const distance = Math.hypot(nx - x, ny - y, nz - z);
      if (feed > 0) seconds += (distance / feed) * 60;
      const extruding = ne > e + 1e-6 && (nx !== x || ny !== y);
      if (extruding) {
        filament += ne - e;
        if (!layerZ.length || Math.abs(nz - layerZ[layerZ.length - 1]) > 1e-4) { layerZ.push(nz); layerStart.push(print.length / 6); }
        print.push(x, y, z, nx, ny, nz);
      } else if (distance > 0) {
        travel.push(x, y, z, nx, ny, nz);
      }
      x = nx; y = ny; z = nz; e = ne;
    }
  }
  if (!print.length) throw new Error("This G-code has no printing moves to draw");
  const layers = layerZ.length;
  // Printer coordinates have Z up; the viewer has Y up.
  const toScene = (array) => {
    const out = new Float32Array(array.length);
    for (let i = 0; i < array.length; i += 3) { out[i] = array[i]; out[i + 1] = array[i + 2]; out[i + 2] = -array[i + 1]; }
    return out;
  };
  const colors = new Float32Array(print.length);
  const minZ = layerZ[0];
  const maxZ = layerZ[layers - 1];
  const color = new THREE.Color();
  for (let i = 0; i < print.length; i += 3) {
    color.setHSL(0.66 - 0.66 * ((print[i + 2] - minZ) / (maxZ - minZ || 1)), 0.75, 0.55);
    colors[i] = color.r; colors[i + 1] = color.g; colors[i + 2] = color.b;
  }
  const printGeometry = new THREE.BufferGeometry();
  printGeometry.setAttribute("position", new THREE.BufferAttribute(toScene(print), 3));
  printGeometry.setAttribute("color", new THREE.BufferAttribute(colors, 3));
  const printLines = new THREE.LineSegments(printGeometry, new THREE.LineBasicMaterial({ vertexColors: true }));
  const travelGeometry = new THREE.BufferGeometry();
  travelGeometry.setAttribute("position", new THREE.BufferAttribute(toScene(travel), 3));
  const travelLines = new THREE.LineSegments(travelGeometry, new THREE.LineBasicMaterial({ color: 0x888888, transparent: true, opacity: 0.25 }));
  travelLines.visible = false;

  const canvasBox = h("div", "gcode-canvas");
  const slider = h("input", "comic-slider");
  slider.type = "range";
  slider.min = "1";
  slider.max = String(layers);
  slider.value = String(layers);
  const layerLabel = h("span", "comic-counter muted");
  const travelToggle = h("label", "gcode-toggle", h("input", ""), " Travel moves");
  const travelBox = travelToggle.querySelector("input");
  travelBox.type = "checkbox";
  const minutes = seconds / 60;
  const facts = [
    ["Layers", `${layers.toLocaleString()} (${minZ.toFixed(2)}–${maxZ.toFixed(2)} mm)`],
    ["Filament", `${(filament / 1000).toFixed(2)} m`],
    ["Time (estimated)", comments.time || comments.print_time ? clock(Number(comments.time || comments.print_time)) : minutes >= 60 ? `${Math.floor(minutes / 60)} h ${Math.round(minutes % 60)} min` : `${Math.round(minutes)} min`],
    ["Sliced by", comments["generated by"] || comments.flavor || ""],
  ].filter(([, v]) => v);
  body.replaceChildren(h("div", "gcode-view", canvasBox, h("div", "comic-tools", h("span", "muted", "Layer"), slider, layerLabel, travelToggle),
    h("dl", "track-facts", facts.map(([k, v]) => [h("dt", "muted", k), h("dd", "", v)]))));

  const renderer = new THREE.WebGLRenderer({ antialias: true });
  renderer.setPixelRatio(window.devicePixelRatio || 1);
  canvasBox.append(renderer.domElement);
  const scene = new THREE.Scene();
  scene.background = new THREE.Color(cssColor("--bg-2", "#1a1a1a"));
  scene.add(printLines, travelLines);
  const box = new THREE.Box3().setFromObject(printLines);
  const size = box.getSize(new THREE.Vector3());
  const middle = box.getCenter(new THREE.Vector3());
  const grid = new THREE.GridHelper(Math.max(size.x, size.z) * 1.4, 20, 0x555555, 0x333333);
  grid.position.set(middle.x, box.min.y, middle.z);
  scene.add(grid);
  const camera = new THREE.PerspectiveCamera(40, 1, 0.1, 100000);
  const reach = Math.max(size.x, size.y, size.z) * 1.6 + 10;
  camera.position.set(middle.x + reach * 0.7, middle.y + reach * 0.8, middle.z + reach);
  const controls = new OrbitControls(camera, renderer.domElement);
  controls.target.copy(middle);
  controls.update();
  const draw = () => renderer.render(scene, camera);
  controls.addEventListener("change", draw);
  const showLayers = () => {
    const shown = Number(slider.value);
    const end = shown >= layers ? print.length / 6 : layerStart[shown];
    printGeometry.setDrawRange(0, end * 2);
    layerLabel.textContent = `${shown} / ${layers} · ${layerZ[shown - 1].toFixed(2)} mm`;
    draw();
  };
  slider.addEventListener("input", showLayers);
  travelBox.addEventListener("change", () => { travelLines.visible = travelBox.checked; draw(); });
  const resize = new ResizeObserver(() => {
    const { clientWidth: w, clientHeight: hgt } = canvasBox;
    if (!w || !hgt) return;
    renderer.setSize(w, hgt);
    camera.aspect = w / hgt;
    camera.updateProjectionMatrix();
    draw();
  });
  resize.observe(canvasBox);
  showLayers();
  return {
    destroy() {
      resize.disconnect();
      controls.dispose();
      printGeometry.dispose();
      travelGeometry.dispose();
      renderer.dispose();
    },
  };
}
