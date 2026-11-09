# Volleyball Studio Rust workspace

This is the **only active application** on the migration branch.
`volley-app` is the Axum server, `volley-core` contains CPU identity,
motion and rally logic, `volley-inference` loads native ONNX Runtime,
`volley-media` invokes FFmpeg, and `volley-web` provides actual WASM state.

Launch from repository root via `start-volley-ui.cmd` (Windows),
or `scripts/start-volley-rust-dev.ps1` (PowerShell). Default
`127.0.0.1:8765`, with homepage redirect to `/studio/rust`.
Use the Windows CMD actions /status, /stop, /restart, /doctor and /check,
or PowerShell -Action Status/Stop/Restart/Doctor/Check. WSL Rust uses
127.0.0.1:8875 by default, NOT the Python legacy port 8765. A second Rust
process is prevented by a canonical-checkout lock, even on another port;
stop/restart refuses to signal Rust while GPU/CPU/montage/Quick Reel is
active. The Dockerfile can build from sources, or package an offline Linux
release via scripts/ci/build_rust_docker_local.sh. Complete setup and
current CUDA/Docker verification status: ../docs/RUST_DEPLOYMENT.md.

Run `cargo fmt --all -- --check`, `cargo check --workspace --offline`
and `cargo test --workspace --offline` from this directory. GPU smoke:
`bash ../scripts/ci/smoke_rust_ort.sh /path/to/libonnxruntime.so cuda full`.

OpenVINO person ReID uses volley-inference/src/reid_openvino.rs and the
bundled ov/person-reidentification-retail-0287 model. Configure the absolute
WSL path VOLLEY_OPENVINO_C_LIB and native LD_LIBRARY_PATH or pass the Windows
launcher option -OpenvinoLibrary /absolute/wsl/path/libopenvino_c.so.
Run cargo test --workspace --offline with VOLLEY_TEST_OPENVINO_C_LIB set
to the same native .so to execute the real-model smoke test. These
temporary, SHA-bound vectors are only visual suggestions, not identities.
Manually reviewed native player reference JPEGs have a separate, versioned,
reversible global athlete album under the isolated Rust data root. Use
POST /api/players/native/references (read/enroll/undo) and GET
/api/players/native/reference-photo; enrollment requires an already confirmed
Track ID assignment in that exact SHA-verified CPU replay. References are
photos, not auto-confirmed identities and not yet computed embedding caches.
This is a **partial rewrite**, not a claim of complete product parity.
Existing Python source is retained only in Git history/checkpoint; legacy
media and snapshots remain readable through Rust data adapters but are
never silently modified. See `../docs/RUST_MIGRATION.md` for missing
features, decisions and test evidence.