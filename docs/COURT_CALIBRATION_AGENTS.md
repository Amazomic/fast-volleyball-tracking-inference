# Court calibration — shared agent change journal

Every worker MUST read this file before starting and before each modification. Reserve paths first; append changes and verification. Never edit another worker's ownership. No agent may erase or revert existing working tree changes. Integrator owns shared entry points after workers finish. Separate worktrees/branches are required if actual concurrent agents are started.

| Worker | Exclusive paths | Scope | Status |
|---|---|---|---|
| geometry | `rust/volley-core/src/court_geometry.rs`, `rust/volley-core/src/court_detection.rs` | Court line extraction, temporal aggregation, geometry validation; no application state | implemented |
| calibration-api | `rust/volley-app/src/video_edits.rs`, `rust/volley-app/src/court_detection.rs` | Frame sampling, calibration API, persistence, preserve manual overrides | implemented |
| preview | `rust/volley-app/static/studio_editor.js`, `rust/volley-app/static/studio.html`, `rust/volley-app/static/studio.css` | Auto-detect button, overlay and manual correction | implemented |
| integrator | `rust/volley-core/src/lib.rs`, `rust/volley-app/src/main.rs`, `docs/COURT_CALIBRATION_AGENTS.md` | Integration, API route, verification, logging | implemented |

## Event log (append-only)
- 2026-11-13: Created ownership map before implementation. Existing dirty files across app/core/web are user work and must remain untouched. No independent agent runtime available in this session; workstreams executed with exclusive file ownership under single coordinator. Do not claim independent agents were launched.
- 2026-11-13 geometry: added rust/volley-core/src/court_detection.rs (painted-line Hough geometry, temporal consensus, net candidate, conservative rejection).
- 2026-11-13 calibration-api: added rust/volley-app/src/court_detection.rs for bounded FFmpeg frame sampling and advisory detection API.
- 2026-11-13 integrator: registered detection module and route; calibration-api added validated source helper in video_edits.rs.
- 2026-11-13 calibration-api: optional normalized net endpoints now persist in video edits; backward-compatible default null.
- 2026-11-13 preview: added automatic detection action, advisory court+net overlay, net endpoint dragging, persisted net alongside manually correctable corners in studio_editor.js and studio.html.
- 2026-11-13 integrator reserves rust/volley-app/static/studio.js exclusively for connecting the saved calibration overlay to the match preview; existing unrelated edits are preserved.
- 2026-11-13 integrator: added saved court/net toggle and lazy source-scoped overlay rendering in match preview (studio.js), with refresh after edits.
- 2026-11-13 geometry: prevented four selected court boundary lines from being relabelled as top net cable; bounded net endpoints.
- 2026-11-13 verification: cargo check -p volley-app --offline PASSED; cargo test -p volley-core court_detection PASSED (3 tests); cargo test -p volley-app video_edits PASSED (4 tests); node --check studio.js and studio_editor.js PASSED; test_studio_preview_source.mjs and test_studio_overlay_continuity.mjs PASSED.
- 2026-11-13 limitation: no empirical accuracy validation against an actual volleyball match in this change; net line detection is advisory, automatic results require manual confirmation and Save. No autonomous parallel agents were launched.
