// The music player: a queue of songs from a folder (and its subfolders), a playlist file, or
// chosen songs, playing in a bar at the bottom of the page while you browse. The ♪ button opens
// a panel with the queue and the folders and playlists played lately. The queue, the song, and
// where it was are kept in this browser, so a reload carries on (paused). Loaded by app.js.

const enc = encodeURIComponent;
const STORE = "ownnas-music";
const RECENTS = "ownnas-music-recents";
/// Larger queues are kept as where they came from, and loaded again.
const KEEP_QUEUE = 4000;

let ctx = null;
const audio = new Audio();
audio.preload = "auto";
let bar = null;
let panel = null;
let queue = []; // { path, name }
let original = null; // the order before shuffling
let index = -1;
let shuffle = false;
let repeat = "off"; // "all", "one"
let source = null; // { kind: "folder" | "playlist" | "songs", path, name }
const infos = new Map(); // path -> what the song's tags say
let failures = 0;
let saveTimer = 0;

export function init(context) {
  ctx = context;
  bar = document.getElementById("music-bar");
  try { audio.volume = Number(localStorage.getItem("ownnas-music-volume") ?? 1); } catch {}
  audio.addEventListener("timeupdate", () => { paintTime(); saveSoon(); });
  audio.addEventListener("durationchange", paintTime);
  audio.addEventListener("play", () => { paintPlaying(); pauseOthers(); });
  audio.addEventListener("pause", () => { paintPlaying(); save(); });
  audio.addEventListener("ended", () => {
    if (repeat === "one") { audio.currentTime = 0; audio.play().catch(() => {}); }
    else next(true);
  });
  audio.addEventListener("playing", () => { failures = 0; });
  audio.addEventListener("error", () => {
    if (index < 0 || !audio.getAttribute("src")) return;
    failures += 1;
    // A song the browser cannot play is skipped, but not a whole queue of them.
    if (failures >= 5) {
      ctx.toast("These songs cannot be played in this browser", true);
      audio.pause();
      return;
    }
    ctx.toast(`Cannot play ${queue[index]?.name || "this song"}; skipping it`, true);
    next(true);
  });
  // Another player on the page (a preview) starting pauses this one.
  document.addEventListener("play", (event) => { if (event.target !== audio && !audio.paused) audio.pause(); }, true);
  document.addEventListener("keydown", (event) => { if (event.key === "Escape" && panel && !panel.hidden) closePanel(); });
  document.addEventListener("pointerdown", (event) => {
    if (panel && !panel.hidden && !panel.contains(event.target) && !event.target.closest("#music-btn, #music-bar")) closePanel();
  });
  if ("mediaSession" in navigator) {
    const handlers = {
      play: () => audio.play().catch(() => {}),
      pause: () => audio.pause(),
      previoustrack: () => previous(),
      nexttrack: () => next(),
      seekto: (details) => { if (details.seekTime != null) audio.currentTime = details.seekTime; },
    };
    for (const [action, handler] of Object.entries(handlers)) {
      try { navigator.mediaSession.setActionHandler(action, handler); } catch {}
    }
  }
}

/// Carries on with the queue kept in this browser (paused, where it was).
export async function restore() {
  let saved = null;
  try { saved = JSON.parse(localStorage.getItem(STORE) || "null"); } catch {}
  if (!saved) return;
  shuffle = !!saved.shuffle;
  repeat = saved.repeat || "off";
  source = saved.source || null;
  let list = saved.queue;
  if (!list && source) {
    try { list = await songsOf(source); } catch { list = null; }
  }
  if (!list || !list.length) return;
  queue = list;
  load(Math.min(Math.max(saved.index || 0, 0), queue.length - 1), false, saved.time || 0);
}

// ---- Loading music ----

function nameOf(path) {
  return path.split("/").pop();
}

async function folderSongs(path) {
  const data = await ctx.api(`/api/music/folder?path=${enc(path)}`);
  if (data.truncated) ctx.toast(`Playing the first ${data.songs.length.toLocaleString()} songs`);
  return data.songs.map((song) => ({ path: song.path, name: song.name }));
}

