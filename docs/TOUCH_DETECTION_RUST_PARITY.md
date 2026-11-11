# CPU Touch detection: neural + independent geometry (2026-11-12)

## Implemented

- rust/volley-core/src/touch_pipeline.rs: CPU-only merging and time-based
  deduplication of neural RAVEL Touch and independent ball-motion proposals.
- rust/volley-core/src/touch_pipeline/geometry.rs: gravity-compensated
  trajectory discontinuity at the observed ball frame, conservative
  same-frame player reach, apparent-size consistency, and extra false-positive
  gates for single-frame ball mislocalization and changes of ball source.
  The existing CPU max_reach_ratio influences both detection paths.
- Neither path invents Track IDs, player identities or confirmed actions.
  Stored technique stays unknown_touch; existing action_prediction
  remains a separately labeled geometry hypothesis.
- rust/volley-app/src/native_rally_preview.rs provides additive
  per-rally estimated_action_counts and unclassified_action_count over
  all contacts in that selected rally, not only 14 preview excerpts.
  rust/volley-app/static/studio.js shows these in the preview and rally cards.

## Preserved contracts

- Reprocessing uses the existing saved GPU fusion; no ONNX/GPU rerun is required.
- Old GPU/CPU snapshots and original media are immutable. New CPU postprocess
  produces distinct revision-scoped files.
- No new definitive serve or block detection without source-backed court/net
  position and rally-start evidence. Those categories remain available via
  independently reviewed action labels; unknown is preferable to fabrication.
- This change restores standalone geometric Touch candidates and the visible
  attack/overhead/dig/low-touch hypotheses. It is not proof of equal or
  better accuracy against the Python checkpoint.

## Verification

- Focused test fixtures: no RAVEL logits but real trajectory reversal,
  RAVEL+geometry deduplication, straight flight, distant player, ball-source
  switch, single-frame ball mislocalization, original same-frame Track ID gate.
- Run: cargo test --manifest-path rust/Cargo.toml --workspace --offline
- Run: cargo clippy --manifest-path rust/Cargo.toml -p volley-core -p volley-app --all-targets --offline -- -D warnings
- UI regression: node scripts/ci/test_studio_rally_cards.mjs
- Required next accuracy gate: manually annotate touches and action types on
  representative game footage; compare precision/recall/F1 and type confusion
  matrix against frozen Python checkpoint aeaf7704 on identical saved
  detections. Tune separate geometry/RAVEL gates on a held-out set, not by
  anecdotal frame counts.
