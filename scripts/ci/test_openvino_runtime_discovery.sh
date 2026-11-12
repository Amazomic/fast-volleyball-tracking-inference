#!/usr/bin/env bash
set -euo pipefail
test_root="$(mktemp -d)"
trap 'rm -rf -- "$test_root"' EXIT
script_dir="$(cd "$(dirname "$0")/.." && pwd)"
[[ -z "$(bash "$script_dir/find-openvino-runtime.sh" "$test_root")" ]]
lib_dir="$test_root/.cache/volleyball-reel-wsl-venv/lib/python3.14/site-packages/openvino/libs"
mkdir -p "$lib_dir"
touch "$lib_dir/libopenvino_c.so.2621"
[[ "$(bash "$script_dir/find-openvino-runtime.sh" "$test_root")" == "$lib_dir/libopenvino_c.so.2621" ]]
printf 'OpenVINO runtime discovery: OK\n'
