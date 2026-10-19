@echo off
rem Stop the EMA2 relay only when the exact port is owned by this project's bin\ema2.exe.
rem A foreign listener is reported and left untouched.
setlocal

set "PORT=%EDGE_MCP_PORT%"
if "%PORT%"=="" set PORT=8765

powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0bridge.ps1" -Action stop -Port %PORT%
exit /b %errorlevel%
