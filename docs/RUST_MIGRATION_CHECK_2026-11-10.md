# Rust migration audit — 2026-11-10

**Acceptance: NO-GO (stage 84/85; stage 85 remains pending).**

## Scratch cleanup

Two older browser-uploaded `drop_<uuid>.mp4` Quick Reel copies and their
matching selection metadata were deleted after validating the latest run
request and protecting its currently running source. Reclaimed exactly
**3,982,779,242 bytes** (~3.98 GB decimal). The current ~1.9 GiB video and
SHA-validated ~3.9 GiB prepared cache were kept. Original media,
music, checkpoints, project data, model weights and the active GPU task
were not modified.

Manual, dry-run-first cleanup: `scripts/ci/prune_stale_quick_reel_uploads.py`.
The script refuses deletion if the expected latest ID has changed.
The Rust application also now performs conservative cleanup of stale
browser-upload copies at the *start of the next run*, under its existing
exclusive Quick Reel lock. Regression covers old/current, recently
selected and original linked files:
`quick_reel::tests::new_reel_collects_only_stale_browser_upload_copies`.
The new Rust binary has not replaced the GPU-busy running process.

## Rechecked

- Real Quick Reel advanced from VballNetV4c to RAVEL-013 but failed
  at **50.11% overall / 42.68% of RAVEL GPU** with an ONNX Runtime
  `CUDA failure 999: unknown error` during CUDA device-to-host
  `cudaMemcpyAsync`. No valid CPU/MP4 result was published. This is a
  long-running CUDA fault, **not** the earlier absent-Track-ID CPU bug.
- After that failure, the native CUDA nine-frame inference smoke passed
  for both models. The short native GPU→fusion→CPU tracklets→Touch→rally
  fixture also passed with SHA-verified snapshots (0 accepted Touch/rallies
  on the artificial short fixture); this does not establish long-run CUDA
  stability or real-match accuracy. A new diagnostic attaches exact
  nine-frame offsets to GPU execution errors.
- Rust workspace: **233 passing tests**; one exclusive repository-lock test
  deliberately filtered while the server is active, three optional special
  integration tests ignored. Cargo format/check passed offline.
- WASM/UI, Quick Reel, players, legacy, ownership, roadmap and stage84
  parity-math JS tests passed. Stage84 evidence gate remains **NO_GO**.
- WSL Docker Engine **29.9.1** accessible; packaging contract passed and
  existing Linux release requires glibc **2.39**. A live container HTTP
  smoke was *not* run while GPU was active.

## Unresolved for stage 85

- A fully successful Rust real-video GPU -> CPU -> FFmpeg Quick Reel with
  verified final MP4 and source SHA receipt.
- Independent human Touch/ID/rally labels; identical source SHA, model
  weights, settings and time range for frozen Python versus Rust.
- Complete, comparable wall-time, process-tree RAM and VRAM metrics.
- Controlled real CUDA and Docker release smokes, rollback evidence.

Do not mark GO, switch master or claim Rust performance superiority
until these independent acceptance gates are met.

## Concurrency / transient UI regression

After the already committed brand-activity update `7900d8d`, the test
browser shims were updated to provide `VolleyballStudioActivity.fetch` and
`document.querySelector`; all six local UI suites passed at that Git
snapshot. Further **uncommitted concurrent** changes to the source UI then
removed `studio-ui-locale` and `studio-ui-settings-label`, while the
current HTTP/WASM tests still expected them. This caused a later working-tree
regression run to fail. Do not interpret those failures as a CUDA/Quick Reel
cleanup defect, nor mark the UI release gate passed: re-run the complete
suite after the separate UI refactor is committed and reconcile tests with
the intended resulting interface. The concurrent UI source changes are not
included in this audit commit.
