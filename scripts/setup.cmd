@echo off
rem Build the EMA2 Rust daemon/CLI and copy the release exe to bin\.
rem Requires cargo on PATH, or set EMA2_CARGO to the full cargo executable path.
setlocal

set "ROOT=%~dp0.."
set "CARGO=%EMA2_CARGO%"
if "%CARGO%"=="" set "CARGO=cargo"

where "%CARGO%" >nul 2>nul
if errorlevel 1 (
  echo [ERROR] cargo not found. Install Rust or set EMA2_CARGO to cargo.exe.
  exit /b 1
)

echo Building release with "%CARGO%"...
call "%CARGO%" build --release --manifest-path "%ROOT%\rust\Cargo.toml"
if errorlevel 1 (
  echo [ERROR] build failed.
  exit /b 1
)

if not exist "%ROOT%\bin" mkdir "%ROOT%\bin"
if errorlevel 1 (
  echo [ERROR] cannot create "%ROOT%\bin".
  exit /b 1
)

copy /Y "%ROOT%\rust\target\release\ema2.exe" "%ROOT%\bin\ema2.exe" >nul
if errorlevel 1 (
  echo [ERROR] failed to copy ema2.exe to bin\.
  exit /b 1
)

echo bin\ema2.exe ready.
