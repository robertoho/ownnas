@echo off
setlocal
cd /d "%~dp0"

if not exist "target\release\ownnas.exe" (
    echo target\release\ownnas.exe was not found. Run build.bat first.
    exit /b 1
)

.\target\release\ownnas.exe serve --root "%USERPROFILE%\Pictures" --username admin
exit /b %ERRORLEVEL%
