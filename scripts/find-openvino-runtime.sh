#!/usr/bin/env bash
set -euo pipefail
root="$1"
for library in "$root"/.cache/volleyball-reel-wsl-venv/lib/python*/site-packages/openvino/libs/libopenvino_c.so* \
  "$root"/.cache/volleyball-reel-wsl-gpu-venv/lib/python*/site-packages/openvino/libs/libopenvino_c.so*;do
  if [[ -f "$library" ]];then
    printf '%s\n' "$library"
    exit 0
  fi
done
