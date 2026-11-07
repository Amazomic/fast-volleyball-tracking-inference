# Volleyball Studio — full Rust rewrite ledger

**Status:** ACTIVE — full Rust rewrite; acceptance and performance comparison happen after implementation.
**Decision:** 2026-11-07. **Branch:** `migration/rust-2026-11-07`.
**Handoff:** `docs/RUST_MIGRATION.md`. **Coverage:** `docs/RUST_REWRITE_SCOPE.md`.

> **User-approved replacement of the old strategy:** First rewrite the **entire application** in Rust, then test and adapt the integrated Rust version, and **only after that** compare it with the Python original. If the outcome does not justify migration, check out the protected Python checkpoint. S00/S01/S02a tests below are historical results, NOT progressive gates or justification for a hybrid rollout.

## 0. Immutable checkpoint and repository state



- Repository: `Amazomic/fast-volleyball-tracking-inference`.

- Local original checkpoint commit: `48e2e1fb1ec7811518cb10011944f09a001a33fb`; local annotated tag `pre-rust-migration-2026-11-07`.

- GitHub-published equivalent checkpoint: `aeaf7704ac4228affe04f8e50fbc52769c44b725`.

- Both checkpoint commits have identical Git tree: `deff7368e636c62360223d9904865e856808947f`; GitHub connector created a new commit object because direct HTTPS push had no credentials.

- **Remote rollback branch:** `checkpoint/pre-rust-2026-11-07` points to `aeaf770`; do not move this branch.

- Parent of the checkpoint: `46c7d92f1b8893bdbd3b3650eab511bccff427d4`.

- `origin/master` had advanced to `72f65a52b25e312c744064bbea08f86de14acfad` by 2026-11-07, including local-test-only PR workflows. This migration branch must incorporate those changes before seeking review.

- Runtime output, `results/`, `project/`, personal photos/videos, `*.bak.*` backups, and scratch files are **not** part of the checkpoint. Leave them on the workstation; do not delete them or include them in Git.

- Workstation: HOME-PC, Windows; project root `C:\Users\amazomic\Desktop\github\fast-volleyball-tracking-inference`. SentinelX is the preferred local operator.

- `master` is not a migration workspace. All stages happen on the branch; submit a PR and do not merge without local test evidence.



## 1. Target architecture and safety contract

1. **Complete Rust application**: project/profile/history management, video preparation, GPU inference, CPU processing, tracking/interpolation, Touch/rallies, ReID/players, photos/statistics, preview, video editing/montage/quick reel, API, jobs, storage and packaging. No Python server, CPU backend, or Python orchestration in the finished product.
2. **Browser UI included**: migrate application behavior from `scripts/volleyball_ui/static/` to Rust/WebAssembly. HTML, CSS and minimal browser bootstrap can remain as platform assets. Interim JavaScript during development is NOT a complete rewrite.
3. **No unnecessary rewrite of vendor runtimes**: CUDA/model providers (ONNX Runtime/OpenVINO) and FFmpeg remain third-party native binaries/libraries invoked from Rust. This does not count as retaining Python business logic.
4. **Preserve semantics**: project -> versioned cached GPU evidence -> versioned CPU interpretation -> players/rallies/stats -> original-quality montage. Preserve existing run formats, saved user projects, raw detections/IDs, manual priorities, golden configuration and RU/EN UI behavior.
5. **No premature cutover**: original Python application and `master` remain intact. Implement Rust independently; run it on a separate development port with separate disposable outputs when necessary. Do not run duplicate servers on 8765, stop live jobs, delete user media, or mutate old project data.
6. **Comparison after full implementation only**: earlier isolated parity or microbenchmarks are historical diagnostics. Measure quality and speed end-to-end on identical real videos and hardware only after the Rust product is fully implemented and functionally adapted.

References: `docs/ARCHITECTURE_INFERENCE_POSTPROCESSING.md`, `docs/UI_LAYOUT_POLICY.md`, `docs/DEVELOPMENT_WORKFLOW.md`, `docs/TESTING.md`.

## 2. Complete rewrite plan — replaces incremental test gates

Progress can be committed in coherent batches and compilation checked, but **no stage-by-stage approval, parity-gating or default switching**. First implement all workstreams, then integrated testing/adaptation, then compare against the original.

