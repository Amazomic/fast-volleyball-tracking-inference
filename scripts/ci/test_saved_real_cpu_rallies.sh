#!/usr/bin/env bash
# Read-only real-evidence CPU Touch -> Rally regression. No GPU or project mutation.
# Usage: bash scripts/ci/test_saved_real_cpu_rallies.sh REPLAY_MANIFEST TOUCH_MANIFEST
set -euo pipefail
[[ "$#" -eq 2 && "$1" == /* && "$2" == /* ]] || {
  echo "Expected absolute paths for replay and linked Touch manifests" >&2; exit 2;
}
replay_manifest="$1"
touch_manifest="$2"
for item in "$replay_manifest" "$touch_manifest"; do
  [[ -f "$item" && ! -L "$item" ]] || {
    echo "Missing or unsafe manifest: $item" >&2; exit 2;
  }
done
source="$(dirname "$replay_manifest")/tracked_frames.jsonl"
[[ -f "$source" && ! -L "$source" ]] || {
  echo "Missing or unsafe tracked source" >&2; exit 2;
}
export VOLLEY_REAL_TRACKED_FILE="$source"
readarray -t fields < <(python3 - "$replay_manifest" "$touch_manifest" <<'PY'
import json,sys
with open(sys.argv[1], encoding="utf-8") as f: replay=json.load(f)
with open(sys.argv[2], encoding="utf-8") as f: touch=json.load(f)
assert replay.get("schema")=="volley-native-cpu-tracking/1"
assert replay.get("file")=="tracked_frames.jsonl"
assert touch.get("schema")=="volley-native-cpu-touch-proposals/1"
assert touch.get("source_replay_id")==replay.get("replay_id")
assert touch.get("source_replay_sha256")==replay.get("sha256")
assert touch.get("project_id")==replay.get("project_id")
assert touch.get("video")==replay.get("video")
assert len(replay.get("sha256",""))==64
for value in (replay["sha256"],replay["frames"],touch["width"],touch["height"]):
    print(value)
PY
)
[[ "${#fields[@]}" -eq 4 ]] || { echo "Incomplete or mismatched CPU provenance" >&2; exit 2; }
export VOLLEY_REAL_TRACK_SHA="${fields[0]}"
export VOLLEY_REAL_TRACK_FRAMES="${fields[1]}"
export VOLLEY_REAL_TRACK_WIDTH="${fields[2]}"
export VOLLEY_REAL_TRACK_HEIGHT="${fields[3]}"
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)"
cd "$repo/rust"
if [[ -f "$HOME/.cargo/env" ]]; then source "$HOME/.cargo/env"; fi
cargo test -p volley-app --offline --locked \
  real_saved_cpu_touch_to_rallies_without_gpu -- --ignored --nocapture
