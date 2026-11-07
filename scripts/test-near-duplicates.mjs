// Photos that look alike, in headless Chrome against a throwaway server: a resized and a slightly
// brighter copy are grouped with the original, a different photo is not, and the extra copies go to
// the Trash (CHROME overrides Chrome's path; OWNNAS_TARGET the target directory).
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import fs from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import zlib from 'node:zlib';

const repo = path.resolve(import.meta.dirname, '..');
const target = process.env.OWNNAS_TARGET || path.join(repo, 'target/debug');
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'ownnas-near-'));
const library = path.join(tmp, 'library');
/// A PNG of `size`×`size` pixels, coloured by `pixel(x, y)` as [r, g, b] for a 0..1 position.
function png(size, pixel) {
  const rows = [];
  for (let y = 0; y < size; y++) {
    const row = Buffer.alloc(1 + size * 3);
    for (let x = 0; x < size; x++) {
      const [r, g, b] = pixel(x / size, y / size);
      row.writeUInt8(Math.max(0, Math.min(255, Math.round(r))), 1 + x * 3);
      row.writeUInt8(Math.max(0, Math.min(255, Math.round(g))), 2 + x * 3);
      row.writeUInt8(Math.max(0, Math.min(255, Math.round(b))), 3 + x * 3);
    }
    rows.push(row);
  }
  const chunk = (type, data) => {
    const length = Buffer.alloc(4);
    length.writeUInt32BE(data.length);
    const crc = Buffer.alloc(4);
    crc.writeUInt32BE(zlib.crc32(Buffer.concat([Buffer.from(type), data])) >>> 0);
    return Buffer.concat([length, Buffer.from(type), data, crc]);
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(size, 0);
  header.writeUInt32BE(size, 4);
  header.set([8, 2, 0, 0, 0], 8);
  return Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk('IHDR', header), chunk('IDAT', zlib.deflateSync(Buffer.concat(rows))), chunk('IEND', Buffer.alloc(0))]);
}
const scene = (light) => (x, y) => [Math.sin(x * 9) * 90 + 120 + light, y * 200 + light, (x * y) * 255 + light];
const other = (x, y) => [((x * 7 + y * 3) % 1) * 255, Math.cos(y * 13) * 120 + 120, x * 255];
fs.mkdirSync(path.join(library, 'Camera'), { recursive: true });
fs.writeFileSync(path.join(library, 'Camera/IMG_1.png'), png(320, scene(0)));
fs.writeFileSync(path.join(library, 'Camera/IMG_1 small.png'), png(120, scene(0)));
fs.writeFileSync(path.join(library, 'Camera/IMG_2 burst.png'), png(320, scene(12)));
fs.writeFileSync(path.join(library, 'Camera/IMG_9 other.png'), png(320, other));
const exists = (rel) => fs.existsSync(path.join(library, rel));

/// A JPEG with EXIF: a camera make and model, and when it was taken.
function jpegWithExif() {
  const JPEG = Buffer.from('/9j/4AAQSkZJRgABAQEASABIAAD/2wBDAP//////////////////////////////////////////////////////////////////////////////////////wgALCAABAAEBAREA/8QAFBABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQABPxA=', 'base64');
  const ascii = (text) => Buffer.from(`${text}\0`, 'latin1');
  const make = ascii('OwnNAS Cam');
  const model = ascii('Test Model 1');
  const taken = ascii('2024:03:09 14:32:05');
  // Little-endian TIFF: IFD0 (Make, Model, pointer to the Exif IFD), then the Exif IFD.
  const ifd0At = 8;
  const ifd0Size = 2 + 3 * 12 + 4;
  const exifAt = ifd0At + ifd0Size;
  const exifSize = 2 + 1 * 12 + 4;
  let dataAt = exifAt + exifSize;
  const data = [];
  const place = (bytes) => { const at = dataAt; data.push(bytes); dataAt += bytes.length; return at; };
  const entry = (tag, type, count, value) => { const b = Buffer.alloc(12); b.writeUInt16LE(tag, 0); b.writeUInt16LE(type, 2); b.writeUInt32LE(count, 4); b.writeUInt32LE(value, 8); return b; };
  const makeAt = place(make);
  const modelAt = place(model);
  const takenAt = place(taken);
  const ifd0 = Buffer.concat([Buffer.from([3, 0]), entry(0x010f, 2, make.length, makeAt), entry(0x0110, 2, model.length, modelAt), entry(0x8769, 4, 1, exifAt), Buffer.alloc(4)]);
  const exifIfd = Buffer.concat([Buffer.from([1, 0]), entry(0x9003, 2, taken.length, takenAt), Buffer.alloc(4)]);
  const tiff = Buffer.concat([Buffer.from('II*\0', 'latin1'), Buffer.from([8, 0, 0, 0]), ifd0, exifIfd, ...data]);
  const payload = Buffer.concat([Buffer.from('Exif\0\0', 'latin1'), tiff]);
  const length = Buffer.alloc(2);
  length.writeUInt16BE(payload.length + 2);
  return Buffer.concat([JPEG.subarray(0, 2), Buffer.from([0xff, 0xe1]), length, payload, JPEG.subarray(2)]);
}
fs.writeFileSync(path.join(library, 'Camera/with exif.jpg'), jpegWithExif());

