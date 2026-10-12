@echo off
rem Double-click to install Hangtime. "install.cmd -Uninstall" removes it.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1" %*
echo.
pause
