# Volleyball Studio Rust workspace

This is the **only active application** on the migration branch.
`volley-app` is the Axum server, `volley-core` contains CPU identity,
motion and rally logic, `volley-inference` loads native ONNX Runtime,
`volley-media` invokes FFmpeg, and `volley-web` provides actual WASM state.

Launch from repository root via `start-volley-ui.cmd` (Windows),
or `scripts/start-volley-rust-dev.ps1` (PowerShell). Default
`127.0.0.1:8765`, with homepage redirect to `/studio/rust`.
Use `-CheckOnly` to compile and validate the ownership/roadmap rules.

Run `cargo fmt --all -- --check`, `cargo check --workspace --offline`
and `cargo test --workspace --offline` from this directory. GPU smoke:
`bash ../scripts/ci/smoke_rust_ort.sh /path/to/libonnxruntime.so cuda full`.

This is a **partial rewrite**, not a claim of complete product parity.
Existing Python source is retained only in Git history/checkpoint; legacy
media and snapshots remain readable through Rust data adapters but are
never silently modified. See `../docs/RUST_MIGRATION.md` for missing
features, decisions and test evidence.