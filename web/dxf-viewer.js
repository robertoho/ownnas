const MAX_DXF_BYTES = 50 * 1024 * 1024;
const MAX_DXF_ENTITIES = 150_000;

function values(record, code) {
  return (record || []).filter((pair) => pair.code === code).map((pair) => pair.value);
}

function value(record, code, fallback = "") {
  return (record || []).find((pair) => pair.code === code)?.value ?? fallback;
}

function number(record, code, fallback = NaN) {
  const parsed = Number(value(record, code, fallback));
  return Number.isFinite(parsed) ? parsed : fallback;
}

function rgbHex(raw) {
  const color = Number(raw);
  if (!Number.isFinite(color)) return null;
  return `#${(color & 0xffffff).toString(16).padStart(6, "0")}`;
}

function recordSections(pairs) {
  const sections = new Map();
  for (let i = 0; i < pairs.length; i += 1) {
    if (pairs[i].code !== 0 || pairs[i].value.toUpperCase() !== "SECTION") continue;
    const name = pairs[i + 1]?.code === 2 ? pairs[i + 1].value.toUpperCase() : "";
    const records = [];
    let current = null;
    for (i += 2; i < pairs.length; i += 1) {
      const pair = pairs[i];
      if (pair.code === 0 && pair.value.toUpperCase() === "ENDSEC") {
        if (current) records.push(current);
        break;
      }
      if (pair.code === 0) {
        if (current) records.push(current);
        current = [{ code: 0, value: pair.value }];
      } else if (current) {
        current.push(pair);
      }
    }
    if (!sections.has(name)) sections.set(name, []);
    sections.set(name, [...sections.get(name), ...records]);
  }
  return sections;
}

function parseEntity(record, layerColors) {
  const type = value(record, 0).toUpperCase();
  const layer = value(record, 8, "0") || "0";
  const colorValue = Number(value(record, 62, 256));
  const trueColor = rgbHex(value(record, 420, NaN));
  const colorIndex = Number.isFinite(colorValue) && colorValue !== 256 ? Math.abs(colorValue) : null;
  const color = trueColor || (colorIndex !== null ? colorIndex : layerColors.get(layer) ?? null);
  const entity = { type, layer, color };
  const x = (code) => number(record, code);
  const y = (code) => number(record, code);

  if (type === "LINE") {
    const points = [[x(10), y(20)], [x(11), y(21)]];
    if (points.flat().every(Number.isFinite)) return { ...entity, points };
  } else if (type === "LWPOLYLINE") {
    const xs = values(record, 10).map(Number);
    const ys = values(record, 20).map(Number);
    const points = xs.map((px, index) => [px, ys[index]]).filter((point) => point.every(Number.isFinite));
    if (points.length >= 2) return { ...entity, points, closed: (number(record, 70, 0) & 1) !== 0 };
  } else if (type === "CIRCLE" || type === "ARC") {
    const center = [x(10), y(20)];
    const radius = number(record, 40);
    if (!center.every(Number.isFinite) || !Number.isFinite(radius) || radius <= 0) return null;
    if (type === "CIRCLE") return { ...entity, center, radius };
    let start = number(record, 50) * Math.PI / 180;
    let end = number(record, 51) * Math.PI / 180;
    if (!Number.isFinite(start) || !Number.isFinite(end)) return null;
    while (end <= start) end += Math.PI * 2;
    const segments = Math.max(12, Math.min(240, Math.ceil((end - start) * 24)));
    const points = [];
    for (let i = 0; i <= segments; i += 1) {
      const angle = start + ((end - start) * i) / segments;
      points.push([center[0] + Math.cos(angle) * radius, center[1] + Math.sin(angle) * radius]);
    }
    return { ...entity, points };
  } else if (type === "ELLIPSE") {
    const center = [x(10), y(20)];
    const major = [x(11), y(21)];
    const ratio = number(record, 40, 1);
    let start = number(record, 41, 0);
    let end = number(record, 42, Math.PI * 2);
    if (end <= start) end += Math.PI * 2;
    if (!center.every(Number.isFinite) || !major.every(Number.isFinite) || !Number.isFinite(ratio) || ratio <= 0) return null;
    const segments = Math.max(16, Math.min(240, Math.ceil((end - start) * 32)));
    const points = [];
    for (let i = 0; i <= segments; i += 1) {
      const angle = start + ((end - start) * i) / segments;
      points.push([
        center[0] + major[0] * Math.cos(angle) - major[1] * ratio * Math.sin(angle),
        center[1] + major[1] * Math.cos(angle) + major[0] * ratio * Math.sin(angle),
      ]);
    }
    return { ...entity, points, closed: end - start >= Math.PI * 2 - 1e-6 };
  } else if (type === "TEXT" || type === "MTEXT") {
    const point = [x(10), y(20)];
    if (!point.every(Number.isFinite)) return null;
    const chunks = [...values(record, 3), value(record, 1)].filter(Boolean);
    const text = chunks.join("").replace(/\\P/gi, "\n").replace(/\\~/g, " ").replace(/\\[A-Za-z][^;]*;/g, "");
    if (!text) return null;
    return { ...entity, point, text, height: Math.abs(number(record, 40, 1)), rotation: number(record, 50, 0) };
  }
  return null;
}