| Workstream | Original sources | New Rust ownership | Status |
|---|---|---|---|
| R00 — checkpoint and coverage | `scripts/`, `src/`, `ov/` | `docs/RUST_REWRITE_SCOPE.md` | BASELINE SAVED |
| R01 — API/job/process management | `scripts/volleyball_ui/server.py`, service modules | `rust/volley-app/` | PARTIAL: native JSON API, preparation and montage jobs, MP4 byte-range responses and preview; inference/CPU/quick-reel jobs pending |
| R02 — projects, profiles, caches | `scripts/volleyball_ui/projects.py`, `analysis_runs.py`, `prepared_videos.py` | `rust/volley-app/` | PARTIAL: projects, profiles, runs/history and autoqueued isolated prepared-video cache; cleanup pending |
| R03 — video preparation and model inference GPU | `src/inference*.py`, `scripts/volleyball_studio/analysis*.py` | `rust/volley-inference/`, `rust/volley-media/` | PARTIAL: native FFmpeg preparation and content-addressed cache; GPU inference pending |
| R04 — trajectories/tracks/Touch/CPU | `src/player_contacts.py`, `scripts/volleyball_studio/reprocess.py` | `rust/volley-core/`, `rust/volley-app/` | PARTIAL: IoU/motion, native RAVEL peak NMS, bounded raw RAVEL cache reader, stable Track ID association and motion/reach diagnostic; full CPU replay and extrema processing pending |
| R05 — ReID, players, rallies and statistics | `src/player_reid.py`, player services | `rust/volley-core/`, `rust/volley-app/` | PARTIAL: native track continuity, full-track cosine ReID from saved NPZ, vetted cached reference matches and read-only suggested groups, CPU rallies; image embedding inference, manual identity writes and confirmed player stats pending |
| R06 — preview, montage, audio, quick reel | `src/make_reels.py`, `scripts/volleyball_ui/*montage*.py`, `quick_reel*.py` | `rust/volley-media/`, `rust/volley-app/` | PARTIAL: original-source montage, native CPU-run-based manual/auto rally selection, music/export; gold markers, vertical crop, quick reel and UI pending |
| R07 — complete UI and localization | `scripts/volleyball_ui/static/` | `rust/volley-web/` (Rust/WASM and assets) | NOT STARTED |
| R08 — packaging, launch and deployment | `start-volley-ui.cmd`, `Dockerfile` | standalone Rust launch/deployment | NOT STARTED |
| R09 — **after R01–R08** full tests and adaptation | All E2E workflows | integrated Rust application | DEFERRED |
| R10 — **after R09** comparison and choice | Frozen original Python vs Rust | quality/perf report and revision choice | DEFERRED |

Detailed functional inventory, required file formats and parity targets: `docs/RUST_REWRITE_SCOPE.md`. A stub, Python bridge or unimplemented HTTP route is not “complete.” Progress commits must state missing functionality. Independent compilation or smoke checks are debugging aids, not final acceptance.

## 3. Final evaluation and rollback protocol

1. Finish the Rust application end to end, including GPU inference, original-quality video export and every existing UI workflow, without Python backend dependence.
2. **Only then** run comprehensive API, CPU/GPU, UI, projects, ReID, touch, rally, statistics, cancellation, cache, rendering and backward-compatibility tests. Adapt and fix the complete Rust version.
3. **Then** compare both on exactly the same videos, model versions, machine/GPU, saved observations and equivalent settings. Evaluate speed, RAM/VRAM, touch precision/recall, player ID accuracy, rally boundaries, video quality and UI responsiveness. Record real measurements rather than assuming Rust wins.
4. Switch production only when the final result is worthwhile. Otherwise restore `checkpoint/pre-rust-2026-11-07` at `aeaf7704ac4228affe04f8e50fbc52769c44b725`, or another explicitly selected known-good Python commit. Preserve all local projects, media and caches and retain the Rust development branch for future work; never force-reset `master`.
5. Draft PR #7 stays unmerged until this final decision. No paid hosted CI assumption; local testing after completion is documented.

**Historical record below is retained unchanged.** Earlier “next gate” comments in S01/S02a are superseded by this decision.

## 4. Stage log (append, never silently rewrite historic results)



### 2026-11-07 — checkpoint complete

- Local checkpoint `48e2e1f`, GitHub-published commit `aeaf770`, exact same tree `deff7368`.

- 73 source/docs/test files committed. Backups, personal media and generated data excluded.

- Direct `git push` returned `could not read Username for 'https://github.com'`; GitHub connector created the exact equivalent tree and two remote branches instead.

- Local migration branch was aligned to the published GitHub commit by a compare-and-swap ref update after verifying identical trees.

- Rust toolchain initially unavailable (`rustc` and `cargo` absent from `PATH`). Windows `winget` has an official `Rustlang.Rustup` package, but installation/build has **not been verified**.

- Remote `master` contained seven newer commits; merge into the migration branch is being resolved, keeping the richer current contributor contract and the detailed local UI rules.



### S00 local tests and merge

**Status:** Local targeted gate PASSED, full matrix NOT TESTED; stage GitHub commit `8ebca9f64f5a4fe57999e0c3af01880a34d13075` (same tree as local merge `04d6810190e9f8ed90e376b10d7b18f25c5d2126`).



- Python policy: `python -m unittest discover -s tests -p test_ci_quality.py` — PASS (11 tests).

- Layout: `python -m unittest discover -s tests -p test_ui_layout_contract.py` — PASS (6 tests).

- Golden defaults: `python -m unittest discover -s tests -p test_cpu_golden_defaults.py` — PASS (4 tests).

