// End-to-end checks for the MCP endpoint and agent keys: mostly attempts to get past a key's
// folders. Run after `cargo build --bin ownnas` (set OWNNAS_TARGET to use a target directory
// other than ./target/debug). Needs Node 22+.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import fs from 'node:fs/promises';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';

const repo = path.resolve(path.dirname(new URL(import.meta.url).pathname), '..');
const binary = path.join(process.env.OWNNAS_TARGET || path.join(repo, 'target/debug'), 'ownnas');
const PASSWORD = 'integration-test-passphrase';

const tmp = await fs.realpath(await fs.mkdtemp(path.join(os.tmpdir(), 'ownnas-mcp-')));
const library = path.join(tmp, 'library');
const write = async (rel, content) => {
  await fs.mkdir(path.dirname(path.join(library, rel)), { recursive: true });
  await fs.writeFile(path.join(library, rel), content);
};
await write('docs/plan.txt', 'the launch plan: zebra crossing\n');
await write('docs/deep/notes.md', 'deep notes about zebra habits\n');
await write('docs/.env', 'API_KEY=zebra-secret\n');
await write('docs/long.txt', 'é'.repeat(2000));
await write('docs/binary.dat', Buffer.from([1, 2, 0, 3]));
await write('docs2/other.txt', 'zebra in the neighbouring folder\n');
await write('secret/private.txt', 'zebra private words\n');
await fs.writeFile(path.join(tmp, 'outside.txt'), 'zebra outside the library\n');
// One white pixel.
await write('docs/dot.png', Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8/5+hHgAHggJ/PchI7wAAAABJRU5ErkJggg==', 'base64'));
await fs.symlink(path.join(library, 'secret'), path.join(library, 'docs/link-to-secret'));
await fs.symlink(path.join(library, 'secret/private.txt'), path.join(library, 'docs/link-to-private.txt'));
await fs.symlink(path.join(tmp, 'outside.txt'), path.join(library, 'docs/link-outside.txt'));

async function fingerprints(dir, out = {}) {
  for (const item of await fs.readdir(dir, { withFileTypes: true })) {
    const full = path.join(dir, item.name);
    if (item.isSymbolicLink()) out[full] = await fs.readlink(full);
    else if (item.isDirectory()) await fingerprints(full, out);
    else {
      const stat = await fs.stat(full);
      out[full] = `${createHash('sha256').update(await fs.readFile(full)).digest('hex')} ${stat.mtimeMs}`;
    }
  }
  return out;
}
const before = await fingerprints(library);

const port = await new Promise((resolve) => {
  const server = net.createServer().listen(0, '127.0.0.1', () => {
    const { port } = server.address();
    server.close(() => resolve(port));
  });
});
const base = `http://127.0.0.1:${port}`;
const server = spawn(binary, ['serve', '--root', library, '--data', path.join(tmp, 'data'), '--addr', `127.0.0.1:${port}`,
  '--username', 'admin', '--password', PASSWORD], { stdio: 'ignore', env: { ...process.env, OWNNAS_PASSWORD: PASSWORD } });

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
let cookie = '';
// A stuck request should fail the test, not hang it.
const plainFetch = globalThis.fetch;
globalThis.fetch = (url, options = {}) => plainFetch(url, { signal: AbortSignal.timeout(30_000), ...options });

async function api(method, url, body, headers = {}) {
  const response = await fetch(base + url, {
    method,
    headers: { cookie, 'x-ownnas': '1', ...(body ? { 'content-type': 'application/json' } : {}), ...headers },
    body: body ? JSON.stringify(body) : undefined,
  });
  const set = response.headers.getSetCookie?.() ?? [];
  if (set.length) cookie = set[0].split(';')[0];
  const text = await response.text();
  let json = null;
  try { json = JSON.parse(text); } catch {}
  return { status: response.status, json, headers: response.headers };
}

let nextId = 1;
async function mcp(token, method, params, headers = {}) {
  const response = await fetch(`${base}/mcp`, {
    method: 'POST',
    headers: { 'content-type': 'application/json', accept: 'application/json, text/event-stream', authorization: `Bearer ${token}`, ...headers },
    body: JSON.stringify({ jsonrpc: '2.0', id: nextId++, method, params }),
  });
  const text = await response.text();
  let json = null;
  try { json = JSON.parse(text); } catch {}
  return { status: response.status, headers: response.headers, json };
}

async function tool(token, name, args = {}) {
  const { status, json } = await mcp(token, 'tools/call', { name, arguments: args });
  assert.equal(status, 200);
  assert.ok(json.result, `${name} should give a result: ${JSON.stringify(json)}`);
  return json.result;
}
const data = (result) => JSON.parse(result.content[0].text);
const REFUSED = 'Not found, or outside the folders this key can use.';
async function refused(token, name, args) {
  const result = await tool(token, name, args);
  assert.equal(result.isError, true, `${name} ${JSON.stringify(args)} should be refused`);
  assert.equal(result.content[0].text, REFUSED);
  assert.equal(JSON.stringify(result).includes('private words'), false);
}

let failed = false;
try {
  let up = false;
  for (let i = 0; i < 100 && !up; i++) {
    try { up = (await fetch(`${base}/api/me`)).status > 0; } catch { await sleep(100); }
  }
  assert.ok(up, 'server started');
  const login = await api('POST', '/api/login', { username: 'admin', password: PASSWORD });
  assert.equal(login.status, 200);

  // Making keys.
  const made = await api('POST', '/api/agent-keys', { name: 'docs reader', folders: ['docs'] });
  assert.equal(made.status, 200, JSON.stringify(made.json));
  const key = made.json.token;
  assert.match(key, /^ownnas_mcp_[0-9a-f]{64}$/);
  for (const [folders, level] of [[['secret/../..'], 'read'], [['docs/.env'], 'read'], [['docs/link-outside.txt'], 'read'], [[], 'read'], [['docs'], 'edit']]) {
    assert.equal((await api('POST', '/api/agent-keys', { name: 'bad', folders, level })).status, 400, JSON.stringify(folders));
  }
  const listed = await api('GET', '/api/agent-keys');
  assert.equal(listed.json.keys.length, 1);
  assert.deepEqual(listed.json.keys[0].folders, ['docs']);
  assert.equal(JSON.stringify(listed.json).includes(key), false, 'the secret is never shown again');
  const sessionToken = (await api('POST', '/api/login', { username: 'admin', password: PASSWORD, token: true })).json.token;
  assert.ok(sessionToken);

  // Only an administrator can manage keys.
  assert.equal((await api('POST', '/api/users', { username: 'plainuser', password: 'another-long-passphrase', admin: false })).status, 200);
  const adminCookie = cookie;
  cookie = '';
  await api('POST', '/api/login', { username: 'plainuser', password: 'another-long-passphrase' });
  assert.equal((await api('GET', '/api/agent-keys')).status, 403);
  assert.equal((await api('POST', '/api/agent-keys', { name: 'mine', folders: ['docs'] })).status, 403);
  assert.equal((await api('POST', '/api/agent-keys/revoke', { id: 1 })).status, 403);
  assert.equal((await api('GET', '/api/agent-keys/log')).status, 403);
  cookie = adminCookie;
  // Managing accounts must not hang the server (each of these once held the database lock twice).
  assert.equal((await api('POST', '/api/users/password', { username: 'plainuser', password: 'yet-another-long-passphrase' })).status, 200);
  assert.equal((await api('POST', '/api/users/admin', { username: 'plainuser', admin: true })).status, 200);
  assert.equal((await api('POST', '/api/users/remove', { username: 'plainuser' })).status, 200);
  // A browser form cannot make a key: the same-site check still applies.
  assert.equal((await fetch(`${base}/api/agent-keys`, { method: 'POST', headers: { cookie, 'content-type': 'application/json', origin: 'https://evil.example' }, body: '{"name":"x","folders":["docs"]}' })).status, 403);

  // Signing in: a key opens only /mcp, and only a key opens /mcp.
  assert.equal((await mcp('nonsense', 'ping')).status, 401);
  assert.equal((await mcp(sessionToken, 'ping')).status, 401, 'a session token does not open /mcp');
  const anonymous = await fetch(`${base}/mcp`, { method: 'POST', headers: { cookie, 'content-type': 'application/json' }, body: '{"jsonrpc":"2.0","id":1,"method":"ping"}' });
  assert.equal(anonymous.status, 401, 'nor does a cookie');
  assert.match(anonymous.headers.get('www-authenticate'), /^Bearer/);
  for (const url of ['/api/list?path=docs', '/api/raw?path=secret/private.txt', '/api/me', '/api/users', '/api/agent-keys', '/api/search?q=zebra']) {
    const response = await fetch(base + url, { headers: { authorization: `Bearer ${key}` } });
    assert.equal(response.status, 401, `${url} must refuse an agent key`);
  }
  const socket = await fetch(`${base}/api/terminals`, { headers: { authorization: `Bearer ${key}` } });
  assert.equal(socket.status, 401, 'terminals must refuse an agent key');

  // The protocol.
  assert.equal((await mcp(key, 'ping', {}, { origin: 'https://evil.example' })).status, 403);
  assert.equal((await mcp(key, 'ping', {}, { 'mcp-protocol-version': '1999-01-01' })).status, 400);
  assert.equal((await fetch(`${base}/mcp`, { headers: { authorization: `Bearer ${key}` } })).status, 405);
  const init = await mcp(key, 'initialize', { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'test', version: '1' } });
  assert.equal(init.json.result.protocolVersion, '2025-06-18');
  assert.deepEqual(init.json.result.capabilities, { tools: {} });
  assert.match(init.json.result.instructions, /"docs"/);
  const newer = await mcp(key, 'initialize', { protocolVersion: '2099-01-01', capabilities: {}, clientInfo: { name: 'test', version: '1' } });
  assert.notEqual(newer.json.result.protocolVersion, '2099-01-01');
  assert.equal((await mcp(key, 'ping')).json.error, undefined);
  assert.equal((await mcp(key, 'nope')).json.error.code, -32601);
  const note = await fetch(`${base}/mcp`, { method: 'POST', headers: { authorization: `Bearer ${key}`, 'content-type': 'application/json' }, body: '{"jsonrpc":"2.0","method":"notifications/initialized"}' });
  assert.equal(note.status, 202);
  const notJson = await fetch(`${base}/mcp`, { method: 'POST', headers: { authorization: `Bearer ${key}` }, body: '{nope' });
  assert.equal(notJson.status, 400);
  assert.equal((await mcp(key, 'tools/call', { name: 'run_shell', arguments: {} })).json.error.code, -32602);
  const tools = (await mcp(key, 'tools/list')).json.result.tools;
  assert.deepEqual(tools.map((t) => t.name).sort(), ['get_info', 'get_thumbnail', 'list_folder', 'read_text', 'search']);
  assert.ok(tools.every((t) => t.annotations.readOnlyHint === true && t.inputSchema.type === 'object'));

  // Listing.
  const start = data(await tool(key, 'list_folder'));
  assert.deepEqual(start.entries.map((e) => e.path), ['docs']);
  const docs = data(await tool(key, 'list_folder', { path: 'docs' }));
  const names = docs.entries.map((e) => e.name).sort();
  assert.deepEqual(names, ['binary.dat', 'deep', 'dot.png', 'long.txt', 'plan.txt']);
  assert.equal(docs.more, false);
  const page = data(await tool(key, 'list_folder', { path: 'docs', limit: 2, offset: 1 }));
  assert.equal(page.entries.length, 2);
  assert.equal(page.more, true);

  // Every way out.
  const escapes = ['secret', 'secret/private.txt', 'docs2', 'docs2/other.txt', 'docs/../secret/private.txt', '../outside.txt', 'docs/../../outside.txt',
    'docs/link-to-secret', 'docs/link-to-secret/private.txt', 'docs/link-to-private.txt', 'docs/link-outside.txt', 'docs/.env', '.ownnas-trash', 'docs/missing.txt',
    tmp + '/outside.txt', 'docs\\..\\secret\\private.txt'];
  for (const name of ['read_text', 'get_info', 'get_thumbnail', 'list_folder']) {
    for (const target of escapes) await refused(key, name, { path: target });
  }
  await refused(key, 'search', { query: 'zebra', path: 'secret' });
  await refused(key, 'search', { query: 'zebra', path: 'docs/link-to-secret' });
  await refused(key, 'read_text', { path: '' });

  // Search stays inside: by name, by word, and by tag.
  const tagged = async (rel) => assert.equal((await api('POST', '/api/tags', { path: rel, tag: 'zebra' })).status, 200);
  await tagged('secret/private.txt');
  await tagged('docs/plan.txt');
  const inside = (hits) => hits.every((hit) => hit.path === 'docs' || hit.path.startsWith('docs/'));
  const words = data(await tool(key, 'search', { query: 'zebra' }));
  const paths = words.hits.map((hit) => hit.path).sort();
  assert.ok(inside(words.hits), JSON.stringify(paths));
  assert.ok(paths.includes('docs/plan.txt'), JSON.stringify(paths));
  assert.equal(paths.some((p) => /env|private|other|link/.test(p)), false, JSON.stringify(paths));
  const byName = data(await tool(key, 'search', { query: 'private' }));
  assert.deepEqual(byName.hits, [], JSON.stringify(byName));
  const byTag = data(await tool(key, 'search', { query: 'tag:zebra' }));
  assert.deepEqual(byTag.hits.map((hit) => hit.path), ['docs/plan.txt']);
  assert.equal((await tool(key, 'search', { query: '  ' })).isError, true);

  // Reading.
  const plan = await tool(key, 'read_text', { path: 'docs/plan.txt' });
  assert.equal(plan.content[1].text, 'the launch plan: zebra crossing\n');
  assert.equal(data(plan).nextOffset, null);
  assert.equal(data(await tool(key, 'read_text', { path: '/docs/plan.txt' })).path, 'docs/plan.txt');
  let text = '';
  let offset = 0;
  let parts = 0;
  while (offset !== null && parts < 10) {
    const part = await tool(key, 'read_text', { path: 'docs/long.txt', offset, limit: 1025 });
    text += part.content[1].text;
    offset = data(part).nextOffset;
    parts++;
  }
  assert.equal(text, 'é'.repeat(2000), 'parts join up without breaking a character');
  assert.equal(parts, 4);
  assert.equal((await tool(key, 'read_text', { path: 'docs/binary.dat' })).isError, true);
  assert.equal((await tool(key, 'read_text', { path: 'docs/dot.png' })).isError, true);
  assert.equal((await tool(key, 'read_text', { path: 'docs' })).isError, true);

  // Details and pictures.
  const info = data(await tool(key, 'get_info', { path: 'docs/plan.txt' }));
  assert.equal(info.kind, 'text');
  assert.equal(info.size, 32);
  assert.deepEqual(info.tags, ['zebra']);
  assert.equal(info.protected, false);
  assert.equal((await api('POST', '/api/protect', { paths: ['docs/plan.txt'], protect: true })).status, 200);
  assert.equal(data(await tool(key, 'get_info', { path: 'docs/plan.txt' })).protected, true);
  const picture = (await tool(key, 'get_thumbnail', { path: 'docs/dot.png' })).content[0];
  assert.equal(picture.type, 'image');
  assert.match(picture.mimeType, /^image\//);
  assert.ok(Buffer.from(picture.data, 'base64').length > 10);

  // The log has what happened, refusals too, and never a secret.
  const log = (await api('GET', '/api/agent-keys/log?limit=1000')).json.entries;
  assert.ok(log.some((entry) => entry.tool === 'read_text' && entry.path === 'secret/private.txt' && entry.outcome === 'refused'));
  assert.ok(log.some((entry) => entry.tool === 'read_text' && entry.path === 'docs/plan.txt' && entry.outcome === 'ok'));
  assert.ok(log.some((entry) => entry.tool === 'search' && entry.detail === 'zebra'));
  assert.equal(JSON.stringify(log).includes(key), false);
  assert.ok((await api('GET', '/api/agent-keys')).json.keys[0].lastUsedAt > 0);

  // A folder keeps its key through a move made in OwnNAS, and the Trash ends it.
  const whole = (await api('POST', '/api/agent-keys', { name: 'everything', folders: [''] })).json.token;
  const top = data(await tool(whole, 'list_folder'));
  assert.deepEqual(top.entries.map((e) => e.name).sort(), ['docs', 'docs2', 'secret']);
  await refused(whole, 'read_text', { path: 'docs/.env' });
  const narrow = (await api('POST', '/api/agent-keys', { name: 'other', folders: ['secret'] })).json;
  assert.equal(data(await tool(narrow.token, 'read_text', { path: 'secret/private.txt' })).path, 'secret/private.txt');
  const moved = await api('POST', '/api/rename', { path: 'secret', name: 'hush' });
  assert.equal(moved.status, 200, JSON.stringify(moved.json));
  assert.equal(data(await tool(narrow.token, 'read_text', { path: 'hush/private.txt' })).path, 'hush/private.txt');
  await refused(narrow.token, 'read_text', { path: 'docs/plan.txt' });
  const trashed = await api('DELETE', '/api/entry?path=hush');
  assert.equal(trashed.status, 200, JSON.stringify(trashed.json));
  await refused(narrow.token, 'list_folder', { path: 'hush' });
  assert.deepEqual(data(await tool(narrow.token, 'list_folder')).entries, [], 'its only folder is in the Trash');
  assert.deepEqual(data(await tool(narrow.token, 'search', { query: 'zebra' })).hits, []);

  // Revoking works at once.
  assert.equal((await api('POST', '/api/agent-keys/revoke', { id: made.json.id })).status, 200);
  assert.equal((await mcp(key, 'ping')).status, 401);
  assert.ok((await api('GET', '/api/agent-keys/log?id=' + made.json.id)).json.entries.length > 0, 'the log stays');

  // Nothing an agent did changed the library. (The admin moved `secret` above; everything else
  // must be exactly as it was.)
  const after = await fingerprints(library);
  for (const [file, print] of Object.entries(before)) {
    if (file.includes('/secret/')) continue;
    assert.equal(after[file], print, `${file} changed`);
  }
  console.log('ok: MCP endpoint and agent keys');
} catch (err) {
  failed = true;
  console.error(err);
} finally {
  server.kill();
  await sleep(200);
  await fs.rm(tmp, { recursive: true, force: true });
}
process.exit(failed ? 1 : 0);
