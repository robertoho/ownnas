// Moving thumbnails, in headless Chrome against a throwaway server, with real clips made by
// ffmpeg: an iPhone-like Live Photo in HEVC, Google's and Samsung's ways of keeping a video inside
// a photo, and a photo marked as a Motion Photo with no video (CHROME overrides Chrome's path;
// OWNNAS_TARGET the target directory).
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import fs from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';

const repo = path.resolve(import.meta.dirname, '..');
const target = process.env.OWNNAS_TARGET || path.join(repo, 'target/debug');
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'ownnas-motion-'));
const library = path.join(tmp, 'library');
fs.mkdirSync(library);
const ffmpeg = (...args) => {
  const run = spawnSync('ffmpeg', ['-y', '-hide_banner', '-loglevel', 'error', ...args], { encoding: 'utf8' });
  assert.equal(run.status, 0, run.stderr);
};
const LAVFI = ['-f', 'lavfi', '-i', 'testsrc=size=720x1280:rate=30:duration=2'];
ffmpeg(...LAVFI, '-frames:v', '1', path.join(tmp, 'photo.jpg'));
ffmpeg(...LAVFI, '-c:v', 'libx265', '-tag:v', 'hvc1', '-pix_fmt', 'yuv420p', path.join(tmp, 'hevc.mov'));
ffmpeg(...LAVFI, '-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-f', 'mp4', path.join(tmp, 'h264.mp4'));
const photo = fs.readFileSync(path.join(tmp, 'photo.jpg'));
const h264 = fs.readFileSync(path.join(tmp, 'h264.mp4'));
const xmp = (flag) => Buffer.from(`http://ns.adobe.com/xap/1.0/ GCamera:MotionPhoto="${flag}"`);
const withComment = (mark) => {
  const block = Buffer.alloc(4);
  block[0] = 0xff; block[1] = 0xfe;
  block.writeUInt16BE(mark.length + 2, 2);
  return Buffer.concat([photo.subarray(0, 2), block, mark, photo.subarray(2)]);
};
// Apple: a video of the same name beside the photo.
fs.writeFileSync(path.join(library, 'IMG_0001.JPG'), photo);
fs.copyFileSync(path.join(tmp, 'hevc.mov'), path.join(library, 'IMG_0001.MOV'));
// Google: marked at the start, the video after the picture.
fs.writeFileSync(path.join(library, 'PXL_0002.MP.jpg'), Buffer.concat([withComment(xmp(1)), h264]));
// Samsung: marked in the trailer at the end.
fs.writeFileSync(path.join(library, 'sam_0003.jpg'), Buffer.concat([photo, Buffer.from('MotionPhoto_Data'), h264, Buffer.from('MotionPhoto_Data....SEFT')]));
// Not moving, though marked, and plain.
fs.writeFileSync(path.join(library, 'still_marked.jpg'), withComment(xmp(0)));
fs.writeFileSync(path.join(library, 'plain.jpg'), photo);

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
const MOVING = ['IMG_0001.JPG', 'PXL_0002.MP.jpg', 'sam_0003.jpg'];

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
    throw new Error(`Timed out waiting for ${what}. Page: ${await js('document.body.innerText.slice(0, 400)')}`);
  };
  await send('Page.enable');
  await send('Runtime.enable');
  for (let i = 0; i < 100; i++) {
    try { await fetch(`${base}api/me`); break; } catch { await sleep(100); }
  }
  await send('Page.navigate', { url: base });
  await until(`document.readyState === 'complete'`, 'the page');
  const login = await fetch(`${base}api/login`, { method: 'POST', headers: { 'content-type': 'application/json', 'x-ownnas': '1' }, body: JSON.stringify({ username: 'admin', password: PASSWORD, token: true }) });
  const { token } = await login.json();
  assert.equal(await js(`fetch('/api/login', { method: 'POST', headers: { 'content-type': 'application/json', 'x-ownnas': '1' }, body: JSON.stringify({ username: 'admin', password: ${JSON.stringify(PASSWORD)} }) }).then((r) => r.status)`), 200);
  const authed = { authorization: `Bearer ${token}`, 'x-ownnas': '1' };

  // The index finds which photos move, and only those.
  const askKnown = async () => (await (await fetch(`${base}api/photo/motion/known`, { method: 'POST', headers: { ...authed, 'content-type': 'application/json' }, body: JSON.stringify({ paths: [...MOVING, 'still_marked.jpg', 'plain.jpg'] }) })).json()).moving.sort();
  let moving = [];
  for (let i = 0; i < 100 && moving.length < 3; i++) {
    moving = await askKnown();
    await sleep(200);
  }
  assert.deepEqual(moving, [...MOVING].sort());

  // Each gives a clip any browser can play, small and silent, HEVC and all.
  for (const name of MOVING) {
    const response = await fetch(`${base}api/photo/motion/clip?path=${encodeURIComponent(name)}`, { headers: authed });
    assert.equal(response.status, 200, name);
    const clip = path.join(tmp, 'clip.mp4');
    fs.writeFileSync(clip, Buffer.from(await response.arrayBuffer()));
    const probe = spawnSync('ffprobe', ['-v', 'error', '-show_entries', 'stream=codec_type,codec_name,width,height', '-of', 'csv=p=0', clip], { encoding: 'utf8' });
    assert.match(probe.stdout, /^h264,(270|268),480,video|^h264,video,270,480/m, `${name}: ${probe.stdout}`);
    assert.ok(!probe.stdout.includes('audio'), name);
    assert.ok(fs.statSync(clip).size < 400_000, `${name} is ${fs.statSync(clip).size} bytes`);
  }
  assert.equal((await fetch(`${base}api/photo/motion/clip?path=plain.jpg`, { headers: authed })).status, 404);

  // The file list: badges on those that move, and hovering plays the clip over the picture.
  await send('Page.navigate', { url: base });
  await until(`document.querySelectorAll('#files .motion-badge').length === 3`, 'the badges');
  assert.equal(await js(`document.querySelector("#files [data-path='plain.jpg'] .motion-badge")`), null);
  const centre = await js(`(() => { const box = document.querySelector("#files [data-path='IMG_0001.JPG'] .thumb").getBoundingClientRect(); return [box.x + box.width / 2, box.y + box.height / 2]; })()`);
  await send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: centre[0], y: centre[1] });
  await until(`(() => { const video = document.querySelector("#files [data-path='IMG_0001.JPG'] video.motion-thumb.is-playing"); return video && video.currentTime > 0 && video.videoWidth > 0; })()`, 'the clip to play');
  assert.equal(await js(`document.querySelector('video.motion-thumb').muted`), true);
  await send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: 5, y: 5 });
  await until(`!document.querySelector('video.motion-thumb')`, 'the clip to stop');

  // The Gallery as well.
  await send('Page.navigate', { url: `${base}#gallery` });
  await until(`document.querySelectorAll('.gallery-tile .motion-badge').length === 3`, 'the Gallery badges');
  const inGallery = await js(`(() => { const box = document.querySelector(".gallery-tile[data-path='sam_0003.jpg'] .gallery-thumb").getBoundingClientRect(); return [box.x + box.width / 2, box.y + box.height / 2]; })()`);
  await send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: inGallery[0], y: inGallery[1] });
  await until(`document.querySelector(".gallery-tile[data-path='sam_0003.jpg'] video.motion-thumb.is-playing")?.currentTime > 0`, 'the Gallery clip to play');
  if (process.env.SCREENSHOT) fs.writeFileSync(process.env.SCREENSHOT, Buffer.from((await send('Page.captureScreenshot', { format: 'png' })).result.data, 'base64'));
  assert.deepEqual(problems, []);
  console.log('Moving thumbnail tests passed in Chrome.');
} finally {
  chrome.kill();
  server.kill();
  await sleep(300);
  fs.rmSync(tmp, { recursive: true, force: true, maxRetries: 5 });
}
