const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

const source = fs.readFileSync(require('node:path').join(__dirname, '../web/app.js'), 'utf8');
const actualStart = source.indexOf('$("search").addEventListener("keydown"');
const end = source.indexOf('$("search-go").addEventListener', actualStart);
assert.ok(actualStart >= 0 && end > actualStart, 'search keyboard handler is present');

let keydown;
let matches = [];
const calls = [];
const context = {
  state: { filter: 'report' },
  $: () => ({ addEventListener(_type, callback) { keydown = callback; } }),
  displayedEntries: () => matches,
  openEntry: entry => { calls.push(['open', entry.path]); return Promise.resolve(); },
  runDeepSearch: () => { calls.push(['deep-search']); return Promise.resolve(); },
  clearDeepSearch() {},
  toast(message) { calls.push(['toast', message]); },
};
vm.createContext(context);
vm.runInContext(source.slice(actualStart, end), context);

function pressEnter() {
  let prevented = false;
  keydown({ key: 'Enter', preventDefault() { prevented = true; } });
  return prevented;
}

matches = [{ path: 'Reports/Q3.pdf', dir: false }];
assert.equal(pressEnter(), true);
assert.deepEqual(calls.shift(), ['open', 'Reports/Q3.pdf']);

matches = [{ path: 'one.txt' }, { path: 'two.txt' }];
assert.equal(pressEnter(), true);
assert.deepEqual(calls.shift(), ['deep-search']);

matches = [];
assert.equal(pressEnter(), true);
assert.deepEqual(calls.shift(), ['deep-search']);

console.log('PASS: Enter opens one visible quick-filter match and retains recursive search for zero or multiple matches.');
