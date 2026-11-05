// Runs ownnascli against a throwaway server (set OWNNAS_TARGET for a target directory other than
// ./target/debug). Build first: cargo build -p ownnas -p ownnascli. Needs Node 22+.
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createHash, randomBytes } from 'node:crypto';
import fs from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';

const repo = path.resolve(import.meta.dirname, '..');
const target = process.env.OWNNAS_TARGET || path.join(repo, 'target/debug');
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'ownnascli-test-'));
const library = path.join(tmp, 'library');
const here = path.join(tmp, 'here');
const home = path.join(tmp, 'home');
for (const dir of [library, here, home]) fs.mkdirSync(dir, { recursive: true });
const PASSWORD = randomBytes(12).toString('hex');
const env = { ...process.env, HOME: home, USERPROFILE: home, APPDATA: home, XDG_CONFIG_HOME: path.join(home, '.config'), OWNNAS_PASSWORD: PASSWORD };

const port = await new Promise((done) => {
  const probe = net.createServer().listen(0, '127.0.0.1', () => {
    const { port } = probe.address();
    probe.close(() => done(port));
  });
});
const server = spawn(path.join(target, 'ownnas'), ['serve', '--root', library, '--data', path.join(tmp, 'data'), '--addr', `127.0.0.1:${port}`, '--username', 'admin', '--password', PASSWORD], { stdio: 'ignore', env });
const address = `http://127.0.0.1:${port}`;

function cli(...args) {
  const run = spawnSync(path.join(target, 'ownnascli'), args, { env, encoding: 'utf8' });
  return { code: run.status, out: run.stdout, err: run.stderr };
}
function ok(...args) {
  const run = cli(...args);
  assert.equal(run.code, 0, `ownnascli ${args.join(' ')}: ${run.err}`);
  return run.out;
}
function refused(pattern, ...args) {
  const run = cli(...args);
  assert.notEqual(run.code, 0, `ownnascli ${args.join(' ')} should fail`);
  assert.match(run.err, pattern);
}
const write = (root, rel, data) => {
  const file = path.join(root, rel);
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, data);
};
const read = (root, rel) => fs.readFileSync(path.join(root, rel), 'utf8');
const exists = (root, rel) => fs.existsSync(path.join(root, rel));
const there = (rel) => exists(library, rel);
const sha = (file) => createHash('sha256').update(fs.readFileSync(file)).digest('hex');
const inTrash = () => {
  const found = [];
  const walk = (dir) => {
    for (const item of fs.readdirSync(dir, { withFileTypes: true })) {
      if (item.isDirectory()) walk(path.join(dir, item.name));
      else found.push(item.name);
    }
  };
  if (fs.existsSync(path.join(library, '.ownnas-trash'))) walk(path.join(library, '.ownnas-trash'));
  return found;
};

