# Rust-only build. External ONNX Runtime/CUDA libraries must be supplied
# separately for real GPU inference; no Python image, uv or venv.
FROM rust:1-bookworm AS build
WORKDIR /build
COPY rust/ rust/
RUN cargo build --manifest-path rust/Cargo.toml --locked --release -p volley-app

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates ffmpeg libgomp1 libstdc++6 \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=build /build/rust/target/release/volley-app /usr/local/bin/volley-app
COPY models/ /app/models/
COPY ov/ /app/ov/
ENV VOLLEY_REPOSITORY_ROOT=/app \
    VOLLEY_RUST_DATA_ROOT=/data \
    VOLLEY_RUST_BIND=127.0.0.1:8765
VOLUME ["/data"]
# Loopback-only listener by design. Use --network host on a trusted Linux
# host; do not expose this unauthenticated local file API to a network.
EXPOSE 8765
ENTRYPOINT ["/usr/local/bin/volley-app"]