async function playlistSongs(path) {
  const viewers = await import("/assets/viewers.js");
  const response = await fetch(`/api/raw?path=${enc(path)}`, { credentials: "same-origin" });
  if (!response.ok) throw new Error("Could not read the playlist");
  const entry = { path, name: nameOf(path) };
  return viewers.parsePlaylist(entry.name, await response.text())
    .map((item) => ({ ...item, ...viewers.libraryTarget(entry, item.location) }))
    .filter((item) => item.path)
    .map((item) => ({ path: item.path, name: item.title || nameOf(item.path) }));
}

function songsOf(from) {
  return from.kind === "playlist" ? playlistSongs(from.path) : folderSongs(from.path);
}

/// Plays every song in a folder and its subfolders.
export async function playFolder(path, { shuffled = false } = {}) {
  const songs = await folderSongs(path);
  if (!songs.length) {
    ctx.toast("No music in this folder");
    return;
  }
  start(songs, 0, { kind: "folder", path, name: path ? nameOf(path) : "Library" }, shuffled);
}

/// Plays a playlist file (.m3u, .m3u8, .pls, .xspf): the songs in it that are in the library.
export async function playPlaylist(path, { shuffled = false, append = false } = {}) {
  const songs = await playlistSongs(path);
  if (!songs.length) {
    ctx.toast("None of this playlist's songs are in the library");
    return;
  }
  if (append) {
    addToQueue(songs);
    return;
  }
  start(songs, 0, { kind: "playlist", path, name: nameOf(path).replace(/\.[^.]+$/, "") }, shuffled);
}

/// Plays some songs, starting with `first`.
export function playSongs(songs, first = 0) {
  start(songs.map((song) => ({ path: song.path, name: song.name || nameOf(song.path) })), first, { kind: "songs", path: "", name: "" }, false);
}

export function addToQueue(songs) {
  const added = songs.map((song) => ({ path: song.path, name: song.name || nameOf(song.path) }));
  if (!queue.length) {
    playSongs(added);
    return;
  }
  queue.push(...added);
  if (original) original.push(...added);
  source = { kind: "songs", path: "", name: "" };
  ctx.toast(added.length === 1 ? `Added ${added[0].name} to the queue` : `Added ${added.length} songs to the queue`);
  save();
  paintBar();
  paintPanel();
}

function start(songs, first, from, shuffled) {
  queue = songs.slice();
  original = null;
  source = from;
  shuffle = false;
  index = first;
  if (shuffled) setShuffle(true, first);
  if (from.kind !== "songs") remember(from, songs.length);
  failures = 0;
  load(shuffled ? 0 : first, true);
}

function load(at, play, time = 0) {
  index = at;
  const song = queue[index];
  if (!song) return;
  audio.src = `/api/raw?path=${enc(song.path)}`;
  if (time) {
    audio.addEventListener("loadedmetadata", () => { audio.currentTime = time; }, { once: true });
  }
  if (play) audio.play().catch(() => {});
  paintBar();
  paintPanel();
  describe(song);
  save();
}

// ---- Controls ----

function toggle() {
  if (!queue.length) return;
  if (audio.paused) audio.play().catch(() => {});
  else audio.pause();
}

function next(auto = false) {
  if (!queue.length) return;
  if (index + 1 < queue.length) load(index + 1, true);
  else if (repeat === "all") load(0, true);
  else if (!auto) load(index, false);
  else { audio.pause(); audio.currentTime = 0; paintPlaying(); }
}

function previous() {
  if (!queue.length) return;
  if (audio.currentTime > 3 || index === 0) { audio.currentTime = 0; return; }
  load(index - 1, true);
}

/// Shuffles what comes after the song playing (or puts the queue back in order).
function setShuffle(on, keep = index) {
  if (on === shuffle) return;
  const current = queue[keep];
  if (on) {
    original = queue.slice();
    const rest = queue.filter((_, i) => i !== keep);
    for (let i = rest.length - 1; i > 0; i -= 1) {
      const j = Math.floor(Math.random() * (i + 1));
      [rest[i], rest[j]] = [rest[j], rest[i]];
    }
    queue = current ? [current, ...rest] : rest;
    index = current ? 0 : -1;
  } else if (original) {
    queue = original;
    original = null;
    index = Math.max(0, queue.indexOf(current));
  }
  shuffle = on;
}

