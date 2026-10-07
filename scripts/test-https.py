#!/usr/bin/env python3
"""Integration checks for native HTTPS; run after cargo build --bin ownnas."""
import http.client
import json
from pathlib import Path
import signal
import socket
import ssl
import subprocess
import tempfile
import time

binary = Path(__file__).resolve().parent.parent / 'target/debug/ownnas'
with tempfile.TemporaryDirectory(prefix='ownnas-https-') as tmp:
    root = Path(tmp)
    certs = root / 'tls'
    subprocess.run([str(binary), 'tls', 'generate', '--out', str(certs), '--host', 'localhost', '--host', '127.0.0.1'], check=True)
    old_ca = (certs / 'ca.crt').read_bytes()
    assert subprocess.run([str(binary), 'tls', 'generate', '--out', str(certs), '--host', 'localhost'], capture_output=True).returncode != 0
    assert (certs / 'ca.crt').read_bytes() == old_ca
    assert subprocess.run([str(binary), 'serve', '--tls-cert', str(certs / 'server.crt')], capture_output=True).returncode != 0
    (root / 'library').mkdir()
    with socket.socket() as s:
        s.bind(('127.0.0.1', 0))
        port = s.getsockname()[1]
    command = [str(binary), 'serve', '--root', str(root / 'library'), '--data', str(root / 'data'), '--addr', f'127.0.0.1:{port}', '--username', 'admin', '--password', 'integration-test-passphrase', '--tls-cert', str(certs / 'server.crt'), '--tls-key', str(certs / 'server.key')]
    with (root / 'server.log').open('w+') as log:
        proc = subprocess.Popen(command, stdout=log, stderr=log)
        try:
            context = ssl.create_default_context(cafile=str(certs / 'ca.crt'))
            for _ in range(100):
                try:
                    conn = http.client.HTTPSConnection('127.0.0.1', port, context=context, timeout=1)
                    conn.request('GET', '/api/health')
                    response = conn.getresponse()
                    assert response.status == 200
                    assert json.loads(response.read())['ok']
                    conn.close()
                    break
                except ssl.SSLCertVerificationError:
                    raise
                except (ConnectionError, OSError):
                    if proc.poll() is not None:
                        log.seek(0)
                        raise AssertionError(log.read())
                    time.sleep(.1)
            else:
                raise AssertionError('HTTPS startup timed out')
            for ctx, hostname in [(ssl.create_default_context(), '127.0.0.1'), (context, 'wrong.example')]:
                try:
                    with socket.create_connection(('127.0.0.1', port), timeout=2) as raw:
                        with ctx.wrap_socket(raw, server_hostname=hostname):
                            pass
                except ssl.SSLCertVerificationError:
                    pass
                else:
                    raise AssertionError('Untrusted CA or wrong hostname was accepted')
            conn = http.client.HTTPSConnection('localhost', port, context=context, timeout=2)
            conn.request('POST', '/api/login', json.dumps({'username': 'admin', 'password': 'integration-test-passphrase'}), {'Content-Type': 'application/json', 'x-ownnas': '1'})
            response = conn.getresponse()
            assert response.status == 200, response.read()
            assert 'Secure' in response.getheader('Set-Cookie')
            response.read()
            conn.close()
            proc.send_signal(signal.SIGINT)
            assert proc.wait(timeout=8) == 0
        finally:
            if proc.poll() is None:
                proc.kill()
                proc.wait()
    # A second process must reuse the saved folder and account without prompting.
    restart = command.copy()
    i = restart.index('--root')
    del restart[i:i + 2]
    i = restart.index('--username')
    del restart[i:i + 4]
    with (root / 'restart.log').open('w+') as log:
        proc = subprocess.Popen(restart, stdout=log, stderr=log, stdin=subprocess.DEVNULL)
        try:
            for _ in range(100):
                try:
                    conn = http.client.HTTPSConnection('127.0.0.1', port, context=context, timeout=1)
                    conn.request('GET', '/api/health')
                    assert conn.getresponse().status == 200
                    conn.close()
                    break
                except (ConnectionError, OSError):
                    if proc.poll() is not None:
                        log.seek(0)
                        raise AssertionError(log.read())
                    time.sleep(.1)
            else:
                raise AssertionError('Restart without --root failed')
            proc.send_signal(signal.SIGINT)
            assert proc.wait(timeout=8) == 0
        finally:
            if proc.poll() is None:
                proc.kill()
                proc.wait()
    bad = command.copy()
    bad[-1] = str(certs / 'ca.key')
    assert subprocess.run(bad, capture_output=True, timeout=10).returncode != 0
print('PASS: HTTPS health/login, secure cookies, CA trust, hostname validation, key mismatch, required flags, overwrite refusal, graceful shutdown, and restart without --root or credentials.')
