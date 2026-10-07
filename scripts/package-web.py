#!/usr/bin/env python3
"""Package an existing Windows release as a static FTP upload folder."""
import argparse
import hashlib
import html
import json
from datetime import datetime, timezone
from pathlib import Path
import shutil
import zipfile

root = Path(__file__).resolve().parent.parent
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--version', default='0.1.0', help='Version of the existing executable')
parser.add_argument('--binary', type=Path, default=root / 'target/release/ownnas.exe')
parser.add_argument('--output', type=Path, default=root / 'dist/vps')
parser.add_argument('--base-url', help='Public HTTPS URL of the uploaded folder (enables update manifest)')
args = parser.parse_args()
if not args.version or any(c not in '0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ.-' for c in args.version):
    parser.error('Version must contain only letters, digits, dots, or hyphens')
if args.base_url and not args.base_url.startswith('https://'):
    parser.error('--base-url must start with https://')
if not args.binary.is_file() or args.binary.read_bytes()[:2] != b'MZ':
    parser.error(f'Windows executable not found: {args.binary}')

out = args.output
version_dir = out / f'v{args.version}'
version_dir.mkdir(parents=True, exist_ok=True)
binary = version_dir / 'ownnas-x86_64-pc-windows-msvc.exe'
shutil.copyfile(args.binary, binary)
archive = version_dir / f'ownnas-{args.version}-windows-x86_64.zip'
readme = '''OwnNAS for Windows (x86_64)

Extract this ZIP into a writable folder, then double-click start.bat.
On first start, enter the folder to share, username, and password.
Later starts reuse the saved folder and account.
To change the folder, run: start.bat --root "D:\\media"
Open http://127.0.0.1:8787 in your browser. Stop with Ctrl+C.

The web interface is embedded in ownnas.exe.
Accounts and cache are stored in ownnas-data next to the executable when
using start.bat. Keep that folder outside the folder you share.
'''
with zipfile.ZipFile(archive, 'w', zipfile.ZIP_DEFLATED) as z:
    z.write(binary, 'ownnas/ownnas.exe')
    z.writestr('ownnas/README.txt', readme.replace('\n', '\r\n'))
    z.writestr('ownnas/start.bat', '@echo off\r\nsetlocal\r\ncd /d "%~dp0"\r\nownnas.exe serve --open %*\r\nset "OWNNAS_EXIT=%ERRORLEVEL%"\r\nif not "%OWNNAS_EXIT%"=="0" pause\r\nexit /b %OWNNAS_EXIT%\r\n')
with zipfile.ZipFile(archive) as z:
    assert z.testzip() is None
    assert z.read('ownnas/ownnas.exe') == binary.read_bytes()
checksums = ''.join(f'{hashlib.sha256(p.read_bytes()).hexdigest()}  {p.name}\n' for p in (binary, archive))
(version_dir / 'SHA256SUMS.txt').write_text(checksums)
zip_url = f'v{args.version}/{archive.name}'
exe_url = f'v{args.version}/{binary.name}'
out.joinpath('index.html').write_text(f'''<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Download OwnNAS</title><style>
body{{font:17px/1.6 system-ui,sans-serif;background:#101827;color:#edf2fa;max-width:680px;margin:10vh auto;padding:24px}}
a{{color:#93c5fd}}.button{{display:inline-block;background:#2563eb;color:white;padding:12px 22px;border-radius:8px;text-decoration:none}}small{{color:#b6c2d4}}
</style></head><body><h1>OwnNAS</h1><p>Share your folders with a private web interface.</p>
<h2>Windows download</h2><p>Version {html.escape(args.version)} · Windows x86_64</p>
<p><a class="button" href="{zip_url}" download>Download Windows ZIP</a></p>
<p>Extract the ZIP, run <code>start.bat</code>, and follow the prompts to choose your shared folder and create an account.
Then open <a href="http://127.0.0.1:8787">http://127.0.0.1:8787</a>.</p>
<p><a href="{exe_url}" download>Standalone executable</a> · <a href="v{args.version}/SHA256SUMS.txt">SHA-256 checksums</a></p>
<small>The web interface is included in the executable.</small></body></html>
''', encoding='utf-8')
if args.base_url:
    manifest = {'version': args.version, 'notes': f'OwnNAS {args.version} for Windows',
                'publishedAt': datetime.now(timezone.utc).isoformat().replace('+00:00', 'Z'),
                'minVersion': '0.1.0', 'artifacts': {'x86_64-pc-windows-msvc': {
                    'url': f'{args.base_url.rstrip("/")}/{exe_url}',
                    'sha256': hashlib.sha256(binary.read_bytes()).hexdigest(), 'size': binary.stat().st_size}}}
    out.joinpath('latest.json').write_text(json.dumps(manifest, indent=2) + '\n')
else:
    out.joinpath('latest.json').unlink(missing_ok=True)
# A signature from an earlier package must not accompany a new manifest.
out.joinpath('latest.json.sig').unlink(missing_ok=True)
print(f'Upload the contents of {out} into your VPS website directory using FTP.')
print('ZIP integrity and packaged executable verified.')
if not args.base_url:
    print('For automatic updates, rerun with --base-url https://YOUR-DOMAIN/PATH')