- UI smoke: `node tests/players_workspace_layout.cjs` — PASS in Chrome at 1440x900, 1440x1000, 1920x1080, 800x900, 480x812.

- JS contracts: `node --test tests/diagnostics_controls.cjs tests/players_render_stability.cjs` — PASS (2 tests).

- `git diff --check` — PASS.

- Initial baseline before test repair had 2 obsolete expectations: CPU golden fixture asserted 0.35 despite documented revision 4 (0.44); policy fixture mocked previous revision 3, which equals 4-1 and bypassed the revision guard. Test expectations repaired without changing the CPU defaults or the policy implementation.

- Real-video accuracy benchmarks, GPU execution, full Python suite, Docker build and all browser scenarios — **NOT TESTED** in S00.

- Latest `origin/master` brought seven newer commits and a richer contributor contract. Conflict in `AGENTS.md` resolved by preserving its newer contract and moving pre-existing detailed UI stability rules to `docs/UI_LAYOUT_POLICY.md`.

- `origin/master` / PR golden policy requires properly annotated evidence when changing the CPU golden manifest. The pre-migration checkpoint contains revision 4; do **not** fabricate evidence or weaken the gate to merge it. Check PR policy separately.





### S01 Rust kernel and parity — 2026-11-07

**Status:** S01 local acceptance PASSED and GitHub-published commit `2d251b0c86e53fdd0ee62b4b5133cac06585d3c3` (same Git tree as local `ed5bb80114efa09e830619e3374ba232f45e52d7`). No default behavior change.



- New crate `rust/volley-core` (Rust 1.99.1; versioned ABI 1, `cdylib` plus `rlib`), implemented `bbox_iou` with the exact epsilon/degenerate-box semantics of CPU replay.

- Added `scripts/volleyball_studio/rust_core.py` ctypes adapter, `tests/test_rust_core_contract.py` parity tests and optional call site in `scripts/volleyball_studio/replay_player_tracking.py`. Default `VOLLEY_RUST_IOU` unset uses Python. Set to `1` only after building. Missing DLL or incorrect ABI fails explicitly; never silently substitutes an algorithm.

- Build: `cargo test --manifest-path rust/volley-core/Cargo.toml` — PASS (4 Rust tests); `cargo build --manifest-path rust/volley-core/Cargo.toml` — PASS; `cargo fmt --manifest-path rust/volley-core/Cargo.toml -- --check` — PASS.

- `VOLLEY_RUST_REQUIRED=1 python -m unittest discover -s tests -p test_rust_core_contract.py` — PASS (3 Python tests, including 2,506 fixed and seeded random bbox pairs across compiled Rust and original Python).

- `python -m unittest discover -s tests -p test_cpu_player_identities.py` — PASS (3); `python -m unittest discover -s tests -p test_ci_quality.py` — PASS (11); `python -m py_compile ...` and `git diff --check` — PASS.

- Windows 32-bit Python 3.14 cannot load x86_64 Rust DLL: separately tested explicit diagnostic `Rust volley-core DLL requires 64-bit Python on Windows` — PASS. Parity was tested with `python` 3.11.9 **64-bit**. Keep 32-bit Python on Python fallback or install Python 3.14 x64, do not misrepresent this as production Python 3.14 testing.

- Microbenchmark (same Windows host, 80,000 scalar calls, seeded inputs): Python IoU **0.0477 s**, cached Rust ctypes IoU **0.1264 s**, identical checksum **270.905997457**. An earlier uncached adapter took **15.3821 s** due to repeated file checks; caching fixed it. Scalar FFI is still slower than pure Python for this tiny operation, so do **not enable Rust by default**. Future stages should move whole loops/batches across the boundary and benchmark end-to-end CPU replay before opting in.

- `tests/test_player_three_second_gap.py` — **NOT TESTED** on this Python 3.11 environment: import fails because `cv2` is missing. No runtime library installs, GPU jobs, media, or user caches were modified to force this test.

- Full E2E project replay, annotated real-match accuracy, Docker and GPU — **NOT TESTED**. S01 implements only deterministic geometry/parity, not an overall faster tracker.

- Rollback: leave `VOLLEY_RUST_IOU` unset; if necessary restore the entire branch to S00 remote `8ebca9f64f5a4fe57999e0c3af01880a34d13075` (retain user data and checkpoint).





### S02a Rust batched motion validation — 2026-11-07

**Status:** Local deterministic-parity gate PASSED; S02a GitHub commit `4ccb60bbf907a586cfb8d6d88b1c0c4d2156e412` (same tree as local `0e45b46e97c24289e52e9391f0c66664c1fe3a46`). S02 overall remains INCOMPLETE pending annotated-match evaluation and other touch-peak logic.



- Scope: move the gravity-corrected velocity fits and motion-break selection for RAVEL touch evidence to `rust/volley-core/src/motion.rs`. Safe C ABI `volley_ball_motion_batch_v1` consumes saved ball observations and all target peak frames in one call. No model, raw cache, golden thresholds, identity assignment or montage data is changed.

