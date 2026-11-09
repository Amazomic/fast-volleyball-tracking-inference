#!/usr/bin/env bash
# Native optimized Linux ELF, HTTP + compiled WASM, read-only temporary data.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd -P)"
bin="$root/rust/target/release/volley-app"
[[ -x "$bin" ]] || { echo "Run cargo build --release -p volley-app first" >&2;exit 1; }
port=18977
tmp="$(mktemp -d /tmp/volley-rust-release-XXXXXXXX)"
pid=''
cleanup(){
  if [[ -n "$pid" ]];then
    kill -TERM "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  fi
  rm -rf -- "$tmp"
}
trap cleanup EXIT
if (echo >/dev/tcp/127.0.0.1/"$port") 2>/dev/null;then
  echo "Release smoke port already occupied, refuse to touch existing process" >&2
  exit 2
fi
cd "$root/rust"
VOLLEY_REPOSITORY_ROOT="$root" VOLLEY_RUST_DATA_ROOT="$tmp/data" \
  VOLLEY_RUST_BIND="127.0.0.1:$port" \
  "$bin" >"$tmp/server.log" 2>&1 &
pid=$!
ready=''
for _ in $(seq 1 50);do
  if ready="$(curl -fsS --max-time 2 "http://127.0.0.1:$port/internal/rewrite-status" 2>/dev/null)";then
    break
  fi
  if ! kill -0 "$pid" 2>/dev/null;then
    cat "$tmp/server.log" >&2;exit 1
  fi
  sleep .1
done
[[ "$ready" == *'"production_ready":false'* &&
   "$ready" == *'"shutdown_safe":true'* &&
   "$ready" == *'"active_jobs":[]'* ]] ||
  { echo "Release readiness is not honest" >&2;cat "$tmp/server.log" >&2;exit 1; }
curl -fsS --max-time 3 "http://127.0.0.1:$port/studio/rust" -o "$tmp/studio.html"
grep -q 'studio-nav-quick' "$tmp/studio.html"
curl -fsS --max-time 3 "http://127.0.0.1:$port/studio/rust/selection.wasm" -o "$tmp/client.wasm"
od -An -tx1 -N4 "$tmp/client.wasm"|grep -q '00 61 73 6d'
kill -TERM "$pid"
wait "$pid"
pid=''
if curl -fsS --max-time 1 "http://127.0.0.1:$port/internal/rewrite-status" >/dev/null 2>&1;then
  echo "Release binary still listening after graceful SIGTERM" >&2;exit 1
fi
echo "Rust optimized release: live HTTP, honest status, real embedded WASM and SIGTERM passed"
