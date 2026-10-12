#!/usr/bin/env python3
"""End-to-end checks for the sync API and the ownnas-sync client.

Run after `cargo build --bin ownnas --bin ownnas-sync`. Set OWNNAS_TARGET to use a
different target directory than ./target/debug.
"""
import http.client
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time

target = Path(os.environ.get('OWNNAS_TARGET', Path(__file__).resolve().parent.parent / 'target/debug'))
server_bin = target / 'ownnas'
client_bin = target / 'ownnas-sync'
PASSWORD = 'integration-test-passphrase'


def free_port():
    with socket.socket() as s:
        s.bind(('127.0.0.1', 0))
        return s.getsockname()[1]


def wait_for(port, proc):
    for _ in range(100):
        if proc.poll() is not None:
            raise SystemExit('server exited early')
        try:
            conn = http.client.HTTPConnection('127.0.0.1', port, timeout=1)
            conn.request('GET', '/api/health')
            if conn.getresponse().status == 200:
                return
        except OSError:
            time.sleep(0.1)
    raise SystemExit('server did not start')


def request(port, method, path, body=None, headers=None):
    conn = http.client.HTTPConnection('127.0.0.1', port, timeout=10)
    headers = dict(headers or {})
    data = body
    if isinstance(body, dict):
        data = json.dumps(body)
        headers['content-type'] = 'application/json'
    conn.request(method, path, body=data, headers=headers)
    response = conn.getresponse()
    return response.status, response.read()


def client(*args, ok=True):
    result = subprocess.run([str(client_bin), *map(str, args)], capture_output=True, text=True,
                            env={**os.environ, 'OWNNAS_PASSWORD': PASSWORD})
    if ok and result.returncode != 0:
        raise AssertionError(f'ownnas-sync {args} failed:\n{result.stdout}\n{result.stderr}')
    return result


def sync(folder):
    return client('run', folder, '-v')


def write(path, text, mtime=None):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)
    if mtime is not None:
        os.utime(path, (mtime, mtime))


def files(folder):
    """Synced content of a folder as {relative path: text}, without client metadata."""
    out = {}
    for path in sorted(folder.rglob('*')):
        rel = path.relative_to(folder).as_posix()
        if rel.startswith('.ownnas-sync') or rel.startswith('.ownnas-trash') or path.name == '.DS_Store':
            continue
        out[rel] = None if path.is_dir() else path.read_text()
    return out


