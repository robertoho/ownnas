// Uploading straight to the Gallery: each photo is hashed here and looked for in the library
// before it is sent, so what is already there is reported, and not sent, as it goes.
const K = new Uint32Array([
  0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
  0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
  0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
  0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
]);

/// SHA-256, a piece at a time. The browser's own needs HTTPS and the whole file in memory, and a
/// server at home is often plain HTTP, and videos are large.
export class Sha256 {
  constructor() {
    this.state = new Uint32Array([0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19]);
    this.block = new Uint8Array(64);
    this.filled = 0;
    this.length = 0;
    this.words = new Uint32Array(64);
  }

  compress(data, at) {
    const w = this.words;
    for (let i = 0; i < 16; i++, at += 4) w[i] = (data[at] << 24) | (data[at + 1] << 16) | (data[at + 2] << 8) | data[at + 3];
    for (let i = 16; i < 64; i++) {
      const a = w[i - 15];
      const b = w[i - 2];
      const s0 = ((a >>> 7) | (a << 25)) ^ ((a >>> 18) | (a << 14)) ^ (a >>> 3);
      const s1 = ((b >>> 17) | (b << 15)) ^ ((b >>> 19) | (b << 13)) ^ (b >>> 10);
      w[i] = (w[i - 16] + s0 + w[i - 7] + s1) | 0;
    }
    const h = this.state;
    let [a, b, c, d, e, f, g, last] = h;
    for (let i = 0; i < 64; i++) {
      const s1 = ((e >>> 6) | (e << 26)) ^ ((e >>> 11) | (e << 21)) ^ ((e >>> 25) | (e << 7));
      const t1 = (last + s1 + ((e & f) ^ (~e & g)) + K[i] + w[i]) | 0;
      const s0 = ((a >>> 2) | (a << 30)) ^ ((a >>> 13) | (a << 19)) ^ ((a >>> 22) | (a << 10));
      const t2 = (s0 + ((a & b) ^ (a & c) ^ (b & c))) | 0;
      last = g; g = f; f = e; e = (d + t1) | 0;
      d = c; c = b; b = a; a = (t1 + t2) | 0;
    }
    h[0] += a; h[1] += b; h[2] += c; h[3] += d; h[4] += e; h[5] += f; h[6] += g; h[7] += last;
  }

  update(data) {
    this.length += data.length;
    let at = 0;
    if (this.filled) {
      const take = Math.min(64 - this.filled, data.length);
      this.block.set(data.subarray(0, take), this.filled);
      this.filled += take;
      at = take;
      if (this.filled < 64) return;
      this.compress(this.block, 0);
      this.filled = 0;
    }
    for (; at + 64 <= data.length; at += 64) this.compress(data, at);
    if (at < data.length) {
      this.block.set(data.subarray(at), 0);
      this.filled = data.length - at;
    }
  }

  hex() {
    const bits = this.length * 8;
    const pad = new Uint8Array((this.filled < 56 ? 56 : 120) - this.filled + 8);
    pad[0] = 0x80;
    const tail = new DataView(pad.buffer);
    tail.setUint32(pad.length - 8, Math.floor(bits / 2 ** 32));
    tail.setUint32(pad.length - 4, bits % 2 ** 32);
    this.update(pad);
    return [...this.state].map((word) => word.toString(16).padStart(8, "0")).join("");
  }
}

const CHUNK = 4 * 1024 * 1024;
const AT_ONCE = 3;
const MEDIA = /\.(jpe?g|png|gif|webp|avif|heic|heif|jxl|bmp|tiff?|dng|cr2|cr3|nef|arw|raf|orf|rw2|pef|srw|mp4|m4v|mov|webm|mkv|avi|3gp|mts|m2ts|mpe?g)$/i;

export async function hashFile(file, stopped = () => false) {
  const hash = new Sha256();
  for (let at = 0; at < file.size; at += CHUNK) {
    if (stopped()) return null;
    hash.update(new Uint8Array(await file.slice(at, at + CHUNK).arrayBuffer()));
    // Lets the page breathe between pieces.
    await new Promise((done) => setTimeout(done));
  }
  return hash.hex();
}

function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (key === "class") node.className = value;
    else if (key.startsWith("on")) node.addEventListener(key.slice(2), value);
    else if (value !== false) node.setAttribute(key, value === true ? "" : value);
  }
  node.append(...children);
  return node;
}

