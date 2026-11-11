#!/usr/bin/env bash
# Per-checkout Volleyball Studio controller; no Python, no forced kill, no data purge.
set -euo pipefail
action="$1"
repo="$2"
bind="$3"
data_root="$4"
if [[ "$data_root" == __default__ ]];then data_root="$HOME/.cache/volley-rust-sandbox";fi
mode="$5"
ort_lib="$6"
openvino_lib="$7"
case "$action" in start|stop|status|restart|doctor|check) ;; *) echo "Unknown action" >&2;exit 2;; esac
case "$bind" in 127.0.0.1:*) ;; *) echo "Only localhost IPv4 bind is allowed" >&2;exit 2;; esac
port="$(printf '%s' "$bind"|cut -d: -f2)"
if [[ ! "$port" =~ ^[0-9]{4,5}$ ]] || (( 10#$port < 1024 || 10#$port > 65535 ));then
  echo "Invalid local port" >&2;exit 2
fi
repo="$(cd "$repo" && pwd -P)"
[[ -f "$repo/rust/Cargo.toml" && -f "$repo/rust/volley-app/Cargo.toml" ]] ||
  { echo "Volleyball Studio Cargo workspace missing" >&2;exit 2; }
cd "$repo/rust"
if [[ -f "$HOME/.cargo/env" ]];then source "$HOME/.cargo/env";fi
url="http://$bind/internal/rewrite-status"
binary="$repo/rust/target/debug/volley-app"
safe_path(){
  [[ "$1" == /* && "$1" != *"/../"* && "$1" != *"/.." ]]
}
for arg in "$data_root" "$ort_lib" "$openvino_lib";do
  case "$arg" in __not_configured__|__readonly__) ;; *) safe_path "$arg" ||
    { echo "Unsafe runtime path: $arg" >&2;exit 2;};; esac
done
[[ "$mode" == isolated || "$mode" == readonly ]] ||
  { echo "Invalid storage mode" >&2;exit 2; }
read_status(){
  curl --fail --silent --connect-timeout 1 --max-time 3 "$url" 2>/dev/null
}
json_pid(){
  local match
  match="$(printf '%s' "$1"|grep -oE '"pid":[0-9]+'|head -1 || true)"
  [[ -n "$match" ]] || return 1
  printf '%s' "$match"|cut -d: -f2
}
is_owned(){
  local response="$1" pid exe cwd
  [[ "$response" == *'"name":"Volleyball Studio - development instance"'* || "$response" == *'"name":"Volleyball Studio - standalone Rust rewrite (development only)"'* ]]||return 1
  pid="$(json_pid "$response")" ||return 1
  [[ "$pid" =~ ^[0-9]+$ && -e "/proc/$pid/exe" && -e "/proc/$pid/cwd" ]]||return 1
  exe="$(readlink -f "/proc/$pid/exe")"
  cwd="$(readlink -f "/proc/$pid/cwd")"
  [[ "$cwd" == "$repo/rust" ]] || return 1
  local debug="$repo/rust/target/debug/volley-app"
  local release="$repo/rust/target/release/volley-app"
  if [[ "$exe" == "$debug" || "$exe" == "$release" ]];then
    return 0
  fi
  # Rebuilding a running Rust ELF atomically replaces the on-disk file.
  # Linux then reports /proc/<pid>/exe as <exact original path> (deleted).
  # Accept only OUR exact old executable, and additionally verify argv[0].
  [[ "$exe" == "$debug (deleted)" || "$exe" == "$release (deleted)" ]] || return 1
  local cmdline=''
  IFS= read -r -d '' cmdline < "/proc/$pid/cmdline" || return 1
  [[ "$cmdline" == "$debug" || "$cmdline" == "$release" ]]
}
listener(){
  local data
  data="$(read_status)"||return 1
  if ! is_owned "$data";then
    echo "Port $port belongs to an unknown service or another checkout" >&2
    return 2
  fi
  printf '%s' "$data"
}
port_busy(){
  # Kernel listener inventory avoids an unbounded WSL /dev/tcp connect.
  if command -v ss >/dev/null 2>&1;then
    ss -H -ltn "sport = :$port" | grep -q .
    return
  fi
  local result=0
  timeout 2 bash -c "echo >/dev/tcp/127.0.0.1/$port" 2>/dev/null || result=$?
  # A stalled fallback probe means UNKNOWN: refuse to bind/kill anything.
  [[ "$result" -eq 0 || "$result" -eq 124 ]]
}
stop_owned(){
  local response pid reason
  if response="$(listener)";then
    if [[ "$response" != *'"shutdown_safe":true'* ||
          "$response" != *'"active_jobs":[]'* ]];then
      echo "Rust jobs still active; refusing to stop/restart" >&2
      return 3
    fi
    pid="$(json_pid "$response")"
    is_owned "$response" || { echo "Rust PID changed" >&2;return 3; }
    echo "Graceful SIGTERM to verified Volleyball Studio PID $pid"
    kill -TERM "$pid"
    for _ in $(seq 1 100);do
      if ! kill -0 "$pid" 2>/dev/null;then
        echo "Volleyball Studio stopped";return 0
      fi
      if [[ ! -e "/proc/$pid/exe" ]] || ! is_owned "$response";then
        echo "Volleyball Studio exited";return 0
      fi
      sleep .1
    done
    echo "Volleyball Studio did not exit cleanly; not escalating" >&2
    return 3
  else
    reason=$?
    [[ "$reason" -ne 2 ]] || return 2
    if port_busy;then
      echo "Port $port is occupied by an unidentified listener; never kill it" >&2
      return 2
    fi
    echo "Volleyball Studio is not running at $bind"
  fi
}
# The native ONNX Runtime GPU library is installed in the WSL user's
# private runtime directory. UI works without it, GPU jobs do not.
native_ort_root="$HOME/.local/share/volley-rust/ort-1.22.0"
if [[ "$ort_lib" == __not_configured__ &&
      -s "$native_ort_root/lib/libonnxruntime.so.1.22.0" ]];then
  ort_lib="$native_ort_root/lib/libonnxruntime.so.1.22.0"
fi
configure_ort_environment(){
  [[ "$ort_lib" != __not_configured__ ]] || return 0
  [[ -f "$ort_lib" ]] ||
    { echo "ORT library not found: $ort_lib" >&2; return 3; }
  export ORT_DYLIB_PATH="$ort_lib"
  previous_path="$(printenv LD_LIBRARY_PATH || true)"
  export LD_LIBRARY_PATH="$(dirname "$ort_lib")"
  if [[ "$ort_lib" == "$native_ort_root"/lib/* ]];then
    for directory in "$native_ort_root/deps/nvidia/"*/lib;do
      [[ -d "$directory" ]] || continue
      LD_LIBRARY_PATH="$directory:$LD_LIBRARY_PATH"
    done
  fi
  if [[ -n "$previous_path" ]];then
    LD_LIBRARY_PATH="$LD_LIBRARY_PATH:$previous_path"
  fi
  export LD_LIBRARY_PATH
}
doctor(){
  configure_ort_environment
  echo "Volleyball Studio dependencies: $repo"
  for command_name in cargo ffmpeg ffprobe curl;do
    command -v "$command_name" >/dev/null ||
      { echo "Missing $command_name" >&2;return 1; }
  done
  for model in \
    models/VballNetV4c_seq9_grayscale_20260908_213829.onnx \
    models/RAVEL-VB-013-9f.onnx \
    ov/person-reidentification-retail-0287.xml;do
    if [[ ! -s "$repo/$model" ]];then
      echo "Missing model: $model" >&2;return 1
    fi
  done
  echo "Rust, FFmpeg, FFprobe and required model files: OK (no Python service)"
  echo "Resource limits: CPU cores $(nproc), available memory:"
  grep '^MemAvailable:' /proc/meminfo || true
  if [[ -x /usr/lib/wsl/lib/nvidia-smi ]];then
    /usr/lib/wsl/lib/nvidia-smi --query-gpu=name,memory.total,memory.free \
      --format=csv,noheader || true
  elif command -v nvidia-smi >/dev/null;then
    nvidia-smi --query-gpu=name,memory.total,memory.free \
      --format=csv,noheader || true
  else
    echo "NVIDIA GPU diagnostics unavailable (native GPU inference not verified)"
  fi
  if command -v docker >/dev/null;then
    echo "Docker installed: $(docker --version)"
  fi
  local library="$ort_lib"
  if [[ "$library" == __not_configured__ ]];then
    library="$(printenv ORT_DYLIB_PATH || true)"
  fi
  if [[ -z "$library" || ! -f "$library" ]];then
    echo "ORT_DYLIB_PATH not configured: CUDA inference NOT READY; no CPU fallback" >&2
    return 3
  fi
  if ! command -v nm >/dev/null;then
    echo "Binutils nm missing: cannot verify ONNX shared library" >&2;return 3
  fi
  if ! nm -D "$library" 2>/dev/null|grep OrtGetApiBase >/dev/null;then
    echo "Invalid ONNX Runtime shared library: $library" >&2;return 3
  fi
  local provider="$(dirname "$library")/libonnxruntime_providers_cuda.so"
  if [[ ! -f "$provider" ]];then
    echo "CUDA provider shared library missing near ORT: $provider" >&2;return 3
  fi
  if command -v ldd >/dev/null;then
    if ldd "$provider" 2>&1|grep 'not found';then
      echo "CUDA provider has missing shared dependencies" >&2;return 3
    fi
  fi
  echo "ONNX Runtime + CUDA provider libraries found; live GPU smoke still required"
}
check(){
  node "$repo/scripts/ci/check_rust_ownership.mjs" --self-test
  node "$repo/scripts/ci/check_rust_roadmap.mjs" --check --self-test
  cargo check --workspace --offline --locked
}
case "$action" in
  doctor) doctor;;
  check) check;;
  status)
    if response="$(listener)";then
      echo "Volleyball Studio: $bind, PID $(json_pid "$response")"
      if [[ "$response" == *'"shutdown_safe":true'* ]];then
        echo "Idle: safe to request graceful stop"
      else
        echo "Busy: do NOT restart during running jobs"
      fi
    else
      result=$?
      [[ "$result" -ne 2 ]]||exit 2
      if port_busy;then echo "Unknown listener; no action taken" >&2;exit 2;fi
      echo "Volleyball Studio is not running at $bind"
    fi
    ;;
  stop) stop_owned;;
  start|restart)
    if [[ "$action" == restart ]];then stop_owned;fi
    if response="$(listener)";then
      echo "Volleyball Studio already running at $bind (PID $(json_pid "$response"))"
      exit 0
    else
      result=$?
      [[ "$result" -ne 2 ]]||exit 2
    fi
    if port_busy;then echo "Port $port occupied; refusing start" >&2;exit 2;fi
    if [[ "$mode" == readonly ]];then
      unset VOLLEY_RUST_DATA_ROOT
    else
      export VOLLEY_RUST_DATA_ROOT="$data_root"
    fi
    export VOLLEY_RUST_BIND="$bind" VOLLEY_REPOSITORY_ROOT="$repo"
    configure_ort_environment
    if [[ "$openvino_lib" != __not_configured__ ]];then
      [[ -f "$openvino_lib" ]]||{ echo "OpenVINO library missing" >&2;exit 2; }
      export VOLLEY_OPENVINO_C_LIB="$openvino_lib"
      export LD_LIBRARY_PATH="$(dirname "$openvino_lib")${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
    fi
    cargo build --locked --offline -p volley-app
    echo "Starting ONE Volleyball Studio on http://$bind/studio (foreground, Ctrl+C stops)"
    exec "$binary"
    ;;
esac