function cycleRepeat() {
  repeat = repeat === "off" ? "all" : repeat === "all" ? "one" : "off";
  paintBar();
  save();
}

/// Stops and empties the queue (and hides the bar).
function clear() {
  audio.pause();
  audio.removeAttribute("src");
  audio.load();
  queue = [];
  original = null;
  index = -1;
  source = null;
  try { localStorage.removeItem(STORE); } catch {}
  if ("mediaSession" in navigator) navigator.mediaSession.metadata = null;
  paintBar();
  paintPanel();
}

function removeAt(at) {
  const song = queue[at];
  queue.splice(at, 1);
  if (original) original.splice(original.indexOf(song), 1);
  if (!queue.length) { clear(); return; }
  if (at < index) index -= 1;
  else if (at === index) load(Math.min(index, queue.length - 1), !audio.paused);
  source = { kind: "songs", path: "", name: "" };
  save();
  paintPanel();
}

// ---- What is playing ----

async function describe(song) {
  let info = infos.get(song.path);
  if (!info) {
    try {
      info = await ctx.api(`/api/audio/info?path=${enc(song.path)}`);
    } catch {
      info = {};
    }
    infos.set(song.path, info);
  }
  if (queue[index] !== song) return;
  paintBar();
  if ("mediaSession" in navigator) {
    navigator.mediaSession.metadata = new MediaMetadata({
      title: titleOf(song),
      artist: info.artist || info.albumArtist || "",
      album: info.album || "",
      artwork: info.hasCover ? [{ src: coverUrl(song), sizes: "512x512" }] : [],
    });
  }
}

function titleOf(song) {
  return infos.get(song.path)?.title || song.name.replace(/\.[^.]+$/, "");
}

function subtitleOf(song) {
  const info = infos.get(song.path) || {};
  return [info.artist || info.albumArtist, info.album].filter(Boolean).join(" · ") || song.path.split("/").slice(0, -1).join("/") || "Library";
}

function coverUrl(song) {
  return `/api/audio/cover?path=${enc(song.path)}`;
}

// ---- Keeping it in this browser ----

function saveSoon() {
  if (saveTimer) return;
  saveTimer = setTimeout(() => { saveTimer = 0; save(); }, 5000);
}

function save() {
  if (!queue.length) return;
  const big = queue.length > KEEP_QUEUE && source && source.kind !== "songs" && !shuffle;
  const state = {
    queue: big ? null : queue,
    index,
    time: audio.currentTime || 0,
    shuffle: big ? false : shuffle,
    repeat,
    source,
  };
  try { localStorage.setItem(STORE, JSON.stringify(state)); } catch {
    // Too big for this browser's storage: keep where it came from instead.
    try { localStorage.setItem(STORE, JSON.stringify({ ...state, queue: null, shuffle: false })); } catch {}
  }
}

function recents() {
  try { return JSON.parse(localStorage.getItem(RECENTS) || "[]"); } catch { return []; }
}

function remember(from, count) {
  const list = recents().filter((item) => !(item.kind === from.kind && item.path === from.path));
  list.unshift({ kind: from.kind, path: from.path, name: from.name, count, at: Date.now() });
  try { localStorage.setItem(RECENTS, JSON.stringify(list.slice(0, 12))); } catch {}
}

function forget(item) {
  const list = recents().filter((other) => !(other.kind === item.kind && other.path === item.path));
  try { localStorage.setItem(RECENTS, JSON.stringify(list)); } catch {}
  paintPanel();
}

/// Saves the queue as a playlist file in the folder shown in the browser.
async function saveAsPlaylist() {
  const folder = ctx.currentPath();
  const name = await ctx.askText("Save the queue as a playlist", "Name", source?.name || "Playlist", "Save");
  if (!name) return;
  const created = await ctx.api("/api/create", { method: "POST", json: { path: folder, name: name.trim(), kind: "m3u8" } });
  const base = folder ? folder.split("/") : [];
  const lines = ["#EXTM3U"];
  for (const song of queue) {
    lines.push(`#EXTINF:-1,${titleOf(song)}`);
    lines.push(relative(base, song.path));
  }
  await ctx.api("/api/write", { method: "POST", json: { path: created.path, content: `${lines.join("\n")}\n` } });
  ctx.toast(`Saved ${created.path}`);
  ctx.refresh();
}

