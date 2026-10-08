import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';

const source = fs.readFileSync(new URL('../web/app.js', import.meta.url), 'utf8');
const start = source.indexOf('function transferPath(');
const end = source.indexOf('async function readDrop(', start);
assert.ok(start >= 0 && end > start, 'folder transfer helpers are present');

const calls = [];
const existing = new Set(['Project', 'Project (1)']);
const dialog = {
  returnValue: 'keep',
  showModal() {},
  querySelectorAll: () => [{ hidden: false, value: 'keep', focus() {} }],
  addEventListener(_name, callback) { callback(); },
};
const elements = {
  'folder-paste-title': { textContent: '' },
  'folder-paste-copy': { textContent: '' },
  'folder-paste-merge': { hidden: false },
  'folder-paste-dialog': dialog,
};
const context = {
  state: { path: '', me: {} },
  $: id => elements[id],
  pasteFileName: file => file.name || 'paste.bin',
  uploadSummary: () => 'Upload finished',
  toast: message => calls.push(['toast', message]),
  async api(url, options) {
    if (url === '/api/upload/conflicts') {
      return { items: options.json.names.map(name => ({ name, exists: existing.has(name), dir: existing.has(name) })) };
    }
    if (url === '/api/upload/directories') calls.push(['directories', options.json.names]);
    return { ok: true };
  },
  async uploadFiles(files, names, destination) {
    calls.push(['upload', names, destination, files.length]);
    return { saved: files.length, skipped: 0 };
  },
  async load(path) { calls.push(['load', path]); },
};
vm.createContext(context);
vm.runInContext(source.slice(start, end), context);

const file = { name: 'inside.txt', size: 4, lastModified: 1, type: 'text/plain' };
const leaf = { kind: 'file', name: 'inside.txt', async getFile() { return file; } };
const empty = { kind: 'directory', name: 'empty', async *entries() {} };
const folder = {
  kind: 'directory', name: 'Project',
  async *entries() {
    yield ['empty', empty];
    yield ['inside.txt', leaf];
  },
};
const transfer = await context.readTransfer({
  items: [{ kind: 'file', async getAsFileSystemHandle() { return folder; } }],
});
assert.deepEqual(JSON.parse(JSON.stringify(transfer.files.map(item => item.name))), ['Project/inside.txt']);
assert.deepEqual(JSON.parse(JSON.stringify([...transfer.directories].sort())), ['Project', 'Project/empty']);

await context.pasteExternalTransfer(transfer, 'target');
const directoryCall = calls.find(call => call[0] === 'directories');
assert.deepEqual(JSON.parse(JSON.stringify(directoryCall[1])), ['Project (2)', 'Project (2)/empty']);
const uploadCall = calls.find(call => call[0] === 'upload');
assert.deepEqual(JSON.parse(JSON.stringify(uploadCall)), ['upload', ['Project (2)/inside.txt'], 'target', 1]);
assert.ok(calls.some(call => call[0] === 'toast' && call[1] === 'Upload finished'));

dialog.returnValue = 'merge';
await context.pasteExternalTransfer(transfer, 'target');
const mergedUpload = calls.filter(call => call[0] === 'upload').at(-1);
assert.deepEqual(JSON.parse(JSON.stringify(mergedUpload)), ['upload', ['Project/inside.txt'], 'target', 1]);

await context.pasteExternalTransfer({ files: [], directories: ['EmptyOnly'] }, 'target');
const emptyFolders = calls.filter(call => call[0] === 'directories').at(-1);
assert.deepEqual(JSON.parse(JSON.stringify(emptyFolders[1])), ['EmptyOnly']);

console.log('PASS: clipboard directory traversal preserves empty folders, merges, and keep-both renames the whole tree.');
