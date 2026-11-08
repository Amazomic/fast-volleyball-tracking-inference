# volley-app — Rust-native Volleyball Studio

**Development only; full rewrite is not finished.** This is a standalone Rust backend (Axum/Tokio), not a Python subprocess bridge. It occupies a separate loopback port (`8875`) and has no production `/api/health` until the complete replacement exists. Existing Python/hybrid Studio on `8765` must not be interrupted.

## Workspace

The four maintained crates share `rust/Cargo.toml` and `rust/Cargo.lock`:

- `volley-core`: deterministic geometry and motion kernels (historic S01/S02a).
- `volley-media`: FFprobe metadata, rotation, target resolution, H.264/30 FPS readiness, schema-2 SHA256 prepared identities, JPEG preview rendering, bounded nine-frame raw-video windowing and original-quality montage planning.
- `volley-app`: standalone HTTP server, JSON project/catalog/history APIs, isolated preparation and manual montage jobs, audio upload, FFmpeg orchestration and byte-range MP4 streaming.
- `volley-inference`: native ONNX Runtime model sessions and one-clip execution; complete GPU streaming remains pending.

For Windows, use the isolated WSL development launcher from the repository root:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/start-volley-rust-dev.ps1 -CheckOnly
powershell -ExecutionPolicy Bypass -File scripts/start-volley-rust-dev.ps1
# Inspect existing records read-only without touching Python data:
powershell -ExecutionPolicy Bypass -File scripts/start-volley-rust-dev.ps1 -ReadOnly
```

The launcher accepts `-Distro`, `-Bind 127.0.0.1:8875` and an optional absolute WSL `-DataRoot` for sandbox writes. It refuses the Python production port `8765`, non-loopback binding and unsafe/relative data roots; `-CheckOnly` compiles without launching the server. Rust dependencies must already be cached for offline Cargo. The default isolated data root is `$HOME/.cache/volley-rust-sandbox` within WSL.

Alternatively, from repository root in WSL Ubuntu-25.04:

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
POST /api/players/library/native  read/create/rename athlete, SHA revision guard (isolated edits)
POST /api/players/continuity-preview  provisional Track ID groups and 4s gap suggestions for selected run
POST /api/players/reid-native/preview   cached native ReID grouping and vetted references (read-only)
POST /api/players/reid-native/groups    read/merge/split candidate groups with source/owner checks (isolated root)
POST /api/players/identity/native      read or manually assign/unassign/exclude/restore IDs (isolated root)
POST /api/players/statistics/native    preview/recalculate per-player CPU actions, versioned report (isolated root)
POST /api/project-create        experimental isolated registry only
POST /api/project-update        experimental isolated registry only
POST /api/project-archive/native  cancel this Rust project's preparation/montage and recoverably archive (isolated)
GET  /api/project-preparation?project_id=...  preparation states, enqueue missing when isolated
POST /api/project-preparation/retry    retry failed/preparation-paused jobs (isolated only)
POST /api/project-preparation/cancel   pause and cancel this project's preparation jobs
POST /api/montage/native/start         experimental manual montage job from original frames
GET  /api/montage/native/status        current montage stage, percent, outcome
POST /api/montage/native/cancel        cancel current montage (job_id)
GET  /api/montage/native/library?project_id=...  saved versioned Rust MP4s
GET  /api/montage/native/media?project_id=...&job_id=...  MP4 with HTTP Range
POST /api/montage/native/audio         multipart file upload (field file, max 90 MiB)
POST /api/montage/native/plan          read-only full-rally selection, athlete and gold marked/unmarked filters
POST /api/montage/native/start-from-run  isolated full-rally rendering with verified-player/gold focus
POST /api/rallies/native/marks       read/set gold rally flags (isolation + SHA revision required)
POST /api/quick-reel/native/select   register chosen local video paths without copying (isolated)
POST /api/quick-reel/native/start    start independent video preparation; GPU/CPU not yet available
GET  /api/quick-reel/native/status   latest Quick Reel stage or ?id=uuid
POST /api/quick-reel/native/cancel  cancel only the currently active Quick Reel
POST /api/inference/native/preflight    inspect VballNet/RAVEL sidecars, optionally open ORT sessions
POST /api/inference/native/clip-preview  one real 9-frame ONNX inference from a project prepared video
POST /api/inference/native/window-scan   multi-window FFmpeg→persistent ONNX scan (1–120 windows; no cache writes)
POST /api/inference/native/evidence/save  isolated immutable raw-model candidate snapshot (not GPU run)
GET  /api/inference/native/evidence?project_id=...&evidence_id=...  list/read bounded snapshots
POST /api/cpu/touch-preview          read-only RAVEL Touch proposals and motion-confirmed contacts for explicit run/video
```

