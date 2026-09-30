@echo off
setlocal
cd /d "%~dp0"

cargo build --release
if errorlevel 1 (
    echo Build failed.
    exit /b 1
)

echo Built target\release\ownnas.exe
exit /b 0
