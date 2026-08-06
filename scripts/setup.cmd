@echo off
rem Build the EMA2 Rust relay and copy the release exe to bin\.
rem Requires the Rust toolchain (cargo on PATH, or D:\Rust\cargo.cmd).
setlocal

set "ROOT=%~dp0.."

set "CARGO=cargo"
where cargo >nul 2>nul || set "CARGO=D:\Rust\cargo.cmd"

echo Building release...
call "%CARGO%" build --release --manifest-path "%ROOT%\rust\Cargo.toml"
if errorlevel 1 (
  echo [ERROR] build failed.
  exit /b 1
)

copy /Y "%ROOT%\rust\target\release\ema2.exe" "%ROOT%\bin\ema2.exe" >nul
echo bin\ema2.exe ready.
