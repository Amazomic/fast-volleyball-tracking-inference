# Contact technique classification port (Python → Rust)

Scope: restore **automatic, unverified** technique hypotheses for source-verified
native CPU Touch events. The original Python implementation remains in git
`master` and is not invoked at runtime.

## API mapping

| Python baseline | Rust |
|---|---|
| `src/player_contacts.py::describe_technique(depth_ratio, posture_ratio)` | `action_prediction::predict(PositionEvidence)`, with the same depth/posture thresholds |
| `PlayerStore._compute_standing_heights` (61 observed boxes, 80th percentile) | `touch_pipeline::standing_height`, centered on the detected source frame |
| `ContactDetector._depth_height_px` (ball radius / 10.5 cm × 185 cm) | `action_prediction::predict` perspective height |
| `ContactDetector._classify` (court-dependent block/serve) | `CourtContext` discriminator; **not issued** by the touch pipeline without verified court and rally context |
| `player_interaction.contacts[].contact_type` | `PhysicalTouchCandidate.action_prediction` (optional, **not authoritative**) |
| Manual reviewed game actions | Unchanged `action_review::classify`; explicit human action remains separate |

## Source and output contract

`technique: "unknown_touch"` remains the authoritative *unreviewed* value in
`native_cpu_touch_runs/<id>/touch_candidates.jsonl`. The additive
`action_prediction` object contains a geometric action kind, the
`contact_geometry_v1` method, depth/posture ratios and **combined detection
evidence score**, which is **not a calibrated probability of action type**.
Only events linked to an observed player Track ID on the *same* source frame
can acquire this field. Old snapshots with no field remain valid.

The preview API exposes `action_prediction` on each event; the browser
labels it `≈` / `эвристика`. The project statistics API aggregates estimated
action types separately from touch counts and human-verified identity.
Manual action review and athlete identity are not inferred from this field.

## Ported characterization tests

| Original Python behavior | Rust test |
|---|---|
| Ball above head → attack | `python_describe_technique_attack_overhead_dig_low` |
| Head height → overhead pass | same |
| Chest/knee height → dig | same |
| Below body → low touch | same |
| Wide/crouched posture suppresses overhead pass | `python_wide_posture_changes_head_level_pass_to_dig` |
| Missing/invalid player evidence has no confident type | `invalid_or_unattached_geometry_is_not_an_action` |
| Court-relative serve/block only when court/rally context is supplied | `serve_and_block_require_verified_context` |
| Preserve unreviewed Touch source while emitting predictions | `touch_pipeline::tests::changing_ball_motion_plus_ravel_peaks_and_reach_proposes_contact` |
| Preview remains source-SHA verified | `native_rally_preview::tests::paged_preview_has_safe_original_clips_and_evidence_labels` |

## Deliberate differences and remaining limitations

- The Python `player_contacts.py` path used an additional court homography,
  rally phase, reach ranking, player alternatives, ball perspective mismatch
  weighting and serve-toss deduplication. Those cannot be reconstructed from
  the current motion-gated Touch source alone. **Automatic block and serve are
  not yet emitted** without that evidence. Manually reviewed labels remain
  supported. This is a partial, versioned technique recovery, not feature
  parity and not a calibrated ML classifier.
- Python OpenVINO clip classifier `src/classify_contacts.py` was a **separate**
  classification mechanism. The expected `ov/action_clf_r2plus1d18_verified.xml`
  weights are absent from this repository. We neither invent logits nor
  silently substitute another model.
- Source snapshots are immutable; changing classification logic requires a
  new CPU Touch/rally version. Legacy results are never overwritten.
- Rust retains unknown on absent/bad player geometry, missing same-frame ID
  and invalid perspective evidence. This is intentionally more conservative.

## Verification

On Windows HOME-PC: `cargo test --workspace --offline` and
`cargo clippy --workspace --all-targets --offline -- -D warnings`;
`cargo fmt --all`. The historical Python video-dependent tests were not
run here (Python runtime removed from the active branch). Full A/B action
precision/recall and calibration on labeled matches are still pending.