const PASSWORD = randomBytes(12).toString('hex');
const freePort = () => new Promise((done) => {
  const probe = net.createServer().listen(0, '127.0.0.1', () => {
    const { port } = probe.address();
    probe.close(() => done(port));
  });
});
const port = await freePort();
const server = spawn(path.join(target, 'ownnas'), ['serve', '--root', library, '--data', path.join(tmp, 'data'), '--addr', `127.0.0.1:${port}`, '--username', 'admin', '--password', PASSWORD], { stdio: 'ignore', env: { ...process.env, HOME: tmp, USERPROFILE: tmp } });
const base = `http://127.0.0.1:${port}/`;
const debugPort = await freePort();
const chromePath = process.env.CHROME || (process.platform === 'darwin' ? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' : 'google-chrome');
const chrome = spawn(chromePath, ['--headless=new', '--disable-gpu', '--window-size=1300,900', `--remote-debugging-port=${debugPort}`, `--user-data-dir=${path.join(tmp, 'profile')}`, 'about:blank'], { stdio: 'ignore' });
const sleep = (ms) => new Promise((done) => setTimeout(done, ms));

let failed = false;
try {
  let targets;
  for (let i = 0; i < 60 && !targets?.length; i++) {
    await sleep(200);
    try { targets = (await (await fetch(`http://127.0.0.1:${debugPort}/json`)).json()).filter((item) => item.type === 'page'); } catch { /* starting */ }
  }
  const ws = new WebSocket(targets[0].webSocketDebuggerUrl);
  await new Promise((done) => { ws.onopen = done; });
  let nextId = 0;
  const waiting = new Map();
  const problems = [];
  ws.onmessage = (event) => {
    const message = JSON.parse(event.data);
    if (message.id && waiting.has(message.id)) {
      waiting.get(message.id)(message);
      waiting.delete(message.id);
    }
    if (message.method === 'Runtime.exceptionThrown') problems.push(message.params.exceptionDetails.exception?.description ?? message.params.exceptionDetails.text);
  };
  const send = (method, params = {}) => new Promise((done) => {
    const id = ++nextId;
    waiting.set(id, done);
    ws.send(JSON.stringify({ id, method, params }));
  });
  const js = async (expression) => {
    const answer = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
    if (answer.result.exceptionDetails) throw new Error(answer.result.exceptionDetails.exception?.description ?? answer.result.exceptionDetails.text);
    return answer.result.result.value;
  };
  const until = async (expression, what, tries = 200) => {
    for (let i = 0; i < tries; i++) {
      try {
        if (await js(expression)) return;
      } catch (err) {
        if (/SyntaxError/.test(err.message)) throw err;
      }
      await sleep(100);
    }
    throw new Error(`Timed out waiting for ${what}. Page: ${JSON.stringify(await js('document.body.innerText.slice(0, 1200)'))}`);
  };

  await send('Page.enable');
  await send('Runtime.enable');
  for (let i = 0; i < 100; i++) {
    try { await fetch(`${base}api/me`); break; } catch { await sleep(100); }
  }
  await send('Page.navigate', { url: base });
  await until(`document.readyState === 'complete'`, 'the page');
  const signIn = `fetch('/api/login', { method: 'POST', headers: { 'content-type': 'application/json', 'x-ownnas': '1' }, body: JSON.stringify({ username: 'admin', password: ${JSON.stringify(PASSWORD)} }) }).then((r) => r.status)`;
  assert.equal(await js(signIn), 200);
  const call = (method, url, body) => js(`fetch(${JSON.stringify(url)}, { method: ${JSON.stringify(method)}, headers: { 'content-type': 'application/json', 'x-ownnas': '1' }, ${body ? `body: ${JSON.stringify(JSON.stringify(body))}` : ''} }).then(async (r) => ({ status: r.status, json: await r.json().catch(() => null) }))`);
  await send('Page.navigate', { url: `${base}?signed-in#/Camera` });
  await until(`document.querySelector('.card[data-path="Camera/IMG_1.png"]')`, 'the photos');
  await js(`(() => { const files = document.getElementById('files'); const box = files.getBoundingClientRect(); files.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: box.right - 5, clientY: box.bottom - 5 })); return true; })()`);
  await until(`document.querySelector('#menu:not([hidden]) [data-menu="nearDuplicates"]')`, 'the menu item');
  // The search index may still be reading the library: ask again until it answers.
  for (let i = 0; i < 40; i++) {
    await js(`document.querySelector('dialog.near-dialog')?.close(); openNearDuplicates('Camera'); true`);
    await until(`!document.querySelector('dialog.near-dialog [data-near="status"]').textContent.startsWith('Comparing')`, 'the comparison');
    if (!(await js(`document.querySelector('dialog.near-dialog [data-near="status"]').textContent.includes('still reading')`))) break;
    await new Promise((done) => setTimeout(done, 300));
  }
  const status = await js(`document.querySelector('dialog.near-dialog [data-near="status"]').textContent`);
  assert.match(status, /^1 group of photos that look alike, 2 that could go \(\d+ photos compared\)/, status);
  const group = await js(`[...document.querySelectorAll('dialog.near-dialog .near-photo')].map((photo) => [photo.querySelector('.near-name').textContent, photo.querySelector('input').checked])`);
  assert.deepEqual(group[0], ['IMG_1.png', false], 'the largest is kept');
  assert.deepEqual(group.slice(1).map(([name]) => name).sort(), ['IMG_1 small.png', 'IMG_2 burst.png']);
  assert.ok(group.slice(1).every(([, ticked]) => ticked));
  await js(`document.querySelector('dialog.near-dialog [data-near="trash"]').click(), true`);
  for (let i = 0; i < 50 && exists('Camera/IMG_2 burst.png'); i++) await new Promise((done) => setTimeout(done, 100));
  assert.ok(exists('Camera/IMG_1.png') && exists('Camera/IMG_9 other.png'));
  assert.equal(exists('Camera/IMG_1 small.png') || exists('Camera/IMG_2 burst.png'), false, 'the copies are in the Trash');
  await until(`document.querySelector('dialog.near-dialog [data-near="status"]').textContent.startsWith('No photos that look alike')`, 'nothing left to group');
  await js(`document.querySelector('dialog.near-dialog').close(); true`);

  // The preview of a photo shows its details (EXIF), when opened.
  await js(`openEntry({ path: 'Camera/with exif.jpg', name: 'with exif.jpg', dir: false, size: 0, modified: 0, kind: 'image' }); true`);
  await until(`!document.getElementById('preview-exif').hidden`, 'the photo details strip');
  await js(`document.getElementById('exif-toggle').click(), true`);
  await until(`document.getElementById('exif-details').textContent.includes('Test Model 1')`, 'the camera');
  const exifText = await js(`document.getElementById('exif-details').textContent`);
  assert.match(exifText, /OwnNAS Cam/);
  assert.match(exifText, /2024/);
  assert.match(await js(`document.getElementById('exif-summary').textContent`), /Test Model 1/);
  await js(`document.getElementById('preview-close').click(); true`);
  await js(`openEntry({ path: 'Camera/IMG_1.png', name: 'IMG_1.png', dir: false, size: 0, modified: 0, kind: 'image' }); true`);
  await until(`!document.getElementById('exif-details').hidden && !document.getElementById('exif-details').textContent.includes('Reading')`, 'stays open for the next photo');

  assert.deepEqual(problems, [], 'no errors in the page');
  console.log('ok: photos that look alike');
} catch (err) {
  failed = true;
  console.error(err);
} finally {
  chrome.kill();
  server.kill();
  await sleep(300);
  fs.rmSync(tmp, { recursive: true, force: true });
}
process.exit(failed ? 1 : 0);