function parsePolyline(records, start, layerColors) {
  const header = records[start];
  const layer = value(header, 8, "0") || "0";
  const points = [];
  let i = start + 1;
  for (; i < records.length && value(records[i], 0).toUpperCase() === "VERTEX"; i += 1) {
    const point = [number(records[i], 10), number(records[i], 20)];
    if (point.every(Number.isFinite)) points.push(point);
  }
  if (value(records[i], 0).toUpperCase() === "SEQEND") i += 1;
  const colorValue = Number(value(header, 62, 256));
  const trueColor = rgbHex(value(header, 420, NaN));
  const color = trueColor || (Number.isFinite(colorValue) && colorValue !== 256 ? Math.abs(colorValue) : layerColors.get(layer) ?? null);
  return {
    next: i,
    entity: points.length >= 2
      ? { type: "POLYLINE", layer, color, points, closed: (number(header, 70, 0) & 1) !== 0 }
      : null,
  };
}

function getBounds(entities) {
  const bounds = { minX: Infinity, minY: Infinity, maxX: -Infinity, maxY: -Infinity };
  const add = (x, y) => {
    if (!Number.isFinite(x) || !Number.isFinite(y)) return;
    bounds.minX = Math.min(bounds.minX, x); bounds.maxX = Math.max(bounds.maxX, x);
    bounds.minY = Math.min(bounds.minY, y); bounds.maxY = Math.max(bounds.maxY, y);
  };
  for (const entity of entities) {
    if (entity.points) entity.points.forEach(([x, y]) => add(x, y));
    if (entity.center) {
      add(entity.center[0] - entity.radius, entity.center[1] - entity.radius);
      add(entity.center[0] + entity.radius, entity.center[1] + entity.radius);
    }
    if (entity.point) add(entity.point[0], entity.point[1]);
  }
  if (!Number.isFinite(bounds.minX)) throw new Error("No supported drawing geometry was found in this DXF file.");
  if (bounds.maxX - bounds.minX < 1e-8) { bounds.minX -= 0.5; bounds.maxX += 0.5; }
  if (bounds.maxY - bounds.minY < 1e-8) { bounds.minY -= 0.5; bounds.maxY += 0.5; }
  return bounds;
}

export function parseDxf(source, { maxEntities = MAX_DXF_ENTITIES } = {}) {
  const lines = String(source || "").replace(/^\uFEFF/, "").split(/\r?\n/);
  const pairs = [];
  for (let i = 0; i + 1 < lines.length; i += 2) {
    const code = Number.parseInt(lines[i].trim(), 10);
    if (Number.isFinite(code)) pairs.push({ code, value: lines[i + 1].trim() });
  }
  if (pairs.length < 4) throw new Error("This file does not contain a readable ASCII DXF drawing.");
  const sections = recordSections(pairs);
  const entityRecords = sections.get("ENTITIES");
  if (!entityRecords) throw new Error("This DXF file has no ENTITIES section.");

  const layerColors = new Map();
  for (const record of sections.get("TABLES") || []) {
    if (value(record, 0).toUpperCase() !== "LAYER") continue;
    const name = value(record, 2, "0");
    const trueColor = rgbHex(value(record, 420, NaN));
    const color = Number(value(record, 62, 7));
    layerColors.set(name, trueColor || (Number.isFinite(color) ? Math.abs(color) : 7));
  }

  const entities = [];
  let skipped = 0;
  for (let i = 0; i < entityRecords.length;) {
    const type = value(entityRecords[i], 0).toUpperCase();
    if (type === "POLYLINE") {
      const parsed = parsePolyline(entityRecords, i, layerColors);
      i = Math.max(i + 1, parsed.next);
      if (parsed.entity) entities.push(parsed.entity); else skipped += 1;
    } else {
      const entity = parseEntity(entityRecords[i], layerColors);
      if (entity) entities.push(entity);
      else skipped += 1;
      i += 1;
    }
    if (entities.length > maxEntities) throw new Error(`This drawing is too large to preview (limit: ${maxEntities.toLocaleString()} entities).`);
  }
  if (!entities.length) throw new Error("No supported drawing geometry was found in this DXF file.");
  const layers = [...new Set(entities.map((entity) => entity.layer))].sort((a, b) => a.localeCompare(b));
  return { entities, layers, skipped, bounds: getBounds(entities) };
}

