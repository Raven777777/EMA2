@echo off
rem Start the EMA2 Rust relay as a detached, hidden background process.
rem Uses scripts\bridge.ps1: exact numeric port + executable path verification.
rem Usage: scripts\start-bridge.cmd  (optionally set EDGE_MCP_PORT)
setlocal

set "PORT=%EDGE_MCP_PORT%"
if "%PORT%"=="" set PORT=8765

powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0bridge.ps1" -Action start -Port %PORT%
exit /b %errorlevel%