- The existing `scripts/volleyball_studio/reprocess.py` preserves the Python path unless the explicit environment flag `VOLLEY_RUST_MOTION=1` is set. `_ravel_contacts` batches proposed peak frames for one Rust call rather than copying the ball track for each candidate.

- Rust uses parameter values from `src/player_contacts.py` passed by Python in `scripts/volleyball_studio/rust_core.py`, avoiding diverging numerical defaults. The output schema, rounding, frame selection, rejection of gravity-only flight and touch gates were preserved.

- `cargo test --manifest-path rust/volley-core/Cargo.toml` — PASS (7 Rust tests, including the 4 S01 tests). `cargo build` — PASS. `cargo fmt --manifest-path rust/volley-core/Cargo.toml` applied formatting and `cargo fmt -- --check` is required before commit.

- `VOLLEY_RUST_REQUIRED=1 python -m unittest discover -s tests -p test_rust_motion_contract.py` — PASS (4 Python tests, 90 seeded noisy trajectories plus straight/gravity/reversal/speed-gain/sparse tracks, default-vs-Rust identical dictionaries, and malformed input diagnostics).

- Existing `python -m unittest discover -s tests -p test_ravel_touch_motion.py` — PASS (7 tests); with `VOLLEY_RUST_MOTION=1` — PASS (7 tests). Identical input touch results in both modes.

- Synthetic microbenchmark on same HOME-PC: 500 saved trajectory observations, 96 candidate frames per run, 25 runs; Python **0.1227 s**, Rust batch **0.0179 s** (~6.9x faster on this microbenchmark), with exact matching evidence. This is **NOT** a measured full-video / production speedup.

- Real annotated-match acceptance, GPU inference, CPU full quality suite, 64-bit Python 3.14 environment and Docker — **NOT TESTED**. Do not change golden defaults or automatically enable Rust. Retain original Python path for rollback by simply unsetting `VOLLEY_RUST_MOTION`.

- Next gate S02b: port the remaining Touch candidate/peak or CPU geometry work only after repeatable baseline fixtures are available, and compare actual Touch precision/recall and event timing on the same cached inference. Do not equate synthetic equality with real-match quality.



## 5. Handoff to the next AI / engineer

1. Inspect branch, tracked status, `docs/RUST_REWRITE_SCOPE.md`, and this ledger. Work in `migration/rust-2026-11-07`, do not discard untracked media/backup files.
2. **Implement the entire Rust application before the formal test/adaptation/comparison phase.** No intermediate opt-in rollout, Python fallback as the target architecture, or blocked progression due to per-kernel benchmark.
3. Preserve public API contracts, old files/project history, all GPU inference and CPU features, 2+ language UI, cached media, cancellation, player identity workflow, montage, quick reel, presets and settings.
4. Rust-native application owns orchestration. FFmpeg and model runtime can be external. Frontend application logic must become Rust/WASM, not remain the previous JavaScript permanently.
5. Checkpoints are allowed: commit and publish coherent incomplete work, mark gaps accurately in `docs/RUST_REWRITE_SCOPE.md`; compilation checks are okay. Do not call placeholders complete. Do not touch the production `master` or existing job.
6. Once ALL parts are implemented, comprehensively test/adapt; only then compare real project outcomes to the Python checkpoint and decide cutover/rollback. No fabricated metrics.
7. Publish via GitHub connector if local `git push` still has no credentials. Draft PR: https://github.com/Amazomic/fast-volleyball-tracking-inference/pull/7.

## 6. Superseding decision — 2026-11-07

- User explicitly required full rewrite first, tests/adaptation second, comparison third and Git rollback if unsatisfactory.
- S00, S01 and S02a work survives as reusable code; their opt-in hybrid is not the desired final product.
- Existing running hybrid service is not proof of full Rust completion. No production cutover is authorized.
- This decision changes development order only, not data safety or acceptance standards.

## 7. R01 initial implementation checkpoint — 2026-11-07

**Status: PARTIAL (NOT A COMPLETE RUST APPLICATION).** Started an independent Rust backend at `rust/volley-app/` using Axum/Tokio, with explicit incomplete-build status and a separate loopback-only development port (8875). Its `GET /api/projects` reads `results/projects/projects.json` natively and preserves existing project records without invoking Python or writing user data. `GET /internal/rewrite-status` returns `migration_complete=false` and `production_ready=false`. The app deliberately does **not** expose the normal `/api/health` endpoint, so it cannot masquerade as production API v28. Exclusive port binding rejects duplicate listeners.

- Linux WSL cargo toolchain available and used to compile dependencies; `cargo check --manifest-path rust/volley-app/Cargo.toml` **PASSED**, with one unused-import warning corrected afterward.
- All remaining server endpoints, persistent jobs, GPU inference, full postprocessing, montage and Rust/WASM UI: **NOT IMPLEMENTED**. Full tests, real-video metrics and quality comparison: **DEFERRED** until full rewrite, per user decision.
- The existing service on `127.0.0.1:8765`, old Python checkpoint and saved projects were not modified by this checkpoint. The new Rust server has not replaced or taken over production.

