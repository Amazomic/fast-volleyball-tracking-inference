<#
Starts the sole Rust Studio backend in WSL. Never starts or stops any
legacy Python process. Ctrl+C stops this foreground session.
#>
[CmdletBinding()]
param(
    [string]$Distro = "Ubuntu-25.04",
    [string]$Bind = "127.0.0.1:8765",
    [string]$DataRoot = "",
    [string]$OpenvinoLibrary = "",
    [switch]$ReadOnly,
    [switch]$CheckOnly
)
$ErrorActionPreference = "Stop"
if ($Bind -notmatch '^127\.0\.0\.1:([0-9]{2,5})$') {
    throw "The Rust development server must bind to 127.0.0.1:<port>."
}
$port = [int]$Matches[1]
if ($port -lt 1024 -or $port -gt 65535) {
    throw "Choose a valid unprivileged port."
}
if ($OpenvinoLibrary -and ($OpenvinoLibrary -notmatch '^/' -or $OpenvinoLibrary -match '(^|/)\.\.(/|$)')) {
    throw "OpenvinoLibrary must be an absolute WSL shared-library path without parent traversal."
}if ($ReadOnly -and $DataRoot) {
    throw "ReadOnly and DataRoot are mutually exclusive."
}
if ($DataRoot -and ($DataRoot -notmatch '^/' -or $DataRoot -match '(^|/)\.\.(/|$)')) {
    throw "DataRoot must be an absolute WSL path without parent traversal."
}
# Fail closed if any process already serves this port. A valid Rust instance
# is reused; legacy Python or unknown listeners are never killed/replaced.
if (-not $CheckOnly) {
    $socket = New-Object System.Net.Sockets.TcpClient
    try {
        $socket.Connect("127.0.0.1", $port)
        try {
            $existing = Invoke-RestMethod -Uri ("http://127.0.0.1:{0}/internal/rewrite-status" -f $port) -TimeoutSec 3 -ErrorAction Stop
        }
        catch {
            throw "Port $port is occupied by a server that does not identify as Rust Studio. Stop it manually after its jobs finish."
        }
        if ($existing.name -notlike 'Volleyball Studio*Rust*' -or -not $existing.pid) {
            throw "Port $port belongs to an unrecognized service, refusing to start a second application."
        }
        Write-Host ("Rust Studio is already running at http://127.0.0.1:{0}/ (PID {1})." -f $port, $existing.pid)
        return
    }
    catch [System.Net.Sockets.SocketException] {
        # The port is free; the Rust binary will acquire it exclusively.
    }
    finally {
        $socket.Dispose()
    }
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
openvino_lib="$6"
cd "$repo/rust"
if [ -f "$HOME/.cargo/env" ]; then source "$HOME/.cargo/env"; fi
if [ "$operation" = check ]; then
  if ! command -v node >/dev/null 2>&1; then
    echo "Node.js is required for the Rust ownership audit in -CheckOnly mode" >&2
    exit 1
  fi
  node "$repo/scripts/ci/check_rust_ownership.mjs" --self-test
  node "$repo/scripts/ci/check_rust_roadmap.mjs" --check --self-test
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
if [ "$openvino_lib" != "__not_configured__" ]; then
  if [ ! -f "$openvino_lib" ]; then
    echo "Configured OpenVINO C library not found: $openvino_lib" >&2
    exit 1
  fi
  export VOLLEY_OPENVINO_C_LIB="$openvino_lib"
  if [ -n "$(printenv LD_LIBRARY_PATH || true)" ]; then
    export LD_LIBRARY_PATH="$(dirname "$openvino_lib"):$LD_LIBRARY_PATH"
  else
    export LD_LIBRARY_PATH="$(dirname "$openvino_lib")"
  fi
fiexec cargo run --offline -p volley-app
'@
$script = $script.Replace("`r`n", "`n")
$wslDataArg = if ($DataRoot) { $DataRoot } else { "__default__" }
Write-Host "Volleyball Studio Rust ($operation): $Bind, WSL $Distro, mode=$readOnlyFlag"
# PowerShell 5.1 on Windows splits multiline bash -c arguments and pipes
# native stdin as CRLF. Write an ephemeral UTF-8/LF script and run it as an
# actual file through WSL. Settings stay positional argv, never shell source.
$tmp = Join-Path $env:TEMP ("volley-rust-launch-" + [guid]::NewGuid().ToString("N") + ".sh")
try {
    [System.IO.File]::WriteAllText(
        $tmp, $script.Replace("`r`n", "`n"),
        (New-Object System.Text.UTF8Encoding($false))
    )
    $tmpWsl = (& wsl.exe -d $Distro --exec wslpath -a $tmp)
    if ($LASTEXITCODE -ne 0 -or -not $tmpWsl) {
        throw "Cannot translate temporary Rust launcher to WSL."
    }
    $ovArg = if ($OpenvinoLibrary) { $OpenvinoLibrary } else { "__not_configured__" }
    & wsl.exe -d $Distro --exec bash $tmpWsl.Trim() $repoWsl $Bind $wslDataArg $operation $readOnlyFlag $ovArg
    if ($LASTEXITCODE -ne 0) {
        throw "Rust developer launcher failed with exit code $LASTEXITCODE."
    }
}
finally {
    Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue
}
