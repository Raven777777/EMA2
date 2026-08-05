@echo off
rem Start the EMA2 Rust relay as a detached, hidden background process.
rem Usage: scripts\start-bridge.cmd  (optionally set EDGE_MCP_PORT)
setlocal

set "EXE=%~dp0..\bin\ema2.exe"
set "PORT=%EDGE_MCP_PORT%"
if "%PORT%"=="" set PORT=8765

if not exist "%EXE%" (
  echo [ERROR] %EXE% not found. Build it first: cd rust ^&^& cargo build --release
  exit /b 1
)

netstat -ano | findstr ":%PORT%" | findstr "LISTENING" >nul
if not errorlevel 1 (
  echo EMA2 already running on port %PORT%.
  exit /b 0
)

powershell -NoProfile -Command "Start-Process -FilePath '%EXE%' -ArgumentList 'run','--port',%PORT% -WindowStyle Hidden"
echo EMA2 started on port %PORT% (MCP http://127.0.0.1:%PORT%/mcp).
echo Edge extension should auto-connect within a few seconds.
