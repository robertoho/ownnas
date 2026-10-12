const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const source = fs.readFileSync(path.join(__dirname, '../web/app.js'), 'utf8');
const start = source.indexOf('async function pasteClipboard(');
const end = source.indexOf('function uploadFiles(', start);
assert.ok(start >= 0 && end > start, 'pasteClipboard is present');

async function paste(clipboard, dest, failOn = null) {
  const calls = [];
  const context = {
    state: { clipboard, path: dest },
    parentPath: (p) => (p.includes('/') ? p.slice(0, p.lastIndexOf('/')) : ''),
    api: async (url, options) => {
      calls.push([url, options.json.path]);
      if (options.json.path === failOn) throw new Error('disk full');
      return {};
    },
    renderClipboard() {},
    paintSelection() {},
    invalidateFolderSizes() {},
    load: async () => {},
    toast: (message, error) => calls.push(['toast', message, !!error]),
  };
  vm.runInNewContext(`${source.slice(start, end)}; result = pasteClipboard(dest);`, Object.assign(context, { dest }));
  await context.result;
  return { clipboard: context.state.clipboard, calls };
}

(async () => {
  const items = [{ path: 'a/x.txt', name: 'x.txt' }, { path: 'a/y.txt', name: 'y.txt' }];

  let result = await paste({ mode: 'copy', items }, 'b');
  assert.equal(result.clipboard, null, 'a copy is cleared once pasted');

  result = await paste({ mode: 'cut', items }, 'b');
  assert.equal(result.clipboard, null, 'a move is cleared once pasted');

  result = await paste({ mode: 'copy', items }, 'b', 'a/y.txt');
  assert.deepEqual(JSON.parse(JSON.stringify(result.clipboard)), { mode: 'copy', items: [items[1]] }, 'failed items stay');

  result = await paste({ mode: 'cut', items }, 'a');
  assert.equal(result.clipboard.items.length, 2, 'nothing to do in the same folder keeps the clipboard');
  assert.deepEqual(result.calls, [['toast', 'Already in this folder', false]]);

  console.log('PASS: pasting clears the file clipboard except for items that failed.');
})().catch((err) => {
  console.error(err);
  process.exit(1);
});