## 8. Full rewrite continuation: native project/catalog/history/media layers — 2026-11-07

**Implementation status: PARTIAL; application not ready to replace Python.**

- Consolidated new Rust code into a three-crate workspace at `rust/Cargo.toml` with **one** `rust/Cargo.lock`: `volley-core` (historical Rust kernels), `volley-app` (new Axum/Tokio API) and `volley-media` (native FFprobe/media metadata).
- `rust/volley-app/src/projects.rs`: project registry summaries in the actual browser `{"projects":[...]}` shape, source video availability/cached-analysis pointers, read-only source video catalog, and native create/update of project registries. Experimental writes require `VOLLEY_RUST_DATA_ROOT` pointing at an **isolated absolute directory** outside the original `results/`, and origin-checked POST. Registry writes use a dedicated file lock and atomic JSON files. This does **not yet queue video preparation**.
- `rust/volley-app/src/catalog.rs`: native global profile listing, athlete library listing, source video listing, resource estimates and `GET /api/video-info`.
- `rust/volley-app/src/history.rs`: native analysis/experiment timing history built from saved complete runs and valid stage records.
- `rust/volley-app/src/analysis_runs.rs`: native GPU run and CPU snapshot/version enumeration, run renames/hidden IDs, cache/source ownership validation and deterministic legacy CPU-setting hashes. Does **not** alter saved project files or materialize branches.
- `rust/volley-app/src/storage.rs`: JSON handling, no-traversal identifiers, origin checks, isolated data root guard and atomic writes.
- `rust/volley-media/src/lib.rs`: FFprobe metadata (rotation, display/coded size, duration, auto analysis width), exact H.264/YUV420P/30 FPS/AAC/no-rotation readiness filter for skipping unnecessary transcodes. External FFmpeg/FFprobe remain native tools; **not Python**.
- `rust/volley-app/src/main.rs`: dev-only loopback listener on port `8875` (configurable), no fake production `/api/health`, route list and accurate `migration_complete=false` flag. Existing Studio on port `8765` remains untouched.
- Verified after code changes in Ubuntu-25.04: `cargo fmt --all --manifest-path rust/Cargo.toml -- --check`, `cargo check --workspace --manifest-path rust/Cargo.toml` and `cargo build --workspace --manifest-path rust/Cargo.toml` — **PASS**. Full functional/API/browser tests and behavior validation are **DEFERRED** until the full rewrite, per user instruction.
- **Not attempted by design:** full Python-vs-Rust parity, real annotated match comparison, GPU inference, FFmpeg renders, all-app E2E and production cutover. These occur after implementation of all workstreams under the owner decision. Native Rust registry creation is isolated from the existing user data until that stage.
- Outstanding compatibility concerns for the later adaptation phase: legacy imported player identities and source aliases, project preparation queue on create/update, source metadata refresh semantics, CPU run manifest variations, and full browser API coverage.

## 9. Native Rust video-preparation lifecycle — 2026-11-07

**Status: PARTIAL IMPLEMENTATION; NOT the complete Rust application.** This work continues R01–R03 under the user-directed **finish whole Rust rewrite first → integrate/test/adapt → compare Python** sequence.

- `rust/volley-media/src/preparation.rs`: new content-addressed video-preparation plan, schema-2 fingerprint (canonical source path, byte size, nanosecond modification time, requested width, FPS 30) and Python-compatible sorted/ASCII-escaped SHA256 cache identifier. Detect already analysis-ready MP4 and skip transcode entirely. Cached prepared MP4 is reused only when the saved identity and ready status match and the file is nonempty.
- `rust/volley-app/src/preparation.rs`: Rust/Tokio one-at-a-time FFmpeg queue and per-project subscribers; shared source identity deduplication across projects; exclusive prepared-output file lock across Rust processes; unique partial filenames; FFmpeg progress parsing; atomic ready manifest publication; cancellation and cleanup of spawned child process; errors persisted to isolated dev status files. Input videos remain read-only.
- The preparation status endpoint returns the established `project_id/videos/ready/total` response structure, including per-video `state/progress/width/fps/error/size_mb/preparation`. Explicit cancel persists a per-project pause marker until a deliberate retry so refresh/polling cannot restart cancelled work.
- New/updated Rust sandbox projects enqueue preparation automatically; if background scheduling fails after the project record was durably committed, do not turn it into a create failure that would produce duplicate projects on retry.
- Development writes require an absolute `VOLLEY_RUST_DATA_ROOT` outside **the entire original repository**, not merely outside `results/`. The root is resolved and checked before directory creation. Existing Python UI and data on 8765 remain untouched. The default read-only Rust server cannot launch preparation.
- Routes: `GET /api/project-preparation?project_id=...`, `POST /api/project-preparation/retry`, `POST /api/project-preparation/cancel`. These are still experimental Rust routes, not a claim of all-API completeness.
- Rust modules use native FFprobe/FFmpeg commands, **not Python**. Cached source metadata/probe plans are reused while source size/mtime remain identical; no expensive GPU inference occurs during video normalization.
- **Development diagnostics only:** `cargo fmt --all`, `cargo fmt --all -- --check`, `cargo check --workspace` and `cargo build --workspace` in WSL Ubuntu completed; formal functional, UI, GPU, cancellation under real FFmpeg load, cross-project historical-cache adoption and Python-vs-Rust comparisons remain **DEFERRED until whole rewrite** by the user's request.
- Remaining: model inference, tracking, ReID, GPU/CPU job orchestration, old-project prepared-cache promotion, media streaming, preview/UI, montage/reels, packaging and full end-to-end verification.

