# Volleyball Studio — staged Rust migration ledger

**Status:** ACTIVE — migration branch, no production Rust replacement enabled yet.
**Started:** 2026-11-07. **Primary branch:** `migration/rust-2026-11-07`.
**Canonical handoff document:** `docs/RUST_MIGRATION.md`. Update it **in the same commit as every completed stage**.

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

## 1. Non-negotiable architecture contract

1. Project -> immutable versioned GPU inference -> separately versioned CPU interpretation -> preview / player identity / statistics -> original-source montage.
2. GPU is expensive: preserve raw evidence, scores, detections, original Track IDs and model metadata once per input/model/resolution. Do not rerun it to tune CPU thresholds.
3. CPU stages may reconstruct tracks, motion, touch peaks, rallies and ReID. They may not invent GPU detections. Mark interpolated coordinates; do not combine two concurrently visible people into one confirmed identity.
4. Manual player assignments outrank automatic proposals. ReID cosine similarity is not a calibrated probability. Preserve the existing golden CPU defaults without quality evidence.
5. Maintain exact backward-compatible serialized formats and project history while introducing Rust. Every CPU run remains independently reproducible and selectable.
6. Preview must stay responsive and stable during data loading; keep existing JavaScript/CSS front end until an independently validated replacement exists. Preserve original-quality output video.
7. No switch to Rust without comparing identical cached GPU evidence, existing Python results, and annotated real volleyball matches. Do not treat synthetic unit tests as accuracy proof.
8. Do not edit or remove user data. Do not stop any running inference for development.

Authoritative architecture: `docs/ARCHITECTURE_INFERENCE_POSTPROCESSING.md`.
UI constraints: `docs/UI_LAYOUT_POLICY.md`. Dev/test policy: `docs/DEVELOPMENT_WORKFLOW.md` and `docs/TESTING.md`.

## 2. Migration strategy and stage gates

Keep the running Python implementation as the oracle and fallback. Start with **pure deterministic CPU functions** and transparent JSON/JSONL interop. Avoid a big-bang rewrite. Before a stage is committed, pass its unit tests, parity fixtures, existing relevant regression tests, and record exact commands/results here. Benchmarks must use the same clips and machine.

| Stage | Scope / source paths | Rust destination (proposed) | Acceptance gate | State |
|---|---|---|---|---|
| S00 | Freeze checkpoint, sync the latest `master`, document migration and baseline | `docs/RUST_MIGRATION.md` | clean tracked status, policy tests, reconciliation documented | LOCAL TEST GATE PASSED |
| S01 | Toolchain, isolated Rust crate, deterministic data-contract kernel, fixtures & fallback adapter | `rust/volley-core/{Cargo.toml,src/lib.rs}`, `tests/test_rust_core_contract.py` | `cargo fmt --check`, `cargo test`, Python golden parity (no GPU) | NOT STARTED |
| S02 | Ball trajectory, acceleration and touch-peak candidates; preserve RAVEL-013 evidence | `rust/volley-core/src/touches.rs`; `src/player_contacts.py` adapter | same touch timestamps/IDs before switch; annotated-match quality >= current | NOT STARTED |
| S03 | CPU player tracking and up-to-4s gap interpolation, occupancy conflicts | `rust/volley-core/src/tracking.rs`; `scripts/volleyball_studio/replay_player_tracking.py` | deterministic same-track tests; player ID fragmentation/recall not worse | NOT STARTED |
| S04 | ReID similarity, reference photo grouping, manual precedence | `rust/volley-core/src/reid.rs`; `scripts/volleyball_ui/player_identity_groups.py` | match ordering, no simultaneous-identity merges, photo/test parity | NOT STARTED |
| S05 | Rally boundaries, contacts, reports and CPU versioning | `rust/volley-core/src/rallies.rs`; `scripts/volleyball_studio/reprocess.py` | no clipped rallies, stable API schemas, golden regression gate | NOT STARTED |
| S06 | Job orchestration and API boundary, cancellation, cache discipline | `rust/volley-server/`; `scripts/volleyball_ui/server.py` | project lifecycle/API and UI tests, backward compatibility | NOT STARTED |
| S07 | Optional GPU ONNX/OpenVINO adapters (only if measured useful) | `rust/volley-inference/` | same model outputs/timings/providers with GPU verified, no silent CPU fallback | NOT STARTED |
| S08 | FFmpeg orchestration, reel build/export | `rust/volley-media/`; `src/make_reels.py` | original-quality image/audio, montage timing and cancellation tests | NOT STARTED |
| S09 | Packaging, performance bench, eliminate redundant Python | `rust/`, launcher, Dockerfile, README | full local regression, real-video metrics, rollback proven | NOT STARTED |

**Gate for each stage:** record source baseline SHA, Python oracle commands, Rust tests, parity diff, benchmarks, changes in defaults (normally none), remaining risks, resulting commit SHA and remote PR branch SHA. If a test is impossible, write **NOT TESTED** and do not mark behaviorally dependent stage complete. Keep legacy path active until the gate passes.

## 3. Release / fallback mechanics

- Version contract before replacing functions. For early stages, Rust is isolated or opt-in: Python continues to serve all current projects.
- Keep the last known-good Rust interface and project file formats backward compatible; no unsafe in-place storage migration.
- Rollback to GitHub branch `checkpoint/pre-rust-2026-11-07` for the initial state. For later stages use the preceding stage commit; do not reset `master`.
- Use branches/PRs per `docs/DEVELOPMENT_WORKFLOW.md`; no hosted Actions assumption, because CI billing was disabled.
- Store benchmark numbers only from measured executions. Current known baseline is **NOT YET MEASURED** for this migration session.

## 4. Stage log (append, never silently rewrite historic results)

### 2026-11-07 — checkpoint complete
- Local checkpoint `48e2e1f`, GitHub-published commit `aeaf770`, exact same tree `deff7368`.
- 73 source/docs/test files committed. Backups, personal media and generated data excluded.
- Direct `git push` returned `could not read Username for 'https://github.com'`; GitHub connector created the exact equivalent tree and two remote branches instead.
- Local migration branch was aligned to the published GitHub commit by a compare-and-swap ref update after verifying identical trees.
- Rust toolchain initially unavailable (`rustc` and `cargo` absent from `PATH`). Windows `winget` has an official `Rustlang.Rustup` package, but installation/build has **not been verified**.
- Remote `master` contained seven newer commits; merge into the migration branch is being resolved, keeping the richer current contributor contract and the detailed local UI rules.

### S00 local tests and merge
**Status:** Local targeted gate PASSED, full matrix NOT TESTED; stage commit and remote mirror pending.

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


### S01 Rust kernel and parity
**Status:** NOT STARTED. Do not mark done until actual Rust compilation and parity test succeed.

## 5. Instructions for the next AI / engineer

1. Inspect `git status -sb`, `git log -5 --oneline`, and `git fetch origin`; never discard unstaged user modifications or personal data.
2. Read this document, `AGENTS.md`, `docs/UI_LAYOUT_POLICY.md`, and `docs/ARCHITECTURE_INFERENCE_POSTPROCESSING.md`.
3. Confirm stage log vs actual Git HEAD and GitHub branch; do not infer a stage was pushed from a local commit.
4. Confirm host toolchain `cargo --version` / `rustc --version` before modifying the Rust crate.
5. Run a Python/oracle regression test **before** changing any CPU algorithm, then write the Rust equivalent and compare real cached analysis evidence.
6. Record actual tests, benchmarks, failures and precise rollback steps in the section above; commit the document together with the stage only once its gate passes.
7. Never merge migration work to `master` without review. If remote updates while working, reconcile before PR.
