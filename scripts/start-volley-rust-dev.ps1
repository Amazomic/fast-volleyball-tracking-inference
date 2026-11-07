<#
Starts only the Rust development server in WSL. Never starts or stops the
production Python Studio on :8765. Ctrl+C stops this foreground session.
#>
[CmdletBinding()]
param(
    [string]$Distro = "Ubuntu-25.04",
    [string]$Bind = "127.0.0.1:8875",
    [string]$DataRoot = "",
    [switch]$ReadOnly,
    [switch]$CheckOnly
)
$ErrorActionPreference = "Stop"
if ($Bind -notmatch '^127\.0\.0\.1:([0-9]{2,5})$') {
    throw "The Rust development server must bind to 127.0.0.1:<port>."
}
$port = [int]$Matches[1]
if ($port -eq 8765 -or $port -lt 1024 -or $port -gt 65535) {
    throw "Choose an unprivileged port other than 8765."
}
if ($ReadOnly -and $DataRoot) {
    throw "ReadOnly and DataRoot are mutually exclusive."
}
if ($DataRoot -and ($DataRoot -notmatch '^/' -or $DataRoot -match '(^|/)\.\.(/|$)')) {
    throw "DataRoot must be an absolute WSL path without parent traversal."
}
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$repoWsl = (& wsl.exe -d $Distro --exec wslpath -a $repo)
if ($LASTEXITCODE -ne 0 -or -not $repoWsl) {
    throw "Could not translate repository path to WSL (check the distro name)."
}
$repoWsl = $repoWsl.Trim()
$operation = if ($CheckOnly) { "check" } else { "run" }
$readOnlyFlag = if ($ReadOnly) { "readonly" } else { "isolated" }
# Bash receives the path and settings as positional arguments, not as shell
# interpolated source. This also works when the Windows repository path has spaces.
$script = @'
set -euo pipefail
repo="$1"
bind="$2"
data_root="$3"
operation="$4"
mode="$5"
cd "$repo/rust"
if [ -f "$HOME/.cargo/env" ]; then source "$HOME/.cargo/env"; fi
if [ "$operation" = check ]; then
  exec cargo check --workspace --offline
fi
export VOLLEY_RUST_BIND="$bind"
if [ "$mode" = readonly ]; then
  unset VOLLEY_RUST_DATA_ROOT
elif [ "$data_root" != "__default__" ]; then
  export VOLLEY_RUST_DATA_ROOT="$data_root"
else
  export VOLLEY_RUST_DATA_ROOT="$HOME/.cache/volley-rust-sandbox"
fi
exec cargo run --offline -p volley-app
'@
$script = $script.Replace("`r`n", "`n")
$wslDataArg = if ($DataRoot) { $DataRoot } else { "__default__" }
Write-Host "Volleyball Studio Rust ($operation): $Bind, WSL $Distro, mode=$readOnlyFlag"
& wsl.exe -d $Distro --exec bash -c $script "volley-rust" $repoWsl $Bind $wslDataArg $operation $readOnlyFlag
if ($LASTEXITCODE -ne 0) {
    throw "Rust developer launcher failed with exit code $LASTEXITCODE."
}
