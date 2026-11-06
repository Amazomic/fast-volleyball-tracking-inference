@echo off
setlocal
set "ROOT=%~dp0"
set "RESTART_SWITCH="
if /I "%~1"=="/restart" set "RESTART_SWITCH=-Restart"

rem One control path: reuse healthy UI, restart via its API, or start if absent.
rem Never pkill by name and never launch a second copy after an uncertain restart.
powershell -NoProfile -ExecutionPolicy Bypass -File "%ROOT%scripts\volleyball_ui\ensure-fresh.ps1" %RESTART_SWITCH%
set "RC=%ERRORLEVEL%"

if "%RC%"=="0" (
  echo Volley Studio is already available at http://127.0.0.1:8765
  echo Reusing the open browser tab; no extra server or browser window created.
  exit /b 0
)
if "%RC%"=="2" (
  echo A job is running. Restart is blocked to protect its results.
  echo Open http://127.0.0.1:8765 when it finishes.
  exit /b 2
)
if not "%RC%"=="1" (
  echo Volley Studio restart failed or could not be verified.
  echo NOT launching another server. Check http://127.0.0.1:8765
  pause
  exit /b 3
)

rem ROOT has a trailing slash that breaks WSL --cd quoting unless stripped.
set "ROOT_FOR_WSL=%ROOT:~0,-1%"
for /f "delims=" %%L in ('wsl.exe -d Ubuntu-25.04 -e wslpath -u "%ROOT_FOR_WSL%"') do set "WSL_ROOT=%%L"
if not defined WSL_ROOT (
  echo Cannot resolve the repository path in WSL Ubuntu-25.04.
  pause
  exit /b 1
)
echo Starting the only Volley Studio server at http://127.0.0.1:8765
start "" /min powershell -NoProfile -WindowStyle Hidden -Command "Start-Sleep -Seconds 3; Start-Process 'http://127.0.0.1:8765'"
wsl.exe -d Ubuntu-25.04 --cd "%WSL_ROOT%" -e .venv/bin/python -m scripts.volleyball_ui
echo Volley Studio server stopped.
pause
