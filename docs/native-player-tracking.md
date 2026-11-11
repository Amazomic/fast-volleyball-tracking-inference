# Native player tracking: Track ID continuity

## Processing contract

- Source: decoded RAVEL player_candidates from the immutable GPU fusion.
- Output: a new, versioned CPU replay under native_cpu_replays. Older CPU
  results are never rewritten by a tracker change.
- A Track ID represents a provisional trajectory, not a verified athlete.
  Athlete identity is established through separate ReID/manual review.
- Native CPU replay is validated at 30 FPS. The default grace period after
  the last detection is 90 frames (3 seconds). Old saved settings with
  a shorter gap automatically upgrade to 90; configurable maximum is 180.
- During occlusion or missed detection, the live ID remains available for
  reassignment. Max-weight bipartite matching takes box overlap, predicted
  motion and size into account, with one-to-one IDs each frame.
- Close competing matches increment ambiguous_detections, not the ID counter.

## Limits

Bounding-box geometry alone cannot prove an athlete's identity across
occlusions, identical uniforms or abrupt changes in position. Tracks
can still be mismatched or fragmented. Long-term named identity requires
ReID appearance evidence and/or explicit human confirmation. Treat CPU
Track IDs as provisional.

To verify on the real video, compare provisional_track_count,
ambiguous_detections and observed ID switches in CPU replays from the
same GPU fusion. Check false joins when two players are visible at once.
A lower ID count alone is not sufficient evidence of correctness.

## Commands

    cargo fmt --manifest-path rust/Cargo.toml --all -- --check
    cargo clippy --manifest-path rust/Cargo.toml -p volley-app --all-targets -- -D warnings
    cargo test --manifest-path rust/Cargo.toml -p volley-core
    cargo test --manifest-path rust/Cargo.toml -p volley-app cpu_retrack::tests

Implementation:
- rust/volley-core/src/tracker.rs
- rust/volley-core/src/tracker/association.rs
- rust/volley-app/static/studio.html

## Short-track confirmation (2026-11-11)

- TrackingOptions.min_track_observations (default 12, range 6–90)
  requires actual observed frames, not interpolated frames or wall-clock
  duration. The 90-frame disappearance window remains unchanged.
- The CPU replay first associates all provisional tracklets and verifies the
  immutable source SHA; a second streaming pass removes every occurrence of
  tracklets below the threshold from tracked_players. No filtered ID can
  become the source of a Touch candidate, player statistics or a photo index.
- Raw GPU fusion and the raw_player_candidates copy are preserved for later
  reprocessing. Provisional numeric ID values are not renumbered after filtering
  (gaps between IDs are expected; don't assume a contiguous range).
- Each CPU replay manifest now provides tentative_track_count,
  discarded_short_track_ids and discarded_short_track_detections alongside
  the remaining provisional_track_count and tracked_player_detections.
  These metrics are also written to the Studio pipeline log.
- A one-pass confirmation filter cannot retrospectively delete an ID's first
  few frames, hence the bounded two-pass stream with checksums on GPU input,
  provisional stream and final saved output. Temporary files are never published.
- This is noise suppression, not visual ReID or guaranteed identity across
  crossings, identical uniforms and longer than three-second gaps.
  Compare new CPU variants to the same GPU source and manually inspect both
  missing actual people and spurious merges before accepting the results.
- Previously saved CPU versions are immutable and unchanged. Start CPU-only
  postprocessing from the selected GPU analysis to create a new revision.

## Rally-scoped identity review (2026-11-12)

Player ID counts, review cards, best representative photos, ReID extraction
and player-statistics output are now derived from the **selected CPU rally
revision**, not from every frame of the video.

- The rally intervals are SHA-verified against the exact saved rally/Touch
  version, with matching replay SHA, video and project. Only
  detected_start_frame .. detected_end_frame (inclusive) count as play.
  Suggested montage context and time between rallies never qualify.
- The underlying GPU fusion and the complete CPU tracking replay stay
  immutable: the rally classifier and its Touch detector need continuous
  source evidence, so filtering *before* rally discovery would form a
  dependency cycle.
- The derived Track ID index counts only real detection frames within
  those intervals. An ID must meet the saved CPU tracking
  min_track_observations threshold (legacy fallback: 12) **within play**,
  regardless of how long it was seen in breaks.
- The player gallery and manual assign/exclude endpoints take an explicit
  rally_run_id; ID assignments outside its verified playing intervals are
  rejected. Photos and newly extracted ReID embeddings use the best
  within-play frame, never a frame from the interval between plays.
  Historical embeddings for a selected ID are recomputed when a rally
  scope is specified.
- Saved manual ID links remain attached to the immutable CPU replay to
  avoid losing previous review work. Counts and ReID groups in the UI
  show only IDs present inside the currently selected rally run.
- Preview overlay suppresses player-ID rectangles outside detected play;
  ball observations remain available for free-video playback.
- Project statistics verify the complete original CPU/Touch SHA and
  same-frame ID references, but include Touch counts and predicted actions
  only when the Touch frame lies within verified rally boundaries.
- A rally run with zero candidate intervals yields zero reviewable IDs.
  All intervals are proposal boundaries, not manually confirmed scoring.
  Court-only spatial filtering is a separate issue: a bystander visible
  **during** a rally is not automatically identified as an actual player.

Existing CPU revisions are not rewritten. Selecting a different CPU
postprocessing revision changes the player-review scope automatically.
