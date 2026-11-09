# Rust Studio deployment — stage 82/85

**Status: isolated migration preview, NOT an accepted production release.**
The Rust launcher never starts/stops Python or changes original videos,
historical results, or the global player cache.

## Windows and WSL — one working instance

Prerequisites: Ubuntu-25.04 in WSL, Rust offline toolchain, FFmpeg and
FFprobe, Node.js only for CI, local NVIDIA GPU and explicitly provisioned
Linux ONNX Runtime for CUDA inference.

Use from the checkout root:

~~~bat
start-volley-ui.cmd
start-volley-ui.cmd /status
start-volley-ui.cmd /stop
start-volley-ui.cmd /restart
start-volley-ui.cmd /doctor
start-volley-ui.cmd /check
~~~

The app stays in the foreground at http://127.0.0.1:8875/studio/rust.
Legacy Python port 8765 is never automatically touched or killed.
A second launch reuses the running Rust process, and an exclusive file
lock based on the **canonical repository root** prevents a second
instance on another port or with a different Rust data directory.

PowerShell supports optional settings and the same safe actions:

~~~powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/start-volley-rust-dev.ps1 -Action Start
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/start-volley-rust-dev.ps1 -Action Status
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/start-volley-rust-dev.ps1 -Action Restart
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/start-volley-rust-dev.ps1 -Action Doctor
~~~

Optional parameters: -Bind 127.0.0.1:8875, -Distro Ubuntu-25.04,
-DataRoot /absolute/wsl/path, -ReadOnly, -OrtLibrary /absolute/wsl/path/
libonnxruntime.so and -OpenvinoLibrary /absolute/wsl/path/libopenvino_c.so.
Writable Rust data defaults to $HOME/.cache/volley-rust-sandbox outside
the repository. Original models, videos, Python runs and source code
are not modified.

Before sending SIGTERM, the WSL controller verifies the returned PID,
the expected Rust executable path and checkout working directory using
/proc, and GET /internal/rewrite-status must report shutdown_safe=true
and active_jobs=[]. GPU inference, FFmpeg montage, CPU processing, video
preparation, and Quick Reel jobs block restart. SIGINT and SIGTERM both
use native Tokio graceful shutdown; no force kill, Python process kill,
or generic port-based termination is present. Full shutdown race fault
injection is deferred to stage83.

## Native offline Linux release

~~~bash
cd rust
cargo fmt --all -- --check
cargo test --workspace --offline --locked
cargo check --workspace --offline --locked
cargo build --release --offline --locked -p volley-app
~~~

Output: rust/target/release/volley-app (Linux ELF x86-64, NOT a
Windows executable). The Rust backend embeds its HTML/CSS/JS and the
compiled Rust WebAssembly UI, without a Python application dependency.
FFmpeg/FFprobe and a compatible Linux ONNX Runtime/CUDA provider remain
external native dependencies. A Windows ONNX DLL cannot substitute for
a WSL Linux shared library.

## Docker image and local build

The SAME Dockerfile has a default source-built final stage and an opt-in
prebuilt target. Both share a Debian Trixie (glibc 2.41) FFmpeg runtime, a nonroot
volley user, SIGTERM shutdown and localhost-only port 8875.

~~~bash
docker build -f Dockerfile -t volley-rust:source .
bash scripts/ci/build_rust_docker_local.sh
~~~

The second command compiles the release binary offline and creates a
temporary context containing ONLY the compiled executable plus models
and OV assets. This avoids downloading the large Rust builder image.
The resulting image is called volley-rust-stage82:local.

Run only on a TRUSTED local Linux/WSL host with Docker host networking:

~~~bash
docker run --rm --network host --cpus=8 --memory=12g \
  -v volley-rust-data:/data volley-rust-stage82:local
~~~

For native CUDA inference, provide trusted, compatible Linux ONNX
Runtime + CUDA/cuDNN provider shared libraries and NVIDIA container
support. Example requiring actual host libraries:

~~~bash
docker run --rm --network host --gpus all --cpus=8 --memory=12g \
  -v volley-rust-data:/data \
  -v /host/onnxruntime/lib:/opt/ort:ro \
  -e ORT_DYLIB_PATH=/opt/ort/libonnxruntime.so \
  -e LD_LIBRARY_PATH=/opt/ort \
  volley-rust-stage82:local