These routes are implementations of persisted formats, not a claim of complete API parity. Rust project creation/update now queues native preparation. Compatible original MP4s are reused without recoding; other sources are converted by native FFmpeg to H.264/30 FPS/YUV420P and stored in the separate content-addressed development cache. One converter runs at a time; file locks prevent duplicate publication, status tracks FFmpeg progress, and cancellation is persisted per project until an explicit retry. Existing projects may use legacy storage conventions not fully supported; player catalog loading does not yet run legacy photo/identity backfills.

**Not implemented:** gold right-click context-menu UI, complete player/ball overlays or auto-follow/vertical crop, audio mixing with source sound, complete browser montage UI contract, cross-project adoption of old prepared results, general uploads, complete model/GPU inference and CPU touch inference, live ReID embedding extraction/manual photo operations, native report UI, on-demand fallback video segments, Quick Reel GPU/CPU inference and final export, full API, Rust/WASM browser UI and desktop packaging.

Development compilation/smoke probes are allowed. User-directed order: finish all Rust functionality **first**, then comprehensive tests/adaptation, then compare final Rust vs saved Python; rollback to `checkpoint/pre-rust-2026-11-07` if needed. See `docs/RUST_REWRITE_SCOPE.md` and `docs/RUST_MIGRATION.md`.

## Experimental native montage request

The original `/api/montage/native/start` is a **manual frame-range export API**; it still trusts caller-supplied offsets and is not evidence of a complete rally. Prefer the new CPU-bound `/api/montage/native/plan` (read-only) followed by `/api/montage/native/start-from-run` (isolated output), which validate the selected saved CPU run, reconstruct rally boundaries and select only stable IDs from that run. These are still **experimental routes**, not the production browser's full montage contract. Rally intervals are inferred, not proven serve-to-point ground truth.

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

## Experimental CPU-run rally selection

`POST /api/montage/native/plan` accepts a JSON object such as:

```json
{
  "project_id": "1234abcdef01",
  "run_id": "run_20261107_123456",
  "duration": 120,
  "selection_strategy": "attacks",
  "min_rally": 1.5,
  "min_confidence": 0.5,
  "gap_sec": 3,
  "context_sec": 1.5,
  "rally_selection_mode": "auto",
  "min_action_counts": {"attack": 1}
}
```

Read-only planning works against existing project/CPU result files. The response contains `candidates`, `selected`, selected duration and stable `rally_id` values. For manually chosen groups, change `rally_selection_mode` to `manual` and pass the saved `selected_rally_ids` from the same current plan; stale/foreign IDs are rejected. The server rechecks the exact named CPU run and cache ownership on every request. Automatic selections never shorten a detected rally to fill the duration.

`POST /api/montage/native/start-from-run` accepts the same selection fields plus optional `audio_file`, `audio_volume` and `encoder`, and performs video encoding **only** when `VOLLEY_RUST_DATA_ROOT` is an isolated sandbox that contains the relevant project/CPU snapshots. This route derives frames and time offsets server-side; no arbitrary timestamp override is accepted. The old manual `/start` route still trusts its explicit clip coordinates and should not be interpreted as CPU-verified. Both APIs are development-only. Full browser integrations, gold marking, player ReID and formal rendered-video tests remain outstanding.

