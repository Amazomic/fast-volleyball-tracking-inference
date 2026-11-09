# Quick Reel: real CPU Touch provenance regression (2026-11-10)

## Root cause and fix

The old Touch pipeline associated a temporal RAVEL peak with a player box
within ±2 frames. Strict CPU rally reconstruction correctly required the
referenced provisional Track ID and selected ball to be observed **on the
exact Touch frame**, so valid motion evidence could abort the reel.

The production fix is in `rust/volley-core/src/touch_pipeline.rs`
(commit `9460a06`): when the same-frame player is absent, retain the
motion-supported Touch as **unknown player**; when the exact frame lacks
selected-ball evidence, do not publish that candidate. The strict rally
reader in `rust/volley-app/src/cpu_rallies.rs` remains unchanged.

## Real source evidence

Failed Quick Reel `e1b6e19fab5a42aca591387cfbd2c3e6`,
`VID_20261031_190829.mp4`:

- 91,944 CPU tracked frames.
- 779 old motion-supported Touch candidates.
- 748 with a same-frame Track ID, 31 with a foreign-frame Track ID.
- 5 contacts missing same-frame selected-ball evidence.
- Both original JSONL SHA-256 values matched their saved manifests.

These are **observations of the old saved artifacts**, not proof that the
new Quick Reel successfully rendered an MP4. A new Quick Reel started while
testing and pruned the old transient run as designed; no original source
video or project data was modified by this inspection.

## Tests

- Existing deterministic unit tests:
  `cargo test -p volley-core touch_pipeline::tests --offline`
- CPU Touch writer through strict CPU rally reader:
  `cargo test -p volley-app source_scoped_cpu_touch_then_rallies --offline`
- For an available real SHA-bound CPU replay **and its matching Touch
  manifest**, use the read-only integration test:

  ```bash
  bash scripts/ci/test_saved_real_cpu_rallies.sh \
    /absolute/replay/manifest.json \
    /absolute/touch/manifest.json
  ```

  The test reprocesses CPU evidence in an isolated temporary directory.
  It does not rerun ONNX, alter the historical run or overwrite GPU data.
  Requires both manifests to belong to the same replay and video.

- Audit any existing Track/Touch JSONL pair, without modifying it:

  ```bash
  python3 scripts/ci/audit_native_touch_refs.py \
    /absolute/replay/tracked_frames.jsonl \
    /absolute/touch/touch_candidates.jsonl
  ```

A passing CPU regression does **not** establish end-to-end GPU/FFmpeg
success. The complete Quick Reel run still needs to finish and produce
a verified final MP4 before release acceptance can be marked PASS.
