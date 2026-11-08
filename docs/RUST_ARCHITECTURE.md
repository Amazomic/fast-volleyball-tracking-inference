# Rust migration — canonical architecture and anti-duplication contract

**Status:** binding architecture for branch migration/rust-2026-11-07. Full rewrite is NOT COMPLETE. Preserve master, the pre-Rust checkpoint and all user media.

## Root cause / what went wrong

Stages 14–21 ported algorithms and readers of legacy Python-compatible evidence (players.json, events.json, game-tracks.json and cached NPZ ReID vectors). Stages 38–51 added an independent native pipeline (GPU detections.jsonl → fused frames → tracked_frames.jsonl → touch_candidates.jsonl → rally_candidates.jsonl). New legacy/native APIs for identity, gold marks, rallies and previews were developed without a named common domain owner. Stage counts were incorrectly interpreted as feature completeness. This is a process failure, not a reason to erase functioning adapters.

**Never remove an existing legacy source reader merely because a native counterpart exists.** Preserve user files and on-disk schemas. First put shared policy and storage behind one owner, then replace downstream calls, then retire an adapter only with actual compatibility tests.

## Canonical application pipeline

1. Project + global profile + media link → fingerprinted prepared-video cache (shared across projects).
2. Immutable model evidence: RAVEL-013 and VballNet → verified fusion. GPU inference runs once per model/config/video, never on a CPU threshold change.
3. Independent, versioned CPU interpretation: provisional player tracking and ball motion → motion-supported Touch proposals → rally proposals → calibrated/verified event classification (future).
4. Identity layer: provisional Track IDs → ReID feature encoder → existing cosine/grouping engine → unconfirmed suggestions → explicit reviewed identity assignments and rejected IDs. An embedding score is NOT a real-world identity probability; 3–4s geometric interpolation cannot certify a player.
5. Domain consumers of **the same versioned evidence and reviewed decisions**: preview, athlete statistics, gold bookmarks, full-rally source-resolution montage. Quick Reel starts a separate fresh GPU/CPU run, reusing only compatible prepared media.

Every input must declare source_kind (legacy_cpu_cache OR native_gpu_cpu_replay), schema_version, project/video/run, source SHA/fingerprint and frame-rate/coordinate convention. Legacy run_id and native replay_id are not exchangeable; numeric Track IDs are not global. Every result is either immutable evidence or a revisioned human decision, never both.

## Feature ownership and what is genuinely duplicated

Machine-readable ledger: docs/RUST_CANONICAL_OWNERS.json; mandatory audit: scripts/ci/check_rust_ownership.mjs. The **complete numbered roadmap**, including the last recorded stage X / planned total Y, next stage and explicit future acceptance criteria, is docs/RUST_ROADMAP.md (generated from docs/RUST_ROADMAP.json). Its consistency gate is scripts/ci/check_rust_roadmap.mjs --check --self-test. The current 85-stage denominator is a planning estimate, not an indication of finished-feature percentage.

| Domain | Canonical implementation | Adapters; preserve until migrated | Action |
|---|---|---|---|
| ReID similarity/grouping | rust/volley-core/src/reid.rs | reid_native.rs, reid_npz.rs, reid_group_edits.rs | REUSE existing cosine and clustering; add native embeddings + photo enrollment, never another similarity algorithm |
| Player identity | one source-aware identity decision service (PENDING) | player_identity.rs legacy; native_player_tracks.rs native | Consolidate assign/unassign/exclude/revise policy; do not mix Track IDs across source versions |
| Touch evidence | rust/volley-core/src/touch.rs, motion.rs | touch_preview.rs legacy; cpu_touches.rs native | Unify policy, retain source parsing |
| Rally policy | rust/volley-core/src/rally_reconstruction.rs native; legacy compatibility via rallies.rs | cpu_rallies.rs, native_rally_preview.rs | Align output contracts, retain provisional/confirmed distinctions |
| Gold bookmarks | common revisioned decision/storage policy | rally_marks.rs legacy; native_rally_marks.rs native | Keep historical identifiers and JSON formats, share storage/CAS |
| Montage | rust/volley-app/src/montage.rs | player_montage.rs, native_rally_preview.rs, rallies.rs | Exactly one renderer, format-aware selection; use original media |
| Shared sidecar I/O | rust/volley-app/src/versioned_store.rs | existing identity and mark adapters | NO independent revision/SHA/CAS helpers |
| GPU inference | rust/volley-inference/ | inference_full.rs, inference_fusion.rs | Immutable detector evidence |
| Rust UI | rust/volley-web/ | temporary rust/volley-app/static/ DOM bridge | Browser JS must not contain new business decisions |
| Quick Reel | rust/volley-app/src/quick_reel.rs | preparation, inference, montage services | Fresh independent inference, no project analysis reuse |

