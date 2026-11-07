# Volley Studio — contributor and coding-agent contract

The goal is **less code and fewer moving parts without reducing the quality of
real volleyball analysis or exported reels**. Read
[docs/DEVELOPMENT_WORKFLOW.md](docs/DEVELOPMENT_WORKFLOW.md) before making changes
and [docs/ARCHITECTURE_INFERENCE_POSTPROCESSING.md](docs/ARCHITECTURE_INFERENCE_POSTPROCESSING.md)
before touching model output, identities or CPU replay.

## Invariants

- Project → immutable GPU inference → separately versioned CPU interpretation
  → preview, player identities, reports and original-source montage.
- GPU inference is expensive and runs only when the recorded model inputs change.
  New thresholds, ReID, touches, rallies and Track ID associations must be
  reinterpretable on CPU from recorded evidence.
- A Track ID proposal is **not** a verified person. Concurrent visible people
  cannot share one confirmed identity; manual assignments are authoritative.
- Never promote a new default in
  `scripts/volleyball_ui/static/cpu-defaults.json` without comparable
  annotated-match measurements, a revision bump and a documented report.
- Do not silently degrade video output resolution, FPS, codec or use a lower-
  quality proxy as the montage source.
- Never edit active user project data under `project/` or `results/` as part
  of development. Do not terminate an active user analysis to run tests.

## Change process

1. Branch from `master`; do not push directly to the default branch.
2. State user-visible behavior, observable failure, intended invariant and
   acceptance tests. Prefer deleting unused complexity over adding adapters.
3. Change the **smallest authoritative module**. Avoid duplicating defaults,
   UI state or cache ownership. Preserve old project formats via explicit
   compatibility tests when necessary.
4. Add a failing regression test, implement the fix, run quick and targeted
   suites. For CPU or model changes, compare the same saved inference and
   manually annotated clips before/after, not only synthetic examples.
5. Check source quality (maintainability, reuse, size, latency, CPU/GPU work)
   and output quality (touch/rally/player accuracy and reel fidelity).
6. Submit a PR with paths, test evidence, risk and rollback. Do not claim
   Docker, GPU or browser E2E passed unless it actually ran.
7. Merge only after locally completed and documented applicable checks; perform human review for high-risk
   default/model/data migrations. Revert or correct regressions instead of
   lowering test expectations.

## Tests

- Quick policy: `python -m unittest discover -s tests -p test_ci_quality.py`
- JS UI contracts: `node --test tests/*.cjs`
- Python (after `uv sync --frozen --extra dev`):
  `PYTHONPATH=.:src uv run --no-sync pytest -q tests`
- Local checks: `python -m scripts.ci.run_checks` (or `--full`).
- There is no hosted CI merge gate. Report skipped checks as NOT TESTED.
- Full guidance: [docs/TESTING.md](docs/TESTING.md).

Keep code and comments in the original module's language. Describe every new
configuration option (meaning, unit, default, effect) and make UI errors clear.

## UI stability policy

Keep the detailed no-layout-jump, scroll, cache-busting and browser-acceptance invariants in [docs/UI_LAYOUT_POLICY.md](docs/UI_LAYOUT_POLICY.md). Read it before modifying UI, layout, client-side reloads or player tables.

## Rust migration — complete rewrite first (2026-11-07)

Read [docs/RUST_MIGRATION.md](docs/RUST_MIGRATION.md) and [docs/RUST_REWRITE_SCOPE.md](docs/RUST_REWRITE_SCOPE.md). The user explicitly chose a **complete Rust rewrite first**, followed by **integrated testing and adaptation**, then **Python-vs-Rust comparison**. For this migration only, this sequence supersedes the per-module regression-test-first workflow above: commit genuine implementation checkpoints, allow compiler/smoke diagnostics, and defer full parity/quality gates until all Rust functionality exists. The final Rust program must not require Python services, CPU/business logic or orchestration; Rust/WASM replaces browser application behavior. The original Python branch and immutable rollback checkpoint must remain intact. Never confuse a partially working Rust kernel or hybrid server with completion.
