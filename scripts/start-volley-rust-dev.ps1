<#
PowerShell entrypoint for ONE isolated Rust Studio on Windows/WSL.
Start (foreground), Status, Stop, Restart, Doctor, Check.
Never starts/stops Python, kills unknown processes or restarts while native
GPU/CPU/Quick Reel/montage/preparation jobs are in progress.
Default port 8875 deliberately avoids legacy Python Studio 8765.
#>
[CmdletBinding()]
param(
    [ValidateSet("Start","Status","Stop","Restart","Doctor","Check")]
    [string]$Action="Start",
    [string]$Distro="Ubuntu-25.04",
    [string]$Bind="127.0.0.1:8875",
    [string]$DataRoot="",
    [string]$OrtLibrary="",
    [string]$OpenvinoLibrary="",
    [switch]$ReadOnly,
    [switch]$CheckOnly
)
$ErrorActionPreference="Stop"
if($CheckOnly){$Action="Check"}
if($Bind -notmatch '^127\.0\.0\.1:([0-9]{4,5})$'){
    throw "Rust Studio may only bind to 127.0.0.1:<unprivileged-port>"
}
$port=[int]$Matches[1]
if($port -lt 1024 -or $port -gt 65535){throw "Invalid unprivileged port"}
if($Distro -notmatch '^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,80}$'){
    throw "Invalid WSL distro identifier"
}
if($ReadOnly -and $DataRoot){throw "ReadOnly and DataRoot are mutually exclusive"}
foreach($arg in @($DataRoot,$OrtLibrary,$OpenvinoLibrary)){
    if($arg -and ($arg -notmatch '^/' -or $arg -match '(^|/)\.\.(/|$)')){
        throw "Runtime library / data root must be an absolute safe WSL path"
    }
}
$repo=(Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$repoWsl=(& wsl.exe -d $Distro --exec wslpath -a $repo)
if($LASTEXITCODE -ne 0 -or -not $repoWsl){throw "Cannot translate repository path into WSL"}
$repoWsl=$repoWsl.Trim()
$scriptWsl=$repoWsl+"/scripts/volley-rust.sh"
$data=if($DataRoot){$DataRoot}else{"__default__"}
$mode=if($ReadOnly){"readonly"}else{"isolated"}
$ort=if($OrtLibrary){$OrtLibrary}else{"__not_configured__"}
$openvino=if($OpenvinoLibrary){$OpenvinoLibrary}else{"__not_configured__"}
Write-Host ("Rust Studio action={0} WSL={1} bind={2} storage={3}" -f $Action,$Distro,$Bind,$mode)
& wsl.exe -d $Distro --exec bash $scriptWsl $Action.ToLowerInvariant() $repoWsl $Bind $data $mode $ort $openvino
if($LASTEXITCODE -ne 0){
    throw "Rust Studio $Action failed (exit $LASTEXITCODE). No Python process was changed."
}
