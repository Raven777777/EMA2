@echo off
rem Stop the EMA2 relay (kills the process listening on the configured port).
setlocal
set "PORT=%EDGE_MCP_PORT%"
if "%PORT%"=="" set PORT=8765
for /f "tokens=5" %%p in ('netstat -ano ^| findstr ":%PORT%" ^| findstr "LISTENING"') do (
  taskkill /PID %%p /F >nul 2>&1
)
echo EMA2 stopped (port %PORT%).
