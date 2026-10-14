// End-to-end checks for the Gallery: the photo timeline, nested virtual albums, and tags.
// Run after `cargo build --bin ownnas` (set OWNNAS_TARGET to use another target directory).
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs/promises';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';

const repo = path.resolve(path.dirname(new URL(import.meta.url).pathname), '..');
const binary = path.join(process.env.OWNNAS_TARGET || path.join(repo, 'target/debug'), 'ownnas');
const PASSWORD = 'integration-test-passphrase';

// A 1×1 JPEG and PNG: enough for the index, which only reads what it needs.
const JPEG = Buffer.from('/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/wAALCAABAAEBAREA/8QAFAABAAAAAAAAAAAAAAAAAAAACf/EABQQAQAAAAAAAAAAAAAAAAAAAAD/2gAIAQEAAD8AKp//2Q==', 'base64');
const PNG = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==', 'base64');

const tmp = await fs.mkdtemp(path.join(os.tmpdir(), 'ownnas-gallery-'));
const library = path.join(tmp, 'library');
const files = {
  'trip/a.jpg': [JPEG, '2020-05-01T10:00:00Z'],
  'trip/b.jpg': [JPEG, '2021-05-01T10:00:00Z'],
  'home/c.png': [PNG, '2022-05-01T10:00:00Z'],
  'docs/notes.txt': [Buffer.from('not a photo'), '2023-01-01T00:00:00Z'],
  '.hidden/secret.jpg': [JPEG, '2023-01-01T00:00:00Z'],
  'Photos Library.photoslibrary/originals/p.jpg': [JPEG, '2023-01-01T00:00:00Z'],
};
for (const [rel, [bytes, when]] of Object.entries(files)) {
  await fs.mkdir(path.dirname(path.join(library, rel)), { recursive: true });
  await fs.writeFile(path.join(library, rel), bytes);
  await fs.utimes(path.join(library, rel), new Date(when), new Date(when));
}

/// Every file in the library with a hash of its content and its modification time.
async function snapshot() {
  const out = {};
  async function walk(dir, rel) {
    for (const entry of await fs.readdir(dir, { withFileTypes: true })) {
      const full = path.join(dir, entry.name);
      const key = rel ? `${rel}/${entry.name}` : entry.name;
      if (entry.isDirectory()) await walk(full, key);
      else {
        const stat = await fs.stat(full);
        out[key] = `${crypto.createHash('sha256').update(await fs.readFile(full)).digest('hex')}@${stat.mtimeMs}`;
      }
    }
  }
  await walk(library, '');
  return out;
}

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

async function call(method, url, body) {
  const response = await fetch(base + url, {
    method,
    headers: { cookie, 'x-ownnas': '1', ...(body ? { 'content-type': 'application/json' } : {}) },
    body: body ? JSON.stringify(body) : undefined,
  });
  const text = await response.text();
  let data = {};
  try { data = JSON.parse(text); } catch { data = { text }; }
  return { status: response.status, data };
}

const paths = (items) => items.map((item) => item.path);

