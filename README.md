# Volleyball Studio — Rust

**Active implementation: Rust only.** The Python application, old browser UI, old CLI,
Python dependency lockfile and Python test suite have been removed from this migration
branch. They remain recoverable in the immutable Git checkpoint
`pre-rust-migration-2026-11-07` and the remote
`checkpoint/pre-rust-2026-11-07` branch. Do not restore a Python fallback.

Volleyball Studio is a local volleyball video analysis workspace. The intended
pipeline is **project → prepared video → immutable VballNet/RAVEL GPU evidence →
versioned CPU player tracks/Touch/rallies → reviewed players → original-quality montage**.
FFmpeg, ONNX Runtime and CUDA are native external runtimes, not Python components.

**Rust-миграция: 59/85; следующий 60/85 (плановый объём).**
[Stage map](docs/RUST_ROADMAP.md) ·
[Migration journal](docs/RUST_MIGRATION.md) ·
[Architecture and owners](docs/RUST_ARCHITECTURE.md).

## Start on Windows

Run `start-volley-ui.cmd` at the repository root, or in PowerShell:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/start-volley-rust-dev.ps1
```

Open **http://127.0.0.1:8765/**. The launcher runs the **Rust binary in WSL
Ubuntu-25.04**, not the removed Python application. If this port already hosts
Rust Studio, it reuses that process; an unrelated or legacy listener is not
stopped or replaced. Closing the foreground WSL process with Ctrl+C shuts it
down cleanly. Automated forced restart is intentionally disabled to protect
running analysis and montage jobs.

Rust creates and updates **only** its isolated persistent data directory:
`$HOME/.cache/volley-rust-sandbox` inside WSL. Existing project, media,
`project/` and `results/` data are read-only compatibility inputs and are
never silently migrated, overwritten or deleted.

## Development

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/start-volley-rust-dev.ps1 -CheckOnly
```

Or from WSL at the repository root:

```bash
cd rust
cargo fmt --all -- --check
cargo check --workspace --offline
cargo test --workspace --offline
node ../scripts/ci/check_rust_ownership.mjs --self-test
node ../scripts/ci/check_rust_roadmap.mjs --check --self-test
node ../scripts/ci/test_volley_web_wasm.mjs
```

Crates: `volley-app` (Axum HTTP/jobs), `volley-core` (CPU evidence),
`volley-inference` (native ONNX), `volley-media` (FFmpeg/exports), and
`volley-web` (WASM client state). Model files stay under `models/` and
`ov/`. No Python interpreter, venv, pip, uv or Python web server is needed
for the application. The optional native ONNX GPU smoke test is
`scripts/ci/smoke_rust_ort.sh` and requires external ONNX Runtime and GPU
libraries.

## Current limitations

This is the **sole application implementation, not yet a completed product**.
The native Rust GPU→CPU pipeline and experimental analysis/player/preview/
montage UI exist, with immutable replay artifacts. Detection-based tracks and
Touch/rally proposals are not yet verified player identities or authoritative
game events. Remaining work includes complete Quick Reel GPU/CPU/render
orchestration, image-based ReID reference enrollment, comprehensive browser
UI/UX and real-match acceptance, automatic game-event classification and
release packaging. The former Python UI/features have **not** all been
reimplemented; no fallback is supplied.

Do not infer product readiness from the stage counter. Record each tested
change in the append-only migration journal and stage map. Keep the checkpoint,
original media and Git history intact. See `AGENTS.md` for repository policy.