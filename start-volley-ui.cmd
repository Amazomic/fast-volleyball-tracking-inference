@echo off
setlocal
set "ROOT=%~dp0"
rem Studio uses localhost port 8875 and never replaces other services.
if /I "%~1"=="/restart" goto restart
if /I "%~1"=="/stop" goto stop
if /I "%~1"=="/status" goto status
if /I "%~1"=="/doctor" goto doctor
if /I "%~1"=="/check" goto check
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%ROOT%scripts\start-volley-rust-dev.ps1" %*
exit /b %ERRORLEVEL%
:restart
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%ROOT%scripts\start-volley-rust-dev.ps1" -Action Restart
exit /b %ERRORLEVEL%
:stop
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%ROOT%scripts\start-volley-rust-dev.ps1" -Action Stop
exit /b %ERRORLEVEL%
:status
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%ROOT%scripts\start-volley-rust-dev.ps1" -Action Status
exit /b %ERRORLEVEL%
:doctor
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%ROOT%scripts\start-volley-rust-dev.ps1" -Action Doctor
exit /b %ERRORLEVEL%
:check
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%ROOT%scripts\start-volley-rust-dev.ps1" -Action Check
exit /b %ERRORLEVEL%