function colorFor(entity, layerColors, theme) {
  let color = entity.color;
  if (color == null) color = layerColors?.get(entity.layer);
  if (typeof color === "string" && color.startsWith("#")) return color;
  if (color == null || color === 7) return theme.text;
  if (color === 8 || color === 9) return theme.muted;
  const basic = { 1: "#ff4d4d", 2: "#ffd43b", 3: "#36c982", 4: "#35c5d8", 5: "#6f8cff", 6: "#d878eb" };
  if (basic[color]) return basic[color];
  const hue = ((Math.abs(color) * 137.508) % 360).toFixed(1);
  return `hsl(${hue} 75% 62%)`;
}

function boundsForEntities(entities) {
  return getBounds(entities);
}

export function disposeDxfViewer() {
  viewerGeneration += 1;
  activeLoadController?.abort();
  activeLoadController = null;
  if (!activeViewer) return;
  activeViewer.destroy();
  activeViewer = null;
}

let activeViewer = null;
let activeLoadController = null;
let viewerGeneration = 0;

export async function mountDxfViewer(container, { url, name, size = 0 }) {
  disposeDxfViewer();
  const generation = viewerGeneration;
  if (/\.dwg$/i.test(name)) {
    container.innerHTML = `<div class="cad-viewer-message"><strong>DWG preview is unavailable in this build.</strong><p>DWG is a proprietary binary CAD format. Save or export the drawing as DXF in a CAD application, then open the DXF here.</p></div>`;
    return;
  }
  if (!/\.dxf$/i.test(name)) throw new Error("Unsupported CAD drawing format.");
  if (Number(size) > MAX_DXF_BYTES) throw new Error("DXF previews are limited to 50 MB.");

  const controller = new AbortController();
  activeLoadController = controller;
  const response = await fetch(url, { credentials: "same-origin", cache: "no-store", signal: controller.signal });
  if (!response.ok) throw new Error("Could not load this DXF drawing.");
  const contentLength = Number(response.headers.get("content-length") || 0);
  if (contentLength > MAX_DXF_BYTES) throw new Error("DXF previews are limited to 50 MB.");
  const source = await response.text();
  if (generation !== viewerGeneration) return;
  activeLoadController = null;
  if (source.length > MAX_DXF_BYTES) throw new Error("DXF previews are limited to 50 MB.");
  const drawing = parseDxf(source);

  const wrap = document.createElement("div");
  wrap.className = "cad-viewer";
  wrap.innerHTML = `<div class="cad-viewer-toolbar"><button type="button" data-cad="fit">Fit drawing</button><label>Layer <select data-cad="layer"><option value="">All layers</option></select></label><span class="cad-viewer-hint">Drag to pan · scroll to zoom</span></div><div class="cad-viewer-canvas"><canvas aria-label="DXF drawing"></canvas></div><p class="cad-viewer-status" role="status"></p>`;
  container.replaceChildren(wrap);
  const canvas = wrap.querySelector("canvas");
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("Canvas drawing is not available in this browser.");
  const layerSelect = wrap.querySelector("select");
  for (const layer of drawing.layers) {
    const option = document.createElement("option"); option.value = layer; option.textContent = layer;
    layerSelect.appendChild(option);
  }
  const status = wrap.querySelector(".cad-viewer-status");
  const theme = () => {
    const styles = getComputedStyle(document.documentElement);
    return {
      bg: styles.getPropertyValue("--bg-2").trim() || "#20251f",
      line: styles.getPropertyValue("--line").trim() || "#424b40",
      text: styles.getPropertyValue("--text").trim() || "#edf1e7",
      muted: styles.getPropertyValue("--muted").trim() || "#9da696",
    };
  };
  const camera = { scale: 1, x: 0, y: 0 };
  let width = 1, height = 1, dpr = 1, dragging = false, lastX = 0, lastY = 0;
  let destroyed = false;

  function selectedEntities() {
    const layer = layerSelect.value;
    return layer ? drawing.entities.filter((entity) => entity.layer === layer) : drawing.entities;
  }

  function resize() {
    if (destroyed) return;
    const rect = canvas.parentElement.getBoundingClientRect();
    width = Math.max(1, rect.width); height = Math.max(1, rect.height);
    dpr = Math.min(window.devicePixelRatio || 1, 2);
    canvas.width = Math.round(width * dpr); canvas.height = Math.round(height * dpr);
    draw();
  }

  function fit() {
    const bounds = boundsForEntities(selectedEntities());
    const pad = 28;
    const worldWidth = bounds.maxX - bounds.minX, worldHeight = bounds.maxY - bounds.minY;
    camera.scale = Math.max(1e-8, Math.min((width - pad * 2) / worldWidth, (height - pad * 2) / worldHeight));
    camera.x = (width - worldWidth * camera.scale) / 2 - bounds.minX * camera.scale;
    camera.y = (height - worldHeight * camera.scale) / 2 - bounds.minY * camera.scale;
    draw();
  }

  function draw() {
    if (destroyed || !ctx) return;
    const colors = theme();
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.fillStyle = colors.bg; ctx.fillRect(0, 0, canvas.width, canvas.height);
    ctx.setTransform(dpr * camera.scale, 0, 0, -dpr * camera.scale, dpr * camera.x, dpr * (height - camera.y));
    ctx.lineWidth = 1 / Math.max(camera.scale, 1e-8);
    ctx.lineCap = "round"; ctx.lineJoin = "round";
    const entities = selectedEntities();
    for (const entity of entities) {
      ctx.strokeStyle = colorFor(entity, null, colors);
      ctx.fillStyle = ctx.strokeStyle;
      if (entity.center) {
        ctx.beginPath(); ctx.arc(entity.center[0], entity.center[1], entity.radius, 0, Math.PI * 2); ctx.stroke();
      } else if (entity.points?.length) {
        ctx.beginPath(); ctx.moveTo(entity.points[0][0], entity.points[0][1]);
        for (let i = 1; i < entity.points.length; i += 1) ctx.lineTo(entity.points[i][0], entity.points[i][1]);
        if (entity.closed) ctx.closePath();
        ctx.stroke();
      } else if (entity.point) {
        const screenX = entity.point[0] * camera.scale + camera.x;
        const screenY = height - (entity.point[1] * camera.scale + camera.y);
        const fontSize = Math.max(8, Math.min(40, entity.height * camera.scale));
        const lines = entity.text.split("\n");
        ctx.save(); ctx.setTransform(dpr, 0, 0, dpr, dpr * screenX, dpr * screenY);
        ctx.rotate(-(entity.rotation || 0) * Math.PI / 180);
        ctx.font = `${fontSize}px sans-serif`; ctx.textBaseline = "top";
        lines.forEach((line, index) => ctx.fillText(line, 0, index * fontSize * 1.15));
        ctx.restore();
      }
    }
    const skipped = drawing.skipped ? ` · ${drawing.skipped} unsupported entities skipped` : "";
    status.textContent = `${entities.length.toLocaleString()} entities · ${drawing.layers.length.toLocaleString()} layers${skipped}`;
  }

  function onPointerDown(event) {
    if (event.button !== 0) return;
    dragging = true; lastX = event.clientX; lastY = event.clientY; canvas.setPointerCapture(event.pointerId);
  }
  function onPointerMove(event) {
    if (!dragging) return;
    const dx = event.clientX - lastX, dy = event.clientY - lastY;
    lastX = event.clientX; lastY = event.clientY;
    camera.x += dx; camera.y -= dy; draw();
  }
  function onPointerUp() { dragging = false; }
  function onWheel(event) {
    event.preventDefault();
    const rect = canvas.getBoundingClientRect();
    const x = event.clientX - rect.left, y = event.clientY - rect.top;
    const worldX = (x - camera.x) / camera.scale;
    const worldY = (height - y - camera.y) / camera.scale;
    const next = Math.max(1e-8, Math.min(camera.scale * (event.deltaY < 0 ? 1.15 : 1 / 1.15), 1e8));
    camera.scale = next; camera.x = x - worldX * next; camera.y = height - y - worldY * next;
    draw();
  }
  wrap.querySelector('[data-cad="fit"]').addEventListener("click", fit);
  layerSelect.addEventListener("change", fit);
  canvas.addEventListener("pointerdown", onPointerDown);
  canvas.addEventListener("pointermove", onPointerMove);
  canvas.addEventListener("pointerup", onPointerUp);
  canvas.addEventListener("pointercancel", onPointerUp);
  canvas.addEventListener("wheel", onWheel, { passive: false });
  const observer = typeof ResizeObserver === "function" ? new ResizeObserver(resize) : null;
  if (observer) observer.observe(canvas.parentElement); else window.addEventListener("resize", resize);
  const viewer = {
    destroy() {
      destroyed = true; observer?.disconnect();
      window.removeEventListener("resize", resize);
      canvas.removeEventListener("pointerdown", onPointerDown);
      canvas.removeEventListener("pointermove", onPointerMove);
      canvas.removeEventListener("pointerup", onPointerUp);
      canvas.removeEventListener("pointercancel", onPointerUp);
      canvas.removeEventListener("wheel", onWheel);
      wrap.remove();
    },
  };
  activeViewer = viewer;
  resize(); fit();
  return viewer;
}