try {
  for (let tries = 0; ; tries++) {
    try {
      await fetch(`${address}/api/me`);
      break;
    } catch (err) {
      if (tries > 100) throw err;
      await new Promise((done) => setTimeout(done, 100));
    }
  }

  refused(/Not signed in/, 'ls');
  refused(/Sign-in failed/, 'login', address, '--username', 'nobody');
  ok('login', address, '--username', 'admin');
  assert.match(ok('status'), /as admin/);
  const profile = path.join(home, '.config/ownnascli/profile.json');
  if (process.platform !== 'win32') assert.equal(fs.statSync(profile).mode & 0o777, 0o600, 'the sign-in is private');
  assert.ok(!fs.readFileSync(profile, 'utf8').includes(PASSWORD), 'the password is not saved');

  // Copying a folder up, then again with nothing to do.
  const up = path.join(here, 'up');
  for (let i = 0; i < 60; i++) write(up, `many/f${i}.txt`, `file ${i}`);
  write(up, 'one word/naïve & odd#name?.txt', 'odd');
  write(up, 'empty.txt', '');
  fs.mkdirSync(path.join(up, 'blank/inner'), { recursive: true });
  assert.match(ok('cp', up, 'ownnas:Backup', '--dry-run'), /would copy many\/f0\.txt/);
  assert.ok(!exists(library, 'Backup'), 'a dry run changes nothing');
  assert.match(ok('cp', up, 'ownnas:Backup', '-p', '8'), /^62 copied/);
  assert.equal(read(library, 'Backup/many/f59.txt'), 'file 59');
  assert.equal(read(library, 'Backup/one word/naïve & odd#name?.txt'), 'odd');
  assert.ok(fs.statSync(path.join(library, 'Backup/blank/inner')).isDirectory(), 'empty folders are made');
  assert.equal(fs.statSync(path.join(library, 'Backup/many/f1.txt')).mtimeMs | 0, fs.statSync(path.join(up, 'many/f1.txt')).mtimeMs | 0, 'modification times travel');
  assert.match(ok('cp', up, 'ownnas:Backup'), /^0 copied .*62 already there/);

  // Only what changed goes again, and cp removes nothing.
  write(up, 'many/f1.txt', 'changed');
  fs.rmSync(path.join(up, 'many/f2.txt'));
  assert.match(ok('cp', up, 'ownnas:Backup'), /^1 copied/);
  assert.equal(read(library, 'Backup/many/f1.txt'), 'changed');
  assert.ok(exists(library, 'Backup/many/f2.txt'));

  // --checksum finds what size and date cannot: the same size and date, other contents.
  // (A whole second, which comes back exactly: a date with milliseconds can come back 1 ms off.)
  const damaged = path.join(library, 'Backup/many/f3.txt');
  fs.utimesSync(path.join(up, 'many/f3.txt'), 1_700_000_000, 1_700_000_000);
  assert.match(ok('cp', up, 'ownnas:Backup'), /^1 copied/);
  fs.writeFileSync(damaged, 'FILE 3');
  fs.utimesSync(damaged, 1_700_000_000, 1_700_000_000);
  assert.match(ok('cp', up, 'ownnas:Backup'), /^0 copied/, 'without it, the damage goes unseen');
  assert.equal(read(library, 'Backup/many/f3.txt'), 'FILE 3');
  assert.match(ok('cp', up, 'ownnas:Backup', '--checksum', '-n'), /would copy many\/f3\.txt/);
  assert.match(ok('cp', up, 'ownnas:Backup', '--checksum'), /^1 copied/);
  assert.equal(read(library, 'Backup/many/f3.txt'), 'file 3');

  // Sync removes to the Trash, and refuses to empty a destination from an empty source.
  assert.match(ok('sync', up, 'ownnas:Backup', '-n'), /would remove many\/f2\.txt/);
  assert.ok(exists(library, 'Backup/many/f2.txt'));
  assert.match(ok('sync', up, 'ownnas:Backup'), /1 removed/);
  assert.ok(!exists(library, 'Backup/many/f2.txt'));
  assert.ok(inTrash().includes('f2.txt'), 'it went to the Trash');
  const nothing = path.join(here, 'nothing');
  fs.mkdirSync(nothing);
  refused(/remove everything/, 'sync', nothing, 'ownnas:Backup');
  assert.ok(exists(library, 'Backup/many/f1.txt'));

  // Down, with times kept, so a second run has nothing to do.
  const down = path.join(here, 'down');
  assert.match(ok('cp', 'ownnas:Backup', down), /^61 copied/);
  assert.equal(read(down, 'many/f1.txt'), 'changed');
  assert.equal(read(down, 'one word/naïve & odd#name?.txt'), 'odd');
  assert.match(ok('cp', 'ownnas:Backup', down), /^0 copied .*61 already there/);
  // A local sync sets removed files aside.
  write(down, 'extra.txt', 'extra');
  assert.match(ok('sync', 'ownnas:Backup', down), /1 removed/);
  assert.ok(!exists(down, 'extra.txt'));
  const deleted = fs.readdirSync(path.join(down, '.ownnas-sync/deleted'));
  assert.equal(read(path.join(down, '.ownnas-sync/deleted', deleted[0]), 'extra.txt'), 'extra');

  // One file, each way, to a folder and to a name.
  write(here, 'single.txt', 'single');
  ok('cp', path.join(here, 'single.txt'), 'ownnas:Backup');
  assert.equal(read(library, 'Backup/single.txt'), 'single');
  assert.match(ok('cp', path.join(here, 'single.txt'), 'ownnas:Backup'), /Already there/);
  ok('cp', path.join(here, 'single.txt'), 'ownnas:Backup/named.txt');
  assert.equal(read(library, 'Backup/named.txt'), 'single');
  ok('cp', 'ownnas:Backup/named.txt', path.join(here, 'fetched.txt'));
  assert.equal(read(here, 'fetched.txt'), 'single');
  ok('cp', 'ownnas:Backup/named.txt', down);
  assert.equal(read(down, 'named.txt'), 'single');
  assert.match(ok('cp', 'ownnas:Backup/named.txt', down), /Already there/);

  // Listing, folders, moving, removing.
  assert.match(ok('ls', 'ownnas:Backup'), /many\//);
  assert.match(ok('ls', 'ownnas:Backup'), /named\.txt/);
  ok('mkdir', 'ownnas:Deep/er/still');
  assert.ok(fs.statSync(path.join(library, 'Deep/er/still')).isDirectory());
  ok('mv', 'ownnas:Backup/named.txt', 'ownnas:Backup/renamed.txt');
  assert.ok(exists(library, 'Backup/renamed.txt') && !exists(library, 'Backup/named.txt'));
  ok('mv', 'ownnas:Backup/renamed.txt', 'ownnas:Deep');
  assert.ok(exists(library, 'Deep/renamed.txt'));
  ok('mv', 'ownnas:Deep/renamed.txt', 'ownnas:Deep/er/again.txt');
  assert.ok(exists(library, 'Deep/er/again.txt'));
  refused(/is a folder/, 'rm', 'ownnas:Deep');
  refused(/in the Trash/, 'rm', 'ownnas:.ownnas-trash/f2.txt');
  refused(/top of the library/, 'rm', 'ownnas:');
  refused(/not on the server/, 'rm', 'ownnas:nope');
  ok('rm', 'ownnas:Deep/er/again.txt');
  assert.ok(!exists(library, 'Deep/er/again.txt') && inTrash().includes('again.txt'));
  ok('rm', '-r', 'ownnas:Deep');
  assert.ok(!exists(library, 'Deep'));
  refused(/One of the paths/, 'cp', up, down);
  refused(/Both paths/, 'cp', 'ownnas:a', 'ownnas:b');

  // A change on the server between listing and sending is not overwritten: the server's check
  // is exercised through the API by its own tests; here a changed file simply goes across.
  write(library, 'Backup/single.txt', 'edited on the server');
  assert.match(ok('cp', path.join(here, 'single.txt'), 'ownnas:Backup'), /Copied/);
  assert.equal(read(library, 'Backup/single.txt'), 'single');

  // A big download cut off goes on from where it stopped, and ends up identical.
  const big = path.join(library, 'big.bin');
  fs.writeFileSync(big, randomBytes(64 * 1024 * 1024));
  const expected = sha(big);
  const stamp = fs.statSync(big);
  const fetched = path.join(here, 'big.bin');
  const head = fs.readFileSync(big).subarray(0, 20 * 1024 * 1024);
  const hash = [...Buffer.from('big.bin')].reduce((h, b) => BigInt.asUintN(64, (h ^ BigInt(b)) * 0x100000001b3n), 0xcbf29ce484222325n);
  const part = path.join(here, `.ownnas-dl-resume-${hash.toString(16).padStart(16, '0')}-${stamp.size}-${Math.trunc(stamp.mtimeMs)}`);
  fs.writeFileSync(part, head);
  assert.match(ok('cp', 'ownnas:big.bin', fetched), /went on from 20\.0 MB/);
  assert.equal(sha(fetched), expected);
  assert.ok(!fs.existsSync(part), 'the partial file is gone');
  // A partial file that does not match is found out, and fetched again whole.
  fs.rmSync(fetched);
  fs.writeFileSync(part, randomBytes(20 * 1024 * 1024));
  ok('cp', 'ownnas:big.bin', fetched);
  assert.equal(sha(fetched), expected);
  // The folder route resumes too, and really does ask for only a part.
  const response = await fetch(`${address}/api/login`, { method: 'POST', headers: { 'content-type': 'application/json', 'x-ownnas': '1' }, body: JSON.stringify({ username: 'admin', password: PASSWORD, token: true }) });
  const { token } = await response.json();
  const partial = await fetch(`${address}/api/sync/file?path=big.bin`, { headers: { authorization: `Bearer ${token}`, range: 'bytes=67108860-' } });
  assert.equal(partial.status, 206);
  assert.equal(partial.headers.get('content-range'), 'bytes 67108860-67108863/67108864');
  assert.equal(partial.headers.get('x-ownnas-size'), '67108864', 'the stamp is the whole file\'s');
  assert.equal((await partial.arrayBuffer()).byteLength, 4);

  // Photos to a workspace: what is already there is told and not sent.
  const JPEG = (seed) => Buffer.concat([Buffer.from([0xff, 0xd8, 0xff, 0xd9]), Buffer.from(seed)]);
  const call = async (method, url, body) => (await fetch(`${address}${url}`, { method, headers: { authorization: `Bearer ${token}`, 'x-ownnas': '1', 'content-type': 'application/json' }, body: body && JSON.stringify(body) })).json();
  fs.mkdirSync(path.join(library, 'Family'));
  fs.mkdirSync(path.join(library, 'Work'));
  fs.writeFileSync(path.join(library, 'Family/here.jpg'), JPEG('in family'));
  fs.writeFileSync(path.join(library, 'Work/there.jpg'), JPEG('in work'));
  await call('POST', '/api/workspaces', { name: 'Family', folders: ['Family'] });
  await call('POST', '/api/workspaces', { name: 'Both', folders: ['Family', 'Work'] });
  await call('POST', '/api/gallery/rescan');
  for (let i = 0; i < 100; i++) {
    const page = await call('GET', '/api/gallery/timeline?limit=200');
    if (page.items.some((item) => item.path === 'Work/there.jpg') && page.items.some((item) => item.path === 'Family/here.jpg')) break;
    await new Promise((done) => setTimeout(done, 100));
  }
  const shots = path.join(here, 'shots');
  fs.mkdirSync(path.join(shots, 'trip'), { recursive: true });
  fs.writeFileSync(path.join(shots, 'copy-of-here.jpg'), JPEG('in family'));
  fs.writeFileSync(path.join(shots, 'copy-of-there.jpg'), JPEG('in work'));
  fs.writeFileSync(path.join(shots, 'new.jpg'), JPEG('new'));
  fs.writeFileSync(path.join(shots, 'trip/new-twin.jpg'), JPEG('new'));
  fs.writeFileSync(path.join(shots, 'here.jpg'), JPEG('same name, other photo'));
  fs.writeFileSync(path.join(shots, 'trip/deep.MOV'), JPEG('a video, deeper down'));
  fs.writeFileSync(path.join(shots, 'notes.txt'), 'not a photo');
  refused(/Say where/, 'photos', shots);
  refused(/no workspace called “Nope”.*Family/, 'photos', shots, '--workspace', 'Nope');
  refused(/several folders.*Family, Work/, 'photos', shots, '--workspace', 'Both');
  refused(/is not in “Family”/, 'photos', shots, '--workspace', 'Family', '--to', 'ownnas:Work');
  let told = ok('photos', shots, '--workspace', 'family', '--dry-run');
  assert.match(told, /^3 to upload, 3 already there, 1 not photos or videos, of 6/m, told);
  assert.ok(!there('Family/new.jpg'), 'a dry run sends nothing');
  told = ok('photos', shots, '--workspace', 'family');
  assert.match(told, /already there: .*copy-of-here\.jpg is ownnas:Family\/here\.jpg\n/, told);
  assert.match(told, /already there: .*copy-of-there\.jpg is ownnas:Work\/there\.jpg \(outside this workspace\)/, told);
  assert.match(told, /in this upload/, told);
  assert.match(told, /^3 uploaded .*, 3 already there, 1 not photos or videos, of 6/m, told);
  assert.equal(read(library, 'Family/here.jpg').length, JPEG('in family').length, 'what was there stays');
  assert.ok(fs.readFileSync(path.join(library, 'Family/here (2).jpg')).equals(JPEG('same name, other photo')));
  assert.ok(there('Family/deep.MOV'), 'everything goes straight into the folder');
  assert.ok(!there('Family/copy-of-here.jpg') && !there('Family/notes.txt'));
  // Again straight away, before any scan could have seen them.
  told = ok('photos', shots, '--workspace', 'Family');
  assert.match(told, /^0 uploaded .*6 already there/m, told);
  // --force sends one anyway, under a new name.
  ok('photos', path.join(shots, 'copy-of-here.jpg'), '--to', 'ownnas:Work', '--force');
  assert.ok(there('Work/copy-of-here.jpg'));

  ok('logout');
  refused(/Not signed in/, 'ls');
  console.log('ownnascli tests passed.');
} finally {
  server.kill();
  fs.rmSync(tmp, { recursive: true, force: true });
}
