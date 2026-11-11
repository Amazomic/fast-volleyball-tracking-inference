# Local non-GPU CI gate for the Rust-only Studio (Windows native).
# Builds the WASM UI module, then runs fmt, clippy (-D warnings), the full
# Rust test suite and every browser/contract Node gate that does not need
# GPU, media or Docker.
# GPU/media-dependent stage84 scripts and WSL lifecycle shells are separate
# and must be reported explicitly when skipped (see docs/TESTING.md).
param(
    [switch]$SkipRustTests
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$failures = @()

function Invoke-Gate {
    param([string]$Name, [scriptblock]$Body)
    Write-Host "==> $Name"
    & $Body
    if ($LASTEXITCODE -ne 0) {
        $script:failures += $Name
        Write-Host "FAIL: $Name (exit $LASTEXITCODE)"
    }
}

Push-Location (Join-Path $root "rust")
try {
    # The WASM UI module is a build product (not committed); volley-app
    # embeds it via include_bytes!, so it must exist before any cargo gate.
    Invoke-Gate "build wasm" {
        cargo build -p volley-web --target wasm32-unknown-unknown --release --offline --locked
        if ($LASTEXITCODE -eq 0) {
            Copy-Item "target/wasm32-unknown-unknown/release/volley_web.wasm" `
                "volley-app/static/volley_web.wasm" -Force
        }
    }
    Invoke-Gate "cargo fmt" { cargo fmt --all -- --check }
    Invoke-Gate "cargo clippy" {
        cargo clippy --workspace --all-targets --offline --locked -- -D warnings
    }
    if (-not $SkipRustTests) {
        Invoke-Gate "cargo test" { cargo test --workspace --offline --locked }
    }
} finally {
    Pop-Location
}

# Stage84 parity/benchmark/real-media gates need GPU, ffmpeg media and the
# frozen Python baseline; they are intentionally not part of this gate.
$gpuGates = @(
    "test_rust_parity_84.mjs",
    "test_rust_benchmark_84.mjs",
    "benchmark_rust_parity_84.mjs",
    "audit_python_baseline_84.mjs",
    "test_rust_real_media_84.mjs"
)
$gates = Get-ChildItem (Join-Path $root "scripts/ci") -Filter "*.mjs" |
    Where-Object { $gpuGates -notcontains $_.Name } |
    Sort-Object Name
foreach ($gate in $gates) {
    Invoke-Gate $gate.Name { node $gate.FullName }
}

if ($failures.Count -gt 0) {
    Write-Host ""
    Write-Host "check-all FAILED: $($failures -join ', ')"
    exit 1
}
Write-Host ""
Write-Host "check-all PASSED: fmt, clippy, Rust tests and $($gates.Count) Node gates"