try {
  for (let i = 0; i < 100; i++) {
    try { await fetch(base); break; } catch { await sleep(100); }
  }
  const login = await fetch(`${base}/api/login`, {
    method: 'POST',
    headers: { 'x-ownnas': '1', 'content-type': 'application/json' },
    body: JSON.stringify({ username: 'admin', password: PASSWORD }),
  });
  assert.equal(login.status, 200);
  cookie = login.headers.get('set-cookie').split(';')[0];
  const before = await snapshot();

  // The library is indexed in the background as the server starts.
  let status;
  for (let i = 0; i < 100; i++) {
    status = (await call('GET', '/api/gallery/status')).data;
    if (!status.scanning && status.finishedAt) break;
    await sleep(100);
  }
  assert.equal(status.total, 3, 'only photos, outside hidden folders and Apple Photos’ library');

  // The timeline is newest first and pages by date taken.
  let page = (await call('GET', '/api/gallery/timeline?limit=2')).data;
  assert.deepEqual(paths(page.items), ['home/c.png', 'trip/b.jpg']);
  assert.equal(page.more, true);
  const last = page.items.at(-1);
  page = (await call('GET', `/api/gallery/timeline?limit=2&before=${last.takenAt}&beforePath=${encodeURIComponent(last.path)}`)).data;
  assert.deepEqual(paths(page.items), ['trip/a.jpg']);
  assert.equal(page.items[0].dateSource, 'file');
  assert.equal(page.items[0].kind, 'image');

  // Albums nest like folders.
  const trips = (await call('POST', '/api/albums', { name: 'Trips' })).data.album;
  const italy = (await call('POST', '/api/albums', { name: 'Italy', parentId: trips.id })).data.album;
  assert.equal((await call('POST', '/api/albums', { name: 'trips' })).status, 409);
  assert.equal((await call('POST', '/api/albums', { name: 'Lost', parentId: 9999 })).status, 404);
  let added = await call('POST', '/api/albums/items', { albumId: italy.id, paths: ['trip/a.jpg', 'trip/b.jpg'] });
  assert.equal(added.data.changed, 2);
  assert.equal((await call('POST', '/api/albums/items', { albumId: italy.id, paths: ['docs/notes.txt'] })).status, 400);
  assert.notEqual((await call('POST', '/api/albums/items', { albumId: italy.id, paths: ['../escape.jpg'] })).status, 200);
  await call('POST', '/api/albums/items', { albumId: trips.id, paths: ['home/c.png'] });

  let view = (await call('GET', `/api/albums?id=${italy.id}`)).data;
  assert.deepEqual(view.breadcrumb.map((crumb) => crumb.name), ['Trips', 'Italy']);
  assert.deepEqual(paths(view.items), ['trip/b.jpg', 'trip/a.jpg']);
  view = (await call('GET', `/api/albums?id=${trips.id}`)).data;
  assert.deepEqual(view.albums.map((album) => [album.name, album.count]), [['Italy', 2]]);
  assert.deepEqual(paths(view.items), ['home/c.png'], 'an album shows its own photos, not its sub-albums’');
  view = (await call('GET', '/api/albums')).data;
  assert.deepEqual(view.albums.map((album) => album.name), ['Trips']);
  assert.equal((await call('GET', '/api/albums/tree')).data.albums.length, 2);
  let notes = (await call('GET', '/api/annotations?path=trip%2Fa.jpg')).data;
  assert.deepEqual(notes.albums.map((album) => album.name), ['Trips / Italy']);

  // Moving, renaming, setting the cover, and deleting albums.
  assert.equal((await call('PATCH', '/api/albums', { id: trips.id, move: true, parentId: italy.id })).status, 400);
  const rome = (await call('POST', '/api/albums', { name: 'Rome' })).data.album;
  assert.equal((await call('PATCH', '/api/albums', { id: rome.id, move: true, parentId: italy.id })).status, 200);
  assert.equal((await call('PATCH', '/api/albums', { id: rome.id, name: 'Roma' })).data.album.name, 'Roma');
  assert.equal((await call('PATCH', '/api/albums', { id: italy.id, setCover: true, cover: 'trip/a.jpg' })).data.album.cover, 'trip/a.jpg');

  // Tags show in the Gallery too.
  await call('POST', '/api/tags', { path: 'trip/a.jpg', tag: 'beach' });
  await call('POST', '/api/tags', { path: 'docs/notes.txt', tag: 'beach' });
  assert.deepEqual((await call('GET', '/api/tags/all')).data.tags, [{ tag: 'beach', count: 2 }]);
  assert.deepEqual(paths((await call('GET', '/api/gallery/tag?tag=Beach')).data.items), ['trip/a.jpg'], 'only photos and videos');

  // Hiding a photo leaves it out of the timeline only.
  assert.equal((await call('POST', '/api/gallery/hidden', { paths: ['trip/b.jpg'], hidden: true })).data.changed, 1);
  assert.equal((await call('POST', '/api/gallery/hidden', { paths: ['docs/notes.txt'], hidden: true })).status, 400);
  assert.deepEqual(paths((await call('GET', '/api/gallery/timeline')).data.items), ['home/c.png', 'trip/a.jpg']);
  let days = (await call('GET', '/api/gallery/days')).data;
  assert.equal(days.hidden, 1);
  assert.equal(days.days.reduce((sum, day) => sum + day.count, 0), 2);
  const inAlbum = (await call('GET', `/api/albums?id=${italy.id}`)).data.items;
  assert.deepEqual(inAlbum.map((item) => [item.path, item.hidden]), [['trip/b.jpg', true], ['trip/a.jpg', false]], 'albums still show it');
  assert.deepEqual(paths((await call('GET', '/api/gallery/hidden')).data.items), ['trip/b.jpg']);

  // None of that changed a single file.
  assert.deepEqual(await snapshot(), before, 'albums and the index never touch the library');

  // Files moved by OwnNAS keep their albums: renaming a folder, the Trash and back, copies.
  assert.equal((await call('POST', '/api/rename', { path: 'trip', name: 'travel' })).status, 200);
  view = (await call('GET', `/api/albums?id=${italy.id}`)).data;
  assert.deepEqual(paths(view.items), ['travel/b.jpg', 'travel/a.jpg']);
  assert.equal(view.album.cover, 'travel/a.jpg');
  assert.deepEqual(paths((await call('GET', '/api/gallery/hidden')).data.items), ['travel/b.jpg'], 'the flag follows a renamed folder');
  await call('POST', '/api/gallery/hidden', { paths: ['travel/b.jpg'], hidden: false });
  assert.equal((await call('GET', '/api/gallery/days')).data.hidden, 0);
  const trashed = (await call('DELETE', '/api/entry?path=travel%2Fb.jpg')).data.path;
  assert.ok(trashed);
  assert.deepEqual(paths((await call('GET', `/api/albums?id=${italy.id}`)).data.items), ['travel/a.jpg'], 'the Trash hides it');
  assert.ok(!paths((await call('GET', '/api/gallery/timeline')).data.items).includes(trashed));
  await call('POST', '/api/restore', { path: trashed });
  assert.deepEqual(paths((await call('GET', `/api/albums?id=${italy.id}`)).data.items), ['travel/b.jpg', 'travel/a.jpg']);
  const copy = (await call('POST', '/api/duplicate', { path: 'travel/a.jpg' })).data.path;
  notes = (await call('GET', `/api/annotations?path=${encodeURIComponent(copy)}`)).data;
  assert.deepEqual(notes.albums, [], 'a copy is not added to albums');
  assert.ok(paths((await call('GET', '/api/gallery/timeline')).data.items).includes(copy), 'but shows in the timeline');

  // A file moved outside OwnNAS shows as missing until it is removed from the album.
  await fs.rename(path.join(library, 'home/c.png'), path.join(library, 'c-moved.png'));
  view = (await call('GET', `/api/albums?id=${trips.id}`)).data;
  assert.equal(view.items[0].missing, true);
  await call('POST', '/api/albums/items', { albumId: trips.id, paths: ['home/c.png'], action: 'remove' });
  assert.deepEqual((await call('GET', `/api/albums?id=${trips.id}`)).data.items, []);

  // A rescan finds the moved file and forgets the old path.
  await call('POST', '/api/gallery/rescan');
  for (let i = 0; i < 100; i++) {
    await sleep(100);
    const items = paths((await call('GET', '/api/gallery/timeline')).data.items);
    if (items.includes('c-moved.png') && !items.includes('home/c.png')) break;
    assert.ok(i < 99, 'the rescan picks up changes made outside OwnNAS');
  }

  // Deleting an album deletes the albums inside it, never the photos.
  assert.equal((await call('DELETE', `/api/albums?id=${trips.id}`)).status, 200);
  assert.equal((await call('GET', `/api/albums?id=${italy.id}`)).status, 404);
  assert.deepEqual((await call('GET', '/api/albums')).data.albums.map((album) => album.name), []);
  await fs.access(path.join(library, 'travel/a.jpg'));
  await fs.access(path.join(library, 'travel/b.jpg'));
  console.log('gallery checks passed');
} finally {
  server.kill();
  await fs.rm(tmp, { recursive: true, force: true });
}