/// A song's path from the playlist's folder ("../Album/song.mp3").
function relative(base, path) {
  const parts = path.split("/");
  let same = 0;
  while (same < base.length && same < parts.length - 1 && base[same] === parts[same]) same += 1;
  return [...Array(base.length - same).fill(".."), ...parts.slice(same)].join("/");
}

// ---- The bar ----

const ICONS = {
  play: '<path d="M5 3.5v9l7.5-4.5z" fill="currentColor"/>',
  pause: '<path d="M4.5 3.5h2.5v9H4.5zM9 3.5h2.5v9H9z" fill="currentColor"/>',
  previous: '<path d="M4 3.5v9M12.5 3.5v9L6 8z" fill="currentColor" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round"/>',
  next: '<path d="M12 3.5v9M3.5 3.5v9L10 8z" fill="currentColor" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round"/>',
  shuffle: '<path d="M2 4.5h2.5c3 0 4 7 7 7H14M2 11.5h2.5c1.3 0 2.2-1.3 3-2.8M9.6 6.3c.7-1 1.4-1.8 2-1.8H14M12 2.5l2 2-2 2M12 9.5l2 2-2 2" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/>',
  repeat: '<path d="M3 7V6a1.5 1.5 0 0 1 1.5-1.5H13M11 2.5l2 2-2 2M13 9v1a1.5 1.5 0 0 1-1.5 1.5H3M5 13.5l-2-2 2-2" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/>',
  queue: '<path d="M2.5 4h8M2.5 8h8M2.5 12h5M12.5 9.5v4M12.5 13.5a1.25 1.25 0 1 1-1.25-1.25" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/>',
  close: '<path d="M4 4l8 8M12 4l-8 8" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>',
  volume: '<path d="M2.5 6h2.5l3-2.5v9L5 10H2.5zM10.5 5.5a3.5 3.5 0 0 1 0 5M12.5 3.5a6 6 0 0 1 0 9" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/>',
};

function icon(name) {
  return `<svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">${ICONS[name]}</svg>`;
}

function button(name, label, action, extra = "") {
  return `<button type="button" class="music-btn ${extra}" data-music="${action}" title="${label}" aria-label="${label}">${icon(name)}</button>`;
}

function clock(seconds) {
  if (!Number.isFinite(seconds)) return "0:00";
  const s = Math.floor(seconds);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const rest = String(s % 60).padStart(2, "0");
  return h ? `${h}:${String(m).padStart(2, "0")}:${rest}` : `${m}:${rest}`;
}

function esc(text) {
  return String(text).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
}

function paintBar() {
  if (!bar) return;
  const song = queue[index];
  bar.hidden = !song;
  document.getElementById("music-btn")?.classList.toggle("is-playing", !!song);
  if (!song) {
    bar.replaceChildren();
    return;
  }
  bar.innerHTML = `
    <button type="button" class="music-now" data-music="panel" title="Show the queue">
      <span class="music-cover"><span aria-hidden="true">♪</span><img alt="" hidden></span>
      <span class="music-text"><strong></strong><span class="music-sub"></span></span>
    </button>
    <div class="music-controls">
      ${button("shuffle", "Shuffle", "shuffle", `music-toggle${shuffle ? " is-on" : ""}`)}
      ${button("previous", "Previous", "previous")}
      ${button(audio.paused ? "play" : "pause", audio.paused ? "Play" : "Pause", "toggle", "music-play")}
      ${button("next", "Next", "next")}
      ${button("repeat", repeat === "one" ? "Repeat this song" : repeat === "all" ? "Repeat the queue" : "Repeat", "repeat", `music-toggle${repeat !== "off" ? " is-on" : ""}${repeat === "one" ? " is-one" : ""}`)}
    </div>
    <div class="music-seek">
      <span class="music-time" data-time="now">0:00</span>
      <input type="range" min="0" max="1000" value="0" step="1" aria-label="Position in the song">
      <span class="music-time" data-time="total">0:00</span>
    </div>
    <label class="music-volume" title="Volume">${icon("volume")}<input type="range" min="0" max="1" step="0.05" aria-label="Volume"></label>
    ${button("queue", "Queue and recent music", "panel")}
    ${button("close", "Stop and clear the queue", "clear")}`;
  bar.querySelector(".music-text strong").textContent = titleOf(song);
  bar.querySelector(".music-sub").textContent = `${subtitleOf(song)} · ${index + 1} of ${queue.length}`;
  const img = bar.querySelector(".music-cover img");
  if (infos.get(song.path)?.hasCover) {
    img.src = coverUrl(song);
    img.hidden = false;
    img.onerror = () => { img.hidden = true; };
  }
  const seek = bar.querySelector(".music-seek input");
  seek.addEventListener("input", () => {
    if (audio.duration) audio.currentTime = (seek.value / 1000) * audio.duration;
  });
  const volume = bar.querySelector(".music-volume input");
  volume.value = String(audio.volume);
  volume.addEventListener("input", () => {
    audio.volume = Number(volume.value);
    try { localStorage.setItem("ownnas-music-volume", volume.value); } catch {}
  });
  bar.querySelectorAll("[data-music]").forEach((node) => node.addEventListener("click", () => act(node.dataset.music)));
  paintTime();
}

