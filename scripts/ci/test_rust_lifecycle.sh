#!/usr/bin/env bash
# Full native Rust-only launch/duplicate/stop/restart cycle. Isolated /tmp
# scratch and test port only; no production services or Python involved.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd -P)"
ctl="$root/scripts/volley-rust.sh"
port=18975
tmp="$(mktemp -d /tmp/volley-stage82-lifecycle-XXXXXXXX)"
url="http://127.0.0.1:$port/internal/rewrite-status"
pid=''
cleanup(){
  bash "$ctl" stop "$root" "127.0.0.1:$port" "$tmp/data" isolated \
    __not_configured__ __not_configured__ > /dev/null 2>&1 || true
  if [[ -n "$pid" ]];then wait "$pid" 2>/dev/null || true;fi
  rm -rf -- "$tmp"
}
trap cleanup EXIT
if curl --silent --max-time 1 "$url" >/dev/null || (echo >/dev/tcp/127.0.0.1/"$port") 2>/dev/null;then
  echo "Test port $port already busy: not disturbing an existing service" >&2;exit 2
fi
bash "$ctl" start "$root" "127.0.0.1:$port" "$tmp/data" isolated \
  __not_configured__ __not_configured__ >"$tmp/server.log" 2>&1 &
pid=$!
ready=''
for _ in $(seq 1 50);do
  if ready="$(curl -fsS --max-time 1 "$url" 2>/dev/null)";then break;fi
  if ! kill -0 "$pid" 2>/dev/null;then
    echo "Rust server unexpectedly exited" >&2;cat "$tmp/server.log" >&2;exit 1
  fi
  sleep .2
done
[[ "$ready" == *'"shutdown_safe":true'* && "$ready" == *'"active_jobs":[]'* ]]||
  { echo "Server never reached idle readiness" >&2;tail -50 "$tmp/server.log" >&2;exit 1; }
instance="$(printf '%s' "$ready"|grep -oE '"pid":[0-9]+'|cut -d: -f2)"
[[ "$instance" =~ ^[0-9]+$ ]]||{ echo "No Rust PID" >&2;exit 1; }
bash "$ctl" start "$root" "127.0.0.1:$port" "$tmp/data" isolated \
  __not_configured__ __not_configured__ |grep -q 'already running'
set +e
VOLLEY_REPOSITORY_ROOT="$root" VOLLEY_RUST_DATA_ROOT="$tmp/data-second" \
  VOLLEY_RUST_BIND="127.0.0.1:18976" \
  "$root/rust/target/debug/volley-app" >"$tmp/second.log" 2>&1
duplicate_rc=$?
set -e
if [[ "$duplicate_rc" -eq 0 ]] || ! grep -q 'Another Volleyball Studio Rust instance' "$tmp/second.log";then
  echo "Second Rust instance was not blocked on a different port" >&2;cat "$tmp/second.log" >&2;exit 1
fi
[[ ! -e "$tmp/data-second" ]]||{ echo "Second process touched a new data root" >&2;exit 1; }
bash "$ctl" status "$root" "127.0.0.1:$port" "$tmp/data" isolated \
  __not_configured__ __not_configured__ |grep -q "Rust Studio:"
echo "Requesting graceful test shutdown"
bash "$ctl" stop "$root" "127.0.0.1:$port" "$tmp/data" isolated \
  __not_configured__ __not_configured__
wait "$pid" || true
pid=''
if curl --fail --silent --max-time 1 "$url" >/dev/null;then
  echo "Rust is still serving after safe stop" >&2;exit 1
fi
# Same file lock becomes available again after the previous process exits.
bash "$ctl" restart "$root" "127.0.0.1:$port" "$tmp/data" isolated \
  __not_configured__ __not_configured__ >"$tmp/restart.log" 2>&1 &
pid=$!
ready=''
for _ in $(seq 1 40);do
  if ready="$(curl -fsS --max-time 1 "$url" 2>/dev/null)";then break;fi
  sleep .2
done
[[ "$ready" == *'"shutdown_safe":true'* ]] ||
  { echo "Restart did not bring Rust Studio online" >&2;cat "$tmp/restart.log" >&2;exit 1; }
bash "$ctl" stop "$root" "127.0.0.1:$port" "$tmp/data" isolated \
  __not_configured__ __not_configured__ >/dev/null
wait "$pid" || true
pid=''
echo "Rust lifecycle: start, owned process verification, second-port denial, graceful stop, restart: PASSED"
