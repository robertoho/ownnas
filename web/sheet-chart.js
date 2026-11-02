/**
 * Charts of XLSX workbooks: reads a chart part (DrawingML charts) into a plain description, draws
 * one as SVG from the values of its series, and writes a new chart part. Column, bar, line, area,
 * pie, doughnut and scatter charts are drawn, stacked or not; other kinds keep their place.
 */

const CHART = 'http://schemas.openxmlformats.org/drawingml/2006/chart';
const DRAWING = 'http://schemas.openxmlformats.org/drawingml/2006/main';
const DOC_REL = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships';

// Office's default series colours, when the theme gives none: its six accents.
const DEFAULT_ACCENTS = ['4472C4', 'ED7D31', 'A5A5A5', 'FFC000', '5B9BD5', '70AD47'];
const KINDS = {
  barChart: 'bar', bar3DChart: 'bar', lineChart: 'line', line3DChart: 'line', stockChart: 'line', radarChart: 'line',
  areaChart: 'area', area3DChart: 'area', pieChart: 'pie', pie3DChart: 'pie', ofPieChart: 'pie', doughnutChart: 'doughnut',
  scatterChart: 'scatter', bubbleChart: 'scatter',
};

const child = (node, name, ns = CHART) => node && [...node.children].find(item => item.localName === name && item.namespaceURI === ns) || null;
const children = (node, name, ns = CHART) => (node ? [...node.children].filter(item => item.localName === name && item.namespaceURI === ns) : []);
const val = (node, fallback = null) => node?.getAttribute('val') ?? fallback;
const on = (node, fallback = false) => (node ? !['0', 'false'].includes(node.getAttribute('val') ?? '1') : fallback);

/** A colour of DrawingML (srgbClr, schemeClr, sysClr…, with lumMod/lumOff/shade/tint), as #RRGGBB. */
function drawingColor(fill, theme) {
  const color = fill && [...fill.children].find(item => /Clr$/.test(item.localName));
  if (!color) return null;
  let hex = null;
  if (color.localName === 'srgbClr') hex = color.getAttribute('val');
  else if (color.localName === 'sysClr') hex = color.getAttribute('lastClr') || (color.getAttribute('val') === 'window' ? 'FFFFFF' : '000000');
  else if (color.localName === 'schemeClr') {
    const names = { bg1: 0, tx1: 1, bg2: 2, tx2: 3, lt1: 0, dk1: 1, lt2: 2, dk2: 3, accent1: 4, accent2: 5, accent3: 6, accent4: 7, accent5: 8, accent6: 9, hlink: 10, folHlink: 11 };
    const index = names[color.getAttribute('val')];
    hex = index == null ? null : theme[index] || (index >= 4 && index <= 9 ? DEFAULT_ACCENTS[index - 4] : index === 0 ? 'FFFFFF' : '000000');
  } else if (color.localName === 'prstClr') {
    hex = { black: '000000', white: 'FFFFFF', red: 'FF0000', green: '00FF00', blue: '0000FF', yellow: 'FFFF00', gray: '808080' }[color.getAttribute('val')] || null;
  }
  if (!hex || !/^[0-9a-f]{6}$/i.test(hex)) return null;
  let [r, g, b] = [0, 2, 4].map(i => parseInt(hex.slice(i, i + 2), 16));
  for (const step of color.children) {
    const amount = Number(step.getAttribute('val')) / 100000;
    if (!Number.isFinite(amount)) continue;
    if (step.localName === 'lumMod') [r, g, b] = [r, g, b].map(v => v * amount);
    else if (step.localName === 'lumOff') [r, g, b] = [r, g, b].map(v => v + 255 * amount);
    else if (step.localName === 'shade') [r, g, b] = [r, g, b].map(v => v * amount);
    else if (step.localName === 'tint') [r, g, b] = [r, g, b].map(v => 255 - (255 - v) * amount);
  }
  return `#${[r, g, b].map(v => Math.round(Math.max(0, Math.min(255, v))).toString(16).padStart(2, '0')).join('').toUpperCase()}`;
}

/** The fill and line colours of a shape's properties (c:spPr). */
function shapeColors(spPr, theme) {
  const fill = child(spPr, 'solidFill', DRAWING);
  const line = child(spPr, 'ln', DRAWING);
  return {
    fill: fill ? drawingColor(fill, theme) : child(spPr, 'noFill', DRAWING) ? 'none' : null,
    line: line ? (child(line, 'noFill', DRAWING) ? 'none' : drawingColor(child(line, 'solidFill', DRAWING), theme)) : null,
  };
}

/** Text of a title or series name: the typed runs, or the formula it reads. */
function richText(tx) {
  if (!tx) return null;
  const rich = child(tx, 'rich');
  if (rich) return { text: [...rich.getElementsByTagNameNS(DRAWING, 'p')].map(p => [...p.getElementsByTagNameNS(DRAWING, 't')].map(t => t.textContent).join('')).join('\n') };
  const ref = child(tx, 'strRef');
  if (ref) return { ref: child(ref, 'f')?.textContent || null, cache: cacheOf(ref) };
  const text = child(tx, 'v');
  return text ? { text: text.textContent } : null;
}

