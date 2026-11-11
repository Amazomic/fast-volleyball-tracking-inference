# Volleyball Studio — Rust-only contributor contract

## Single implementation

The checked-out migration branch contains **one** executable Studio: Rust
workspace `rust/Cargo.toml`. Do not add Python servers, Python CPU/GPU
orchestrators, duplicated JavaScript state engines, compatibility subprocesses,
or a second code path implementing a Rust-owned feature. The frozen Python
implementation is available only by checking out the protected checkpoint,
not through a runtime switch. Retain FFmpeg, ORT/CUDA and model weights.

The browser bootstrap can use DOM JavaScript/CSS; gameplay, user identity,
selection and business decisions belong to Rust/Rust-WASM. Existing
legacy cache formats are **data compatibility adapters**, not legacy engines.

## Architecture and source integrity

Project → immutable GPU evidence → versioned CPU replay → confirmed player
links → previews/statistics → original-video montage. Reprocessing on CPU
must not rerun expensive neural inference. Do not infer real athletes from
provisional Track IDs or invent authoritative Touch/rally/game actions.
Preserve exact cache schema, source SHA, saved revisions and isolation
boundaries. Never touch user `project/`, `results/`, original videos,
personal photos or active jobs in development tests.

Canonical feature owners: `docs/RUST_CANONICAL_OWNERS.json`.
Migration scope: `docs/RUST_REWRITE_SCOPE.md`.
Stage map: `docs/RUST_ROADMAP.json` and generated `docs/RUST_ROADMAP.md`.
Append-only handoff: `docs/RUST_MIGRATION.md`.
UI invariants: `docs/UI_LAYOUT_POLICY.md`.

## Procedure

Before implementation, inspect `git log`, the ownership map, existing Rust
modules and source-specific schemas. Extend the owner, never copy an adjacent
module to create a second implementation. Prefer removing an obsolete adapter
over inventing another bridge. Implement coherent workstreams aggressively,
but document missing behavior rather than claiming completion. Each tested
increment receives a migration-branch commit and journal entry. Submit PRs
into `master`; never rewrite the protected Python checkpoint.

Local checks (WSL): `cargo fmt --all -- --check`,
`cargo check --workspace --offline`, `cargo test --workspace --offline`,
`node scripts/ci/test_volley_web_wasm.mjs`, ownership and roadmap
self-tests. Native GPU model tests and real-match quality checks are separate
and must be reported explicitly if skipped. No paid hosted CI is assumed.

The Windows launcher is `start-volley-ui.cmd` → `scripts/start-volley-rust-dev.ps1`
→ Rust binary. It never kills an unrelated process on port 8875. Isolated Rust
writes stay outside the repository; saved legacy data remains read-only.