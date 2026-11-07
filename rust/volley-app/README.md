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
POST /api/players/continuity-preview  provisional Track ID groups and 4s gap suggestions for selected run
POST /api/players/reid-native/preview   cached native ReID grouping and vetted references (read-only)
POST /api/players/reid-native/groups    read/merge/split candidate groups with source/owner checks (isolated root)
POST /api/players/identity/native      read or manually assign/unassign/exclude/restore IDs (isolated root)
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
POST /api/montage/native/plan          read-only auto/manual full-rally selection from chosen CPU run
POST /api/montage/native/start-from-run  isolated rendering of selected CPU-run rallies
POST /api/cpu/touch-preview          read-only RAVEL Touch proposals and motion-confirmed contacts for explicit run/video
```

These routes are implementations of persisted formats, not a claim of complete API parity. Rust project creation/update now queues native preparation. Compatible original MP4s are reused without recoding; other sources are converted by native FFmpeg to H.264/30 FPS/YUV420P and stored in the separate content-addressed development cache. One converter runs at a time; file locks prevent duplicate publication, status tracks FFmpeg progress, and cancellation is persisted per project until an explicit retry. Existing projects may use legacy storage conventions not fully supported; player catalog loading does not yet run legacy photo/identity backfills.

**Not implemented:** gold-marked episode filters, complete player/ball overlays or auto-follow/vertical crop, audio mixing with source sound, complete browser montage UI contract, cross-project adoption of old prepared results, general uploads, complete model/GPU inference and CPU touch inference, live ReID embedding extraction/manual photo operations, personal player statistics, on-demand fallback video segments, quick reel, full API, Rust/WASM browser UI and desktop packaging.

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