/** The values a reference (numRef, strRef, multiLvlStrRef) last had, as the file saved them. */
function cacheOf(ref) {
  const cache = child(ref, 'numCache') || child(ref, 'strCache') || child(ref, 'multiLvlStrCache');
  if (!cache) return [];
  const count = Number(val(child(cache, 'ptCount'), 0));
  const out = new Array(Math.min(count, 100000)).fill(null);
  const points = cache.localName === 'multiLvlStrCache' ? children(children(cache, 'lvl')[0], 'pt') : children(cache, 'pt');
  for (const point of points) {
    const index = Number(point.getAttribute('idx'));
    const text = child(point, 'v')?.textContent ?? null;
    if (index >= 0 && index < out.length) out[index] = cache.localName === 'numCache' && text != null && text !== '' ? Number(text) : text;
  }
  return out;
}

/** Where a series' categories or values come from: { ref, cache, format }, or literal values. */
function dataOf(node) {
  if (!node) return null;
  const ref = child(node, 'numRef') || child(node, 'strRef') || child(node, 'multiLvlStrRef');
  if (ref) {
    const cache = child(ref, 'numCache');
    return { ref: child(ref, 'f')?.textContent || null, cache: cacheOf(ref), format: child(cache, 'formatCode')?.textContent || null, numeric: ref.localName === 'numRef' };
  }
  const literal = child(node, 'numLit') || child(node, 'strLit');
  if (literal) return { ref: null, cache: literalCache(literal), numeric: literal.localName === 'numLit' };
  return null;
}
function literalCache(literal) {
  const count = Number(val(child(literal, 'ptCount'), 0));
  const out = new Array(Math.min(count, 100000)).fill(null);
  for (const point of children(literal, 'pt')) {
    const index = Number(point.getAttribute('idx'));
    const text = child(point, 'v')?.textContent ?? null;
    if (index >= 0 && index < out.length) out[index] = literal.localName === 'numLit' && text != null ? Number(text) : text;
  }
  return out;
}

/** A chart part, read: { title, legend, plots: [{ kind, horizontal, grouping, series }] }. */
export function readChart(xml, theme = []) {
  const space = xml.documentElement;
  const chart = child(space, 'chart');
  if (!chart) return null;
  const titleNode = child(chart, 'title');
  const title = titleNode ? richText(child(titleNode, 'tx')) || { auto: true } : null;
  const plotArea = child(chart, 'plotArea');
  const plots = [];
  let seriesIndex = 0;
  for (const node of plotArea ? [...plotArea.children] : []) {
    const kind = KINDS[node.localName];
    if (!kind) continue;
    const plot = {
      kind,
      type: node.localName,
      horizontal: val(child(node, 'barDir')) === 'bar',
      grouping: val(child(node, 'grouping'), kind === 'bar' ? 'clustered' : 'standard'),
      holeSize: Number(val(child(node, 'holeSize'), 50)),
      // Lines drawn between scatter points, or only the markers (a series can also hide its line).
      scatterLines: kind === 'scatter' && !['marker', 'none'].includes(val(child(node, 'scatterStyle'), 'lineMarker')),
      varyColors: on(child(node, 'varyColors'), kind === 'pie' || kind === 'doughnut'),
      series: [],
    };
    for (const ser of children(node, 'ser')) {
      const order = Number(val(child(ser, 'order'), seriesIndex));
      const colors = shapeColors(child(ser, 'spPr'), theme);
      const marker = child(ser, 'marker');
      const points = new Map();
      for (const dPt of children(ser, 'dPt')) {
        const color = shapeColors(child(dPt, 'spPr'), theme).fill;
        if (color) points.set(Number(val(child(dPt, 'idx'), 0)), color);
      }
      plot.series.push({
        order,
        name: richText(child(ser, 'tx')),
        categories: dataOf(child(ser, 'cat') || child(ser, 'xVal')),
        values: dataOf(child(ser, 'val') || child(ser, 'yVal')),
        fill: colors.fill, line: colors.line, points,
        marker: marker ? val(child(marker, 'symbol'), 'auto') !== 'none' : kind === 'line' || kind === 'scatter',
        smooth: on(child(ser, 'smooth')),
      });
      seriesIndex++;
    }
    if (plot.series.length) plots.push(plot);
  }
  const legendNode = child(chart, 'legend');
  return {
    title,
    autoTitleDeleted: on(child(chart, 'autoTitleDeleted')),
    legend: legendNode ? val(child(legendNode, 'legendPos'), 'r') : null,
    visibleOnly: on(child(chart, 'plotVisOnly'), true),
    plots,
  };
}

// --- Drawing ------------------------------------------------------------------------------------

