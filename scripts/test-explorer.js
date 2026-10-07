// Focused behavior checks for grouping, Details controls, and type-to-filter.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const source = fs.readFileSync(require('node:path').join(__dirname, '../web/app.js'), 'utf8');
let handler, editable = false, dialog = false, calls = [], inputEvents = 0;
const entries = [
  { path: 'one.stl', name: 'one.stl', kind: 'model3d', dir: false, size: 12, modified: 0, tags: ['alpha', 'beta'] },
  { path: 'two.pdf', name: 'two.pdf', kind: 'pdf', dir: false, size: 42, modified: 0, tags: [] },
];
const nodes = Object.fromEntries(['app-view', 'preview', 'settings', 'library', 'menu', 'menu-sub'].map(id => [id, { hidden: id !== 'app-view' }]));
nodes.search = { value: '', focus() { calls.push('focus'); }, setSelectionRange() {}, dispatchEvent() { inputEvents++; } };
nodes.files = { style: { setProperty: (key, value) => calls.push([key, value]) } };
const context = {
  columnGeneration: 0,
  undoFileOperation: () => calls.push("undo"),
  localStorage: { getItem: () => null },
  state: { me: {}, path: 'folder', filter: '', selected: new Set(), groupBy: 'none', collapsedGroups: new Set(), detailWidths: [280,112,128,176,176], sort: 'name', direction: 'asc' },
  sortedEntries: () => entries,
  window: { addEventListener(_, fn) { handler = fn; } },
  document: { querySelector: () => dialog ? {} : null },
  $: id => nodes[id], Event: class {}, esc: String,
  closeMenu() {}, selectedEntries: () => [entries[0]],
  go: p => { calls.push(['go', p]); return Promise.resolve(); },
  openEntry: e => { calls.push(['open', e.path]); return Promise.resolve(); },
  moveSelection: (x,y) => calls.push(['move',x,y]), toast() {},
};
vm.createContext(context);
const groupStart = source.indexOf('function fileTypeLabel(');
vm.runInContext(source.slice(groupStart, source.indexOf('function renderCrumbs()',groupStart)), context);
assert.equal(context.fileTypeLabel(entries[0]), 'STL 3D model');
context.state.groupBy = 'tag';
assert.equal(context.groupEntries(entries).length, 3);
assert.equal(context.displayedEntries().length, 2, 'tag duplicates must not duplicate keyboard selection');
context.state.collapsedGroups.add('tag:alpha');
assert.equal(context.displayedEntries().length, 2, 'another expanded tag keeps the model visible');
context.state.collapsedGroups.add('tag:beta');
assert.equal(context.displayedEntries().length, 1);
assert.equal(context.dateGroup({modified: 0}), 'Unknown date');
const today = new Date(2026, 9, 2, 12);
assert.equal(context.dateGroup({modified: new Date(2026,9,1,12).getTime()/1000}, today), 'Yesterday');
context.applyDetailWidths();
assert.ok(calls.some(([key,value]) => key === '--detail-columns' && value.includes('minmax(280px, 1fr)')));
const start = source.indexOf('window.addEventListener("keydown", (event) => {');
vm.runInContext(source.slice(start, source.indexOf('window.addEventListener("paste"', start)), context);
function press(key, extra = {}) {
  let prevented = false;
  handler({ key, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, isComposing: false,
    target: { closest: selector => editable && selector.includes('input') ? {} : null },
    preventDefault() { prevented = true; }, ...extra });
  return prevented;
}
assert.ok(press('x'));
assert.equal(nodes.search.value, 'x'); assert.equal(inputEvents, 1);
context.state.filter = 'x'; assert.ok(press('y')); assert.equal(nodes.search.value, 'xy');
assert.ok(press("z", {metaKey: true})); assert.ok(calls.includes("undo"));
editable = true; assert.equal(press('z'), false); editable = false;
dialog = true; assert.equal(press('z'), false); dialog = false;
nodes.preview.hidden = false; assert.equal(press('z'), false); nodes.preview.hidden = true;
context.parentPath = () => ''; assert.ok(press('ArrowUp', {metaKey: true}));
assert.ok(calls.some(call => Array.isArray(call) && call[0] === 'go'));
// Render actual Details/group markup with a minimal DOM adapter.
const host = { style: { setProperty() {} }, setAttribute() {}, querySelectorAll: () => [], innerHTML: '' };
nodes.files = host; nodes.empty = {}; nodes.banner = {};
Object.assign(context, {
  entryIcon: () => '<svg/>', typeIcon: () => '<svg></svg>', formatDate: () => 'date', formatDateShort: () => 'date',
  formatSize: n => `${n} B`, formatFolderSize: () => 'Folder', tagColor: () => 'red', tagInitials: tag => tag,
  observeModelThumbnails: () => {}, updateSelectionSummary: () => {},
});
Object.assign(context.state, {view: 'list', entries, truncated: false, clipboard: null, selected: new Set(), folderSizes: new Map(), groupBy: 'type', collapsedGroups: new Set()});
const renderStart = source.indexOf('function renderFiles()');
vm.runInContext(source.slice(renderStart, source.indexOf('async function load(', renderStart)), context);
context.renderFiles();
assert.ok(host.innerHTML.includes('details-header'));
assert.equal((host.innerHTML.match(/data-detail-sort=/g) || []).length, 5);
assert.ok(host.innerHTML.includes('STL 3D model'));
assert.ok(host.innerHTML.includes('data-detail-resize="4"'));
assert.ok(host.innerHTML.includes('role="rowgroup"'));
assert.ok(host.innerHTML.includes('data-model-path="one.stl"'));
context.state.view = 'icons'; context.renderFiles();
assert.ok(!host.innerHTML.includes('details-header'));
assert.ok(host.innerHTML.includes('file-group-items'));
vm.runInContext(source.slice(source.indexOf('const FOLDER_ICONS ='), source.indexOf('function sortedEntries(')), context);
entries.push({path:'Projects',name:'Projects',kind:'folder',dir:true,modified:0,tags:[],folderAppearance:{color:'#a855f7',icon:'star'}});
context.state.groupBy = 'none';
for (const view of ['icons', 'masonry', 'list']) {
  context.state.view = view; context.renderFiles();
  assert.ok(host.innerHTML.includes('color:#a855f7'), `${view} displays folder color`);
  assert.ok(host.innerHTML.includes('folder-custom-icon'), `${view} displays custom folder icon`);
}
console.log('PASS: tag/date/type grouping, collapsed groups, selection deduplication, Details widths, type-to-filter, editor/dialog exclusions, and Command+Up.');
