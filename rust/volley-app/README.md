# volley-app — Rust-native Volleyball Studio

**Development only; full rewrite is not finished.** This is a standalone Rust backend (Axum/Tokio), not a Python subprocess bridge. It occupies a separate loopback port (`8875`) and has no production `/api/health` until the complete replacement exists. Existing Python/hybrid Studio on `8765` must not be interrupted.

## Workspace

The three maintained crates share `rust/Cargo.toml` and `rust/Cargo.lock`:

- `volley-core`: deterministic geometry and motion kernels (historic S01/S02a).
- `volley-media`: FFprobe metadata, rotation, target resolution, H.264/30 FPS readiness, schema-2 SHA256 prepared identities, JPEG preview rendering and original-quality montage planning.
- `volley-app`: standalone HTTP server, JSON project/catalog/history APIs, isolated preparation and manual montage jobs, audio upload, FFmpeg orchestration and byte-range MP4 streaming.

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

The dev server rejects writes without an isolated directory. It rejects writing anywhere inside (or above) the original repository, including `results/`, `project/`, and models. Project source videos are read only; the originals are never overwritten.

## Native endpoints implemented so far

```
GET  /internal/rewrite-status     explicit incomplete-build status
GET  /api/projects               summaries, video availability, existing cache references
GET  /api/videos                 video listing
GET  /api/video-info?video=...   FFprobe source metadata
GET  /api/video-view?project_id=...&video=...   prepared-media playback metadata
GET  /api/video-media?project_id=...&video=...  HTTP 200/206 byte-range MP4
GET  /api/video-preview?video=...&time=...&mode=...  FFmpeg JPEG frame
GET  /api/analysis-profiles      saved global profile catalog
GET  /api/analysis-runs?project_id=...
GET  /api/analysis-history?project_id=...
GET  /api/analysis-resources     CPU thread count
GET  /api/players/library       existing global athlete catalog
POST /api/project-create        experimental isolated registry only
POST /api/project-update        experimental isolated registry only
GET  /api/project-preparation?project_id=...  preparation states, enqueue missing when isolated
POST /api/project-preparation/retry    retry failed/preparation-paused jobs (isolated only)
POST /api/project-preparation/cancel   pause and cancel this project's preparation jobs
POST /api/montage/native/start         experimental manual montage job from original frames
GET  /api/montage/native/status        current montage stage, percent, outcome
POST /api/montage/native/cancel        cancel current montage (job_id)
GET  /api/montage/native/library?project_id=...  saved versioned Rust MP4s
GET  /api/montage/native/media?project_id=...&job_id=...  MP4 with HTTP Range
POST /api/montage/native/audio         multipart file upload (field file, max 90 MiB)
```

These routes are implementations of persisted formats, not a claim of complete API parity. Rust project creation/update now queues native preparation. Compatible original MP4s are reused without recoding; other sources are converted by native FFmpeg to H.264/30 FPS/YUV420P and stored in the separate content-addressed development cache. One converter runs at a time; file locks prevent duplicate publication, status tracks FFmpeg progress, and cancellation is persisted per project until an explicit retry. Existing projects may use legacy storage conventions not fully supported; player catalog loading does not yet run legacy photo/identity backfills.

**Not implemented:** automatic rally ranking/selection, full manual montage UI contract and filters, player/ball overlays or auto-follow/vertical crop, audio mixing with source sound, cross-project adoption of old prepared results, general uploads, complete model/GPU inference, complete CPU postprocessing, full ReID/manual players, player statistics and rallies, on-demand fallback video segments, quick reel, full API, Rust/WASM browser UI and desktop packaging.

Development compilation/smoke probes are allowed. User-directed order: finish all Rust functionality **first**, then comprehensive tests/adaptation, then compare final Rust vs saved Python; rollback to `checkpoint/pre-rust-2026-11-07` if needed. See `docs/RUST_REWRITE_SCOPE.md` and `docs/RUST_MIGRATION.md`.

## Experimental native montage request

This is a **manual frame-range export API**, not yet the normal browser's automatic rally-selection contract. Every frame range is expressed on the 30 FPS analysis timeline; its original-recording seek is `analysis_offset_sec + start_frame / 30`. The offset currently comes from the caller; validation against the selected saved analysis run and exact reconstructed rally IDs is **not yet implemented**. Do not use it as proof that a selected rally is complete.

```json
{
  "project_id": "1234abcdef01",
  "clips": [
    {"video": "match.mp4", "start_frame": 360, "end_frame": 580, "analysis_offset_sec": 0}
  ],
  "encoder": "auto",
  "audio_file": null,
  "audio_volume": 40
}
```

POST to `/api/montage/native/start` with `Content-Type: application/json`, using a project already created inside the independent Rust sandbox. Poll `/api/montage/native/status`; cancel using `/api/montage/native/cancel` and the returned `job_id`. When complete, list `/api/montage/native/library?project_id=...` or stream from the returned media URL. Optional music must first be imported with multipart `POST /api/montage/native/audio` (form field `file`); the returned `audio_file` is an opaque filename under isolated Rust storage, not a local machine path.

One montage can render at once; clip extraction preserves native video resolution/FPS and uses FFmpeg H.264 high-quality CRF15 or NVENC CQ17, retrying on CPU on encoder failure. Uniform compatible intermediate streams are concatenated with stream copy; incompatible ones are padded to the largest source resolution and re-encoded. Each output has its own versioned folder and a `selection.json` manifest. These **implementation capabilities are not final product acceptance**. Complete end-to-end functional checks happen only after all Rust workstreams are rewritten, per the migration decision.