**Reality:** Legacy gold/rally/identity adapters are distinct file schemas and currently have consumers. Removing their routes today would lose legacy project features. Shared sidecar primitives are to be extracted now; full domain unification remains PENDING. Renaming modules is NOT deduplication.

## Mandatory workflow for EVERY new stage / every AI handoff

**0. INVENTORY FIRST.** Read this file, docs/RUST_ROADMAP.md (current X / total Y and numbered remaining stages), docs/RUST_REWRITE_SCOPE.md, last 5 sections of docs/RUST_MIGRATION.md and inspect git log --all for the feature. Search all Rust modules, routes, event structs and stores for related functionality. Run both the ownership audit and roadmap consistency gate. Classify the request as EXISTS / LEGACY-ADAPTER-ONLY / PARTIAL-NATIVE / MISSING. If it already exists: extend its canonical owner or create only a typed source adapter. Do not implement a parallel module.

**1. CONTRACT BEFORE CODE.** Identify owner ledger row; input source kind, schema, version and time coordinates; immutable input and mutable sidecar path; exactly which stages are provisional; consumer list, old-project compatibility and tested migrations. Any new endpoint or schema must update the machine ledger in the SAME commit. If functionality crosses two stores, define the adapter boundary explicitly.

**2. IMPLEMENT VERTICALLY.** Extend pure algorithms in volley-core, infrastructure and APIs in volley-app, UI/WASM in volley-web. Centralize validation, no-symlink bounded I/O, atomic persistence, CAS and project locks. Never silently convert provisional Track IDs/Touch/rallies into verified athlete actions. Never copy or mutate Python user data. No second state store without a documented versioned migration plan.

**3. VERIFY.** Run cargo fmt --all -- --check, cargo test --workspace --offline, cargo check --workspace --offline -q, node scripts/ci/check_rust_ownership.mjs, and targeted Rust/WASM + JavaScript checks for UI modifications. For inference changes run real CUDA smoke; for storage changes use an isolated real-MP4 integration fixture. Check source bytes and invariant statuses, not just HTTP 200. Final full match/benchmark comparison is only after whole rewrite per owner decision.

**4. COMMIT AND PUBLISH EVERY COHERENT STAGE.** Commit code + test + ownership ledger + migration diary + updated 1-to-N roadmap together; a recorded stage must show its number out of the planned total (e.g. 53 / 85), the next stage and the remaining tasks. Run node scripts/ci/check_rust_roadmap.mjs --check --self-test. Record SHA and active branch. Verify published Git tree exactly matches local tree before advancing the migration ref. Never alter master, production port 8765 or original user video/cache. Do not let later agents repeat work only because the diary says PARTIAL.

**5. MARK COMPLETENESS HONESTLY.** COMPLETE means canonical service works on new native evidence, preserves previously saved projects with a tested adapter, connects all consumers and UI, and passes focused tests. Otherwise PARTIAL. Handoff must name tested commit SHA, affected paths, open adapters, and next step.

## Automated safety gate

- Checker rejects duplicate API route literals, missing canonical owners or adapters, conflicting canonical persistence paths, and undocumented additions to guarded player/rally/montage API areas.
- Every feature has exactly one owner; new routes or modules require explicit ledger updates and source-format justification.
- Avoid new *-native2.rs, *-v2.rs or *-rewrite.rs escape hatches; extend owner and add an adapter instead.
- Do not create another ReID grouping engine while the missing work is native vector extraction; do not invent another montage exporter or parallel gold-state manager.
- In review, test both legacy adapter and new native route before deleting a duplicate implementation.

## Ordered remaining consolidation

1. Factor out duplicated SHA-256 revision + bounded no-symlink reads/atomic writes into versioned_store.rs, shared by legacy and native adapters; preserve JSON content exactly.
2. Introduce EvidenceSource = LegacyCpuRun | NativeCpuReplay. Extract the common identity decision semantics from player_identity.rs and native_player_tracks.rs, leaving thin readers/writers.
3. Feed native track/photo embeddings into existing ReID evaluator and manually reviewed owner policy. Implement actual native encoder and photo reference enrollment; never infer identity from query_index.
4. Give provisional native and legacy annotated rally candidates a common typed selection and bookmark interface without converting IDs or marking provisional contacts authoritative.
5. Integrate athlete stats, Quick Reel, end-to-end UI and real-match validation. Switch production only after full parity comparison with immutable Python checkpoint.

**Progress must be measured against missing behaviors and number of duplicate authorities, never number of commits or stages.**