## Experimental RAVEL-013 Touch preview

Read-only `POST /api/cpu/touch-preview` example:

```json
{
  "project_id": "1234abcdef01",
  "run_id": "run_20261107_123456",
  "video": "match.mp4",
  "threshold": 0.5,
  "smooth": 5,
  "min_gap": 10,
  "max_reach_ratio": 0.35
}
```

The route reads immutable RAVEL-013 probabilities from the exact run's inference cache and saves nothing. It returns time-smoothed `peaks` and `confirmed_contacts` from its chosen CPU game tracks if available; unverified player identities remain null. It requires the specific saved project/run/video mapping, not an arbitrary filesystem path. It does not use a Python subprocess or GPU. This is **only an algorithm diagnostic**, not a replacement for the complete CPU postprocessing command or final touch statistics.

## Experimental continuity preview

Use `POST /api/players/continuity-preview` on the standalone Rust development server (currently port 8875 by default):

```json
{
  "project_id": "1234abcdef01",
  "run_id": "run_20261107_123456",
  "video": "match.mp4",
  "extended_gap_sec": 4.0
}
```

It reads existing project-scoped inference/CPU player observations and exclusions. `continuity.proposed_links` are conservatively merged **candidate** groups for short gaps; `continuity.extended_suggestions` are unmerged proposals for longer gaps up to the chosen limit. Both remain `confirmed: false`; they are not personal identities or calibrated ReID percentages. No GPU work or writes occur. The Touch diagnostic also returns `unconfirmed_candidate_touch_counts` for physically validated touches, without modifying verified ID labels or persistent statistics. Cross-run identity consistency, ReID and manual confirmation UI remain outstanding.

## Experimental ReID archive preview

Read-only POST /api/players/reid-native/preview body:

~~~json
{
  "project_id": "1234abcdef01",
  "run_id": "run_20261107_123456",
  "video": "match.mp4",
  "threshold_percent": 85
}
~~~

