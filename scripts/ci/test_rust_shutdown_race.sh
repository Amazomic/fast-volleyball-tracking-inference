#!/usr/bin/env bash
# Fault-inject an IN-FLIGHT HTTP upload during SIGTERM. A correct Rust server
# MUST survive the first signal, then stop after the request is gone.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd -P)"
port=18983
url="http://127.0.0.1:$port/internal/rewrite-status"
binary="$root/rust/target/debug/volley-app"
[[ -x "$binary" ]] || { echo "Build native Rust debug binary first" >&2;exit 1; }
tmp="$(mktemp -d /tmp/volley-rust-fault83-XXXXXXXX)"
pid=''
fd_open=false
cleanup(){
  if [[ "$fd_open" == true ]];then
    exec 7>&- 2>/dev/null || true
    exec 7<&- 2>/dev/null || true
  fi
  if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null;then
    kill -TERM "$pid" || true
    for _ in $(seq 1 30);do
      if ! kill -0 "$pid" 2>/dev/null;then break;fi
      sleep .1
    done
    if kill -0 "$pid" 2>/dev/null;then
      echo "WARNING test Rust PID $pid still running; no force-kill used" >&2
    fi
  fi
  rm -rf -- "$tmp"
}
trap cleanup EXIT
if (echo >/dev/tcp/127.0.0.1/"$port") 2>/dev/null;then
  echo "Reserved fault injection port is busy; refuse touching that process" >&2;exit 2
fi
cd "$root/rust"
VOLLEY_REPOSITORY_ROOT="$root" VOLLEY_RUST_DATA_ROOT="$tmp/data" \
  VOLLEY_RUST_BIND="127.0.0.1:$port" \
  "$binary" >"$tmp/server.log" 2>&1 &
pid=$!
ready=''
for _ in $(seq 1 50);do
  if ready="$(curl --fail --silent --max-time 1 "$url" 2>/dev/null)";then break;fi
  if ! kill -0 "$pid" 2>/dev/null;then
    echo "Rust server died before fault test" >&2;cat "$tmp/server.log" >&2;exit 1
  fi
  sleep .1
done
[[ "$ready" == *'"shutdown_safe":true'* ]] ||
 { echo "Rust test server never became idle" >&2;exit 1; }
# Hold a multipart request open by sending headers plus only a few bytes of
# the declared 1MB body. This is an in-flight server request.
# Real DNS-rebinding and cross-site GET requests must be denied before any
# auto-preparation route can access local project storage.
for scenario in host origin fetch;do
  case "$scenario" in
    host) header='Host: attacker.example:18983';;
    origin) header='Origin: http://attacker.example';;
    fetch) header='Sec-Fetch-Site: cross-site';;
  esac
  code="$(curl --silent --max-time 3 --output /dev/null --write-out '%{http_code}' \
    -H "$header" "http://127.0.0.1:$port/api/project-preparation?project_id=000000000000")"
  [[ "$code" == 403 || ( "$scenario" == host && "$code" == 404 ) ]] ||
    { echo "Cross-site attack not denied ($scenario HTTP $code)" >&2;exit 1; }
done
exec 7<>/dev/tcp/127.0.0.1/"$port"
fd_open=true
printf 'POST /api/quick-reel/native/upload HTTP/1.1\r\nHost: 127.0.0.1:%s\r\nContent-Type: multipart/form-data; boundary=rust83\r\nContent-Length: 1000000\r\nConnection: keep-alive\r\n\r\n--rust83\r\nContent-Disposition: form-data; name="file"; filename="incomplete.mp4"\r\nContent-Type: video/mp4\r\n\r\ninterrupted-payload' "$port" >&7
sleep .3
kill -TERM "$pid"
sleep .35
if ! kill -0 "$pid" 2>/dev/null;then
  echo "REGRESSION: SIGTERM killed Rust during an in-flight upload" >&2
  cat "$tmp/server.log" >&2;exit 1
fi
status="$(curl --fail --silent --max-time 2 "$url")"
[[ "$status" == *'"production_ready":false'* ]] ||
 { echo "Server did not survive busy request SIGTERM" >&2;exit 1; }
grep -q 'Rust shutdown denied' "$tmp/server.log" ||
 { echo "No audit log for denied unsafe shutdown" >&2;exit 1; }
exec 7>&-
exec 7<&-
fd_open=false
for _ in $(seq 1 50);do
  if ! find "$tmp/data/quick_reels/uploads" -maxdepth 1 -name 'drop_*.mp4' \
    -print -quit 2>/dev/null |grep -q .;then break;fi
  sleep .1
done
if find "$tmp/data/quick_reels/uploads" -maxdepth 1 -name 'drop_*.mp4' \
  -print -quit 2>/dev/null|grep -q .;then
  echo "Aborted upload left a partial file" >&2;exit 1
fi
rss_kb="$(awk '/^VmRSS:/{print $2}' /proc/"$pid"/status)"
[[ "$rss_kb" =~ ^[0-9]+$ && "$rss_kb" -lt 524288 ]] ||
 { echo "Idle Rust RSS exceeds 512MiB: $rss_kb KiB" >&2;exit 1; }
kill -TERM "$pid"
for _ in $(seq 1 50);do
  if ! kill -0 "$pid" 2>/dev/null;then break;fi
  sleep .1
done
if kill -0 "$pid" 2>/dev/null;then
  echo "Rust process failed to exit after upload closed" >&2;exit 1
fi
wait "$pid" || true
pid=''
echo "Fault83 real SIGTERM race: busy multipart refused shutdown; aborted upload discarded; idle graceful stop, RSS $rss_kb KiB PASSED"