function paintTime() {
  if (!bar || bar.hidden) return;
  const seek = bar.querySelector(".music-seek input");
  if (!seek) return;
  if (document.activeElement !== seek) seek.value = audio.duration ? String(Math.round((audio.currentTime / audio.duration) * 1000)) : "0";
  bar.querySelector('[data-time="now"]').textContent = clock(audio.currentTime);
  bar.querySelector('[data-time="total"]').textContent = clock(audio.duration);
  if ("mediaSession" in navigator && audio.duration && navigator.mediaSession.setPositionState) {
    try { navigator.mediaSession.setPositionState({ duration: audio.duration, position: audio.currentTime, playbackRate: 1 }); } catch {}
  }
}

function paintPlaying() {
  const play = bar?.querySelector(".music-play");
  if (play) {
    play.innerHTML = icon(audio.paused ? "play" : "pause");
    play.title = audio.paused ? "Play" : "Pause";
    play.setAttribute("aria-label", play.title);
  }
  panel?.querySelector(".music-panel-play")?.replaceChildren(document.createTextNode(audio.paused ? "Play" : "Pause"));
}

function act(action) {
  if (action === "toggle") toggle();
  if (action === "next") next();
  if (action === "previous") previous();
  if (action === "shuffle") {
    setShuffle(!shuffle);
    paintBar();
    paintPanel();
    save();
  }
  if (action === "repeat") cycleRepeat();
  if (action === "panel") togglePanel();
  if (action === "clear") clear();
}

/// Plays whatever the browser shows; other players on the page stop.
function pauseOthers() {
  for (const media of document.querySelectorAll("audio, video")) {
    if (media !== audio && !media.paused) media.pause();
  }
}

// ---- The panel ----

export function togglePanel() {
  if (panel && !panel.hidden) closePanel();
  else openPanel();
}

function openPanel() {
  if (!panel) {
    panel = document.createElement("aside");
    panel.className = "music-panel";
    panel.setAttribute("aria-label", "Music");
    document.body.append(panel);
  }
  panel.hidden = false;
  paintPanel();
  panel.querySelector(".is-now")?.scrollIntoView({ block: "center" });
}

function closePanel() {
  if (panel) panel.hidden = true;
}

