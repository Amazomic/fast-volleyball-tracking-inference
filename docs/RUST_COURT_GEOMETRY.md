# Court geometry in native Rust CPU postprocessing

Source parity:
- https://github.com/asigatchov/fast-volleyball-tracking-inference/blob/master/src/court_transformer.py
- https://github.com/asigatchov/fast-volleyball-tracking-inference/blob/master/src/player_contacts.py

## Implemented

The four image-normalized corners saved by the Analysis editor through /api/video-edit
are loaded on every CPU Touch run from the source-scoped video_edits JSON.
Normalized points are invariant under the aspect-preserving resize of prepared media.
The original movie and existing GPU inference snapshots are not modified.

volley-core::court_geometry::CourtTransform computes the 8-DOF
image-to-court homography, equivalent to Python cv2.getPerspectiveTransform.

Destination coordinates are in meters:
- Point 1: (-9, -4.5)
- Point 2: (+9, -4.5)
- Point 3: (+9, +4.5)
- Point 4: (-9, +4.5)
- Net: longitudinal coordinate x=0.

Mark points 1 -> 2 along one long sideline and 3 -> 4 along the other.
A self-intersecting, non-convex or singular court is rejected on save.
Off-camera normalized corners in the range [-0.5, 1.5] are supported.

The Python-compatible thresholds:
- Ground player anchor: center-bottom of an observed player bounding box.
- On-court corridor: abs(x) <= 9 + 3.5 m, abs(y) <= 4.5 + 1.5 m.
- Court side: near/far derived from longitudinal x and stabilized by local
  per-Track-ID median over the bounded frame window.
- Block hypothesis: overhead touch above the head within 1.6 m of the net.
- Serve hypothesis: within 1.2 m of an end line, with at least 15 observed
  held-ball frames, at least 9 frames of released flight, and outgoing
  speed gain >=1.2. Location in the serve zone alone is insufficient.

Only on-court player boxes enter the existing RAVEL/motion-backed CPU Touch
detector when court markup is available. RTMPose and source-frame Track ID
checks remain in place. Court data enriches an unverified action prediction;
it does not certify the identity of a player or a played action.

Touch and Rally manifests store court_revision and court_context_enabled.
The studio CPU reuse key includes the same revision. Changing the court
markup starts a new CPU postprocessing branch while permitting GPU fusion
reuse. Preview motion-event metadata includes court position and context.

## Remaining gaps compared with the entire Python pipeline

This stage ports the court-aware portion of contact detection; it is not
full functional equivalence with every Python rally/possession method:

- Full-game rally-start logic, serve-toss deduplication (up to 2.5 seconds),
  alternatives ranked by possession rules (up to three touches per side),
  and full RallyContactStateMachine are not yet ported.
- Python may use additional net top/post keypoints; four ground corners
  alone cannot resolve net top height or the true ground projection of
  an airborne ball. No airborne image pixel is treated as a ground point.
- Stable side presently uses a bounded window, not the entire 90-minute
  player track.
- Court filtering currently happens for Touch player association, not in
  the immutable GPU detections or initial CPU tracking pass.
- The authoritative unreviewed technique remains unknown_touch; predictions
  are clearly marked provisional.

## Files

- rust/volley-core/src/court_geometry.rs
- rust/volley-app/src/video_edits.rs
- rust/volley-app/src/court_touch.rs
- rust/volley-app/src/cpu_touches.rs
- rust/volley-app/src/cpu_rallies.rs
- rust/volley-app/src/native_rally_preview.rs
- rust/volley-app/src/studio_pipeline.rs
- rust/volley-app/static/studio.html
- rust/volley-app/static/studio.js

Tests:
  cd rust
  cargo test -p volley-core court_geometry --offline
  cargo test -p volley-app --offline
  cargo clippy -p volley-core -p volley-app --all-targets --offline -- -D warnings

Regression scenarios: valid four-corner calibration, changed calibration,
missing calibration, off-court spectator, overhead block near net, serve
preparation and a faster outgoing ball, strict source-lineage validation.

## Stage 2: provisional rally-contact context (2026-11-12)

Implemented as an independent CPU-only substep after saved Touch proposals
are replayed and SHA-256 verified. Neither VballNet, RAVEL nor RTMPose
needs to run again.

- rust/volley-core/src/rally_contact_context.rs: deterministic Safe Rust
  translation of Python player_contacts._merge_serve_toss,
  _settle_serve and RallyContactStateMachine._advance.
- rust/volley-app/src/cpu_rallies/court_history.rs: bounded replay history
  (up to 10 minutes at 30 FPS, maximum 512 contextual Touch candidates).
- rust/volley-app/src/cpu_rallies.rs: apply the model only within the
  accepted rally start/end; consume same-frame Track IDs and the selected ball.
- rust/volley-app/src/native_rally_preview.rs: cards and overall timeline
  consume the saved rally context; reject references to foreign-frame IDs.
- rust/volley-app/static/studio.js: display contextual provisional types.
- rust/volley-app/src/studio_pipeline.rs: existing CPU results without the
  updated context version are not silently reused as current results.

The original Touch JSONL is never modified. Both the initial toss and
subsequent strike remain in the forensic source; the context marks the
toss as a non-gameplay touch and selects the strongest observed impact
within Python's two-second server-repeat window.

A toss requires an observed ball apex at least four ball radii above both
contacts, within 2.5 seconds, made by the same observed provisional Track
ID in the serve zone with outgoing speed gain at least 1.2. To reject a
one-frame wrong-ball glitch, three flight samples are required and the
second-highest observed ball position is used, instead of Python's single
minimum.

For each accepted rally, the model stores provisional serving side,
reception phase, consecutive touches per side, third/fourth-touch runs
and side switches. These are diagnostic hypotheses, never proof of a
rules violation, scored point, confirmed identity or authoritative action.

Output additions:
- rally_candidates.jsonl: court_contact_context, estimated_gameplay_contacts
- rally manifest: court_contact_context_version
- preview cards: raw_motion_event_count, motion_event_count,
  ignored_serve_toss_count
- timeline: raw_touch_candidates; touch_frames excludes only the
  contextual tosses from accepted rallies with court evidence.

Remaining parity work:
1. Player alternatives ranked by ball-size perspective, reach and
   source-frame evidence, then disambiguated with receiver/possession context.
2. Confidence- and radius-weighted stable ball-side voting, avoiding
   unreliable planar projection of airborne balls.
3. Rally starts, serving player origins and stable team sides over
   long occlusions / set-side switches.
4. Differential checks on human-reviewed matches versus Python outputs.
