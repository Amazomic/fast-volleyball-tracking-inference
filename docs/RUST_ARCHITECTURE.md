> Stage 61: the global athlete album uses only isolated Rust writes in
> players/native_reference_photos/<player_id>/index.json with bounded JPEG
> blobs, manually reviewed source replay metadata, CAS revision, per-athlete
> lock, and explicit undo. The enrollment API refuses unlinked or excluded
> Track IDs, changed SHA-replays and removed project athletes. No photo
> reference triggers an automatic player assignment.
> Stage 60: volley-inference/src/reid_openvino.rs loads bundled 0287 XML/BIN
> through OpenVINO native C ABI. FFmpeg crop BGR24 -> f32 CHW 1x3x256x128 ->
> 256 float32 L2-normalized visual features. Both weights and preprocessing
> are SHA-256 fingerprinted. Native HTTP adapter binds the extraction to
> exact verified replay SHA and remains read-only, without automatic identity.
> Statistics/montage identity contract (2026-11-09):
> rust/volley-core/src/identity_review.rs::resolve_reviewed is the single
> source of confirmed Track ID owners. Saved ReID groups are **suggestions**
> only; group membership never grants an athlete identity or statistic.
> Both native and historical JSON readers use the same resolver, with
> source-specific storage adapters and unchanged immutable evidence.
> Manual Track ID mutation policy owner: rust/volley-core/src/identity_review.rs.
> Native and historical-cache HTTP handlers are only evidence-bound JSON adapters.
> No separate rules for assign, unassign, exclude, restore are permitted.
> 2026-11-09: This branch has a Rust-only executable and no Python application source. The old code is in the frozen Git checkpoint; legacy on-disk evidence remains a read-only compatibility format. Product completion is not implied.

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
| Source provenance/Track ID scoping | rust/volley-core/src/evidence_source.rs | player_identity.rs legacy and native_player_tracks.rs native | Source-kind discriminator, legacy unknown FPS/SHA, native full SHA/30 FPS and scope-qualified Track IDs; not a binding algorithm |
| ReID similarity/grouping | rust/volley-core/src/reid.rs | reid_native.rs, reid_npz.rs, reid_group_edits.rs | REUSE existing cosine and clustering; add native embeddings + photo enrollment, never another similarity algorithm |
| Player identity | one source-aware identity decision service (PENDING) | player_identity.rs legacy; native_player_tracks.rs native | Consolidate assign/unassign/exclude/revise policy; do not mix Track IDs across source versions |
| Touch evidence | rust/volley-core/src/touch.rs, motion.rs | touch_preview.rs legacy; cpu_touches.rs native | Unify policy, retain source parsing |
| Rally policy | rust/volley-core/src/rally_reconstruction.rs native; legacy compatibility via rallies.rs | cpu_rallies.rs, native_rally_preview.rs | Align output contracts, retain provisional/confirmed distinctions |
| Gold bookmarks | common revisioned decision/storage policy | rally_marks.rs legacy; native_rally_marks.rs native | Keep historical identifiers and JSON formats, share storage/CAS |
| Montage | rust/volley-app/src/montage.rs | player_montage.rs, native_rally_preview.rs, rallies.rs | Exactly one renderer, format-aware selection; use original media |
| Shared sidecar I/O | rust/volley-app/src/versioned_store.rs | existing identity and mark adapters | NO independent revision/SHA/CAS helpers |
| GPU inference | rust/volley-inference/ | inference_full.rs, inference_fusion.rs | Immutable detector evidence |
| Rust/WASM UI state, RU/EN and preferences | rust/volley-web/src/lib.rs, compiled volley_web.wasm | rust/volley-app/src/studio_web.rs and static/studio_ui.js plus feature-specific DOM/HTTP controllers | Rust validates navigation, load epochs, settings and text IDs; JS stays only for DOM/media/file and same-origin transport, no server business policies |
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

### Stage 81: compiled Rust/WASM UI, not a JavaScript business-policy fork

The single Rust source rust/volley-web/src/lib.rs is the canonical owner for navigation among 6 tabs, validated locale/theme/density/reduced-motion settings, generation tokens for cancel-safe asynchronous data loading and translation IDs. rust/volley-web/src/studio_i18n.tsv is compiled into volley_web.wasm using include_str!, and the same source hosts the existing stable montage selection engine. The browser adapter rust/volley-app/static/studio_ui.js validates the ABI, loads the actual WASM from the same-origin Rust Axum server, stores only visual preferences in versioned browser localStorage, renders Rust text and updates localized DOM incrementally. All 163 static UI labels and 20 aria-label/title/placeholder values with Russian text are covered by the embedded RU/EN source. Existing native browser feature controllers (analysis, reviewed highlights, players, Quick Reel, legacy archives) continue to use unchanged same-origin native Rust endpoints and must not re-implement evidence or GPU/CPU business logic. Desktop/phone navigation remains accessible.

The GUI-only settings have **zero effect** on analysis profiles or immutable inference artifacts. The separate GPU launch panel exposes explicit per-run stride 1..9, VRAM cap 0..131072 MB (0 means unlimited) and CPU thread count 1..128. Golden request defaults remain stride=9, VRAM cap=0, CPU=8, device=cuda; JS validates before requests and native Rust revalidates. No GPU or CPU run is started on locale/theme changes.

**Evidence and limitations:** actual target wasm32-unknown-unknown release binary is built and executed by Node WebAssembly.instantiate with bounds/translation tests; 6-tab browser DOM simulation covers navigation, deep links, async data loading and stale-response rejection, language settings persistence, resource flags and explicit GPU request flags. Rust Axum HTTP tests verify static asset wiring. Chrome GUI automation and dynamic API error-message localization are not fully verified at this stage; a successful CUDA run and full Python-versus-Rust feature/quality benchmark are NOT implied. Existing JS is a necessary browser DOM/network adapter and is not a second source of truth for domain policies.