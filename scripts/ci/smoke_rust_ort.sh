#!/usr/bin/env bash
# Development-only Rust ONNX Runtime preflight. This script never writes
# source recordings, project results, or the Python Studio's cache.
set -euo pipefail

library="${1:?Usage: smoke_rust_ort.sh /absolute/path/to/libonnxruntime.so [cpu|cuda]}"
device="${2:-cpu}"
if [[ "$device" != cpu && "$device" != cuda ]]; then
  echo "Choose cpu or cuda" >&2; exit 2
fi
if [[ ! -f "$library" ]]; then
  echo "ORT shared library not found: $library" >&2; exit 2
fi
if [[ "$library" != /* ]]; then
  echo "ORT shared library must use an absolute path" >&2; exit 2
fi
port=8879
base="http://127.0.0.1:$port"
if curl --silent --max-time 1 "$base/internal/rewrite-status" >/dev/null; then
  echo "Refusing to use port $port: a server already exists" >&2; exit 2
fi
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
if [[ -f "$HOME/.cargo/env" ]]; then source "$HOME/.cargo/env"; fi
log=$(mktemp /tmp/volley-rust-ort-XXXXXX.log)
status=$(mktemp /tmp/volley-rust-ort-XXXXXX.json)
pid=''
cleanup() {
  if [[ -n "$pid" ]]; then kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; fi
  rm -f -- "$log" "$status"
}
trap cleanup EXIT
export ORT_DYLIB_PATH="$library"
if [[ "$device" == cuda ]]; then
  # ORT GPU wheels install cuBLAS/CuDNN in sibling nvidia/<component>/lib
  # directories. The Rust process needs the native dynamic loader paths;
  # it never invokes the Python package or interpreter for model inference.
  packages=$(dirname "$(dirname "$(dirname "$library")")")
  if [[ -d "$packages/nvidia" ]]; then
    for native_lib_dir in "$packages"/nvidia/*/lib; do
      if [[ -d "$native_lib_dir" ]]; then
        export LD_LIBRARY_PATH="$native_lib_dir:${LD_LIBRARY_PATH:-}"
      fi
    done
  fi
fi
export VOLLEY_RUST_BIND="127.0.0.1:$port"
unset VOLLEY_RUST_DATA_ROOT
cargo run --offline --manifest-path rust/Cargo.toml -p volley-app >"$log" 2>&1 &
pid=$!
ready=false
for _ in $(seq 1 90); do
  if curl --fail --silent --max-time 1 "$base/internal/rewrite-status" >"$status"; then
    ready=true; break
  fi
  if ! kill -0 "$pid" 2>/dev/null; then
    echo "Rust app exited during start:" >&2; tail -80 "$log" >&2; exit 1
  fi
  sleep 0.2
done
if [[ "$ready" != true ]]; then
  echo "Rust app failed to start:" >&2; tail -80 "$log" >&2; exit 1
fi
python3 - "$status" <<'PY'
import json,sys
item=json.load(open(sys.argv[1], encoding="utf-8"))
assert item["production_ready"] is False
assert item["migration_complete"] is False
assert item["isolated_writes_enabled"] is False
print("Rust server: read-only development mode")
PY
curl --fail --silent --max-time 60 \
  -H "Content-Type: application/json" \
  --data "{\"device\":\"$device\",\"probe_runtime\":true,\"cpu_threads\":2}" \
  "$base/api/inference/native/preflight" >"$status"
python3 - "$status" "$device" <<'PY'
import json,sys
data=json.load(open(sys.argv[1], encoding="utf-8"))
print("ORT preflight",sys.argv[2],"model manifest check:",data.get("model_checks_passed"))
sessions=data.get("runtime_sessions",[])
for item in sessions:
    print("  ",item.get("model"),"session opened:",item.get("session_opened"))
    if item.get("error"): print("   ",str(item["error"])[:400])
if not data.get("model_checks_passed") or len(sessions)!=2 or not all(s.get("session_opened") for s in sessions):
    raise SystemExit(1)
PY
if [[ "${3:-probe}" == infer ]]; then
  echo "Running explicit real-model ONNX forward/decoder integration smoke ($device)"
  VOLLEY_TEST_ORT_DEVICE="$device" cargo test \
    --manifest-path rust/Cargo.toml -p volley-inference --offline \
    native_onnx_clip_infers_and_decodes -- --ignored --nocapture
elif [[ "${3:-probe}" == full ]]; then
  echo "Running native forward/decoder and complete isolated model job tests ($device)"
  VOLLEY_TEST_ORT_DEVICE="$device" cargo test \
    --manifest-path rust/Cargo.toml -p volley-inference --offline \
    native_onnx_clip_infers_and_decodes -- --ignored --nocapture
  VOLLEY_TEST_ORT_DEVICE="$device" cargo test \
    --manifest-path rust/Cargo.toml -p volley-app --offline \
    native_full_model_fixture_end_to_end -- --ignored --nocapture
elif [[ "${3:-probe}" != probe ]]; then
  echo "Third argument must be probe, infer or full" >&2
  exit 2
fi