~~~

Host networking must be enabled on Docker Desktop. Docker bridge
port publishing cannot access a deliberately loopback-only service.
NEVER expose the unauthenticated local-file API publicly via
0.0.0.0 or public port forwarding.

The Docker context excludes original videos, project/ and results/,
model inference caches, local Rust builds and temporary files. The
runtime does not use Python services. Docker images must NOT be
considered tested until an actual image build and running-container
smoke test succeed.
The stage82 local build initially used Debian Bookworm, whose glibc
was incompatible with this release binary; the corrected runtime is
Debian Trixie. Any old cached Bookworm-based image must be rebuilt. Large initial base images and Debian FFmpeg
dependencies may require an accessible package registry.

## Resources and acceptance

Runtime doctor inspects CPU count, available RAM, FFmpeg, model files,
NVIDIA GPU/VRAM, Docker and optional Linux ORT/CUDA provider links.
When required CUDA libraries are absent it fails explicitly with
exit code 3; **there is no implicit CUDA-to-CPU fallback**.

Verified on connected WSL/Windows in stage82: native Linux release
compilation, real Windows-to-WSL localhost HTTP access, Windows
PowerShell and CMD status/check/stop commands, an isolated
start→same-port reuse→second-port rejection→graceful stop→restart
regression, canonical-lock ownership and an active CPU job causing
shutdown_safe=false. The legacy user Python service is never touched.

~~~bash
bash scripts/ci/test_rust_lifecycle.sh
bash scripts/ci/test_rust_release_smoke.sh
node scripts/ci/test_rust_packaging.mjs
~~~

Actual CUDA completed inference requires provisioning native ONNX
Runtime and passing the optional real-model smoke. Full match GPU,
memory, cancellation, security and Python-vs-Rust parity remain stage
83/84 acceptance; stage85 decides on production or rollback.

**Linux ABI gate:** the WSL Ubuntu-25.04 offline prebuilt ELF requires
 GLIBC_2.39. Debian Bookworm (glibc 2.36) is not suitable for that
 executable even if Docker build succeeds. The image now uses Debian
 Trixie (glibc 2.41) to support both the WSL prebuilt binary and
 the source-built binary. At packaging acceptance, validate the
 actual container entrypoint via a running-container HTTP smoke,
 not just docker build.

After the image is available, verify the REAL container:

~~~bash
bash scripts/ci/test_rust_docker_smoke.sh volley-rust-stage82:local
~~~
The test only starts its own disposable non-root container with
 a read-only root filesystem, tmpfs-backed isolated data and
 a local test port. It verifies HTTP and compiled WASM before
 stopping only its own container. Host-networking support on Docker
 Desktop must be enabled.

## Stage83 security and fault-injection acceptance

An Axum lifecycle barrier closes the status-to-SIGTERM race: after
 confirming zero in-flight requests and active native tasks, incoming
 requests are denied. SIGTERM during HTTP/GPU/CPU/FFmpeg work is
 refused and audited, never force-killed. The global request gate
 rejects foreign Host, Origin and Sec-Fetch-Site headers on all routes.

Prepared MP4 cache reuse now requires a SHA-256 and byte-size manifest
 receipt. Old unsigned Python/Rust entries are rebuilt, not trusted.
 Rehashing uses a 1-MiB streaming buffer; quantify repeated hashing
 cost during stage84. JSON sidecars are limited to 32 MiB and reject
 symlinked path components; new temp files are fsynced before rename.

Run scripts/ci/test_rust_faults.sh for all offline Rust/code fault
 tests. Add --require-gpu-docker for a strict gate: it requires a
 real CUDA model integration and running Docker container. Without
 Linux ONNX Runtime or a verified Docker image it exits nonzero.
 Representative match quality and peak VRAM profiling remain stage84.

## Stage83 integrity and release gates

Run bash scripts/ci/test_rust_faults.sh for complete offline fault-injection
and lifecycle regression. Run bash scripts/ci/test_rust_faults.sh
--require-gpu-docker before considering GPU/Docker deployment complete;
it exits nonzero unless genuine CUDA ONNX inference and a running Docker
image have been verified. Windows Git users must keep Bash scripts LF-only;
.gitattributes enforces this for every *.sh file.