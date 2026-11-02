// Checks for the chart module (web/sheet-chart.js): what it writes it reads back, and it draws
// each kind. Needs jsdom, as for test-xlsx-editor.mjs.
import assert from 'node:assert/strict';
import { pathToFileURL } from 'node:url';
const { JSDOM } = await import(pathToFileURL(process.env.XLSX_TEST_DOM || '/tmp/ownnas-docx-test/node_modules/jsdom/lib/api.js'));
const { window } = new JSDOM('');
const { chartSvg, chartXml, niceScale, readChart } = await import('../web/sheet-chart.js');
const parse = text => new window.DOMParser().parseFromString(text, 'application/xml');

assert.deepEqual(niceScale(0, 210), { min: 0, max: 250, step: 50 });
assert.deepEqual(niceScale(-3, 7), { min: -4, max: 8, step: 2 });

const series = ['North', 'South'].map((name, i) => ({
  name: { ref: `Sales!$${'BC'[i]}$1`, text: name },
  categories: "Sales!$A$2:$A$4",
  values: `Sales!$${'BC'[i]}$2:$${'BC'[i]}$4`,
  cache: { name, categories: ['Jan', 'Feb', 'Mar'], values: i ? [8, 9, 11] : [12, 15, null] },
}));
for (const kind of ['column', 'bar', 'line', 'area', 'pie', 'doughnut', 'scatter']) {
  const text = chartXml({ kind, title: 'Sales & <more>', series: kind === 'pie' || kind === 'doughnut' ? series.slice(0, 1) : series, legend: 'b' });
  const xml = parse(text);
  assert.equal(xml.getElementsByTagName('parsererror').length, 0, `${kind}: well formed`);
  const chart = readChart(xml);
  assert.equal(chart.title.text, 'Sales & <more>', `${kind}: title`);
  assert.equal(chart.legend, 'b');
  const plot = chart.plots[0];
  assert.equal(plot.kind, { column: 'bar', bar: 'bar' }[kind] || kind, `${kind}: kind`);
  if (kind === 'bar') assert.ok(plot.horizontal);
  assert.equal(plot.series[0].values.ref, 'Sales!$B$2:$B$4', `${kind}: values`);
  assert.deepEqual(plot.series[0].values.cache, [12, 15, null], `${kind}: the values saved`);
  assert.equal(plot.series[0].name.ref, 'Sales!$B$1');
  assert.deepEqual(plot.series[0].categories.cache, ['Jan', 'Feb', 'Mar'], `${kind}: categories`);
  const data = [plot.series.map(spec => ({ name: spec.name.cache[0], categories: ['Jan', 'Feb', 'Mar'], values: spec.values.cache }))];
  const svg = chartSvg(chart, data, 480, 288);
  const drawn = parse(svg);
  assert.equal(drawn.getElementsByTagName('parsererror').length, 0, `${kind}: the drawing is well formed`);
  assert.match(svg, /Sales &amp; &lt;more&gt;/, `${kind}: the title is drawn, escaped`);
  if (kind === 'column' || kind === 'bar') assert.equal([...drawn.getElementsByTagName('rect')].filter(rect => rect.getAttribute('fill') !== '#FFFFFF' && rect.getAttribute('width') !== '9').length, 5, `${kind}: a bar for each value`);
  if (kind === 'pie') assert.equal(drawn.getElementsByTagName('path').length, 2, 'pie: a slice for each value');
  if (kind === 'line') assert.equal(drawn.getElementsByTagName('circle').length, 5, 'line: a marker for each value');
}
// A stacked chart adds up; a chart without a title has none.
const stacked = readChart(parse(chartXml({ kind: 'column', stacked: true, series })));
assert.equal(stacked.plots[0].grouping, 'stacked');
assert.equal(stacked.title, null);
assert.ok(stacked.autoTitleDeleted);
console.log('Chart writing, reading and drawing tests passed.');
