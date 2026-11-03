@echo off
setlocal
set "ROOT=%~dp0"
powershell -NoProfile -ExecutionPolicy Bypass -File "%ROOT%scripts\volleyball_ui\ensure-fresh.ps1"
if errorlevel 2 (
  echo Volley Studio is processing a job. The running server will not be interrupted.
  echo Open http://127.0.0.1:8765 in your browser.
  echo.
  pause
  exit /b 0
)
if not errorlevel 1 (
  echo Volley Studio is already running at http://127.0.0.1:8765
  echo This console did not start that process, so Ctrl+C cannot stop it.
  echo.
  echo [S] Stop the running UI server
  echo [Q] Close this window, leave the UI running
  choice /c SQ /n /m "Choose S or Q: "
  if errorlevel 2 exit /b 0
  if errorlevel 1 (
    wsl.exe -d Ubuntu-25.04 -e pkill -TERM -f -x ".venv/bin/python -m scripts.volleyball_ui"
    echo Volley Studio UI shutdown requested.
    pause
    exit /b 0
  )
)
rem %~dp0 always ends in a backslash; it escapes the closing quote in WSL's
rem Windows command-line parser and leaves a stray " in the Linux --cd path.
set "ROOT_FOR_WSL=%ROOT:~0,-1%"
for /f "delims=" %%L in ('wsl.exe -d Ubuntu-25.04 -e wslpath -u "%ROOT_FOR_WSL%"') do set "WSL_ROOT=%%L"
if not defined WSL_ROOT (
  echo Cannot resolve the repository path in WSL. Check Ubuntu-25.04.
  pause
  exit /b 1
)
echo Launching Volleyball Studio on http://127.0.0.1:8765
start "" /min powershell -NoProfile -WindowStyle Hidden -Command "Start-Sleep -Seconds 3; Start-Process 'http://127.0.0.1:8765'"
wsl.exe -d Ubuntu-25.04 --cd "%WSL_ROOT%" -e .venv/bin/python -m scripts.volleyball_ui
echo Volleyball Studio stopped.
pause
