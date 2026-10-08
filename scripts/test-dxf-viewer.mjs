// DXF parsing is independent of the canvas, so this exercises the actual browser module in Node.
// Run: node --experimental-vm-modules scripts/test-dxf-viewer.mjs
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import vm from 'node:vm';

const root = path.resolve(path.dirname(new URL(import.meta.url).pathname), '..');
const context = vm.createContext({ console });
const module = new vm.SourceTextModule(await fs.readFile(path.join(root, 'web/dxf-viewer.js'), 'utf8'), { context });
await module.link(() => { throw new Error('DXF viewer should not import external modules'); });
await module.evaluate();

const pairs = [
  [0, 'SECTION'], [2, 'TABLES'], [0, 'TABLE'], [2, 'LAYER'],
  [0, 'LAYER'], [2, 'Outline'], [62, '3'],
  [0, 'LAYER'], [2, 'Notes'], [62, '7'],
  [0, 'ENDTAB'], [0, 'ENDSEC'],
  [0, 'SECTION'], [2, 'ENTITIES'],
  [0, 'LINE'], [8, 'Outline'], [10, '0'], [20, '0'], [11, '4'], [21, '0'],
  [0, 'CIRCLE'], [8, 'Outline'], [10, '5'], [20, '5'], [40, '1'],
  [0, 'LWPOLYLINE'], [8, 'Outline'], [70, '1'], [10, '0'], [20, '0'], [10, '2'], [20, '0'], [10, '2'], [20, '2'],
  [0, 'ARC'], [8, 'Outline'], [10, '10'], [20, '10'], [40, '2'], [50, '0'], [51, '90'],
  [0, 'TEXT'], [8, 'Notes'], [10, '1'], [20, '1'], [40, '0.5'], [1, 'Room label'],
  [0, 'INSERT'], [8, 'Outline'],
  [0, 'ENDSEC'], [0, 'EOF'],
];
const source = pairs.map(([code, value]) => String(code).padStart(2, ' ') + '\n' + value + '\n').join('');
const drawing = module.namespace.parseDxf(source);
assert.equal(drawing.entities.length, 5);
assert.equal(drawing.skipped, 1);
assert.deepEqual([...drawing.layers], ['Notes', 'Outline']);
assert.equal(drawing.entities[0].color, 3);
assert.equal(drawing.entities[2].closed, true);
assert.equal(drawing.entities[4].text, 'Room label');
assert.ok(drawing.entities[3].points.length >= 12, 'arc should be tessellated into a visible curve');
assert.deepEqual({ ...drawing.bounds }, { minX: 0, minY: 0, maxX: 12, maxY: 12 });
assert.throws(() => module.namespace.parseDxf('not a dxf'), /ASCII DXF/);
assert.throws(() => module.namespace.parseDxf(source, { maxEntities: 2 }), /too large to preview/);

console.log('PASS: DXF sections, layers, lines, circles, closed polylines, arcs, text, bounds, malformed input, and entity limits.');
