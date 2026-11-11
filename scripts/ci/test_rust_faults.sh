#!/usr/bin/env bash
# Stage83 regression/fault acceptance. No production switch or Python service.
# --require-gpu-docker turns missing external runtime into production blockers.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd -P)"
require_full=false
if [[ "$#" -gt 0 ]];then
  [[ "$1" == --require-gpu-docker ]] || { echo "Bad CI option" >&2;exit 2; }
  require_full=true
fi
if [[ -f "$HOME/.cargo/env" ]];then source "$HOME/.cargo/env";fi
cd "$root"
node scripts/ci/check_rust_ownership.mjs --self-test
node scripts/ci/check_rust_roadmap.mjs --check --self-test
cargo fmt --manifest-path rust/Cargo.toml --all -- --check
cargo check --manifest-path rust/Cargo.toml --workspace --offline --locked
cargo test --manifest-path rust/Cargo.toml --workspace --offline --locked --quiet -- --test-threads=4
for task in \
  test_volley_web_wasm.mjs \
  test_studio_wasm_ui.mjs \
  test_native_quick_reel_ui.mjs \
  test_removed_archive_routes.mjs \
  test_native_highlights_ui.mjs \
  test_native_stats_ui.mjs \
  test_rust_packaging.mjs;do
  node "scripts/ci/$task"
done
cargo build --manifest-path rust/Cargo.toml --offline --locked -p volley-app
bash scripts/ci/test_rust_lifecycle.sh
bash scripts/ci/test_rust_shutdown_race.sh
cargo build --manifest-path rust/Cargo.toml --offline --locked --release -p volley-app
bash scripts/ci/test_rust_release_smoke.sh
docker_ok=false
if command -v docker >/dev/null 2>&1 &&
   docker image inspect volley-rust-stage82:local >/dev/null 2>&1;then
  bash scripts/ci/test_rust_docker_smoke.sh volley-rust-stage82:local
  docker_ok=true
else
  echo "BLOCKED external: real Docker image/container smoke NOT verified (Debian package registry)"
fi
cuda_ok=false
ort_path="$(printenv ORT_DYLIB_PATH || true)"
if [[ -n "$ort_path" && -f "$ort_path" &&
      -f "$(dirname "$ort_path")/libonnxruntime_providers_cuda.so" ]];then
  echo "CUDA libraries found; run native CUDA model + fusion integration"
  bash scripts/ci/smoke_rust_ort.sh "$ort_path" cuda full
  cuda_ok=true
  echo "Full model GPU smoke complete; representative match VRAM profile remains stage84"
else
  echo "BLOCKED external: native Linux ONNX Runtime CUDA unavailable; GPU/VRAM peak NOT measured"
fi
if [[ "$require_full" == true && ( "$docker_ok" != true || "$cuda_ok" != true ) ]];then
  echo "ACCEPTANCE FAIL: stage84/85 cannot switch production without missing GPU/Docker checks" >&2
  exit 3
fi
echo "Stage83 Rust security/SHA/cancel/fault/isolated-memory regression: PASSED"
echo "GPU/VRAM match and actual Docker container acceptance: PENDING (not counted as passed)"
