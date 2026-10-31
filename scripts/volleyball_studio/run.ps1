param([Parameter(ValueFromRemainingArguments = $true)][string[]]$StudioArgs)
$ErrorActionPreference = 'Stop'
$StudioRoot = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
if (Test-Path (Join-Path $StudioRoot '.venv/Scripts/python.exe')) {
    $env:PYTHONPATH = Join-Path $StudioRoot 'scripts'
    & (Join-Path $StudioRoot '.venv/Scripts/python.exe') -m volleyball_studio @StudioArgs
    exit $LASTEXITCODE
}
$StudioLinuxRoot = (& wsl -d Ubuntu-25.04 --exec wslpath -a $StudioRoot).Trim()
if ($LASTEXITCODE -ne 0) { throw 'Cannot open WSL Ubuntu-25.04' }
& wsl -d Ubuntu-25.04 --cd $StudioLinuxRoot --exec env PYTHONPATH=scripts .venv/bin/python -m volleyball_studio @StudioArgs
exit $LASTEXITCODE