with tempfile.TemporaryDirectory(prefix='ownnas-sync-') as tmp:
    tmp = Path(tmp)
    library = tmp / 'library'
    library.mkdir()
    port = free_port()
    server = f'http://127.0.0.1:{port}'
    command = [str(server_bin), 'serve', '--root', str(library), '--data', str(tmp / 'data'),
               '--addr', f'127.0.0.1:{port}', '--username', 'admin', '--password', PASSWORD]
    with (tmp / 'server.log').open('w+') as log:
        proc = subprocess.Popen(command, stdout=log, stderr=log)
        try:
            wait_for(port, proc)

            # --- API: bearer tokens ---
            status, body = request(port, 'POST', '/api/login',
                                   {'username': 'admin', 'password': PASSWORD, 'token': True},
                                   {'x-ownnas': '1', 'content-type': 'application/json'})
            assert status == 200, body
            token = json.loads(body)['token']
            bearer = {'authorization': f'Bearer {token}'}
            status, body = request(port, 'POST', '/api/login', {'username': 'admin', 'password': PASSWORD},
                                   {'x-ownnas': '1', 'content-type': 'application/json'})
            assert 'token' not in json.loads(body), 'tokens are only returned on request'
            assert request(port, 'GET', '/api/sync/tree')[0] == 401
            assert request(port, 'GET', '/api/sync/tree', headers={'authorization': 'Bearer nope'})[0] == 401

            # --- API: compare-and-swap writes ---
            status, body = request(port, 'PUT', '/api/sync/file?path=api/a.txt&base=none&mtime=1700000000123',
                                   b'hello', bearer)
            assert status == 200, body
            entry = json.loads(body)['entry']
            assert entry == {'path': 'api/a.txt', 'dir': False, 'size': 5, 'mtime': 1700000000123}, entry
            status, body = request(port, 'PUT', '/api/sync/file?path=api/a.txt&base=none', b'again', bearer)
            assert status == 409 and json.loads(body)['current']['size'] == 5, body
            status, _ = request(port, 'PUT', '/api/sync/file?path=api/a.txt&base=5:1700000000123', b'replaced', bearer)
            assert status == 200
            assert (library / 'api/a.txt').read_text() == 'replaced'
            assert not [p for p in (library / 'api').iterdir() if p.name.startswith('.ownnas-part-')]
            status, _ = request(port, 'PUT', '/api/sync/file?path=.ownnas-trash/x&base=none', b'x', bearer)
            assert status == 400, 'Trash is not writable through sync'
            status, _ = request(port, 'PUT', '/api/sync/file?path=../escape&base=none', b'x', bearer)
            assert status == 403
            status, body = request(port, 'POST', '/api/sync/delete', {'path': 'api/a.txt', 'base': '1:1'}, bearer)
            assert status == 409, body
            status, body = request(port, 'POST', '/api/sync/delete', {'path': 'api', 'base': 'dir'}, bearer)
            assert status == 409, 'non-empty folders are not deleted'
            status, body = request(port, 'GET', '/api/sync/tree', headers=bearer)
            tree = json.loads(body)
            assert [e['path'] for e in tree['entries']] == ['api', 'api/a.txt'], tree
            status, body = request(port, 'GET', '/api/sync/file?path=api/a.txt', headers=bearer)
            assert status == 200 and body == b'replaced'
            a = json.loads(request(port, 'GET', '/api/sync/tree', headers=bearer)[1])['entries'][1]
            status, body = request(port, 'POST', '/api/sync/delete',
                                   {'path': 'api/a.txt', 'base': f"{a['size']}:{a['mtime']}"}, bearer)
            assert status == 200 and json.loads(body)['trashed'], body
            status, body = request(port, 'POST', '/api/sync/delete', {'path': 'api', 'base': 'dir'}, bearer)
            assert status == 200, body
            trash = json.loads(request(port, 'GET', '/api/list?path=.ownnas-trash&hidden=1', headers=bearer)[1])
            assert any(e['name'] == 'a.txt' for e in trash['entries']), trash

            # --- Client: two computers syncing the same server folder ---
            pc1, pc2 = tmp / 'pc1', tmp / 'pc2'
            client('setup', pc1, '--server', server, '--username', 'admin', '--remote', 'Shared/Docs')
            client('setup', pc2, '--server', server, '--username', 'admin', '--remote', 'Shared/Docs')
            assert (library / 'Shared/Docs').is_dir()

            write(pc1 / 'notes.txt', 'v1')
            write(pc1 / 'Projects/plan.md', '# plan')
            (pc1 / 'Empty').mkdir()
            write(pc1 / '.DS_Store', 'junk')
            write(library / 'Shared/Docs/from-server.txt', 'server file')
            sync(pc1)
            sync(pc2)
            expected = {'Empty': None, 'Projects': None, 'Projects/plan.md': '# plan',
                        'from-server.txt': 'server file', 'notes.txt': 'v1'}
            assert files(pc2) == expected, files(pc2)
            assert not (library / 'Shared/Docs/.DS_Store').exists()
            assert client('run', pc2).stdout.startswith('0 uploaded, 0 downloaded'), 'second run is a no-op'
            mtime = (pc2 / 'notes.txt').stat().st_mtime
            assert abs(mtime - (pc1 / 'notes.txt').stat().st_mtime) < 0.01, 'modification times are kept'

            # Edits and deletes propagate; deletions are recoverable on both sides.
            write(pc2 / 'notes.txt', 'v2 from pc2')
            (pc2 / 'Projects/plan.md').unlink()
            (pc2 / 'Projects').rmdir()
            sync(pc2)
            sync(pc1)
            assert files(pc1) == {'Empty': None, 'from-server.txt': 'server file', 'notes.txt': 'v2 from pc2'}, files(pc1)
            assert list((pc1 / '.ownnas-sync/deleted').rglob('plan.md')), 'local deletions keep a safety copy'
            assert any(p.name == 'plan.md' for p in (library / '.ownnas-trash').iterdir())

            # Both computers edit the same file: nothing is lost.
            write(pc1 / 'notes.txt', 'pc1 edit', mtime=1_800_000_000)
            write(pc2 / 'notes.txt', 'pc2 edit!', mtime=1_800_000_100)
            sync(pc1)
            out = sync(pc2).stdout
            assert 'conflict: notes.txt' in out, out
            sync(pc1)
            sync(pc2)
            names = files(pc1)
            conflicts = [n for n in names if n.startswith('notes (conflict ')]
            assert names['notes.txt'] == 'pc1 edit' and len(conflicts) == 1, names
            assert names[conflicts[0]] == 'pc2 edit!'
            assert files(pc1) == files(pc2)

            # The same new file created on both sides is not a conflict.
            write(pc1 / 'same.txt', 'identical', mtime=1_800_000_000)
            write(pc2 / 'same.txt', 'identical', mtime=1_800_000_500)
            sync(pc1)
            out = sync(pc2).stdout
            assert 'conflict:' not in out, out

            # A folder deleted on one computer while the other adds to it survives.
            write(pc1 / 'Keep/old.txt', 'old')
            sync(pc1)
            sync(pc2)
            (pc2 / 'Keep/old.txt').unlink()
            (pc2 / 'Keep').rmdir()
            write(pc1 / 'Keep/new.txt', 'new')
            sync(pc1)
            sync(pc2)
            sync(pc1)
            assert files(pc2).get('Keep/new.txt') == 'new', files(pc2)
            assert 'Keep/old.txt' not in files(pc1)

            # Edits on the server itself (web UI, other tools) reach both computers.
            write(library / 'Shared/Docs/from-server.txt', 'edited on the server')
            sync(pc1)
            assert (pc1 / 'from-server.txt').read_text() == 'edited on the server'

            # Dry run changes nothing.
            write(pc1 / 'pending.txt', 'pending')
            out = client('run', pc1, '--dry-run').stdout
            assert 'would upload pending.txt' in out, out
            assert not (library / 'Shared/Docs/pending.txt').exists()

            # Logout revokes the token on the server.
            client('logout', pc2)
            result = client('run', pc2, ok=False)
            assert result.returncode != 0
            print('sync client checks passed')
        finally:
            proc.terminate()
            proc.wait(timeout=10)
