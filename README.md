# Volleyball Studio

**AI-assisted volleyball match analysis and video highlights.**

**English** | [Русский](README.ru.md)

---

## About

**Volleyball Studio** is a local application for analyzing recorded volleyball matches. It helps detect the ball and players, reconstruct trajectories and rallies, examine touches, associate tracked identities with known athletes, and assemble video highlights.

## Features

- **Projects and video:** import recordings, select the analysis interval, annotate court geometry, and prepare footage.
- **Primary GPU analysis:** ball and player detection using VballNet and RAVEL, with an additional RTMPose body-keypoint layer.
- **CPU post-processing:** tracking, court-aware geometry, ball trajectories, touch candidates and types, and rally segmentation.
- **Players:** Track IDs, ReID similarity suggestions, manually confirmed player assignments, and a player photo album.
- **Preview and statistics:** rally playback, trajectory/player/keypoint overlays, filters, and human review.
- **Editing:** favorite rallies, video highlights, and a separate Quick Reel workflow.
- **Analysis versions:** saved GPU evidence with independently configurable post-processing variants.

Automated player identification and touch classification are **not guaranteed to be accurate**. ReID results are suggestions, and uncertain events should be reviewed by a human.

## Architecture

| Component | Responsibility |
| --- | --- |
| `rust/volley-app` | Local server, API, jobs, and studio UI |
| `rust/volley-inference` | Models and native inference runtimes |
| `rust/volley-core` | Tracking, ReID, touches, rallies, and CPU logic |
| `rust/volley-media` | Video preparation and FFmpeg-based editing |
| `rust/volley-web` | WebAssembly UI components |

The active application is developed in **Rust**. The original Python implementation remains available in Git history as the migration baseline. ONNX Runtime, OpenVINO, CUDA, and FFmpeg are external dependencies.

## Run locally on Windows / WSL

Requirements include Windows with WSL (`Ubuntu-25.04` by default), a Rust toolchain inside WSL, FFmpeg/FFprobe, and compatible Linux libraries for the selected models. GPU inference requires a compatible NVIDIA/CUDA setup and ONNX Runtime; OpenVINO ReID requires its native runtime.

From the repository root:

```bat
start-volley-ui.cmd
start-volley-ui.cmd /doctor
start-volley-ui.cmd /status
```

Or use PowerShell:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/start-volley-rust-dev.ps1
```

Studio UI: **http://127.0.0.1:8875/studio**. This is a localhost application; do not expose its API directly to the public internet.

Run Rust tests from `rust/` with `cargo test --workspace --offline`; run the UI check from the repository root with `node scripts/ci/test_studio_wasm_ui.mjs`. See the [deployment guide](docs/RUST_DEPLOYMENT.md) for setup and diagnostics.

## Project status

**Active development / experimental release.** A visible feature is not proof of accuracy on real matches. End-to-end acceptance, comparison with the historical Python implementation, and comprehensive quality measurements are still pending. Original footage and historical results are not intended to be silently overwritten.

**Documentation:** [Architecture](docs/RUST_ARCHITECTURE.md) · [Roadmap](docs/RUST_ROADMAP.md) · [Migration log](docs/RUST_MIGRATION.md) · [Deployment](docs/RUST_DEPLOYMENT.md) · [Developer guide](rust/README.md).
