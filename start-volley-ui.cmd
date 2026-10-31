@echo off
setlocal
set "ROOT=%~dp0"
powershell -NoProfile -Command "try { $r=Invoke-WebRequest -Uri 'http://127.0.0.1:8765/' -UseBasicParsing -TimeoutSec 2; if($r.StatusCode -eq 200){exit 0} } catch {}; exit 1" >nul 2>&1
if not errorlevel 1 (
  start "" "http://127.0.0.1:8765"
  exit /b 0
)
for /f "delims=" %%L in ('wsl.exe -d Ubuntu-25.04 -e wslpath -u "%ROOT%"') do set "WSL_ROOT=%%L"
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