## 10. Rust-native HTTP preview and prepared MP4 streaming — 2026-11-07

**Status: PARTIAL — independent Rust handlers exist; whole application remains INCOMPLETE.**

- `rust/volley-media/src/preview.rs`: Rust-native frame request validation (source/analysis mode, selected width, 30 FPS frame index), ffmpeg image-to-pipe invocation with a 60-second timeout and bounded JPEG output. Original source video is never overwritten.
- `rust/volley-app/src/playback.rs`: `GET /api/video-preview` returns JPEG directly and reads/writes a content-addressed preview cache. With the default read-only Rust mode it only reads preexisting cache and generates misses in memory; experimental writes go strictly to the isolated Rust data root.
- `GET /api/video-view` reports prepared-media availability and player-compatible playback fields; `GET /api/video-media` streams the validated prepared source or cached MP4 from Rust/Tokio with `Accept-Ranges`, `Content-Range`, single-range HTTP 206 and unsatisfiable-range HTTP 416. Player/project membership is checked before serving media; unprepared videos are not silently replaced by the raw original.
- Concurrent FFmpeg preview decodes are bounded by a two-request semaphore. Frame caches derive from original name, source size and nanosecond mtime; media requests never invoke Python.
- Added direct dependencies on tokio-util and percent-encoding for bounded asynchronous stream I/O and safe URL construction; Rust media crate uses Tokio for FFmpeg timeout/process management.
- Development compiler checks are permitted; integrated functional/API/browser tests and annotated Python-vs-Rust evaluation remain **DEFERRED until complete Rust rewrite**, as expressly requested.
- **Not implemented:** original-project legacy prepared-cache migration, fallback segment encoding, player/rally overlays, other preview analysis endpoints, UI/WebAssembly, GPU models, final montage/reel pipeline, comprehensive API compatibility.

## 11. Native-source manual montage engine and sandbox API — 2026-11-07

**Status: PARTIAL implementation only.** This is another part of the complete rewrite; **do not switch the working Python application or declare montage functionality complete**.

- `rust/volley-media/src/montage.rs`: Rust types for manually selected clips and original-recording offsets; exact source seek is `analysis_offset_sec + start_frame / 30`, segment lengths are inclusive end frames and use the native FFprobe nominal frame rate. No use of resized inference/video cache as render input. H.264 high-quality libx264 CRF15/fast CPU path, optional NVENC CQ17/p5 path and fallback. Single clip/concat demuxer stream-copy when all segments are compatible; mixed formats use an FFmpeg concat filter, preserve the largest native source pixel dimensions and pad differing aspect ratios. Optional looped/faded soundtrack audio is muxed without re-encoding the finished video stream.
- `rust/volley-app/src/montage.rs`: bounded manual selected-clips request (80 scenes, up to 600 seconds); project/video membership verification, bounded seek and audio file validation, isolated Rust-only output path under `projects/<id>/montages/<job>/`, per-job `selection.json`, single job slot, progress based on FFmpeg pipe, GPU-to-CPU fallback without backwards progress, explicit cancellation/child termination, safe error tail buffering, durable versioned outputs, saved montage library and byte-range MP4 media access.
- New experimental endpoints: `POST /api/montage/native/start`, `GET /api/montage/native/status`, `POST /api/montage/native/cancel`, `GET /api/montage/native/library`, `GET /api/montage/native/media`, `POST /api/montage/native/audio` for bounded multipart music upload. Uploaded names are generated identifiers, never raw client-supplied paths. Sandbox read/write protection remains in effect. `/api/highlights` and the legacy UI contract have **not** been replaced yet.
- Important remaining work for this pipeline: derive manual scene boundaries/rally IDs and offsets from saved CPU/inference evidence rather than trusting caller-supplied frame ranges/offsets, implement automated score/action/marked filters, track/ball follow framing, preserve user-selected rally sequences, old montage/quick-reel APIs and browser controls, add project-level cancellation/deletion integration and proper handling of cross-run exports.
- **Verified for development:** `cargo fmt --all`, `cargo check --workspace` and `cargo build --workspace` on WSL Ubuntu-25.04 completed without warnings or errors. **Not yet run:** real-video montage generation with the new endpoints, GPU encoder fallback under real load, final rendered quality measurements, UI regression, GPU inference, full automated end-to-end functional validation, or Python-vs-Rust comparison. These remain deferred until full implementation per user instruction.
- The working Studio on `127.0.0.1:8765`, original Git `master`, checkpoint, project videos and analysis caches are unchanged by this migration work.

