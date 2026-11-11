#!/usr/bin/env bash
# Install native ONNX Runtime 1.22 GPU + CUDA 12/cuDNN 9 into the user's
# isolated WSL/Linux directory. Python pip is a package downloader ONLY:
# the running Volleyball Studio / GPU worker remains pure Rust + native .so.
set -euo pipefail
mode="install"
if (( $# > 0 ));then mode="$1";fi
case "$mode" in install|check) ;; *) echo "Usage: $0 [install|check]" >&2;exit 2;; esac
home="$HOME/.local/share/volley-rust/ort-1.22.0"
base="$HOME/.local/share/volley-rust"
archive="$base/onnxruntime-linux-x64-gpu-1.22.0.tgz"
url="https://github.com/microsoft/onnxruntime/releases/download/v1.22.0/onnxruntime-linux-x64-gpu-1.22.0.tgz"
if [[ "$mode" == install ]];then
  command -v curl >/dev/null || { echo "Install curl first" >&2;exit 2; }
  command -v tar >/dev/null || { echo "Install tar first" >&2;exit 2; }
  command -v python3 >/dev/null || { echo "Install python3 and pip first" >&2;exit 2; }
  mkdir -p "$base"
  if [[ ! -s "$archive" ]];then
    tmp="$base/onnxruntime-1.22.download.part"
    trap 'rm -f "$tmp"' EXIT
    curl --fail --location --retry 3 --retry-delay 4 \
      --connect-timeout 25 --speed-limit 4096 --speed-time 60 \
      --output "$tmp" "$url"
    tar -tzf "$tmp" >/dev/null
    mv -f "$tmp" "$archive"
    trap - EXIT
  fi
  # Known SHA-256 of Microsoft's official v1.22.0 Linux x64 GPU release.
  expected="2a19dbfa403672ec27378c3d40a68f793ac7a6327712cd0e8240a86be2b10c55"
  observed="$(sha256sum "$archive"|cut -d' ' -f1)"
  [[ "$observed" == "$expected" ]] ||
    { echo "Official ORT 1.22 GPU archive SHA-256 mismatch; refuse extraction" >&2;exit 3; }
  if [[ ! -s "$home/lib/libonnxruntime.so.1.22.0" ]];then
    work="$(mktemp -d "$base/ort-extract-XXXXXXXX")"
    trap 'rm -rf "$work"' EXIT
    tar -tzf "$archive" >/dev/null
    # Extract only into a fresh isolated directory. The pinned Microsoft
    # official release is HTTPS-delivered; do not use user-supplied tarballs.
    tar -xzf "$archive" -C "$work" --strip-components=1
    [[ -s "$work/lib/libonnxruntime.so.1.22.0" &&
       -s "$work/lib/libonnxruntime_providers_cuda.so" ]] ||
      { echo "Invalid official ORT 1.22 CUDA archive contents" >&2;exit 2; }
    mkdir -p "$home"
    cp -a "$work"/. "$home"/
    rm -rf "$work"
    trap - EXIT
  fi
  if [[ ! -f "$home/deps/nvidia/cudnn/lib/libcudnn.so.9" ||
        ! -f "$home/deps/nvidia/cublas/lib/libcublas.so.12" ||
        ! -f "$home/deps/nvidia/cufft/lib/libcufft.so.11" ||
        ! -f "$home/deps/nvidia/curand/lib/libcurand.so.10" ||
        ! -f "$home/deps/nvidia/cuda_runtime/lib/libcudart.so.12" ||
        ! -f "$home/deps/nvidia/cuda_nvrtc/lib/libnvrtc.so.12" ]];then
    echo "Installing pinned CUDA 12/cuDNN 9 dependencies to isolated user directory"
    # Versions verified with the RTX 5070 Ti; pip is installer only,
    # NOT a Python runtime dependency for the Rust application.
    python3 -m pip install --upgrade --target "$home/deps" --only-binary=:all: \
      --disable-pip-version-check --progress-bar off --timeout 50 --retries 3 \
      'onnxruntime-gpu[cuda,cudnn]==1.22.0' \
      'nvidia-cublas-cu12==12.9.3.13' \
      'nvidia-cuda-nvrtc-cu12==12.9.86' \
      'nvidia-cuda-runtime-cu12==12.9.79' \
      'nvidia-cudnn-cu12==9.28.1.4' \
      'nvidia-cufft-cu12==11.4.1.4' \
      'nvidia-curand-cu12==10.3.10.19' \
      'nvidia-nvjitlink-cu12==12.9.86'
  fi
fi
lib="$home/lib/libonnxruntime.so.1.22.0"
provider="$home/lib/libonnxruntime_providers_cuda.so"
[[ -s "$lib" && -s "$provider" ]] ||
  { echo "Native ONNX Runtime 1.22 CUDA library is missing; run: bash scripts/setup_rust_ort_cuda.sh install" >&2;exit 3; }
export ORT_DYLIB_PATH="$lib"
existing="$(printenv LD_LIBRARY_PATH || true)"
export LD_LIBRARY_PATH="$home/lib"
for cuda_lib in "$home/deps/nvidia/"*/lib;do
  if [[ -d "$cuda_lib" ]];then
    LD_LIBRARY_PATH="$cuda_lib:$LD_LIBRARY_PATH"
  fi
done
if [[ -n "$existing" ]];then LD_LIBRARY_PATH="$LD_LIBRARY_PATH:$existing";fi
export LD_LIBRARY_PATH
if command -v nm >/dev/null;then
  nm -D "$lib" 2>/dev/null|grep -q "OrtGetApiBase" ||
    { echo "Invalid ONNX Runtime 1.22 C API" >&2;exit 3; }
fi
if command -v ldd >/dev/null;then
  missing="$(ldd "$provider" 2>&1|grep 'not found' || true)"
  if [[ -n "$missing" ]];then
    echo "CUDA provider shared library dependencies still missing:" >&2
    echo "$missing" >&2
    exit 3
  fi
fi
echo "Native ORT 1.22 CUDA library: $ORT_DYLIB_PATH"
echo "GPU libraries: $home/deps/nvidia (cuDNN 9, CUDA 12)"
echo "Ready for Rust GPU session smoke (NOT yet proof of successful GPU inference)"
