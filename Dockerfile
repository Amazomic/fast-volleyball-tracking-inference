# syntax=docker/dockerfile:1.7
# CPU-only baseline. CUDA requires a separate image with matching drivers and
# onnxruntime-gpu; do not silently claim GPU acceleration here.
FROM python:3.14-slim-bookworm

COPY --from=ghcr.io/astral-sh/uv:0.9 /uv /uvx /usr/local/bin/

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
       ca-certificates ffmpeg libgl1 libglib2.0-0 libsm6 libxext6 libxrender1 \
    && rm -rf /var/lib/apt/lists/*

ENV PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1 \
    UV_PROJECT_ENVIRONMENT=/opt/volley-venv \
    UV_LINK_MODE=copy \
    UV_NO_PROGRESS=1 \
    PATH="/opt/volley-venv/bin:${PATH}" \
    VOLLEY_UI_ALLOW_REMOTE=1

WORKDIR /app

# The lockfile is authoritative, and sources are copied only after dependencies
# so changing UI code does not reinstall ONNX Runtime/OpenVINO on every build.
COPY pyproject.toml uv.lock .python-version README.md ./
RUN --mount=type=cache,target=/root/.cache/uv \
    uv sync --frozen --no-dev --no-install-project

COPY . .

# Persist analysis runs and imported videos outside the immutable source layer.
RUN groupadd --system --gid 10001 volley \
    && useradd --system --uid 10001 --gid volley --home-dir /home/volley --create-home volley \
    && mkdir -p /app/results /app/project /app/studio_output /app/studio_work \
    && chown -R volley:volley /app/results /app/project /app/studio_output /app/studio_work
USER volley

VOLUME ["/app/results", "/app/project"]
EXPOSE 8765

HEALTHCHECK --interval=30s --timeout=5s --start-period=90s --retries=3 \
    CMD python -c "import json,urllib.request; assert json.load(urllib.request.urlopen('http://127.0.0.1:8765/api/health',timeout=3))['ok']"

# Bind inside the container. On the host publish ONLY 127.0.0.1:8765:8765;
# Studio is a privileged local tool and has no public-network authentication.
CMD ["python", "-m", "scripts.volleyball_ui", "--host", "0.0.0.0", "--port", "8765"]
