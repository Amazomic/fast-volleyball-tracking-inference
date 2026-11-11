#!/usr/bin/env bash
# Explicit REAL native CUDA smoke: both original ONNX models must actually
# load and produce output on GPU. Never report passed from file presence only.
set -euo pipefail
repo="$(cd "$(dirname "$0")/../.." && pwd -P)"
bundle="$HOME/.local/share/volley-rust/ort-1.22.0"
export ORT_DYLIB_PATH="$bundle/lib/libonnxruntime.so.1.22.0"
[[ -s "$ORT_DYLIB_PATH" ]] ||
  { echo "ONNX Runtime 1.22 GPU library absent: run scripts/setup_rust_ort_cuda.sh" >&2;exit 3; }
old="$(printenv LD_LIBRARY_PATH || true)"
export LD_LIBRARY_PATH="$bundle/lib"
for directory in "$bundle/deps/nvidia/"*/lib;do
  if [[ -d "$directory" ]];then
    LD_LIBRARY_PATH="$directory:$LD_LIBRARY_PATH"
  fi
done
if [[ -n "$old" ]];then LD_LIBRARY_PATH="$LD_LIBRARY_PATH:$old";fi
export LD_LIBRARY_PATH
if ldd "$bundle/lib/libonnxruntime_providers_cuda.so" 2>&1|grep 'not found';then
  echo "GPU runtime shared objects missing; real CUDA test NOT RUN" >&2;exit 3
fi
if [[ -x /usr/lib/wsl/lib/nvidia-smi ]];then
  /usr/lib/wsl/lib/nvidia-smi --query-gpu=name,memory.total --format=csv,noheader
elif command -v nvidia-smi >/dev/null;then
  nvidia-smi --query-gpu=name,memory.total --format=csv,noheader
else
  echo "CUDA GPU driver not visible, real smoke cannot run" >&2;exit 3
fi
if [[ -f "$HOME/.cargo/env" ]];then source "$HOME/.cargo/env";fi
cd "$repo/rust"
export VOLLEY_TEST_ORT_DEVICE=cuda
cargo test -p volley-inference --offline --locked \
  native_onnx_clip_infers_and_decodes -- --ignored --nocapture --test-threads=1
echo "Real native Rust VballNetV4c + RAVEL-013 9-frame CUDA inference: PASSED"
if [[ "$#" -gt 0 && "$1" == "--full" ]];then
  cargo test -p volley-app --offline --locked \
    native_full_model_fixture_end_to_end -- --ignored --nocapture --test-threads=1
  echo "Real native Rust CUDA models -> saved detections -> fused frames -> CPU tracking/Touch: PASSED"
fi
