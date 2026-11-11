# Rust-only multi-stage build; project footage and Python services are excluded.
FROM rust:1-bookworm AS build
WORKDIR /build
COPY rust/ rust/
# The WASM UI module is not committed; build it before volley-app, which
# embeds rust/volley-app/static/volley_web.wasm via include_bytes!.
RUN rustup target add wasm32-unknown-unknown \
    && cargo build --manifest-path rust/Cargo.toml --locked --release \
       -p volley-web --target wasm32-unknown-unknown \
    && cp rust/target/wasm32-unknown-unknown/release/volley_web.wasm \
       rust/volley-app/static/volley_web.wasm
RUN cargo build --manifest-path rust/Cargo.toml --locked --release -p volley-app

# Trixie glibc 2.41 supports WSL Ubuntu 25.04 release ABI (requires >=2.39).
# Bookworm glibc 2.36 built an image that would not run the prebuilt ELF.
FROM debian:trixie-slim AS runtime-base
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates ffmpeg libgomp1 libstdc++6 \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system volley \
    && useradd --system --gid volley --home-dir /home/volley --create-home volley \
    && mkdir -p /data \
    && chown volley:volley /data
WORKDIR /app
COPY models/ /app/models/
COPY ov/ /app/ov/
USER volley
ENV VOLLEY_REPOSITORY_ROOT=/app \
    VOLLEY_RUST_DATA_ROOT=/data \
    VOLLEY_RUST_BIND=127.0.0.1:8875
VOLUME ["/data"]
# Native ONNX Runtime, CUDA and cuDNN libraries are external read-only mounts.
# Run with --network host on trusted Linux/WSL. Docker bridge publishing cannot
# reach a deliberately localhost-only Rust API; never expose this API publicly.
EXPOSE 8875
STOPSIGNAL SIGTERM
ENTRYPOINT ["/usr/local/bin/volley-app"]

# Fast local packaging from a separately verified WSL Rust release binary.
# BuildKit only resolves the runtime-base stage; no compiler-image pull.
FROM runtime-base AS prebuilt
COPY prebuilt/volley-app /usr/local/bin/volley-app

# Default docker build always recompiles from Rust sources.
FROM runtime-base AS final
COPY --from=build /build/rust/target/release/volley-app /usr/local/bin/volley-app
