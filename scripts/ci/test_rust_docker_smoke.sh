#!/usr/bin/env bash
# Real non-root Docker process test; no Python, host projects or recordings.
set -euo pipefail
image="volley-rust-stage82:local"
if [[ "$#" -gt 0 ]];then image="$1";fi
port=18979
container="volley-rust-stage82-test-$$"
url="http://127.0.0.1:$port/internal/rewrite-status"
container_id=''
cleanup(){
  if [[ -n "$container_id" ]];then
    docker stop --time 10 "$container_id" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT
if (echo >/dev/tcp/127.0.0.1/"$port") 2>/dev/null;then
  echo "Docker smoke port already used: refusing to disturb existing server" >&2
  exit 2
fi
docker image inspect "$image" >/dev/null ||
  { echo "Docker image missing: build first" >&2; exit 2; }
actual_user="$(docker image inspect --format '{{.Config.User}}' "$image")"
[[ "$actual_user" == volley ]] ||
  { echo "Docker image must run as nonroot volley" >&2; exit 2; }
container_id="$(docker run --detach --rm --name "$container" \
  --network host --cpus=2 --memory=768m --read-only \
  --tmpfs /tmp:rw,mode=1777,size=32m \
  --tmpfs /data:rw,mode=1777,size=128m \
  -e VOLLEY_RUST_BIND="127.0.0.1:$port" \
  "$image")"
ready=''
for _ in $(seq 1 50);do
  if ready="$(curl --fail --silent --max-time 2 "$url" 2>/dev/null)";then
    break
  fi
  if ! docker inspect "$container_id" >/dev/null 2>&1;then
    echo "Docker Rust process quit before listening" >&2
    exit 1
  fi
  sleep .2
done
if [[ "$ready" != *'"production_ready":false'* ||
      "$ready" != *'"shutdown_safe":true'* ]];then
  echo "Docker does not expose honest local Rust status" >&2
  docker logs "$container_id" >&2 || true
  exit 1
fi
html="$(mktemp /tmp/volley-docker-html-XXXXXXXX)"
wasm="$(mktemp /tmp/volley-docker-wasm-XXXXXXXX)"
trap 'rm -f -- "$html" "$wasm";cleanup' EXIT
curl --fail --silent --max-time 3 "http://127.0.0.1:$port/studio/rust" -o "$html"
grep -q 'studio-nav-quick' "$html"
curl --fail --silent --max-time 3 "http://127.0.0.1:$port/studio/rust/selection.wasm" -o "$wasm"
od -An -tx1 -N4 "$wasm"|grep -q '00 61 73 6d'
echo "Docker smoke: non-root, read-only runtime, isolated tmpfs, HTTP and genuine Rust/WASM passed"
