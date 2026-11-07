# volley-app — Rust-native Studio (under construction)

This is a **new, independent Rust application**, not a Python bridge and **not** a production replacement. The original server still owns port `8765`. The development Rust server defaults to loopback port `8875` and deliberately does **not** expose the production `/api/health` readiness endpoint until the complete API and workflows exist.

From the repository root on the Windows workstation (WSL Ubuntu-25.04, Rust 1.99.1):

```bash
cargo run --manifest-path rust/volley-app/Cargo.toml
# optional: VOLLEY_RUST_BIND=127.0.0.1:8875 VOLLEY_REPOSITORY_ROOT=/path/to/repo ...
```

Currently implemented, without Python:
- Rust-native local HTTP listener with single-port exclusive bind and loopback restriction.
- `GET /internal/rewrite-status` explicitly reports `migration_complete=false`.
- `GET /api/projects` reads the original project registry **without writing** any user files and returns existing project records unchanged.

All remaining API endpoints, application jobs, GPU inference, postprocessing, ReID, montage and Rust/WASM UI are **NOT YET IMPLEMENTED**. Do not infer feature completeness from successful compilation. See `docs/RUST_REWRITE_SCOPE.md` for the full inventory. The plan is to finish all behavior first, then test and adapt, and only then compare to the Python checkpoint before considering cutover.
