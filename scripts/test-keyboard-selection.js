// Regression checks for extending and reversing keyboard range selections.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

const source = fs.readFileSync(require('node:path').join(__dirname, '../web/app.js'), 'utf8');
const entries = ['a', 'b', 'c', 'd'].map(path => ({ path, dir: false }));
const state = { selected: new Set(), anchor: '', focus: '', view: 'list' };
const context = {
  state,
  displayedEntries: () => entries,
  findEntry: path => entries.find(entry => entry.path === path),
  paintSelection() {},
  scrollSelectedIntoView() {},
  openEntry() {},
  toast() {},
  $: () => ({ hidden: true }),
};
vm.createContext(context);

const selectionStart = source.indexOf('function selectOnly(');
const selectionEnd = source.indexOf('function gridColumnCount()', selectionStart);
vm.runInContext(source.slice(selectionStart, selectionEnd), context);
const movementStart = source.indexOf('function moveSelection(');
const movementEnd = source.indexOf('function renderClipboard()', movementStart);
vm.runInContext(source.slice(movementStart, movementEnd), context);

const selected = () => [...state.selected].sort();
context.selectOnly('a');
context.moveSelection(0, 1, true);
assert.deepEqual(selected(), ['a', 'b']);
context.moveSelection(0, 1, true);
assert.deepEqual(selected(), ['a', 'b', 'c'], 'repeated Shift+Down extends the selection');
assert.equal(state.anchor, 'a', 'the range anchor remains fixed');
assert.equal(state.focus, 'c', 'the keyboard endpoint advances');
context.moveSelection(0, -1, true);
assert.deepEqual(selected(), ['a', 'b'], 'reversing direction contracts the range');
assert.equal(state.focus, 'b');
context.moveSelection(0, -1, true);
assert.deepEqual(selected(), ['a']);
context.moveSelection(0, -1, true);
assert.deepEqual(selected(), ['a'], 'selection stays in bounds');

console.log('PASS: Shift+Arrow extends, contracts, and clamps range selection.');
