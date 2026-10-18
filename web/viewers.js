// Previews for file formats the browser does not show on its own, loaded when first needed.
// Each `mount…` fills the preview body and returns { destroy } to stop what it started.
// The page's security policy allows no inline styles: sizes and colours come from classes.

const enc = encodeURIComponent;

/// An element with a class and text or children.
function h(tag, className, ...children) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  for (const child of children.flat()) {
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
