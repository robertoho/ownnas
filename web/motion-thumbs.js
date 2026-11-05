// Moving thumbnails: tiles of Live and Motion Photos get a badge and play their moment, on hover
// or, where there is no hover, one at a time as they come into view. Works by watching the page,
// so the Gallery and the file list need not know about it.
const TILES = ".gallery-tile[data-path], #files [data-path]";
const MAY_MOVE = /\.(jpe?g|heic|heif)$/i;
const HOVER_DELAY = 150;

const known = new Map();
let waiting = new Set();
let askTimer = 0;
let playingOnScroll = null;
const scrollQueue = [];
const reducedMotion = matchMedia("(prefers-reduced-motion: reduce)");
const noHover = matchMedia("(hover: none)");

const thumbOf = (tile) => tile.querySelector(".gallery-thumb, .thumb");
const clipUrl = (path) => `/api/photo/motion/clip?path=${encodeURIComponent(path)}`;

function play(tile, { loop, onDone } = {}) {
  const thumb = thumbOf(tile);
  if (!thumb || thumb.querySelector(".motion-thumb")) return null;
  const video = document.createElement("video");
  video.className = "motion-thumb";
  video.muted = true;
  video.defaultMuted = true;
  video.playsInline = true;
  video.autoplay = true;
  video.loop = !!loop;
  // It lies exactly over the picture, cropped the way the picture is.
  const picture = thumb.querySelector("img");
  if (picture) video.style.objectFit = getComputedStyle(picture).objectFit;
  video.addEventListener("playing", () => video.classList.add("is-playing"));
  video.addEventListener("error", () => {
    // No clip to be had (no ffmpeg, or the video has gone): the tile stays a picture.
    known.set(tile.dataset.path, false);
    unmark(tile);
    video.remove();
    onDone?.();
  });
  video.addEventListener("ended", () => {
    stop(video);
    onDone?.();
  });
  video.src = clipUrl(tile.dataset.path);
  thumb.append(video);
  return video;
}

function stop(video) {
  video.classList.remove("is-playing");
  video.pause();
  video.removeAttribute("src");
  video.load();
  video.remove();
}

function mark(tile) {
  if (tile.dataset.motion === "1") return;
  const thumb = thumbOf(tile);
  if (!thumb) return;
  tile.dataset.motion = "1";
  const badge = document.createElement("span");
  badge.className = "motion-badge";
  badge.title = "Moving photo";
  badge.setAttribute("aria-label", "Moving photo");
  badge.textContent = "◉";
  thumb.append(badge);
  if (noHover.matches && !reducedMotion.matches) inView.observe(tile);
}

function unmark(tile) {
  delete tile.dataset.motion;
  thumbOf(tile)?.querySelector(".motion-badge")?.remove();
  inView.unobserve(tile);
}

// Where there is no hover: one at a time, once each, while most of the tile shows.
function nextOnScroll() {
  if (playingOnScroll) return;
  while (scrollQueue.length) {
    const tile = scrollQueue.shift();
    if (!tile.isConnected || tile.dataset.motionPlayed) continue;
    tile.dataset.motionPlayed = "1";
    playingOnScroll = play(tile, {
      onDone: () => {
        playingOnScroll = null;
        nextOnScroll();
      },
    });
    if (playingOnScroll) return;
  }
}

const inView = new IntersectionObserver((entries) => {
  for (const entry of entries) {
    if (entry.isIntersecting) scrollQueue.push(entry.target);
    else {
      const at = scrollQueue.indexOf(entry.target);
      if (at >= 0) scrollQueue.splice(at, 1);
    }
  }
  nextOnScroll();
}, { threshold: 0.6 });

async function ask() {
  askTimer = 0;
  const paths = [...waiting];
  waiting = new Set();
  if (!paths.length) return;
  let moving;
  try {
    const response = await fetch("/api/photo/motion/known", {
      method: "POST",
      headers: { "content-type": "application/json", "x-ownnas": "1" },
      body: JSON.stringify({ paths: paths.slice(0, 5000) }),
    });
    if (!response.ok) return;
    moving = new Set((await response.json()).moving);
  } catch {
    return;
  }
  for (const path of paths) known.set(path, moving.has(path));
  look(document);
}

/// Marks the tiles under `root` already known to move, and asks about the others.
function look(root) {
  const tiles = root.matches?.(TILES) ? [root] : [...root.querySelectorAll(TILES)];
  for (const tile of tiles) {
    const path = tile.dataset.path;
    if (!MAY_MOVE.test(path)) continue;
    if (known.get(path)) mark(tile);
    else if (!known.has(path)) waiting.add(path);
  }
  if (waiting.size && !askTimer) askTimer = setTimeout(ask, 200);
}

export function start() {
  let hovered = null;
  let hoverTimer = 0;
  let hoverVideo = null;
  const leave = () => {
    clearTimeout(hoverTimer);
    if (hoverVideo) stop(hoverVideo);
    hoverVideo = null;
    hovered = null;
  };
  document.addEventListener("pointerover", (event) => {
    if (event.pointerType === "touch" || reducedMotion.matches) return;
    const tile = event.target.closest?.('[data-motion="1"]') || null;
    if (tile === hovered) return;
    leave();
    if (!tile) return;
    hovered = tile;
    // A pointer only crossing the page does not set clips off.
    hoverTimer = setTimeout(() => {
      hoverVideo = play(tile, { loop: true, onDone: () => { hoverVideo = null; } });
    }, HOVER_DELAY);
  });
  // A photo that changed, or a folder opened after indexing, may have a new answer next time.
  new MutationObserver((changes) => {
    for (const change of changes) {
      for (const node of change.addedNodes) {
        if (node.nodeType === 1) look(node);
      }
    }
  }).observe(document.body, { childList: true, subtree: true });
  look(document);
}