function paintPanel() {
  if (!panel || panel.hidden) return;
  const song = queue[index];
  const folder = ctx.currentPath();
  const folderName = folder ? nameOf(folder) : "Library";
  const recent = recents();
  // A long queue shows the songs around the one playing.
  const from = Math.max(0, index - 30);
  const to = Math.min(queue.length, Math.max(index, 0) + 200);
  panel.innerHTML = `
    <div class="music-panel-head">
      <h2>Music</h2>
      <button type="button" class="music-btn" data-act="close" title="Close" aria-label="Close">${icon("close")}</button>
    </div>
    <div class="music-panel-actions">
      <button type="button" data-act="playHere">Play “${esc(folderName)}”</button>
      <button type="button" data-act="shuffleHere">Shuffle “${esc(folderName)}”</button>
    </div>
    ${song ? `
    <section class="music-panel-section">
      <div class="music-panel-row">
        <h3>Queue <span class="muted">${queue.length.toLocaleString()} song${queue.length === 1 ? "" : "s"}${source?.name ? ` · ${esc(source.name)}` : ""}</span></h3>
        <span class="music-panel-tools">
          <button type="button" class="music-panel-play" data-act="toggle">${audio.paused ? "Play" : "Pause"}</button>
          ${ctx.canWrite() ? '<button type="button" data-act="savePlaylist">Save as playlist…</button>' : ""}
          <button type="button" data-act="clear">Clear</button>
        </span>
      </div>
      <ol class="music-queue" start="${from + 1}">
        ${queue.slice(from, to).map((item, offset) => {
          const at = from + offset;
          return `<li class="${at === index ? "is-now" : ""}" data-at="${at}">
            <button type="button" class="music-queue-song" data-act="play" data-at="${at}"><span class="music-queue-title"></span><span class="muted music-queue-where"></span></button>
            <button type="button" class="music-btn" data-act="remove" data-at="${at}" title="Remove from the queue" aria-label="Remove from the queue">${icon("close")}</button>
          </li>`;
        }).join("")}
      </ol>
      ${to < queue.length ? `<p class="muted">…and ${(queue.length - to).toLocaleString()} more</p>` : ""}
    </section>` : '<p class="muted music-panel-empty">Nothing is playing. Play a folder and its subfolders, or a playlist: right-click it, or use the buttons above for the folder you are in.</p>'}
    ${recent.length ? `
    <section class="music-panel-section">
      <h3>Played lately</h3>
      <ul class="music-recents">
        ${recent.map((item, i) => `<li>
          <button type="button" class="music-recent" data-act="recent" data-i="${i}">
            <span class="music-recent-kind" aria-hidden="true">${item.kind === "playlist" ? "☰" : "📁"}</span>
            <span><strong></strong><span class="muted"></span></span>
          </button>
          <button type="button" class="music-btn" data-act="shuffleRecent" data-i="${i}" title="Shuffle" aria-label="Shuffle">${icon("shuffle")}</button>
          <button type="button" class="music-btn" data-act="forget" data-i="${i}" title="Remove from this list" aria-label="Remove from this list">${icon("close")}</button>
        </li>`).join("")}
      </ul>
    </section>` : ""}`;
  // Names go in as text, never as markup.
  panel.querySelectorAll(".music-queue li").forEach((row) => {
    const item = queue[Number(row.dataset.at)];
    row.querySelector(".music-queue-title").textContent = titleOf(item);
    row.querySelector(".music-queue-where").textContent = item.path.split("/").slice(0, -1).join("/") || "Library";
  });
  panel.querySelectorAll(".music-recent").forEach((row) => {
    const item = recent[Number(row.dataset.i)];
    row.querySelector("strong").textContent = item.name || "Library";
    row.querySelector(".muted").textContent = ` ${item.kind === "playlist" ? "Playlist" : "Folder"} · ${Number(item.count || 0).toLocaleString()} songs · ${item.path || "Library"}`;
    row.title = item.path || "Library";
  });
  panel.querySelectorAll("[data-act]").forEach((node) => node.addEventListener("click", () => panelAction(node.dataset.act, node.dataset)));
}

function panelAction(action, data) {
  const recent = recents();
  const fail = (err) => ctx.toast(err.message, true);
  if (action === "close") closePanel();
  if (action === "toggle") toggle();
  if (action === "clear") clear();
  if (action === "play") load(Number(data.at), true);
  if (action === "remove") removeAt(Number(data.at));
  if (action === "savePlaylist") saveAsPlaylist().catch(fail);
  if (action === "playHere") playFolder(ctx.currentPath()).catch(fail);
  if (action === "shuffleHere") playFolder(ctx.currentPath(), { shuffled: true }).catch(fail);
  if (action === "recent" || action === "shuffleRecent") {
    const item = recent[Number(data.i)];
    const shuffled = action === "shuffleRecent";
    (item.kind === "playlist" ? playPlaylist(item.path, { shuffled }) : playFolder(item.path, { shuffled })).catch(fail);
  }
  if (action === "forget") forget(recent[Number(data.i)]);
}