## 12. Native CPU-evidence rally reconstruction and selection — 2026-11-07

**Status: PARTIAL. Not a final replacement for Python rally/UI processing.** This stage implements the selection core before the global E2E test phase requested by the owner.

- `rust/volley-core/src/rallies.rs`: standalone Rust rally grouping from classified ball-track intervals, serve/contact events and saved ranking observations. Neighbouring rally fragments can merge within 3 seconds by default, except across a confirmed technical-return segment or a reliable serve event. Context padding respects nearby rallies and next serves, without claiming the last ball track is proof of point end. Ranking and action counts are derived from saved observations, with per-track feature fallback for old runs. Selection uses intact interval frames; duration budgets do not trim an individual rally. Stable `rally_id` is a SHA256 hash of Python-compatible serialized source video/cache/bounds/track IDs. This Rust core runs without a Python subprocess.
- `rust/volley-app/src/rallies.rs`: read-only API `POST /api/montage/native/plan`. Mandatory `project_id` and `run_id`; resolve only a run visible in the project's Rust run registry (including CPU snapshots), validate per-video cache manifest/source, and read `analysis/game-tracks.json`, `events.json` and optionally `ranking.json`. Automatic mode requires ranking, applies selectable strategy, minimum detected duration/confidence, per-action thresholds and a strict total-time cap. Manual mode admits only `selected_rally_ids` from the same selected cache; its candidates include unranked intervals. Original project video order is preserved. No user-supplied frame/offset is trusted by this route.
- `rust/volley-app/src/montage.rs`: new experimental `POST /api/montage/native/start-from-run` endpoint. It derives clips from the server-selected CPU-run candidates and passes verified frame bounds and analysis offsets into the native-source Rust FFmpeg montage engine. Rendering remains restricted to an isolated writable Rust dev root. The legacy `/api/montage/native/start` still exists for raw manual ranges; it is **not** a validated rally-selection endpoint.
- Updated `rust/volley-app/src/main.rs`, the Rust crate dependency graph and API inventory. These routes are not wired to the production browser and do not switch the app on port `8765`.
- **Remaining**: complete postprocessing Touch/retracking, ReID/player identity, player statistics, persist/reload gold-marked rally filtering, UI rally and montage workflows, event parity and accuracy adaptation, inference/quick reel/render follow-ball/vertical overlays. The saved event classification is consumed, not recomputed by this stage.
- The current build only verifies compilation/formatting; comprehensive model/video tests and accuracy comparison remain deferred until the complete Rust rewrite as requested.

## 13. Rust-only RAVEL Touch peak and motion diagnostic — 2026-11-07

**Status: PARTIAL CPU implementation; not a full project CPU replay.** Migration remains intentionally unmerged into production until all modules are rewritten and evaluated.

- `rust/volley-core/src/touch.rs`: native Touch candidate selection with strongest per-frame RAVEL probability, NumPy-like float32 centered smoothing, stable strongest-first non-maximum suppression, actor assignment near a shifted temporal peak, then verified tracking-ID association by non-interpolated bounding-box IoU >= 0.3 within ±2 frames. Unknown IDs remain unknown; raw model IDs are never treated as saved stable player identities.
- A confirmed RAVEL contact additionally requires a reachable ball observation within ±2 frames and a gravitationally corrected discontinuity in velocity/direction, using the existing native `rust/volley-core/src/motion.rs` computation. A ball flying straight through a player rectangle alone is not a touch. Nearby positives are collapsed into one event. Output `unknown_touch` does not invent a play technique.
- `rust/volley-app/src/touch_preview.rs`: new read-only `POST /api/cpu/touch-preview` for an explicit `project_id`, `run_id`, `video` and optional `threshold`, `smooth`, `min_gap`, `max_reach_ratio`. Resolves the chosen run through saved project registry, validates the immutable inference ownership and RAVEL-013 manifest, streams the saved raw JSONL as bounded frame candidates, associates stable IDs from saved `players.json` or CPU `players.replay.json`, and checks the selected CPU version's classified `game-tracks.json` observations when present. Reuses the saved neural predictions without invoking Python or GPU.
- Diagnostic reports include raw proposal counts, candidate peaks, verified Track IDs, motion-supported contacts and an explicit `motion_evidence_available` flag. If a GPU-only run has no CPU tracks, only candidate peaks are returned; the diagnostic does not fabricate motion-validated contacts. Request concurrency is bounded to one run per development server.
- This endpoint is not persisted back to the original project and does not alter `analysis/events.json`, personal player stats or montage results. No full threshold-dependent postprocessing snapshot is published yet. Fine-grained geometric technique classification, ball extrema, native player tracking, ReID and whole CPU pipeline orchestration remain outstanding.
- Source Python behavior inspected: `scripts/volleyball_studio/touch_peaks.py`, `scripts/volleyball_studio/reprocess.py`, `src/player_contacts.py`. As agreed, development `cargo fmt/check/build` verifies compilation only; real-match accuracy/performance and full E2E parity tests remain deferred until the entire Rust rewrite.

