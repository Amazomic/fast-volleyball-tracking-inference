#!/usr/bin/env bash
# Reproducible local development container from an audited offline Rust
# release build. Copies ONLY model assets and the compiled binary into a
# transient Docker context. No source footage/results/Python service.
set -euo pipefail
repo="$(cd "$(dirname "$0")/../.." && pwd -P)"
if [[ -f "$HOME/.cargo/env" ]];then source "$HOME/.cargo/env";fi
cargo build --locked --offline --release --manifest-path "$repo/rust/Cargo.toml" -p volley-app
release="$repo/rust/target/release/volley-app"
[[ -x "$release" ]] || { echo "Native Linux release binary missing" >&2;exit 1; }
ctx="$(mktemp -d /tmp/volley-rust-docker-XXXXXX)"
trap 'rm -rf -- "$ctx"' EXIT
mkdir -p "$ctx/prebuilt"
cp "$release" "$ctx/prebuilt/volley-app"
cp -a "$repo/models" "$ctx/models"
cp -a "$repo/ov" "$ctx/ov"
cp "$repo/Dockerfile" "$ctx/Dockerfile"
# Only include the release binary and the models; Docker build does not
# read or tar any user recordings, Python projects or local credentials.
cat > "$ctx/.dockerignore" <<'IGNORE'
*
!Dockerfile
!prebuilt/
!prebuilt/volley-app
!models/
!models/**
!ov/
!ov/**
IGNORE
DOCKER_BUILDKIT=1 docker build --pull=false --progress plain \
  --target prebuilt -t volley-rust-stage82:local "$ctx"
echo "Rust container built from offline release: volley-rust-stage82:local"
