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
import { zipSync, strToU8 } from '../web/vendor/three/addons/libs/fflate.module.js';

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
await write('docs/table.csv', 'name,amount\n"Smith, Ann",12.5\n"say ""hi""",3\n');
await write('docs2/other.txt', 'zebra in the neighbouring folder\n');
await write('secret/private.txt', 'zebra private words\n');
await fs.writeFile(path.join(tmp, 'outside.txt'), 'zebra outside the library\n');
// One white pixel.
const DOT = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8/5+hHgAHggJ/PchI7wAAAABJRU5ErkJggg==', 'base64');
await write('docs/dot.png', DOT);
await write('secret/hidden-album.png', DOT);
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
  assert.deepEqual(init.json.result.capabilities, { tools: {}, prompts: {}, resources: {} });
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
  const READING = ['album_photos', 'find_duplicates', 'folder_usage', 'get_info', 'get_thumbnail', 'list_albums', 'list_folder', 'list_notes', 'list_people', 'list_versions', 'person_photos', 'photo_search',
    'photos_taken', 'read_note', 'read_table', 'read_text', 'read_version', 'recent_files', 'search', 'similar_photos'];
  assert.deepEqual(tools.map((t) => t.name).sort(), READING, 'a read key sees no organising tools');
  assert.ok(tools.every((t) => t.annotations.readOnlyHint === true && t.inputSchema.type === 'object'));
  // Reading tools say what they answer, and their answers come as that data too.
  assert.equal(tools.find((t) => t.name === 'get_info').outputSchema.type, 'object');
  assert.ok(tools.filter((t) => t.outputSchema).length >= 15, 'the main reading tools have output schemas');

  // Listing.
  const start = data(await tool(key, 'list_folder'));
  assert.deepEqual(start.entries.map((e) => e.path), ['docs']);
  const docs = data(await tool(key, 'list_folder', { path: 'docs' }));
  const names = docs.entries.map((e) => e.name).sort();
  assert.deepEqual(names, ['binary.dat', 'deep', 'dot.png', 'long.txt', 'plan.txt', 'table.csv']);
  assert.equal(docs.more, false);
  const page = data(await tool(key, 'list_folder', { path: 'docs', limit: 2, offset: 1 }));
  assert.equal(page.entries.length, 2);
  assert.equal(page.more, true);

  // Every way out.
  const escapes = ['secret', 'secret/private.txt', 'docs2', 'docs2/other.txt', 'docs/../secret/private.txt', '../outside.txt', 'docs/../../outside.txt',
    'docs/link-to-secret', 'docs/link-to-secret/private.txt', 'docs/link-to-private.txt', 'docs/link-outside.txt', 'docs/.env', '.ownnas-trash', 'docs/missing.txt',
    tmp + '/outside.txt', 'docs\\..\\secret\\private.txt'];
  for (const name of ['read_text', 'get_info', 'get_thumbnail', 'list_folder', 'read_table', 'list_versions', 'similar_photos']) {
    for (const target of escapes) await refused(key, name, { path: target });
  }
  await refused(key, 'search', { query: 'zebra', path: 'secret' });
  await refused(key, 'search', { query: 'zebra', path: 'docs/link-to-secret' });
  await refused(key, 'read_text', { path: '' });

  // So many refusals in an hour: administrators are told, and the usage shows where.
  const attention = (await api('GET', '/api/agent-keys/attention')).json;
  assert.ok(attention.probing.some((entry) => entry.id === made.json.id && entry.refused >= 20), JSON.stringify(attention));
  const keyUsage = (await api('GET', `/api/agent-keys/usage?id=${made.json.id}`)).json;
  assert.ok(keyUsage.tools.find((row) => row.tool === 'read_text').outcomes.refused >= 16, JSON.stringify(keyUsage.tools));
  assert.ok(keyUsage.days.at(-1).refused >= 20);

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

  // Tables, history, large pictures, photo details.
  const table = data(await tool(key, 'read_table', { path: 'docs/table.csv' }));
  assert.deepEqual(table.rows, [['name', 'amount'], ['Smith, Ann', '12.5'], ['say "hi"', '3']]);
  assert.deepEqual(table.sheets, [{ name: 'table', rows: 3, columns: 2 }]);
  const tablePage = data(await tool(key, 'read_table', { path: 'docs/table.csv', offset: 1, limit: 1 }));
  assert.deepEqual(tablePage.rows, [['Smith, Ann', '12.5']]);
  assert.equal(tablePage.more, true);
  assert.equal((await tool(key, 'read_table', { path: 'docs/table.csv', sheet: 'nope' })).isError, true);
  assert.equal((await tool(key, 'read_table', { path: 'docs/binary.dat' })).isError, true);
  assert.deepEqual(data(await tool(key, 'list_versions', { path: 'docs/plan.txt' })).versions, []);
  await refused(key, 'read_version', { path: 'docs/plan.txt', version: 1 });
  const large = (await tool(key, 'get_thumbnail', { path: 'docs/dot.png', size: 'large' })).content[0];
  assert.equal(large.mimeType, 'image/jpeg');
  assert.ok(data(await tool(key, 'get_info', { path: 'docs/dot.png' })).details, 'photos come with their details');
  const searchPage = data(await tool(key, 'search', { query: 'zebra', offset: 1, limit: 1 }));
  assert.ok(searchPage.hits.length <= 1);

  // Answers come as data too.
  assert.equal((await tool(key, 'get_info', { path: 'docs/plan.txt' })).structuredContent.path, 'docs/plan.txt');

  // Prompts, and files as resources.
  const promptNames = (await mcp(key, 'prompts/list')).json.result.prompts.map((p) => p.name);
  assert.deepEqual(promptNames.sort(), ['find_documents', 'photo_story', 'summarise_folder'], 'tidying is for organising keys');
  const prompt = (await mcp(key, 'prompts/get', { name: 'summarise_folder', arguments: { folder: 'docs' } })).json.result;
  assert.match(prompt.messages[0].content.text, /"docs"/);
  assert.equal((await mcp(key, 'prompts/get', { name: 'tidy_folder', arguments: { folder: 'docs' } })).json.error.code, -32602);
  const resources = (await mcp(key, 'resources/list')).json.result.resources;
  assert.ok(resources.some((r) => r.uri === 'ownnas:///docs/plan.txt'), JSON.stringify(resources));
  assert.equal(resources.some((r) => /env|link|secret/.test(r.uri)), false, JSON.stringify(resources));
  const resource = (await mcp(key, 'resources/read', { uri: 'ownnas:///docs/plan.txt' })).json.result.contents[0];
  assert.equal(resource.text, 'the launch plan: zebra crossing\n');
  assert.equal((await mcp(key, 'resources/read', { uri: 'ownnas:///docs/dot.png' })).json.result.contents[0].mimeType, 'image/jpeg');
  for (const uri of ['ownnas:///secret/private.txt', 'ownnas:///docs/link-to-private.txt', 'ownnas:///docs/.env', 'ownnas:///docs/%2E%2E/secret/private.txt', 'file:///etc/passwd']) {
    const answer = (await mcp(key, 'resources/read', { uri })).json;
    assert.ok(answer.error, uri);
    assert.equal(JSON.stringify(answer).includes('private words'), false);
  }
  assert.ok((await mcp(key, 'resources/templates/list')).json.result.resourceTemplates.length === 1);

  // What changed lately, and what takes up the space: only inside, never hidden files.
  let recent = { files: [] };
  for (let i = 0; i < 50 && !recent.files.length; i++) {
    const answer = await tool(key, 'recent_files');
    if (!answer.isError) recent = data(answer);
    if (!recent.files.length) await sleep(200);
  }
  const recentPaths = recent.files.map((f) => f.path);
  assert.ok(recentPaths.includes('docs/plan.txt'), JSON.stringify(recentPaths));
  assert.ok(recentPaths.every((p) => p.startsWith('docs/') && !p.includes('.env') && !p.includes('link-')), JSON.stringify(recentPaths));
  assert.deepEqual(data(await tool(key, 'recent_files', { since: '2099-01-01' })).files, []);
  await refused(key, 'recent_files', { path: 'secret' });
  const usage = data(await tool(key, 'folder_usage', { path: 'docs' }));
  assert.deepEqual(usage.folders.map((f) => f.path), ['docs/deep']);
  assert.ok(usage.largestFiles.some((f) => f.path === 'docs/long.txt'));
  assert.equal(usage.largestFiles.some((f) => f.path.includes('.env')), false);
  assert.equal(usage.bytes, usage.largestFiles.reduce((sum, f) => sum + f.bytes, 0) + usage.folders.reduce((sum, f) => sum + f.bytes, 0), 'hidden files are not counted');
  await refused(key, 'folder_usage', { path: 'secret' });

  // Duplicates: only copies the key can see, and a copy elsewhere is not even hinted at.
  await write('docs/deep/copy-a.txt', 'the very same words\n');
  await write('docs/deep/copy-b.txt', 'the very same words\n');
  await write('secret/copy-c.txt', 'the very same words\n');
  let dupes = { groups: [] };
  for (let i = 0; i < 50 && !dupes.groups.length; i++) {
    const answer = await tool(key, 'find_duplicates', { path: 'docs' });
    if (!answer.isError) dupes = data(answer);
    if (!dupes.groups.length) await sleep(200);
  }
  assert.deepEqual(dupes.groups.map((g) => g.copies.map((c) => c.path).sort()), [['docs/deep/copy-a.txt', 'docs/deep/copy-b.txt']]);
  assert.equal(dupes.wastedBytes, 20);
  assert.equal(JSON.stringify(dupes).includes('secret'), false);
  await refused(key, 'find_duplicates', { path: 'secret' });
  await fs.rm(path.join(library, 'docs/deep/copy-a.txt'));
  await fs.rm(path.join(library, 'docs/deep/copy-b.txt'));

  // Albums only show with photos the key can see, and never name the others.
  const album = async (name, paths) => {
    const madeAlbum = await api('POST', '/api/albums', { name });
    assert.equal(madeAlbum.status, 200, JSON.stringify(madeAlbum.json));
    const id = madeAlbum.json.album.id;
    const added = await api('POST', '/api/albums/items', { albumId: id, paths });
    assert.equal(added.status, 200, JSON.stringify(added.json));
    return id;
  };
  const shared = await album('Shared trip', ['docs/dot.png', 'secret/hidden-album.png']);
  const hidden = await album('Private trip', ['secret/hidden-album.png']);
  const albums = data(await tool(key, 'list_albums')).albums;
  assert.deepEqual(albums.map((a) => [a.id, a.name, a.photos]), [[shared, 'Shared trip', 1]]);
  assert.deepEqual(data(await tool(key, 'album_photos', { album: shared })).photos.map((p) => p.path), ['docs/dot.png']);
  await refused(key, 'album_photos', { album: hidden });
  await refused(key, 'album_photos', { album: 99999 });
  assert.equal(JSON.stringify(await tool(key, 'list_albums')).includes('Private'), false);
  assert.deepEqual(data(await tool(key, 'list_people')).people, []);
  assert.equal((await tool(key, 'photos_taken', { from: '2000-01-01', to: '2099-12-31' })).isError, false);
  assert.equal((await tool(key, 'photos_taken', { from: 'yesterday', to: '2099-12-31' })).isError, true);
  await refused(key, 'photos_taken', { from: '2000-01-01', to: '2099-12-31', path: 'secret' });
  const taken = data(await tool(key, 'photos_taken', { from: '2000-01-01', to: '2099-12-31' })).photos;
  assert.ok(taken.every((p) => p.path.startsWith('docs/')), JSON.stringify(taken));

  // A read key cannot change anything.
  const readOnly = await tool(key, 'add_tags', { paths: ['docs/plan.txt'], tags: ['x'] });
  assert.equal(readOnly.isError, true);
  assert.match(readOnly.content[0].text, /can only read/);

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
  // Organising: a key that can tag, make folders, rename, move, and use the Trash and albums,
  // only inside its folders.
  await write('work/a.txt', 'a\n');
  await write('work/keep.txt', 'keep\n');
  await write('work/pic.png', DOT);
  const organiser = await api('POST', '/api/agent-keys', { name: 'tidy', folders: ['work'], level: 'organise' });
  assert.equal(organiser.status, 200, JSON.stringify(organiser.json));
  const tidy = organiser.json.token;
  const tidyTools = (await mcp(tidy, 'tools/list')).json.result.tools.map((t) => t.name);
  for (const name of ['add_tags', 'remove_tags', 'create_folder', 'rename', 'move', 'move_to_trash', 'create_album', 'add_to_album', 'remove_from_album']) {
    assert.ok(tidyTools.includes(name), name);
  }
  assert.match((await mcp(tidy, 'initialize', { protocolVersion: '2025-06-18', capabilities: {} })).json.result.instructions, /organise/);
  const exists = async (rel) => fs.access(path.join(library, rel)).then(() => true, () => false);

  // Tags: all or nothing when one path is out of bounds.
  assert.equal(data(await tool(tidy, 'add_tags', { paths: ['work/a.txt'], tags: ['#todo', 'q3'] })).changed, 1);
  assert.deepEqual(data(await tool(tidy, 'get_info', { path: 'work/a.txt' })).tags.sort(), ['q3', 'todo']);
  await refused(tidy, 'add_tags', { paths: ['work/keep.txt', 'docs/plan.txt'], tags: ['x'] });
  assert.deepEqual(data(await tool(tidy, 'get_info', { path: 'work/keep.txt' })).tags, []);
  await tool(tidy, 'remove_tags', { paths: ['work/a.txt'], tags: ['q3'] });
  assert.deepEqual(data(await tool(tidy, 'get_info', { path: 'work/a.txt' })).tags, ['todo']);

  // Folders, renaming and moving.
  assert.equal(data(await tool(tidy, 'create_folder', { path: 'work', name: 'sorted' })).path, 'work/sorted');
  assert.equal((await tool(tidy, 'create_folder', { path: 'work', name: 'sorted' })).isError, true);
  await refused(tidy, 'create_folder', { path: 'docs', name: 'planted' });
  assert.equal(await exists('docs/planted'), false);
  assert.equal(data(await tool(tidy, 'move', { path: 'work/a.txt', to: 'work/sorted' })).path, 'work/sorted/a.txt');
  assert.ok(await exists('work/sorted/a.txt'));
  assert.deepEqual(data(await tool(tidy, 'get_info', { path: 'work/sorted/a.txt' })).tags, ['todo'], 'tags follow a move');
  await refused(tidy, 'move', { path: 'work/keep.txt', to: 'docs' });
  await refused(tidy, 'move', { path: 'docs/plan.txt', to: 'work' });
  await refused(tidy, 'move', { path: 'work/keep.txt', to: '.ownnas-trash' });
  assert.equal((await tool(tidy, 'move', { path: 'work/sorted', to: 'work/sorted' })).isError, true);
  assert.equal(data(await tool(tidy, 'rename', { path: 'work/sorted/a.txt', name: 'b.txt' })).path, 'work/sorted/b.txt');
  assert.equal((await tool(tidy, 'rename', { path: 'work/sorted/b.txt', name: '../escape.txt' })).isError, true);
  assert.equal((await tool(tidy, 'rename', { path: 'work', name: 'renamed' })).isError, true, "a key's own folder stays");
  assert.equal((await tool(tidy, 'move_to_trash', { path: 'work' })).isError, true);

  // The Trash, but never protected files.
  assert.equal((await tool(tidy, 'move_to_trash', { path: 'work/sorted/b.txt' })).isError, false);
  assert.equal(await exists('work/sorted/b.txt'), false);
  assert.ok((await fs.readdir(path.join(library, '.ownnas-trash'), { recursive: true })).some((name) => name.endsWith('b.txt')), 'it is in the Trash');
  assert.equal((await api('POST', '/api/protect', { paths: ['work/keep.txt'], protect: true })).status, 200);
  const kept = await tool(tidy, 'move_to_trash', { path: 'work/keep.txt' });
  assert.equal(kept.isError, true);
  assert.match(kept.content[0].text, /protected/);
  assert.ok(await exists('work/keep.txt'));
  await refused(tidy, 'move_to_trash', { path: 'docs/table.csv' });

  // Albums: its own, or ones with its photos; never the others.
  const tidyAlbum = data(await tool(tidy, 'create_album', { name: 'Work pictures' })).album;
  assert.equal(data(await tool(tidy, 'add_to_album', { album: tidyAlbum, paths: ['work/pic.png'] })).changed, 1);
  assert.deepEqual(data(await tool(tidy, 'album_photos', { album: tidyAlbum })).photos.map((p) => p.path), ['work/pic.png']);
  await refused(tidy, 'add_to_album', { album: tidyAlbum, paths: ['docs/dot.png'] });
  await refused(tidy, 'add_to_album', { album: shared, paths: ['work/pic.png'] });
  assert.equal((await tool(tidy, 'add_to_album', { album: tidyAlbum, paths: ['work/keep.txt'] })).isError, true, 'only photos and videos');
  assert.equal(data(await tool(tidy, 'remove_from_album', { album: tidyAlbum, paths: ['work/pic.png'] })).changed, 1);

  // Every change is in the agent log.
  const tidyLog = (await api('GET', `/api/agent-keys/log?id=${organiser.json.id}&limit=1000`)).json.entries;
  assert.ok(tidyLog.some((entry) => entry.tool === 'move' && entry.outcome === 'ok' && entry.detail.includes('work/sorted')), JSON.stringify(tidyLog.slice(0, 5)));
  assert.ok(tidyLog.some((entry) => entry.tool === 'move' && entry.outcome === 'refused'));
  // Undo: everything the key did today, newest first, back to how it was.
  const tidyChanges = (await api('GET', `/api/agent-keys/changes?id=${organiser.json.id}`)).json.changes;
  assert.ok(tidyChanges.length >= 9, JSON.stringify(tidyChanges.map((c) => c.summary)));
  assert.ok(tidyChanges.every((c) => c.status === 'done'), 'refused and failed calls are not changes');
  assert.ok(tidyChanges.some((c) => c.summary === 'Move work/a.txt into work/sorted' && c.canUndo));
  const undid = await api('POST', '/api/agent-keys/changes/undo-since', { keyId: organiser.json.id, since: 0 });
  assert.equal(undid.status, 200, JSON.stringify(undid.json));
  assert.deepEqual(undid.json.skipped, [], JSON.stringify(undid.json.skipped));
  assert.ok(await exists('work/a.txt'), 'moved, renamed and trashed, and back where it started');
  assert.equal(await exists('work/sorted'), false, 'the folder it made is gone again');
  assert.deepEqual(data(await tool(tidy, 'get_info', { path: 'work/a.txt' })).tags, [], 'its tags are gone');
  assert.equal((await api('GET', `/api/albums?id=${tidyAlbum}`)).status >= 400, true, 'the album it made is gone');
  const again = await api('POST', '/api/agent-keys/changes/undo', { id: tidyChanges[0].id });
  assert.equal(again.status, 409, 'an undone change cannot be undone twice');

  // Sorting a folder into sub-folders: a dry run, the real thing, and undoing it.
  const dry = data(await tool(tidy, 'sort_into_folders', { path: 'work', pattern: '{type}', dry_run: true }));
  assert.deepEqual(Object.keys(dry.folders).sort(), ['work/Documents', 'work/Pictures']);
  assert.ok(await exists('work/a.txt'), 'a dry run moves nothing');
  await refused(tidy, 'sort_into_folders', { path: 'docs', pattern: '{type}' });
  assert.equal((await tool(tidy, 'sort_into_folders', { path: 'work', pattern: 'no tokens' })).isError, true);
  assert.equal(data(await tool(tidy, 'sort_into_folders', { path: 'work', pattern: '{type}' })).moved, 3);
  assert.ok(await exists('work/Documents/a.txt') && await exists('work/Pictures/pic.png') && !(await exists('work/a.txt')));
  const sortChange = (await api('GET', `/api/agent-keys/changes?id=${organiser.json.id}&status=done`)).json.changes.find((c) => c.tool === 'sort_into_folders');
  assert.equal((await api('POST', '/api/agent-keys/changes/undo', { id: sortChange.id })).status, 200);
  assert.ok(await exists('work/a.txt') && await exists('work/pic.png') && !(await exists('work/Documents')), 'all back, and its folders gone');

  // Renaming many files: a dry run, the real thing, and undoing it.
  const renameDry = data(await tool(tidy, 'rename_many', { paths: ['work/a.txt', 'work/pic.png'], pattern: 'item {n:2}', dry_run: true })).renames;
  assert.deepEqual(renameDry.map((item) => item.to), ['work/item 01.txt', 'work/item 02.png']);
  assert.ok(await exists('work/a.txt'));
  await refused(tidy, 'rename_many', { paths: ['work/a.txt', 'docs/plan.txt'], pattern: 'x' });
  assert.equal(data(await tool(tidy, 'rename_many', { paths: ['work/a.txt', 'work/pic.png'], pattern: 'item {n:2}' })).renamed.length, 2);
  assert.ok(await exists('work/item 01.txt') && await exists('work/item 02.png'));
  const renameChange = (await api('GET', `/api/agent-keys/changes?id=${organiser.json.id}&status=done`)).json.changes.find((c) => c.tool === 'rename_many');
  assert.equal((await api('POST', '/api/agent-keys/changes/undo', { id: renameChange.id })).status, 200);
  assert.ok(await exists('work/a.txt') && await exists('work/pic.png'), 'the old names are back');

  // Approval mode: changes wait for a person, and are checked again when carried out.
  const careful = (await api('POST', '/api/agent-keys', { name: 'careful', folders: ['work'], level: 'organise', approve: true })).json;
  const asked = data(await tool(careful.token, 'create_folder', { path: 'work', name: 'inbox' }));
  assert.equal(asked.status, 'pending');
  assert.equal(await exists('work/inbox'), false, 'nothing happens before approval');
  await refused(careful.token, 'create_folder', { path: 'docs', name: 'inbox' });
  const renameAsk = data(await tool(careful.token, 'rename', { path: 'work/a.txt', name: 'z.txt' }));
  assert.equal((await api('GET', '/api/agent-keys')).json.pending[careful.id], 2);
  assert.deepEqual(data(await tool(careful.token, 'pending_changes', { status: 'pending' })).changes.map((c) => c.status), ['pending', 'pending']);
  const approved = await api('POST', '/api/agent-keys/changes/decide', { id: asked.request, approve: true });
  assert.equal(approved.status, 200, JSON.stringify(approved.json));
  assert.equal(approved.json.change.status, 'done');
  assert.ok(await exists('work/inbox'));
  assert.equal((await api('POST', '/api/agent-keys/changes/decide', { id: asked.request, approve: true })).status, 400, 'decided once');
  assert.equal((await api('POST', '/api/agent-keys/changes/decide', { id: renameAsk.request, approve: false })).json.change.status, 'declined');
  assert.ok(await exists('work/a.txt'), 'a declined change is not made');
  const late = data(await tool(careful.token, 'move_to_trash', { path: 'work/a.txt' }));
  assert.equal((await api('POST', '/api/agent-keys/approval', { id: careful.id, approve: false })).status, 200);
  assert.equal(data(await tool(careful.token, 'create_folder', { path: 'work', name: 'direct' })).path, 'work/direct', 'without asking now');
  assert.equal((await api('POST', '/api/agent-keys/revoke', { id: careful.id })).status, 200);
  const afterRevoke = (await api('POST', '/api/agent-keys/changes/decide', { id: late.request, approve: true })).json.change;
  assert.equal(afterRevoke.status, 'declined');
  assert.match(afterRevoke.result, /revoked/);
  assert.ok(await exists('work/a.txt'), 'a revoked key changes nothing, even approved');
  assert.equal((await api('POST', '/api/agent-keys/approval', { id: made.json.id, approve: true })).status, 400, 'read keys have nothing to approve');

  // Writing: new files, passages, rows and copies, each kept in the history and undone.
  await write('notes/plan.md', '# Plan\n\nStep one.\nStep two.\n');
  await write('notes/sheet.csv', 'name,amount\nAnn,1\n');
  const writerKey = (await api('POST', '/api/agent-keys', { name: 'writer', folders: ['notes'], level: 'write' })).json;
  const writer = writerKey.token;
  const writeTools = (await mcp(writer, 'tools/list')).json.result.tools.map((t) => t.name);
  for (const name of ['write_file', 'edit_text', 'append_rows', 'copy', 'move', 'add_tags']) assert.ok(writeTools.includes(name), name);
  assert.equal(tidyTools.includes('write_file'), false, 'organise keys cannot write');
  assert.match((await tool(tidy, 'write_file', { path: 'work/x.txt', text: 'x' })).content[0].text, /cannot write/);
  const read = async (rel) => fs.readFile(path.join(library, rel), 'utf8');

  assert.equal(data(await tool(writer, 'write_file', { path: 'notes/summary.md', text: '# Summary\n' })).path, 'notes/summary.md');
  assert.equal(await read('notes/summary.md'), '# Summary\n');
  assert.match((await tool(writer, 'write_file', { path: 'notes/summary.md', text: 'again' })).content[0].text, /replace: true/);
  assert.equal(data(await tool(writer, 'write_file', { path: 'notes/summary.md', text: 'replaced\n', replace: true })).replaced, true);
  await refused(writer, 'write_file', { path: 'docs/planted.txt', text: 'x' });
  await refused(writer, 'write_file', { path: 'notes/../docs/planted.txt', text: 'x' });
  assert.equal((await tool(writer, 'write_file', { path: 'notes/.env', text: 'x' })).isError, true);
  assert.equal((await tool(writer, 'write_file', { path: 'notes/missing/x.txt', text: 'x' })).isError, true);
  assert.equal(await exists('docs/planted.txt'), false);

  assert.equal((await tool(writer, 'edit_text', { path: 'notes/plan.md', find: 'Step', replace: 'Stage' })).isError, true, 'twice in the file');
  assert.equal((await tool(writer, 'edit_text', { path: 'notes/plan.md', find: 'Step three', replace: 'x' })).isError, true, 'not in the file');
  await tool(writer, 'edit_text', { path: 'notes/plan.md', find: 'Step two.', replace: 'Step two, done.' });
  assert.equal(await read('notes/plan.md'), '# Plan\n\nStep one.\nStep two, done.\n');
  await tool(writer, 'edit_text', { path: 'notes/plan.md', find: 'Step one.', replace: 'Step one, done.' });
  await refused(writer, 'edit_text', { path: 'docs/plan.txt', find: 'zebra', replace: 'horse' });
  assert.equal((await tool(writer, 'edit_text', { path: 'notes/../docs/dot.png', find: 'P', replace: 'Q' })).isError, true);

  assert.equal(data(await tool(writer, 'append_rows', { path: 'notes/sheet.csv', rows: [['Bob, Jr.', 2], ['Cy', null]] })).added, 2);
  assert.equal(await read('notes/sheet.csv'), 'name,amount\nAnn,1\n"Bob, Jr.",2\nCy,\n');
  assert.equal((await tool(writer, 'append_rows', { path: 'notes/plan.md', rows: [['x']] })).isError, true);
  // Office files: paragraphs at the end of a Word document, rows after a sheet's last row.
  const docx = Buffer.from(zipSync({
    '[Content_Types].xml': strToU8('<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>'),
    'word/document.xml': strToU8('<?xml version="1.0"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>First paragraph</w:t></w:r></w:p><w:sectPr/></w:body></w:document>'),
  }));
  const SHEET_NS = 'http://schemas.openxmlformats.org/spreadsheetml/2006/main';
  const REL_NS = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships';
  const xlsx = Buffer.from(zipSync({
    '[Content_Types].xml': strToU8('<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>'),
    '_rels/.rels': strToU8(`<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="${REL_NS}/officeDocument" Target="xl/workbook.xml"/></Relationships>`),
    'xl/workbook.xml': strToU8(`<?xml version="1.0"?><workbook xmlns="${SHEET_NS}" xmlns:r="${REL_NS}"><sheets><sheet name="Costs" sheetId="1" r:id="rId1"/></sheets></workbook>`),
    'xl/_rels/workbook.xml.rels': strToU8(`<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="${REL_NS}/worksheet" Target="worksheets/sheet1.xml"/></Relationships>`),
    'xl/worksheets/sheet1.xml': strToU8(`<?xml version="1.0"?><worksheet xmlns="${SHEET_NS}"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Item</t></is></c><c r="B1" t="inlineStr"><is><t>Cost</t></is></c></row></sheetData></worksheet>`),
  }));
  await write('notes/report.docx', docx);
  await write('notes/costs.xlsx', xlsx);
  assert.equal(data(await tool(writer, 'add_paragraphs', { path: 'notes/report.docx', paragraphs: ['Added by the agent'] })).added, 1);
  const reportText = (await tool(writer, 'read_text', { path: 'notes/report.docx' })).content[1].text;
  assert.match(reportText, /First paragraph[\s\S]*Added by the agent/);
  await tool(writer, 'edit_text', { path: 'notes/report.docx', find: 'First paragraph', replace: 'Opening paragraph' });
  assert.match((await tool(writer, 'read_text', { path: 'notes/report.docx' })).content[1].text, /Opening paragraph/);
  assert.equal(data(await tool(writer, 'append_rows', { path: 'notes/costs.xlsx', rows: [['Paper', 12.5]] })).added, 1);
  assert.deepEqual(data(await tool(writer, 'read_table', { path: 'notes/costs.xlsx' })).rows, [['Item', 'Cost'], ['Paper', '12.5']]);
  assert.equal((await tool(writer, 'append_rows', { path: 'notes/costs.xlsx', sheet: 'Nope', rows: [['x']] })).isError, true);
  assert.equal((await tool(writer, 'add_paragraphs', { path: 'notes/sheet.csv', paragraphs: ['x'] })).isError, true);

  // Any file, from base64; a name already taken gets a number.
  assert.equal(data(await tool(writer, 'upload_file', { path: 'notes', name: 'dot.png', base64: DOT.toString('base64') })).path, 'notes/dot.png');
  assert.ok((await fs.readFile(path.join(library, 'notes/dot.png'))).equals(DOT));
  assert.equal(data(await tool(writer, 'upload_file', { path: 'notes', name: 'dot.png', base64: DOT.toString('base64') })).path, 'notes/dot 2.png');
  await refused(writer, 'upload_file', { path: 'docs', name: 'x.png', base64: DOT.toString('base64') });
  assert.equal((await tool(writer, 'upload_file', { path: 'notes', name: '.hidden', base64: 'eA==' })).isError, true);
  assert.equal((await tool(writer, 'upload_file', { path: 'notes', name: 'x.bin', base64: '***' })).isError, true);

  const copied = data(await tool(writer, 'copy', { path: 'notes/plan.md', to: 'notes' })).path;
  assert.notEqual(copied, 'notes/plan.md');
  assert.ok(await exists(copied));
  await refused(writer, 'copy', { path: 'notes/plan.md', to: 'docs' });
  await refused(writer, 'copy', { path: 'docs/plan.txt', to: 'notes' });
  assert.ok(data(await tool(writer, 'list_versions', { path: 'notes/plan.md' })).versions.length >= 2, 'each change kept a version');

  // Undo it all, newest first: the files are as they were, and the new ones are in the Trash.
  const undoneWrites = await api('POST', '/api/agent-keys/changes/undo-since', { keyId: writerKey.id, since: 0 });
  assert.deepEqual(undoneWrites.json.skipped, [], JSON.stringify(undoneWrites.json));
  assert.equal(await read('notes/plan.md'), '# Plan\n\nStep one.\nStep two.\n');
  assert.equal(await read('notes/sheet.csv'), 'name,amount\nAnn,1\n');
  assert.ok((await fs.readFile(path.join(library, 'notes/report.docx'))).equals(docx), 'the Word document is as it was');
  assert.ok((await fs.readFile(path.join(library, 'notes/costs.xlsx'))).equals(xlsx), 'the spreadsheet is as it was');
  assert.equal(await exists('notes/summary.md'), false);
  assert.equal(await exists('notes/dot.png'), false, 'uploads go to the Trash on undo');
  assert.equal(await exists(copied), false);

  // Writing can wait for approval too.
  const askingWriter = (await api('POST', '/api/agent-keys', { name: 'asking writer', folders: ['notes'], level: 'write', approve: true })).json;
  const askedWrite = data(await tool(askingWriter.token, 'write_file', { path: 'notes/new.md', text: 'hello\n' }));
  assert.equal(askedWrite.status, 'pending');
  assert.equal(await exists('notes/new.md'), false);
  await refused(askingWriter.token, 'write_file', { path: 'docs/new.md', text: 'x' });
  assert.equal((await api('POST', '/api/agent-keys/changes/decide', { id: askedWrite.request, approve: true })).json.change.status, 'done');
  assert.equal(await read('notes/new.md'), 'hello\n');

  // Notes: read and written only when the notes folder is the key's.
  await write('Journal/Groceries.md', '# Groceries\n\n- [ ] milk\n- [ ] eggs\n');
  await write('Journal/Pancakes.md', 'Needs things from [[Groceries]]. #cooking\n');
  assert.equal((await api('POST', '/api/notes/folder', { folder: 'Journal' })).status, 200, 'the notes folder is a setting');
  const docsOnly = (await api('POST', '/api/agent-keys', { name: 'docs only', folders: ['docs'] })).json.token;
  assert.match((await tool(docsOnly, 'list_notes')).content[0].text, /cannot use/, 'not a folder of the docs key');
  const noteKey = (await api('POST', '/api/agent-keys', { name: 'notes', folders: ['Journal'], level: 'write' })).json;
  const noter = noteKey.token;
  const listedNotes = data(await tool(noter, 'list_notes')).notes;
  assert.deepEqual(listedNotes.map((n) => n.title).sort(), ['Groceries', 'Pancakes']);
  assert.deepEqual(listedNotes.find((n) => n.title === 'Groceries').items, [{ text: 'milk', done: false }, { text: 'eggs', done: false }]);
  assert.deepEqual(data(await tool(noter, 'list_notes', { label: 'cooking' })).notes.map((n) => n.title), ['Pancakes']);
  const groceries = data(await tool(noter, 'read_note', { path: 'Journal/Groceries.md' }));
  assert.deepEqual(groceries.linkedFrom, [{ path: 'Journal/Pancakes.md', title: 'Pancakes' }]);
  assert.equal(data(await tool(noter, 'tick_item', { path: 'Journal/Groceries.md', item: 'MILK' })).done, true);
  assert.equal(await fs.readFile(path.join(library, 'Journal/Groceries.md'), 'utf8'), '# Groceries\n\n- [x] milk\n- [ ] eggs\n');
  assert.equal((await tool(noter, 'tick_item', { path: 'Journal/Groceries.md', item: 'milk' })).isError, true, 'ticked already');
  await tool(noter, 'append_to_note', { path: 'Journal/Groceries.md', text: '- [ ] butter' });
  assert.equal(await fs.readFile(path.join(library, 'Journal/Groceries.md'), 'utf8'), '# Groceries\n\n- [x] milk\n- [ ] eggs\n- [ ] butter\n');
  const madeNote = data(await tool(noter, 'create_note', { title: 'Trip: Rome', text: 'Ideas #travel', checklist: ['tickets', 'hotel'] })).path;
  assert.equal(madeNote, 'Journal/Trip Rome.md');
  assert.equal(await fs.readFile(path.join(library, madeNote), 'utf8'), '# Trip: Rome\n\nIdeas #travel\n- [ ] tickets\n- [ ] hotel\n');
  const undoneNotes = await api('POST', '/api/agent-keys/changes/undo-since', { keyId: noteKey.id, since: 0 });
  assert.deepEqual(undoneNotes.json.skipped, []);
  assert.equal(await fs.readFile(path.join(library, 'Journal/Groceries.md'), 'utf8'), '# Groceries\n\n- [ ] milk\n- [ ] eggs\n');
  assert.equal(await exists(madeNote), false);

  // Per key: its own tools, folders it may only read, and its own limits.
  await fs.mkdir(path.join(library, 'notes/inbox'), { recursive: true });
  const limitedKey = (await api('POST', '/api/agent-keys', { name: 'limited', folders: ['notes', 'notes/inbox'], level: 'write' })).json;
  const limited = limitedKey.token;
  const setLimits = (body) => api('POST', '/api/agent-keys/settings', { id: limitedKey.id, ...body });
  assert.equal((await setLimits({ readOnly: ['notes'] })).status, 200);
  assert.match((await tool(limited, 'write_file', { path: 'notes/x.md', text: 'x' })).content[0].text, /read-only/);
  assert.equal(await exists('notes/x.md'), false);
  assert.equal(data(await tool(limited, 'write_file', { path: 'notes/inbox/x.md', text: 'x' })).path, 'notes/inbox/x.md', 'a folder of its own inside a read-only one');
  assert.match((await tool(limited, 'move', { path: 'notes/inbox/x.md', to: 'notes' })).content[0].text, /read-only/);
  assert.match((await tool(limited, 'add_tags', { paths: ['notes/plan.md'], tags: ['x'] })).content[0].text, /read-only/);
  assert.equal(data(await tool(limited, 'read_text', { path: 'notes/plan.md' })).path, 'notes/plan.md', 'reading is still fine');
  assert.equal((await setLimits({ readOnly: ['docs'] })).status, 400, 'only its own folders');
  assert.equal((await setLimits({ tools: ['run_shell'] })).status, 400);
  assert.equal((await setLimits({ tools: [] })).status, 400);
  assert.equal((await setLimits({ tools: ['list_folder', 'read_text', 'write_file'], readOnly: ['notes'] })).status, 200);
  assert.deepEqual((await mcp(limited, 'tools/list')).json.result.tools.map((t) => t.name).sort(), ['list_folder', 'read_text', 'write_file']);
  assert.match((await tool(limited, 'search', { query: 'plan' })).content[0].text, /not given that tool/);
  assert.equal((await api('GET', '/api/agent-keys')).json.keys.find((k) => k.id === limitedKey.id).readOnly[0], 'notes');
  assert.equal((await setLimits({ dailyChanges: 1 })).status, 200);
  assert.equal(data(await tool(limited, 'write_file', { path: 'notes/inbox/y.md', text: 'y' })).status, 'pending', 'one change today already');
  assert.match(data(await tool(limited, 'write_file', { path: 'notes/inbox/z.md', text: 'z' })).why, /1 changes today/);
  assert.equal((await setLimits({ callsPerMinute: 3 })).status, 200);
  const quick = [];
  for (let i = 0; i < 4; i++) quick.push((await mcp(limited, 'ping')).status);
  assert.equal(quick.at(-1), 429, JSON.stringify(quick));

  // Connecting with OAuth (as Claude's custom connectors do): discovery, registration, approval
  // by an administrator, a code traded with PKCE, refreshing, and revoking.
  const unsigned = await fetch(`${base}/mcp`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: '{"jsonrpc":"2.0","id":1,"method":"ping"}' });
  assert.equal(unsigned.status, 401);
  assert.match(unsigned.headers.get('www-authenticate'), /resource_metadata="http:\/\/127\.0\.0\.1:\d+\/\.well-known\/oauth-protected-resource"/);
  const protectedResource = await (await fetch(`${base}/.well-known/oauth-protected-resource`)).json();
  assert.equal(protectedResource.resource, `${base}/mcp`);
  const authServer = await (await fetch(`${protectedResource.authorization_servers[0]}/.well-known/oauth-authorization-server`)).json();
  assert.deepEqual(authServer.code_challenge_methods_supported, ['S256']);
  const registered = await fetch(authServer.registration_endpoint, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ client_name: 'Test Client', redirect_uris: ['http://localhost:4567/callback'] }) });
  assert.equal(registered.status, 201);
  const clientId = (await registered.json()).client_id;
  assert.equal((await fetch(authServer.registration_endpoint, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ redirect_uris: ['http://evil.example/cb'] }) })).status, 400, 'plain http elsewhere');
  const verifier = createHash('sha256').update('a verifier long enough to be random').digest('base64url') + 'xyz';
  const challenge = createHash('sha256').update(verifier).digest('base64url');
  const ask = { response_type: 'code', client_id: clientId, redirect_uri: 'http://localhost:4567/callback', code_challenge: challenge, code_challenge_method: 'S256', state: 'st4te' };
  const authorize = await fetch(`${authServer.authorization_endpoint}?${new URLSearchParams(ask)}`, { redirect: 'manual' });
  assert.equal(authorize.status, 302);
  assert.match(authorize.headers.get('location'), /^\/#oauth\?/);
  assert.equal((await fetch(`${authServer.authorization_endpoint}?${new URLSearchParams({ ...ask, code_challenge_method: 'plain' })}`, { redirect: 'manual' })).status, 400, 'PKCE S256 only');
  assert.equal((await api('GET', `/api/oauth/client?${new URLSearchParams(ask)}`)).json.name, 'Test Client');
  const denied = (await api('POST', '/api/oauth/approve', { ...ask, deny: true })).json.redirect;
  assert.match(denied, /^http:\/\/localhost:4567\/callback\?error=access_denied&state=st4te/);
  const allowed = (await api('POST', '/api/oauth/approve', { ...ask, folders: ['docs'], level: 'read' })).json.redirect;
  const code = new URL(allowed).searchParams.get('code');
  assert.equal(new URL(allowed).searchParams.get('state'), 'st4te');
  const tokenFor = (fields) => fetch(authServer.token_endpoint, { method: 'POST', headers: { 'content-type': 'application/x-www-form-urlencoded' }, body: new URLSearchParams(fields) }).then(async (r) => ({ status: r.status, json: await r.json() }));
  const wrong = await tokenFor({ grant_type: 'authorization_code', code, client_id: clientId, redirect_uri: ask.redirect_uri, code_verifier: 'not it' });
  assert.equal(wrong.json.error, 'invalid_grant', 'a wrong verifier');
  const allowed2 = (await api('POST', '/api/oauth/approve', { ...ask, folders: ['docs'], level: 'read' })).json.redirect;
  const code2 = new URL(allowed2).searchParams.get('code');
  const tokens = await tokenFor({ grant_type: 'authorization_code', code: code2, client_id: clientId, redirect_uri: ask.redirect_uri, code_verifier: verifier });
  assert.equal(tokens.status, 200, JSON.stringify(tokens.json));
  assert.equal(tokens.json.token_type, 'Bearer');
  assert.equal((await tokenFor({ grant_type: 'authorization_code', code: code2, client_id: clientId, redirect_uri: ask.redirect_uri, code_verifier: verifier })).json.error, 'invalid_grant', 'a code works once');
  assert.ok((await mcp(tokens.json.access_token, 'tools/list')).json.result.tools.length > 5);
  await refused(tokens.json.access_token, 'read_text', { path: 'secret/private.txt' });
  const connectedKey = (await api('GET', '/api/agent-keys')).json.keys.find((k) => k.name === 'Test Client (connected)' && !k.revokedAt && k.lastUsedAt);
  assert.ok(connectedKey, 'an agent key, like any other');
  const refreshed = await tokenFor({ grant_type: 'refresh_token', refresh_token: tokens.json.refresh_token, client_id: clientId });
  assert.equal(refreshed.status, 200);
  assert.equal((await mcp(tokens.json.access_token, 'ping')).status, 401, 'the old token stops');
  assert.equal((await mcp(refreshed.json.access_token, 'ping')).status, 200);
  assert.equal((await tokenFor({ grant_type: 'refresh_token', refresh_token: tokens.json.refresh_token, client_id: clientId })).json.error, 'invalid_grant', 'a refresh token works once');
  assert.equal((await api('POST', '/api/agent-keys/revoke', { id: connectedKey.id })).status, 200);
  assert.equal((await mcp(refreshed.json.access_token, 'ping')).status, 401);
  assert.equal((await tokenFor({ grant_type: 'refresh_token', refresh_token: refreshed.json.refresh_token, client_id: clientId })).json.error, 'invalid_grant', 'revoked is revoked');

  // A key stuck in a loop is slowed down, and others are not.
  const busy = (await api('POST', '/api/agent-keys', { name: 'busy', folders: ['docs'] })).json.token;
  const statuses = [];
  for (let i = 0; i < 301; i++) statuses.push((await mcp(busy, 'ping')).status);
  assert.deepEqual([...new Set(statuses.slice(0, 300))], [200]);
  const slowed = await mcp(busy, 'ping');
  assert.equal(slowed.status, 429);
  assert.ok(Number(slowed.headers.get('retry-after')) > 0);
  assert.equal((await mcp(writer, 'ping')).status, 200, 'other keys go on');

  // And nothing outside its folder changed.
  const afterTidy = await fingerprints(library);
  for (const [file, print] of Object.entries(before)) {
    if (file.includes('/secret/')) continue;
    assert.equal(afterTidy[file], print, `${file} changed`);
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