Requires a chosen saved run and existing fresh player_identity_groups/*.npz embedding archive. Rust calculates cosine scores and candidate groups, honoring manual exclusions and reviewed owners. Existing trusted photo-album vectors provide suggested player matches for the project's roster; automatic assignment is disabled. No Python subprocess is used. ReID image encoder and uncached embedding capture, real manual linking UI, model parity checks and full integration tests are still outstanding.

## Experimental manual player identity persistence

`POST /api/players/identity/native` reads the selected project/video/inference and its verified player assignments:

~~~json
{
  "project_id": "1234abcdef01",
  "run_id": "run_20261107_123456",
  "video": "match.mp4",
  "action": "read"
}
~~~

The response includes `revision`, `groups`, `roster`, excluded/rejected IDs, assigned and unassigned counts. To approve an individual ID or a group, submit the **same** project/run/video and the `revision` returned by this read:

~~~json
{
  "project_id": "1234abcdef01",
  "run_id": "run_20261107_123456",
  "video": "match.mp4",
  "action": "assign",
  "track_ids": ["12", "56"],
  "player_id": "existing-project-athlete-id",
  "expected_revision": "sha256-from-read"
}
~~~

Use `group_id` instead of `track_ids` to approve the entire **saved**, non-stale ReID candidate group. `unassign`, `exclude`, and `restore` accept `track_ids` plus `expected_revision`; a reassignment from an already confirmed *different* owner additionally requires `allow_reassign:true`. A stale revision returns HTTP 409, never overwriting another tab's work. Edits are **only** allowed with an explicitly isolated development `VOLLEY_RUST_DATA_ROOT` and are saved under `projects/<project_id>/player_identities.json` there, preserving Python schema 2. The original production file is never written. Manual grouping UI, album photo enrollment and confirmed-statistics generation are separate remaining migration steps.

## Experimental manual ReID grouping

`POST /api/players/reid-native/groups` accepts a project/run/video with `action:"read"` and optional `threshold_percent` (55–99). Its response contains `groups`, `unmatched_ids`, `manual_excluded_ids`, and `revision` (SHA256 or `absent` when the baseline is generated but not yet saved). For example, to explicitly merge two Track IDs:

~~~json
{
  "project_id": "1234abcdef01",
  "run_id": "run_20261107_123456",
  "video": "match.mp4",
  "action": "merge",
  "track_ids": ["5", "12"],
  "expected_revision": "sha256-or-absent-from-read",
  "threshold_percent": 85
}
~~~

To split, change `action` to `split` and list the IDs to remove from candidate groups; no players are deleted. Each edit returns a **new** revision that must be used next time. Conflicting confirmed owners, overlapping tracks, excluded IDs, stale group catalogues and concurrent edits produce errors without writing changes. All group edits require isolated `VOLLEY_RUST_DATA_ROOT` and do not write the original project.

To then **confirm a saved group as a real player**, call `POST /api/players/identity/native` with `action:"assign"`, `group_id`, `player_id`, the current identity `expected_revision`, **and** `expected_group_revision` returned by the group endpoint. The group revision is checked again while holding the exclusive identity lock. A group is a candidate until this separate approval.

## Experimental native player statistics

Read-only preview for one specific saved CPU run:

~~~json
{
  "project_id": "1234abcdef01",
  "run_id": "run_20261107_123456_pp_example",
  "mode": "preview",
  "selected_videos": ["match.mp4"]
}
~~~

Call `POST /api/players/statistics/native`. The response provides `per_video` and `overall` totals, verified project player actions, unassigned and excluded counts, contact deduplication diagnostics, rally filtering and an input source fingerprint. Without selected_videos, all videos present in the chosen run are included.

To publish an immutable/versioned report **only in the isolated Rust development data root**, use `mode:"recalculate"`. The response includes `snapshot_file` and `snapshot_created`; repeated identical inputs reuse the prior report. The source CPU events, GPU player detection cache and original Python identity/photo store are not rewritten. A change in roster, assignments, ReID groups, source CPU evidence or the statistics algorithm invalidates the previous source fingerprint.

Only explicit reviewed Track IDs and a single unambiguous owner inherited through a manually approved candidate group are counted for a named athlete. Automatic appearance or geometric suggestions remain unassigned. The report is not yet wired to the production Players/Preview UI. Real-match quality tests and final parity remain deferred.

## Personalized whole-rally montage

The existing Rust routes `POST /api/montage/native/plan` (read-only) and `POST /api/montage/native/start-from-run` (renders only to the isolated Rust root) now accept these optional fields:

~~~json
{
  "project_id": "1234abcdef01",
  "run_id": "run_20261107_123456_pp_example",
  "selection_strategy": "attacks",
  "player_id": "existing-project-player-id",
  "player_action": "attack",
  "min_player_actions": 1,
  "duration": 120
}
~~~

Choose an actual player ID from the project's existing roster, not a name or a Track ID. A candidate is eligible only if **that verified player** has at least one deduplicated saved CPU attack inside the detected boundaries (or the specifically recognized serve anchor) of a complete rally. A manual selected_rally_ids request uses this same player-focused candidate set and rejects mismatching IDs; it will never crop a rally to a single spike. The API reports `matching_player_rallies`, player-specific action counts and ranking on the scenes. Changing source CPU events or manual identity assignments requires planning again. Inference-only runs and unconfirmed candidates do not qualify. The legacy UI is not yet connected to these fields.

## Experimental gold rally bookmarks

To list existing gold marks in a saved CPU run, call `POST /api/rallies/native/marks`:

~~~json
{"project_id":"1234abcdef01","run_id":"run_20261107_123456","action":"read"}
~~~

The response includes `marks` and `revision`, which is `absent` for an unsaved registry. Mark an existing complete rally by passing the **actual** `rally_id` from `POST /api/montage/native/plan` (or the forthcoming Results UI), the video name and the current revision:

~~~json
{
  "project_id":"1234abcdef01",
  "run_id":"run_20261107_123456",
  "video":"match.mp4",
  "action":"set",
  "rally_id":"24-character-actual-hex-id",
  "marked":true,
  "expected_revision":"revision-from-read",
  "gap_sec":3.0,
  "context_sec":1.5,
  "rally_min_sec":0
}
~~~

Use `marked:false` to remove the gold flag and the freshly returned revision for any follow-up edits. If another tab changed marks, the server rejects the stale revision (HTTP 409). **Write mode is disabled without a separate, writable `VOLLEY_RUST_DATA_ROOT`; the original Python project cannot be modified through this route.**

For `/api/montage/native/plan` or `/api/montage/native/start-from-run`, use `gold_filter:"marked"` to select only gold rallies, or `"unmarked"` to exclude them. Each candidate has `gold_marked` and retains its complete original source boundaries. The mark is bound to the selected CPU run and analysis version and survives harmless context padding changes. Player filters and manual selection work together with the gold filter; the client must refresh the candidate catalogue if an ID no longer matches.

## Experimental standalone Quick Reel staging

The isolated Rust quick-reel pipeline currently **prepares video only**. It does not reuse old GPU detections, trigger Python or pretend that a finished MP4 was created.

Call `POST /api/quick-reel/native/select` after the eventual native desktop file picker chooses actual local files:

~~~json
{"paths":["/mnt/c/Users/username/Videos/match.mp4"]}
~~~

The response returns new source selection IDs. Upload music first using `POST /api/montage/native/audio` multipart field `file`, then call:

~~~json
{
  "selection_ids":["32-character-hex-selection-id"],
  "audio_file":"track_uuid32.mp3"
}
~~~

to `POST /api/quick-reel/native/start`. Track progress through `GET /api/quick-reel/native/status` or `?id=<reel_id>`; cancel through `POST /api/quick-reel/native/cancel` with `{"id":"<reel_id>"}`. Quick Reel reuses **prepared MP4s only** (or skips conversion for analysis-ready sources), then reports `awaiting_native_inference` with `output:null` because native RAVEL/VballNet inference + fresh CPU and MP4 render are not yet ported. This is a development API, not the existing one-click browser Quick Reel. Only the latest scratch run is retained; original project files and Python Quick Reel history are untouched.

## Experimental native ONNX clip kernel

The native `volley-inference` crate uses ONNX Runtime 1.22's **dynamic** C ABI. Deploy an actual matching `onnxruntime.dll` (Windows) or `libonnxruntime.so` (Linux) and set `ORT_DYLIB_PATH`, including working CUDA/CuDNN provider dependencies for GPU mode. The current WSL build does **not** have a validated ONNX Runtime/CUDA shared library; compilation is not evidence that GPU execution is operational. There is no Python inference fallback.

To inspect the model contracts, call `POST /api/inference/native/preflight`:

~~~json
{"device":"cuda","gpu_device_id":0,"gpu_memory_limit_mb":0,"cpu_threads":8,"probe_runtime":false}
~~~

Setting `probe_runtime:true` additionally attempts to construct **real** VballNet and RAVEL sessions and returns provider-specific failures. It does not invoke GPU prediction.

After native video preparation has finished for a selected project, the bounded single-window proof-of-inference path is `POST /api/inference/native/clip-preview`:

~~~json
{
  "project_id":"1234abcdef01",
  "video":"match.mp4",
  "first_frame":600,
  "model":"ball",
  "device":"cuda",
  "gpu_device_id":0,
  "gpu_memory_limit_mb":0
}
~~~

For RAVEL use `"model":"players"`. This reads exactly nine prepared video frames using FFmpeg, builds normalized float model input, executes the actual ONNX graph through Rust/ORT and returns **unconfirmed raw** ball candidates or RAVEL Touch query maxima. All outputs are read-only and are **not** a complete player/ball tracker, finished model run or analyzed montage. Quality/performance and pixel-level comparison to the original Python decoder remain to be performed in the deferred final testing phase.
