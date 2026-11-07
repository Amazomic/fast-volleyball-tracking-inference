# volley-app — Rust-native Volleyball Studio

**Development only; full rewrite is not finished.** This is a standalone Rust backend (Axum/Tokio), not a Python subprocess bridge. It occupies a separate loopback port (`8875`) and has no production `/api/health` until the complete replacement exists. Existing Python/hybrid Studio on `8765` must not be interrupted.

## Workspace

The three maintained crates share `rust/Cargo.toml` and `rust/Cargo.lock`:

- `volley-core`: deterministic geometry and motion kernels (historic S01/S02a).
- `volley-media`: FFprobe metadata, rotation, target resolution and analysis-ready classification (H.264, 30 FPS, AAC and geometry).
- `volley-app`: standalone HTTP server, JSON project/catalog/history APIs, isolated development writes.

From repository root in WSL Ubuntu-25.04:

```bash
cargo check --manifest-path rust/Cargo.toml --workspace
cargo run --manifest-path rust/Cargo.toml -p volley-app
# alternate port: VOLLEY_RUST_BIND=127.0.0.1:8875
```

Default reads from the original `results/` and `project/`, **without writing** user files. To exercise experimental project creation/update, provide an independent absolute writable data directory, e.g.:

```bash
VOLLEY_RUST_DATA_ROOT=/home/amazomic/.cache/volley-rust-sandbox \
cargo run --manifest-path rust/Cargo.toml -p volley-app
```

The dev server rejects writes without an isolated directory. It rejects the original `results/` directory and its ancestors/descendants as writable roots. Project source videos are only inspected; the source files are never changed.

## Native endpoints implemented so far

```
GET  /internal/rewrite-status     explicit incomplete-build status
GET  /api/projects               summaries, video availability, existing cache references
GET  /api/videos                 video listing
GET  /api/video-info?video=...   FFprobe source metadata
GET  /api/analysis-profiles      saved global profile catalog
GET  /api/analysis-runs?project_id=...
GET  /api/analysis-history?project_id=...
GET  /api/analysis-resources     CPU thread count
GET  /api/players/library       existing global athlete catalog
POST /api/project-create        experimental isolated registry only
POST /api/project-update        experimental isolated registry only
```

These routes are implementations of persisted formats, not a claim of complete API parity. In particular, project creation **does not yet queue video preparation** and existing projects may use legacy storage conventions not fully supported. Player catalog loading does not yet run legacy photo/identity backfills.

**Not implemented:** video preparation jobs, uploads, complete model/GPU inference, complete CPU postprocessing, full ReID/manual players, previews and media streaming, player stats, rally detection, edit/montage, quick reel, full API, Rust/WASM browser UI and desktop packaging.

Development compilation/smoke probes are allowed. User-directed order: finish all Rust functionality **first**, then comprehensive tests/adaptation, then compare final Rust vs saved Python; rollback to `checkpoint/pre-rust-2026-11-07` if needed. See `docs/RUST_REWRITE_SCOPE.md` and `docs/RUST_MIGRATION.md`.
