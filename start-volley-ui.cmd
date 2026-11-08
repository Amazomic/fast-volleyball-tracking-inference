@echo off
setlocal
set "ROOT=%~dp0"
if /I "%~1"=="/restart" (
    echo Rust Studio will not force-restart while analysis or montage may be running.
    echo Close the foreground Rust server cleanly with Ctrl+C, then launch again.
    exit /b 2
)
rem Rust is the only application runtime; no Python fallback or auto-kill.
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%ROOT%scripts\start-volley-rust-dev.ps1" %*
set "RC=%ERRORLEVEL%"
if not "%RC%"=="0" (
    echo Rust Studio failed to start; any existing process was left untouched.
    pause
)
exit /b %RC%