const escapeXml = text => String(text).replace(/[&<>"']/g, char => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[char]);
const FONT = 'font-family="Calibri, Carlito, \'Segoe UI\', Arial, sans-serif"';
const textWidth = (text, size) => String(text).length * size * 0.55;

/** Round steps for an axis from `min` to `max`: { min, max, step }. */
export function niceScale(min, max, ticks = 5) {
  if (!Number.isFinite(min) || !Number.isFinite(max)) return { min: 0, max: 1, step: 0.2 };
  if (min === max) { if (min === 0) return { min: 0, max: 1, step: 0.2 }; if (min > 0) min = 0; else max = 0; }
  const span = max - min;
  const raw = span / ticks;
  const power = 10 ** Math.floor(Math.log10(raw));
  const step = [1, 2, 2.5, 5, 10].map(m => m * power).find(s => s >= raw) || 10 * power;
  return { min: Math.floor(min / step + 1e-9) * step, max: Math.ceil(max / step - 1e-9) * step, step };
}

/** A short label for an axis number: as many decimals as the step needs. */
function axisLabel(value, step, format) {
  if (format) { const text = format(value); if (text != null) return text; }
  const places = Math.max(0, Math.min(10, -Math.floor(Math.log10(step) + 1e-9)));
  return Number(value.toFixed(places)).toLocaleString(undefined, { maximumFractionDigits: places });
}

/**
 * Draws a chart as SVG, `width` × `height` pixels. `data` gives each series of each plot
 * resolved: { name, categories: [text], values: [number|null] }. `format(value, code)` shows a
 * number in a format code, as the sheet shows it.
 */
export function chartSvg(chart, data, width, height, { theme = [], format = null } = {}) {
  const out = [];
  const accent = i => {
    const base = theme[4 + (i % 6)] || DEFAULT_ACCENTS[i % 6];
    // After the six accents, darker then lighter turns of them, as Office does.
    const round = Math.floor(i / 6);
    if (!round) return `#${base}`;
    const factor = round % 2 ? 0.6 : 1.4;
    return `#${[0, 2, 4].map(at => Math.round(Math.max(0, Math.min(255, parseInt(base.slice(at, at + 2), 16) * factor))).toString(16).padStart(2, '0')).join('').toUpperCase()}`;
  };
  const plots = chart.plots.map((plot, p) => ({ ...plot, data: data[p] || [] })).filter(plot => plot.data.length);
  const all = plots.flatMap(plot => plot.data.map((series, s) => ({ plot, series, spec: plot.series[s] })));
  const colorOf = (spec, index) => spec.fill && spec.fill !== 'none' ? spec.fill : spec.line && spec.line !== 'none' ? spec.line : accent(index);
  all.forEach((item, i) => { item.color = colorOf(item.spec, item.spec.order ?? i); });
  out.push(`<rect x="0.5" y="0.5" width="${width - 1}" height="${height - 1}" fill="#FFFFFF" stroke="#D9D9D9"/>`);
  let top = 8, bottom = height - 8, left = 8, right = width - 8;
  // The title: the chart's, or the only series' name.
  let titleText = chart.title?.text ?? null;
  if (chart.title && titleText == null) titleText = all.length === 1 ? all[0].series.name || null : chart.title.auto ? 'Chart title' : null;
  if (titleText) {
    const size = Math.min(18, Math.max(11, height / 18));
    out.push(`<text x="${width / 2}" y="${top + size}" text-anchor="middle" font-size="${size.toFixed(1)}" fill="#595959" ${FONT}>${escapeXml(titleText)}</text>`);
    top += size + 10;
  }
  const pieLike = plots.length && ['pie', 'doughnut'].includes(plots[0].kind);
  // The legend: series, or a pie's categories.
  const legendItems = pieLike
    ? (plots[0].data[0]?.categories || []).map((name, i) => ({ name: name ?? String(i + 1), color: plots[0].series[0].points.get(i) || accent(i) }))
    : all.map(item => ({ name: item.series.name || `Series ${all.indexOf(item) + 1}`, color: item.color, line: lined(item), dot: !lined(item) && item.plot.kind === 'scatter' }));
  if (chart.legend && legendItems.length) {
    const size = 10.5;
    if (chart.legend === 'r' || chart.legend === 'l' || chart.legend === 'tr') {
      const widest = Math.min(width * 0.35, Math.max(...legendItems.map(item => textWidth(item.name, size))) + 22);
      const x = chart.legend === 'l' ? left : right - widest;
      const shown = legendItems.slice(0, Math.max(1, Math.floor((bottom - top) / 16)));
      let y = (top + bottom) / 2 - (shown.length * 16) / 2 + 12;
      for (const item of shown) {
        out.push(legendKey(item, x, y - 4), `<text x="${x + 18}" y="${y}" font-size="${size}" fill="#595959" ${FONT}>${escapeXml(clip(item.name, widest - 20, size))}</text>`);
        y += 16;
      }
      if (chart.legend === 'l') left += widest + 6; else right -= widest + 6;
    } else {
      // Along the top or bottom, in rows.
      const rows = [];
      let row = [], used = 0;
      for (const item of legendItems) {
        const w = Math.min(textWidth(item.name, size) + 28, right - left);
        if (used + w > right - left && row.length) { rows.push({ row, used }); row = []; used = 0; }
        row.push({ ...item, w });
        used += w;
      }
      if (row.length) rows.push({ row, used });
      const shownRows = rows.slice(0, 3);
      const block = shownRows.length * 16;
      let y = chart.legend === 't' ? top + 11 : bottom - block + 11;
      for (const { row: items, used: total } of shownRows) {
        let x = (left + right) / 2 - total / 2;
        for (const item of items) {
          out.push(legendKey(item, x, y - 4), `<text x="${x + 18}" y="${y}" font-size="${size}" fill="#595959" ${FONT}>${escapeXml(clip(item.name, item.w - 26, size))}</text>`);
          x += item.w;
        }
        y += 16;
      }
      if (chart.legend === 't') top += block + 4; else bottom -= block + 4;
    }
  }
  if (!plots.length || right - left < 20 || bottom - top < 20) return svg(width, height, out);
  if (pieLike) drawPie(out, plots[0], { left, top, right, bottom }, accent);
  else drawAxes(out, plots, all, { left, top, right, bottom }, format, accent);
  return svg(width, height, out);
}

/** As Excel does, an axis starts at zero unless its values are all far from it. */
function fromZero(min, max) {
  if (min > 0 && min < (max * 5) / 6) return [0, max];
  if (max < 0 && max > (min * 5) / 6) return [min, 0];
  return [min, max];
}

function svg(width, height, parts) {
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${width} ${height}" width="${width}" height="${height}" role="img">${parts.join('')}</svg>`;
}

function clip(text, room, size) {
  const value = String(text);
  if (textWidth(value, size) <= room) return value;
  const keep = Math.max(1, Math.floor(room / (size * 0.55)) - 1);
  return `${value.slice(0, keep)}…`;
}

function legendKey(item, x, y) {
  if (item.dot) return `<circle cx="${x + 6.5}" cy="${y}" r="3.5" fill="${item.color}"/>`;
  return item.line
    ? `<line x1="${x}" y1="${y}" x2="${x + 14}" y2="${y}" stroke="${item.color}" stroke-width="2"/>`
    : `<rect x="${x + 2}" y="${y - 4}" width="9" height="9" fill="${item.color}"/>`;
}

function drawPie(out, plot, box, accent) {
  const series = plot.data[0];
  const spec = plot.series[0];
  const values = series.values.map(value => (typeof value === 'number' && value > 0 ? value : 0));
  const total = values.reduce((sum, value) => sum + value, 0);
  const cx = (box.left + box.right) / 2, cy = (box.top + box.bottom) / 2;
  const radius = Math.max(4, Math.min(box.right - box.left, box.bottom - box.top) / 2 - 4);
  const inner = plot.kind === 'doughnut' ? radius * Math.max(10, Math.min(90, plot.holeSize)) / 100 : 0;
  if (!total) { out.push(`<circle cx="${cx}" cy="${cy}" r="${radius}" fill="none" stroke="#D9D9D9"/>`); return; }
  let angle = -Math.PI / 2;
  const point = (r, a) => `${(cx + r * Math.cos(a)).toFixed(2)} ${(cy + r * Math.sin(a)).toFixed(2)}`;
  values.forEach((value, i) => {
    if (!value) return;
    const sweep = (value / total) * Math.PI * 2;
    const end = angle + sweep;
    const color = spec.points.get(i) || (plot.varyColors ? accent(i) : spec.fill || accent(0));
    const large = sweep > Math.PI ? 1 : 0;
    if (sweep >= Math.PI * 2 - 1e-9) {
      out.push(`<circle cx="${cx}" cy="${cy}" r="${radius}" fill="${color}" stroke="#FFFFFF"/>`);
      if (inner) out.push(`<circle cx="${cx}" cy="${cy}" r="${inner}" fill="#FFFFFF"/>`);
    } else {
      const path = inner
        ? `M${point(radius, angle)} A${radius} ${radius} 0 ${large} 1 ${point(radius, end)} L${point(inner, end)} A${inner} ${inner} 0 ${large} 0 ${point(inner, angle)} Z`
        : `M${cx} ${cy} L${point(radius, angle)} A${radius} ${radius} 0 ${large} 1 ${point(radius, end)} Z`;
      out.push(`<path d="${path}" fill="${color}" stroke="#FFFFFF" stroke-width="1"/>`);
    }
    angle = end;
  });
}

function drawAxes(out, plots, all, box, format, accent) {
  const scatter = plots.every(plot => plot.kind === 'scatter');
  const horizontal = plots.some(plot => plot.kind === 'bar' && plot.horizontal);
  const count = Math.max(0, ...all.map(item => item.series.values.length));
  const categories = all.find(item => item.series.categories?.some(c => c != null))?.series.categories || [];
  // The value range, with stacked plots adding up.
  let min = Infinity, max = -Infinity;
  const take = value => { if (typeof value === 'number' && Number.isFinite(value)) { min = Math.min(min, value); max = Math.max(max, value); } };
  for (const plot of plots) {
    if (plot.grouping === 'percentStacked') { take(0); take(1); continue; }
    if (plot.grouping === 'stacked') {
      for (let i = 0; i < count; i++) {
        let up = 0, down = 0;
        for (const series of plot.data) { const v = series.values[i]; if (typeof v === 'number') { if (v >= 0) up += v; else down += v; } }
        take(up); take(down);
      }
    } else for (const series of plot.data) series.values.forEach(take);
  }
  if (min === Infinity) { min = 0; max = 1; }
  if (plots.some(plot => plot.kind === 'bar' || plot.kind === 'area')) { min = Math.min(0, min); max = Math.max(0, max); }
  [min, max] = fromZero(min, max);
  // Room above the highest value (and below the lowest), as Excel leaves.
  if (max > 0 && !plots.every(plot => plot.grouping === 'percentStacked')) max += (max - min) * 0.05;
  if (min < 0) min -= (max - min) * 0.05;
  const percent = plots.every(plot => plot.grouping === 'percentStacked');
  const scale = niceScale(min, max);
  const numberFormat = all.find(item => item.spec.values?.format && item.spec.values.format !== 'General')?.spec.values.format;
  const label = value => (percent ? `${Math.round(value * 100)}%` : axisLabel(value, scale.step, format && numberFormat ? v => format(v, numberFormat) : null));
  // X values of a scatter chart.
  let xScale = null;
  if (scatter) {
    let xMin = Infinity, xMax = -Infinity;
    for (const item of all) item.series.categories?.forEach((x, i) => { const v = typeof x === 'number' ? x : i + 1; xMin = Math.min(xMin, v); xMax = Math.max(xMax, v); });
    xScale = niceScale(...fromZero(xMin === Infinity ? 0 : xMin, xMax === -Infinity ? 1 : xMax));
  }
  const size = 10;
  const ticks = [];
  for (let v = scale.min; v <= scale.max + scale.step / 2; v += scale.step) ticks.push(Number(v.toPrecision(12)));
  const valueRoom = Math.max(...ticks.map(v => textWidth(label(v), size))) + 6;
  const catLabels = scatter ? [] : Array.from({ length: count }, (_, i) => categories[i] ?? String(i + 1));
  let { left, right, top, bottom } = box;
  if (horizontal) {
    left += Math.min((right - left) * 0.35, Math.max(0, ...catLabels.map(text => textWidth(text, size))) + 6);
    bottom -= size + 6;
  } else {
    left += valueRoom;
    bottom -= size + 8;
  }
  if (scatter) bottom = box.bottom - size - 8;
  top += 4;
  right -= 6;
  if (right - left < 10 || bottom - top < 10) return;
  const span = scale.max - scale.min || 1;
  const along = v => (horizontal ? left + ((v - scale.min) / span) * (right - left) : bottom - ((v - scale.min) / span) * (bottom - top));
  // Gridlines and value labels.
  for (const v of ticks) {
    const at = along(v);
    if (horizontal) {
      out.push(`<line x1="${at.toFixed(1)}" y1="${top}" x2="${at.toFixed(1)}" y2="${bottom}" stroke="#D9D9D9"/>`);
      out.push(`<text x="${at.toFixed(1)}" y="${bottom + size + 4}" text-anchor="middle" font-size="${size}" fill="#595959" ${FONT}>${escapeXml(label(v))}</text>`);
    } else {
      out.push(`<line x1="${left}" y1="${at.toFixed(1)}" x2="${right}" y2="${at.toFixed(1)}" stroke="#D9D9D9"/>`);
      out.push(`<text x="${left - 4}" y="${(at + size / 3).toFixed(1)}" text-anchor="end" font-size="${size}" fill="#595959" ${FONT}>${escapeXml(label(v))}</text>`);
    }
  }
  const zero = along(Math.max(scale.min, Math.min(scale.max, 0)));
  if (scatter) {
    const xSpan = xScale.max - xScale.min || 1;
    const x = v => left + ((v - xScale.min) / xSpan) * (right - left);
    for (let v = xScale.min; v <= xScale.max + xScale.step / 2; v += xScale.step) {
      const at = x(Number(v.toPrecision(12)));
      out.push(`<text x="${at.toFixed(1)}" y="${bottom + size + 4}" text-anchor="middle" font-size="${size}" fill="#595959" ${FONT}>${escapeXml(axisLabel(Number(v.toPrecision(12)), xScale.step))}</text>`);
    }
    out.push(`<line x1="${left}" y1="${zero}" x2="${right}" y2="${zero}" stroke="#BFBFBF"/>`);
    for (const item of all) {
      const points = item.series.values.map((v, i) => {
        const xv = item.series.categories?.[i];
        return typeof v === 'number' ? [x(typeof xv === 'number' ? xv : i + 1), along(v)] : null;
      });
      drawLine(out, points, item.color, lined(item), item.spec.marker, item.spec.smooth);
    }
    return;
  }
  // Category labels, thinned out when they would overlap.
  const slot = (horizontal ? bottom - top : right - left) / Math.max(1, count);
  // Where category i's place starts; a bar chart lists the first at the bottom, as Excel does.
  const slotStart = i => (horizontal ? bottom - slot * (i + 1) : left + slot * i);
  const every = Math.max(1, Math.ceil((horizontal ? size + 2 : Math.max(...catLabels.map(text => textWidth(text, size)), 1) + 6) / slot));
  catLabels.forEach((text, i) => {
    if (i % every) return;
    const at = slotStart(i) + slot / 2;
    if (horizontal) out.push(`<text x="${left - 4}" y="${(at + size / 3).toFixed(1)}" text-anchor="end" font-size="${size}" fill="#595959" ${FONT}>${escapeXml(clip(text, box.right, size))}</text>`);
    else out.push(`<text x="${at.toFixed(1)}" y="${bottom + size + 5}" text-anchor="middle" font-size="${size}" fill="#595959" ${FONT}>${escapeXml(clip(text, slot * every - 2, size))}</text>`);
  });
  // Bars first, so lines and areas over them stay seen; areas before lines.
  const order = { area: 0, bar: 1, line: 2 };
  for (const plot of [...plots].sort((a, b) => order[a.kind] - order[b.kind])) {
    const items = all.filter(item => item.plot === plot);
    const totals = Array.from({ length: count }, (_, i) => plot.data.reduce((sum, s) => sum + (typeof s.values[i] === 'number' ? Math.abs(s.values[i]) : 0), 0));
    const stackedValue = (s, i) => {
      const v = plot.data[s].values[i];
      if (typeof v !== 'number') return null;
      return plot.grouping === 'percentStacked' ? (totals[i] ? v / totals[i] : 0) : v;
    };
    if (plot.kind === 'bar') {
      const stacked = plot.grouping !== 'clustered' && plot.grouping !== 'standard';
      const groups = stacked ? 1 : plot.data.length;
      const width = slot * 0.62 / groups;
      const ups = new Array(count).fill(0), downs = new Array(count).fill(0);
      items.forEach((item, s) => {
        for (let i = 0; i < count; i++) {
          const v = stackedValue(s, i);
          if (v == null) continue;
          let from = 0, to = v;
          if (stacked) { const base = v >= 0 ? ups[i] : downs[i]; from = base; to = base + v; if (v >= 0) ups[i] = to; else downs[i] = to; }
          const a = along(from), b = along(to);
          const start = horizontal ? slotStart(i) + slot * 0.81 - width * ((stacked ? 0 : s) + 1) : slotStart(i) + slot * 0.19 + (stacked ? 0 : width * s);
          const color = item.spec.points.get(i) || (plot.varyColors && plot.data.length === 1 ? accent(i) : item.color);
          out.push(horizontal
            ? `<rect x="${Math.min(a, b).toFixed(1)}" y="${start.toFixed(1)}" width="${Math.abs(b - a).toFixed(1)}" height="${width.toFixed(1)}" fill="${color}"/>`
            : `<rect x="${start.toFixed(1)}" y="${Math.min(a, b).toFixed(1)}" width="${width.toFixed(1)}" height="${Math.abs(b - a).toFixed(1)}" fill="${color}"/>`);
        }
      });
    } else {
      const x = i => left + slot * (i + 0.5);
      const base = new Array(count).fill(0);
      items.forEach((item, s) => {
        const points = [];
        const lower = [];
        for (let i = 0; i < count; i++) {
          const v = stackedValue(s, i);
          if (v == null) { points.push(plot.kind === 'area' ? [x(i), along(base[i])] : null); lower.push([x(i), along(base[i])]); continue; }
          const from = plot.grouping === 'standard' ? 0 : base[i];
          const to = from + v;
          lower.push([x(i), along(plot.grouping === 'standard' ? Math.max(scale.min, Math.min(scale.max, 0)) : from)]);
          points.push([x(i), along(to)]);
          if (plot.grouping !== 'standard') base[i] = to;
        }
        if (plot.kind === 'area') {
          const shape = [...points.filter(Boolean), ...lower.reverse()];
          if (shape.length > 2) out.push(`<path d="M${shape.map(([px, py]) => `${px.toFixed(1)} ${py.toFixed(1)}`).join(' L')} Z" fill="${item.color}"/>`);
        } else drawLine(out, points, item.color, lined(item), item.spec.marker, item.spec.smooth);
      });
    }
  }
  if (horizontal) out.push(`<line x1="${zero}" y1="${top}" x2="${zero}" y2="${bottom}" stroke="#BFBFBF"/>`);
  else out.push(`<line x1="${left}" y1="${zero}" x2="${right}" y2="${zero}" stroke="#BFBFBF"/>`);
}

/** Whether a series is drawn with a line: line charts, and scatter charts with lines, unless the
 * series hides it. */
function lined(item) {
  return (item.plot.kind === 'line' || (item.plot.kind === 'scatter' && item.plot.scatterLines)) && item.spec.line !== 'none';
}

/** A line through points (null ones break it), with markers on them. */
function drawLine(out, points, color, lines, markers, smooth) {
  if (lines) {
    let run = [];
    const flush = () => {
      if (run.length > 1) {
        const d = smooth && run.length > 2 ? smoothPath(run) : `M${run.map(([x, y]) => `${x.toFixed(1)} ${y.toFixed(1)}`).join(' L')}`;
        out.push(`<path d="${d}" fill="none" stroke="${color}" stroke-width="2.25" stroke-linejoin="round" stroke-linecap="round"/>`);
      }
      run = [];
    };
    for (const point of points) { if (point) run.push(point); else flush(); }
    flush();
  }
  if (markers) for (const point of points) if (point) out.push(`<circle cx="${point[0].toFixed(1)}" cy="${point[1].toFixed(1)}" r="3" fill="${color}" stroke="#FFFFFF" stroke-width="0.75"/>`);
}

function smoothPath(points) {
  let d = `M${points[0][0].toFixed(1)} ${points[0][1].toFixed(1)}`;
  for (let i = 0; i < points.length - 1; i++) {
    const [p0, p1, p2, p3] = [points[i - 1] || points[i], points[i], points[i + 1], points[i + 2] || points[i + 1]];
    const c1 = [p1[0] + (p2[0] - p0[0]) / 6, p1[1] + (p2[1] - p0[1]) / 6];
    const c2 = [p2[0] - (p3[0] - p1[0]) / 6, p2[1] - (p3[1] - p1[1]) / 6];
    d += ` C${c1.map(v => v.toFixed(1)).join(' ')} ${c2.map(v => v.toFixed(1)).join(' ')} ${p2[0].toFixed(1)} ${p2[1].toFixed(1)}`;
  }
  return d;
}

// --- Writing ------------------------------------------------------------------------------------

/**
 * A new chart part. `kind` is column, bar, line, area, pie, doughnut or scatter; each series is
 * { name: { ref, text }, categories: ref, values: ref } with references such as Sheet1!$B$2:$B$9,
 * and `cache` gives the values they have now ({ name, categories: [], values: [] }) so apps that
 * don't recalculate charts show them. Excel, LibreOffice and openpyxl read what this writes.
 */
export function chartXml({ kind, title = '', stacked = false, series, legend = 'b' }) {
  const plotKind = { column: 'barChart', bar: 'barChart', line: 'lineChart', area: 'areaChart', pie: 'pieChart', doughnut: 'doughnutChart', scatter: 'scatterChart' }[kind];
  if (!plotKind) throw new Error(`Charts of the kind "${kind}" cannot be made here.`);
  const pieLike = kind === 'pie' || kind === 'doughnut';
  const text = value => escapeXml(value ?? '');
  const strCache = values => `<c:strCache><c:ptCount val="${values.length}"/>${values.map((v, i) => (v == null ? '' : `<c:pt idx="${i}"><c:v>${text(v)}</c:v></c:pt>`)).join('')}</c:strCache>`;
  const numCache = values => `<c:numCache><c:formatCode>General</c:formatCode><c:ptCount val="${values.length}"/>${values.map((v, i) => (typeof v === 'number' && Number.isFinite(v) ? `<c:pt idx="${i}"><c:v>${v}</c:v></c:pt>` : '')).join('')}</c:numCache>`;
  const reference = (ref, values, numeric) => (numeric
    ? `<c:numRef><c:f>${text(ref)}</c:f>${numCache(values)}</c:numRef>`
    : `<c:strRef><c:f>${text(ref)}</c:f>${strCache(values)}</c:strRef>`);
  const accentFill = i => `<c:spPr>${kind === 'line' || kind === 'scatter'
    ? `<a:ln w="28575" cap="rnd"><a:solidFill><a:schemeClr val="accent${(i % 6) + 1}"/></a:solidFill><a:round/></a:ln>`
    : `<a:solidFill><a:schemeClr val="accent${(i % 6) + 1}"/></a:solidFill>`}</c:spPr>`;
  const seriesXml = series.map((item, i) => {
    const name = item.name?.ref
      ? `<c:tx><c:strRef><c:f>${text(item.name.ref)}</c:f>${strCache([item.cache?.name ?? item.name.text ?? ''])}</c:strRef></c:tx>`
      : item.name?.text ? `<c:tx><c:v>${text(item.name.text)}</c:v></c:tx>` : '';
    const numericCategories = item.cache?.categories?.length && item.cache.categories.every(v => typeof v === 'number');
    const categories = item.categories
      ? kind === 'scatter'
        ? `<c:xVal>${reference(item.categories, item.cache?.categories || [], numericCategories)}</c:xVal>`
        : `<c:cat>${reference(item.categories, item.cache?.categories || [], numericCategories)}</c:cat>`
      : '';
    const values = kind === 'scatter'
      ? `<c:yVal>${reference(item.values, item.cache?.values || [], true)}</c:yVal>`
      : `<c:val>${reference(item.values, item.cache?.values || [], true)}</c:val>`;
    const marker = kind === 'line' || kind === 'scatter' ? '<c:marker><c:symbol val="circle"/><c:size val="5"/></c:marker>' : '';
    const tail = kind === 'line' || kind === 'scatter' ? '<c:smooth val="0"/>' : kind === 'bar' || kind === 'column' ? '<c:invertIfNegative val="0"/>' : '';
    // invertIfNegative comes before cat and val; smooth after them.
    const middle = kind === 'bar' || kind === 'column' ? `${tail}${categories}${values}` : `${marker}${categories}${values}${tail}`;
    return `<c:ser><c:idx val="${i}"/><c:order val="${i}"/>${name}${pieLike ? '' : accentFill(i)}${middle}</c:ser>`;
  }).join('');
  const axisIds = '<c:axId val="500000001"/><c:axId val="500000002"/>';
  let plot;
  if (kind === 'column' || kind === 'bar') {
    const grouping = stacked ? 'stacked' : 'clustered';
    plot = `<c:barChart><c:barDir val="${kind === 'bar' ? 'bar' : 'col'}"/><c:grouping val="${grouping}"/><c:varyColors val="0"/>${seriesXml}<c:gapWidth val="150"/>${stacked ? '<c:overlap val="100"/>' : ''}${axisIds}</c:barChart>`;
  } else if (kind === 'line') {
    plot = `<c:lineChart><c:grouping val="${stacked ? 'stacked' : 'standard'}"/><c:varyColors val="0"/>${seriesXml}<c:marker val="1"/>${axisIds}</c:lineChart>`;
  } else if (kind === 'area') {
    plot = `<c:areaChart><c:grouping val="${stacked ? 'stacked' : 'standard'}"/><c:varyColors val="0"/>${seriesXml}${axisIds}</c:areaChart>`;
  } else if (kind === 'scatter') {
    plot = `<c:scatterChart><c:scatterStyle val="lineMarker"/><c:varyColors val="0"/>${seriesXml}${axisIds}</c:scatterChart>`;
  } else if (kind === 'pie') {
    plot = `<c:pieChart><c:varyColors val="1"/>${seriesXml}<c:firstSliceAng val="0"/></c:pieChart>`;
  } else {
    plot = `<c:doughnutChart><c:varyColors val="1"/>${seriesXml}<c:firstSliceAng val="0"/><c:holeSize val="50"/></c:doughnutChart>`;
  }
  const axis = (tag, id, cross, position, extra) => `<c:${tag}><c:axId val="${id}"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="${position}"/>${extra}<c:crossAx val="${cross}"/><c:crosses val="autoZero"/></c:${tag}>`;
  const grid = '<c:majorGridlines><c:spPr><a:ln w="9525"><a:solidFill><a:srgbClr val="D9D9D9"/></a:solidFill></a:ln></c:spPr></c:majorGridlines>';
  const numberAxis = position => axis('valAx', 500000002, 500000001, position, `${grid}<c:numFmt formatCode="General" sourceLinked="1"/><c:majorTickMark val="none"/><c:minorTickMark val="none"/><c:tickLblPos val="nextTo"/>`);
  let axes = '';
  if (kind === 'scatter') {
    axes = axis('valAx', 500000001, 500000002, 'b', '<c:numFmt formatCode="General" sourceLinked="1"/><c:majorTickMark val="out"/><c:minorTickMark val="none"/><c:tickLblPos val="nextTo"/>')
      + numberAxis('l');
  } else if (!pieLike) {
    axes = axis('catAx', 500000001, 500000002, kind === 'bar' ? 'l' : 'b', '<c:numFmt formatCode="General" sourceLinked="1"/><c:majorTickMark val="none"/><c:minorTickMark val="none"/><c:tickLblPos val="nextTo"/>')
      .replace('</c:catAx>', '<c:auto val="1"/><c:lblAlgn val="ctr"/><c:lblOffset val="100"/><c:noMultiLvlLbl val="0"/></c:catAx>')
      + numberAxis(kind === 'bar' ? 'b' : 'l');
  }
  const titleXml = title
    ? `<c:title><c:tx><c:rich><a:bodyPr/><a:lstStyle/><a:p><a:pPr><a:defRPr sz="1400" b="0"/></a:pPr><a:r><a:rPr lang="en-US" sz="1400" b="0"/><a:t>${text(title)}</a:t></a:r></a:p></c:rich></c:tx><c:overlay val="0"/></c:title><c:autoTitleDeleted val="0"/>`
    : '<c:autoTitleDeleted val="1"/>';
  const legendXml = legend ? `<c:legend><c:legendPos val="${legend}"/><c:overlay val="0"/></c:legend>` : '';
  return '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
    + `<c:chartSpace xmlns:c="${CHART}" xmlns:a="${DRAWING}" xmlns:r="${DOC_REL}">`
    + '<c:roundedCorners val="0"/>'
    + `<c:chart>${titleXml}<c:plotArea><c:layout/>${plot}${axes}</c:plotArea>${legendXml}<c:plotVisOnly val="1"/><c:dispBlanksAs val="gap"/></c:chart>`
    + '<c:spPr><a:solidFill><a:schemeClr val="bg1"/></a:solidFill><a:ln w="9525"><a:solidFill><a:srgbClr val="D9D9D9"/></a:solidFill></a:ln></c:spPr>'
    + '</c:chartSpace>';
}