## 14. Native provisional player continuity and safe Touch grouping — 2026-11-07

**PARTIAL implementation.** The target remains a standalone Rust Studio; no migration to the active UI or real player auto-enrollment is authorized by this work.

- `rust/volley-core/src/continuity.rs`: Rust-only geometry tracklet grouping ported from `scripts/volleyball_studio/player_continuity.py`. Extracts first/last frame, normalized box center/height and observation count from non-interpolated, score >=0.3 player detections. Short links require at least 5 observations per Track ID, a <=20-frame gap at 30 FPS, height ratio <=1.18, center distance <=0.040, score >=0.75 and **reciprocally unique** best candidate with a 0.12 margin over competing candidates. Linked tracks cannot temporally overlap. Neither linked candidate groups nor their scores are confirmed real-world person identities.
- Optional extended-gap suggestions last up to **4 seconds**. This is a **different, stricter, read-only suggestion path**: at least 10 observations per track, height ratio <=1.09, normalized center distance <=0.018, score >=0.82, reciprocal uniqueness, plus endpoints at the terminal/head of existing short-gap groups. Long-gap suggestions **never merge Track IDs**, cannot overwrite manual assignments, and do not create player IDs. A four-second track absence does not prove the returning person is the same.
- `rust/volley-app/src/player_continuity.rs`: `POST /api/players/continuity-preview` reads one explicitly selected `project_id`, `run_id` and `video` after verifying project/run/cache ownership. Uses saved `players.replay.json` when available, or the original `players.json`. Reads inference-scoped manually excluded IDs from `results/projects/<project>/player_identities.json` without any write or migration side effect. Response includes schema-1 compatible `proposed_links`, `groups`, `track_to_candidate`, plus separate `extended_suggestions`. Input-size limits and one-at-a-time requests protect memory/desktop responsiveness.
- `rust/volley-app/src/touch_preview.rs`: native read-only Touch diagnostic now checks the saved exclusions before associating a Touch peak to a stored Track ID. For motion-confirmed touches it returns `unconfirmed_candidate_touch_counts` and `touches_without_candidate_id`, keeping the direct stable Track ID unchanged. **These candidate-group counts are not player statistics and must not be used to auto-assign identities.**
- Reused the Rust motion validation from the previous stage. No additional Python subprocess or GPU inference. Primary Python service, original project media, result snapshots and immutable checkpoint remain untouched.
- Still missing: rebuilding original detector IDs with proper 3–4s temporal hysteresis, appearance-based ReID, saved/photo reference matching, manually confirmed group-to-person linking, player touch/statistics snapshots, complete CPU pipeline and UI controls. Compilation and formatting checks are development diagnostics; final real-match testing/adaptation/parity comparison happen only after complete rewrite, per owner instruction.

## 15. Rust-native cached ReID comparison — 2026-11-07

**PARTIAL. Read-only evaluation, not yet a native image embedding encoder or manual identity editor.**

- rust/volley-core/src/reid.rs: native L2-normalized cosine matrix of every available Track ID, top-three nearby compatible matches, threshold 55–99, complete-link clustering without indirect bridges. Co-visible players, conflicting reviewed owners, and ambiguous competing identities cannot be silently merged. Very short tracks need stronger evidence; color fallback embeddings require stronger cosine and close temporal/spatial endpoints. Cosine percentages are not calibrated identity probabilities.
- rust/volley-app/src/reid_npz.rs: bounded ZIP/NumPy archive reader, with float16/32/64 vectors, Unicode ID lists and model fingerprints. Rejects object/pickle arrays, corrupt dimensions and oversized archives. Reuses existing original cached embedding matrices, without running Python or GPU.
- rust/volley-app/src/reid_native.rs: new POST /api/players/reid-native/preview requires a known project, run and video, verifies cache source/model fingerprint, honors excluded IDs, manual splits, manual groups and reviewed player assignments. Reads vetted player album embedding caches for same-model photo reference suggestions only when album index/model signatures match. Self-references are suppressed. Results contain groups, nearest tracks, ranked possible owners, reference counts, and auto_assign_enabled:false. No writes to original inference or identity files.
- Remaining: native OpenVINO photo embedding extraction and new NPZ publication, photo album editing, persisted manual identity links and player-statistics updates, full ReID workflow UI, GPU and CPU pipelines, quality/performance acceptance. No E2E comparisons until migration is complete under the user's plan.
