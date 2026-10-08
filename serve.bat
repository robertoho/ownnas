@echo off
setlocal
pushd "%~dp0"

if not defined OWNNAS_ROOT set "OWNNAS_ROOT=%USERPROFILE%\Pictures"
if not defined OWNNAS_DATA set "OWNNAS_DATA=%LOCALAPPDATA%\OwnNAS"
if not defined OWNNAS_ADDR set "OWNNAS_ADDR=0.0.0.0:8787"

if not exist "target\release\ownnas.exe" (
    echo OwnNAS has not been built yet. Run build.bat first. 1>&2
    popd
    exit /b 1
)

"target\release\ownnas.exe" serve %*
set "EXIT_CODE=%ERRORLEVEL%"
popd
exit /b %EXIT_CODE%
