"""High-fidelity H.264 export with optional NVIDIA hardware acceleration."""
from __future__ import annotations

import os
import subprocess
from functools import lru_cache


@lru_cache(maxsize=1)
def nvenc_available() -> bool:
    try:
        result = subprocess.run(
            ["ffmpeg", "-hide_banner", "-nostdin", "-loglevel", "error",
             "-f", "lavfi", "-i", "color=c=black:s=64x64:r=30",
             "-frames:v", "1", "-an", "-c:v", "h264_nvenc",
             "-f", "null", "-"],
            capture_output=True, timeout=12)
        return result.returncode == 0
    except (OSError, subprocess.TimeoutExpired):
        return False


def encoder_flags(seconds: float, *, prefer: str | None = None) -> list[str]:
    """Use the GPU when profitable, with CPU override for portability."""
    mode = prefer or os.environ.get("VOLLEY_MONTAGE_ENCODER", "auto")
    if mode not in ("auto", "cpu", "gpu"):
        raise ValueError("VOLLEY_MONTAGE_ENCODER: auto, cpu or gpu")
    gpu = mode == "gpu" or (mode == "auto" and seconds >= 7)
    if gpu and nvenc_available():
        return ["-c:v", "h264_nvenc", "-preset", "p5",
                "-rc", "vbr", "-cq", "17", "-b:v", "0",
                "-pix_fmt", "yuv420p"]
    return ["-c:v", "libx264", "-preset", "fast", "-crf", "15",
            "-threads", "8", "-pix_fmt", "yuv420p"]


def encode_ffmpeg(command_factory, seconds: float) -> None:
    """Run an FFmpeg encode, retry on CPU if hardware encoder fails."""
    command = command_factory(encoder_flags(seconds))
    try:
        subprocess.run(command, check=True)
    except subprocess.CalledProcessError:
        if "h264_nvenc" not in command:
            raise
        print("MONTAGE: NVENC failed; retrying with libx264", flush=True)
        subprocess.run(command_factory(encoder_flags(seconds, prefer="cpu")), check=True)