const sizeText = (bytes) => {
  const units = ["B", "KB", "MB", "GB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${unit ? value.toFixed(1) : value} ${units[unit]}`;
};

function send(file, folder, onProgress, onStart) {
  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    const params = new URLSearchParams({ path: folder, conflict: "keep" });
    if (file.lastModified) params.set("mtime", String(file.lastModified));
    xhr.open("POST", `/api/upload?${params}`);
    xhr.setRequestHeader("X-OwnNAS", "1");
    xhr.upload.onprogress = (event) => {
      if (event.lengthComputable) onProgress(event.loaded / event.total);
    };
    xhr.onload = () => {
      let answer = {};
      try { answer = JSON.parse(xhr.responseText); } catch { /* said below */ }
      if (xhr.status >= 200 && xhr.status < 300 && answer.paths?.[0]) resolve(answer.paths[0]);
      else reject(new Error(answer.error || "The upload failed"));
    };
    xhr.onerror = () => reject(new Error("The connection to the server was lost"));
    xhr.onabort = () => reject(Object.assign(new Error("Stopped"), { stopped: true }));
    const body = new FormData();
    body.append("file", file, file.name);
    onStart(xhr);
    xhr.send(body);
  });
}

/// The window: where the photos go, then a line for each photo as it is checked and sent.
export function openPhotoUpload({ workspace, name, folders, api, toast, pickFolder, onUploaded }) {
  let folder = folders[0] ?? null;
  let running = false;
  let stopping = false;
  let uploadedAny = false;
  const active = new Set();
  const seen = new Map();
  const counts = { uploaded: 0, duplicate: 0, failed: 0, skipped: 0 };
  let waitingRows = [];
  let left = 0;

  const rows = el("ul", { class: "photo-upload-rows" });
  const summary = el("p", { class: "photo-upload-summary", role: "status" });
  const input = el("input", { type: "file", multiple: true, hidden: true, accept: "image/*,video/*,.heic,.heif,.dng,.cr2,.cr3,.nef,.arw,.raf,.orf,.rw2", class: "photo-upload-input" });
  const choose = el("button", { type: "button", class: "photo-upload-choose", onclick: () => input.click() }, "Choose photos and videos…");
  const stop = el("button", { type: "button", hidden: true, onclick: () => {
    stopping = true;
    for (const xhr of active) xhr.abort();
  } }, "Stop");
  const close = el("button", { type: "button", class: "photo-upload-close", onclick: () => dialog.close() }, "Close");

  const whereText = el("span", {});
  const where = el("div", { class: "photo-upload-where" }, whereText);
  if (folders.length > 1) {
    const select = el("select", { "aria-label": "Folder", onchange: () => { folder = select.value; } }, ...folders.map((path) => el("option", { value: path }, path)));
    whereText.textContent = "Into";
    where.append(select);
  } else if (workspace !== null && !folders.length) {
    whereText.textContent = `“${name}” has no folders yet, so there is nowhere to put photos. Add a folder to the workspace first.`;
    choose.disabled = true;
  } else if (workspace !== null) {
    whereText.textContent = `Into ${folder}`;
  } else {
    const paint = () => { whereText.textContent = folder === null ? "Choose a folder for them first." : `Into ${folder || "the top of the library"}`; };
    paint();
    choose.disabled = true;
    where.append(el("button", { type: "button", class: "photo-upload-folder", onclick: async () => {
      const picked = await pickFolder({ title: "Upload photos to", ok: "Use this folder", start: folder || "" });
      if (picked === null || picked === undefined) return;
      folder = picked;
      choose.disabled = false;
      paint();
    } }, "Choose folder…"));
  }

  const dialog = el("dialog", { class: "photo-upload" },
    el("h3", {}, workspace === null ? "Upload to the Gallery" : `Upload to ${name}`),
    el("p", { class: "muted" }, "Each photo is compared with the library before it is sent, so what is already there is not sent again."),
    where, summary, rows,
    el("div", { class: "dialog-actions" }, choose, stop, close, input));
  dialog.addEventListener("cancel", (event) => { if (running) event.preventDefault(); });
  dialog.addEventListener("close", () => {
    dialog.remove();
    if (uploadedAny) onUploaded?.();
  });

  const paintSummary = () => {
    const parts = [];
    if (counts.uploaded) parts.push(`${counts.uploaded} uploaded`);
    if (counts.duplicate) parts.push(`${counts.duplicate} already there`);
    if (counts.skipped) parts.push(`${counts.skipped} skipped`);
    if (counts.failed) parts.push(`${counts.failed} failed`);
    if (left) parts.push(`${left} to go`);
    summary.textContent = parts.join(", ");
  };

  function row(file) {
    const note = el("span", { class: "photo-upload-note" }, "Waiting");
    const node = el("li", { class: "photo-upload-row" }, el("span", { class: "photo-upload-name" }, `${file.name} · ${sizeText(file.size)}`), note);
    const set = (text, kind = "") => {
      note.textContent = text;
      node.className = `photo-upload-row${kind ? ` is-${kind}` : ""}`;
      node.querySelector("button")?.remove();
    };
    const offer = (label, run) => {
      node.querySelector("button")?.remove();
      node.append(el("button", { type: "button", onclick: run }, label));
    };
    return { file, node, set, offer };
  }

  async function upload(item, sha256) {
    item.set("Uploading…");
    const path = await send(item.file, folder, (part) => item.set(`Uploading… ${Math.floor(part * 100)}%`), (xhr) => {
      active.add(xhr);
      item.xhr = xhr;
    }).finally(() => active.delete(item.xhr));
    // What arrived, read back on the server.
    item.set("Confirming…");
    const there = await api(`/api/hash?path=${encodeURIComponent(path)}`);
    if (there.sha256 !== sha256) throw new Error("It arrived damaged: its contents do not match. Try it again.");
    counts.uploaded += 1;
    uploadedAny = true;
    item.set(`Uploaded to ${path}`, "uploaded");
  }

  function reportDuplicate(item, text, sha256, kind = "duplicate") {
    counts[kind === "duplicate" ? "duplicate" : "skipped"] += 1;
    item.set(text, "duplicate");
    item.offer("Upload anyway", () => {
      counts[kind === "duplicate" ? "duplicate" : "skipped"] -= 1;
      begin([item], { force: true, known: sha256 });
    });
  }

  async function handle(item, { force, known }) {
    if (stopping) {
      counts.skipped += 1;
      item.set("Not sent: stopped");
      return;
    }
    if (!MEDIA.test(item.file.name) && !/^(image|video)\//.test(item.file.type)) {
      counts.skipped += 1;
      item.set("Skipped: not a photo or video");
      return;
    }
    let sha256 = known;
    try {
      item.set("Comparing with the library…");
      sha256 = known || await hashFile(item.file, () => stopping);
      if (sha256 === null) {
        counts.skipped += 1;
        item.set("Not sent: stopped");
        return;
      }
      if (!force) {
        const earlier = seen.get(sha256);
        if (earlier && earlier !== item) {
          reportDuplicate(item, `The same photo as ${earlier.file.name}, in this upload`, sha256);
          return;
        }
        seen.set(sha256, item);
        const found = await api("/api/gallery/upload/check", {
          method: "POST",
          json: { name: item.file.name, size: item.file.size, sha256, folder, workspace },
        });
        if (found.copy) {
          const whereItIs = workspace === null || found.inWorkspace ? "Already here" : "Already in the library, outside this workspace";
          reportDuplicate(item, `${whereItIs}: ${found.copy}`, sha256);
          return;
        }
        if (found.resized) {
          reportDuplicate(item, `A smaller copy of it is already here: ${found.resized}`, sha256);
          return;
        }
        if (found.blocked) {
          reportDuplicate(item, "Skipped: it was deleted, and kept from being backed up again", sha256, "skipped");
          return;
        }
      }
      await upload(item, sha256);
    } catch (err) {
      // It did not arrive, so another copy of it is no duplicate of this one.
      if (seen.get(sha256) === item) seen.delete(sha256);
      if (err.stopped) {
        counts.skipped += 1;
        item.set("Not sent: stopped");
        return;
      }
      counts.failed += 1;
      item.set(err.message, "failed");
      item.offer("Try again", () => {
        counts.failed -= 1;
        begin([item], { force, known: sha256 });
      });
    }
  }

  async function begin(items, options = {}) {
    waitingRows.push(...items.map((item) => ({ item, options })));
    left += items.length;
    paintSummary();
    if (running) return;
    running = true;
    stopping = false;
    stop.hidden = false;
    choose.disabled = true;
    close.disabled = true;
    const worker = async () => {
      while (waitingRows.length) {
        const next = waitingRows.shift();
        await handle(next.item, next.options);
        left -= 1;
        paintSummary();
      }
    };
    await Promise.all(Array.from({ length: AT_ONCE }, worker));
    running = false;
    stop.hidden = true;
    choose.disabled = false;
    close.disabled = false;
    paintSummary();
    if (counts.failed) toast(`${counts.failed} could not be uploaded: see the list`, true);
  }

  input.addEventListener("change", () => {
    const items = [...input.files].map(row);
    input.value = "";
    if (!items.length) return;
    rows.prepend(...items.map((item) => item.node));
    begin(items);
  });

  document.body.append(dialog);
  dialog.showModal();
}
