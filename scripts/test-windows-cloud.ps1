# End-to-end check of ownnas-cloud (clients/windows) on Windows: a server on this computer, a
# place in Explorer for it, and files going both ways. Run from the repository after
# `cargo build --release -p ownnas -p ownnas-cloud` (the Windows client workflow does).
$ErrorActionPreference = 'Stop'
$bin = Join-Path $PWD 'target\release'
$work = Join-Path $env:RUNNER_TEMP 'ownnas-cloud-test'
if (-not $env:RUNNER_TEMP) { $work = Join-Path $env:TEMP 'ownnas-cloud-test' }
Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
$library = Join-Path $work 'library'
$data = Join-Path $work 'data'
$place = Join-Path $work 'OwnNAS - library'
New-Item -ItemType Directory -Force (Join-Path $library 'Docs'), $data | Out-Null
Set-Content -NoNewline -Path (Join-Path $library 'Docs\readme.txt') -Value 'hello from the server'

function Fail($message) { Write-Host "FAILED: $message"; Get-Content (Join-Path $env:LOCALAPPDATA 'OwnNAS\cloud.log') -ErrorAction SilentlyContinue; exit 1 }
function Wait-Until($what, [scriptblock]$test, $seconds = 30) {
    for ($i = 0; $i -lt $seconds * 2; $i++) { if (& $test) { Write-Host "ok: $what"; return }; Start-Sleep -Milliseconds 500 }
    Fail $what
}

$env:OWNNAS_USER = 'admin'; $env:OWNNAS_PASSWORD = 'test passphrase 123'
# The agent (started by setup) logs each change the server's feed tells of.
$env:OWNNAS_TRACE = '1'
& "$bin\ownnas.exe" user add --data $data | Out-Host
$server = Start-Process -PassThru -NoNewWindow "$bin\ownnas.exe" -ArgumentList 'serve', '--root', $library, '--data', $data, '--addr', '127.0.0.1:8899'
Wait-Until 'the server is up' { try { (Invoke-WebRequest -UseBasicParsing http://127.0.0.1:8899/api/health).StatusCode -eq 200 } catch { $false } }

try {
    & "$bin\ownnas-cloud.exe" setup --server http://127.0.0.1:8899 --username admin --password 'test passphrase 123' --folder $place | Out-Host
    if ($LASTEXITCODE -ne 0) { Fail 'setup' }
    & "$bin\ownnas-cloud.exe" status | Out-Host

    # Folders fill as they are opened; files take no space until read.
    Wait-Until 'the server''s folders show' { Test-Path (Join-Path $place 'Docs\readme.txt') }
    $file = Get-Item (Join-Path $place 'Docs\readme.txt')
    $recall = 0x400000  # FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: a placeholder, not downloaded
    if (-not ([int]$file.Attributes -band $recall)) { Fail "readme.txt was downloaded before it was read (attributes $($file.Attributes))" }
    Write-Host 'ok: files are online-only until read'
    $text = Get-Content -Raw (Join-Path $place 'Docs\readme.txt')
    if ($text -ne 'hello from the server') { Fail "reading gave '$text'" }
    Write-Host 'ok: reading downloads the file'

    Set-Content -NoNewline -Path (Join-Path $place 'Docs\from-windows.txt') -Value 'written in Explorer'
    Wait-Until 'a file made here reaches the server' { (Test-Path (Join-Path $library 'Docs\from-windows.txt')) -and ((Get-Content -Raw (Join-Path $library 'Docs\from-windows.txt')) -eq 'written in Explorer') }

    Set-Content -NoNewline -Path (Join-Path $library 'Docs\from-server.txt') -Value 'made on the server'
    Wait-Until 'a file made on the server shows here' { Test-Path (Join-Path $place 'Docs\from-server.txt') }

    # A downloaded file changed on the server: the new version shows here.
    Set-Content -NoNewline -Path (Join-Path $library 'Docs\readme.txt') -Value 'changed on the server'
    Wait-Until 'a change on the server reaches a downloaded file' { (Get-Content -Raw (Join-Path $place 'Docs\readme.txt')) -eq 'changed on the server' }

    Rename-Item (Join-Path $place 'Docs\from-windows.txt') 'renamed.txt'
    Wait-Until 'a rename here happens on the server' { Test-Path (Join-Path $library 'Docs\renamed.txt') }

    Remove-Item (Join-Path $library 'Docs\from-server.txt')
    Wait-Until 'a deletion on the server shows here' { -not (Test-Path (Join-Path $place 'Docs\from-server.txt')) }

    Write-Host 'Windows Explorer round trip passed.'
} finally {
    & "$bin\ownnas-cloud.exe" remove http://127.0.0.1:8899 | Out-Host
    Get-Process ownnas-cloud-agent -ErrorAction SilentlyContinue | Stop-Process -Force
    Stop-Process -Id $server.Id -Force -ErrorAction SilentlyContinue
    Get-Content (Join-Path $env:LOCALAPPDATA 'OwnNAS\cloud.log') -ErrorAction SilentlyContinue | Select-Object -Last 40
}
