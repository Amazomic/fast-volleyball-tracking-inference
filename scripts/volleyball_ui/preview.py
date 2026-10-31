"""Video metadata and representative frame previews for the local dashboard.

The original video is unchanged. Frames are decoded with FFmpeg, which applies
rotation metadata, and all cache writes stay under results/ui_previews/.
"""
from __future__ import annotations

import hashlib
import json
import math
import os
import subprocess
import threading
from functools import lru_cache
from pathlib import Path

from . import data

AUTO_MAX_WIDTH = 1280
_PREVIEW_LOCK = threading.Lock()


def _ffprobe(path: Path) -> dict:
    result = subprocess.run(
        ["ffprobe", "-v", "error", "-show_streams", "-show_format",
         "-of", "json", str(path)],
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=40,
        check=False,
    )
    if result.returncode:
        raise RuntimeError("Не удалось прочитать параметры видео: " +
                           result.stderr.decode(errors="replace")[-350:])
    return json.loads(result.stdout)


@lru_cache(maxsize=32)
def _read_info(path: str, size: int, mtime_ns: int) -> dict:
    source = Path(path)
    info = _ffprobe(source)
    stream = next((s for s in info.get("streams", []) if s.get("codec_type") == "video"), None)
    if not stream:
        raise ValueError("В файле нет видеодорожки")
    coded_width = int(stream["width"])
    coded_height = int(stream["height"])
    rotation = 0
    for side in stream.get("side_data_list", []):
        if side.get("rotation") is not None:
            rotation = int(float(side["rotation"]))
            break
    else:
        rotation = int(float(stream.get("tags", {}).get("rotate", 0)))
    swapped = (rotation % 180) != 0
    display_width = coded_height if swapped else coded_width
    display_height = coded_width if swapped else coded_height
    # One target width for source and derived display; NEVER upscale by default.
    auto_width = max(2, min(AUTO_MAX_WIDTH, display_width) // 2 * 2)
    duration = float(info.get("format", {}).get("duration") or stream.get("duration") or 0)
    return {
        "width": display_width, "height": display_height, "rotation": rotation,
        "coded_width": coded_width, "coded_height": coded_height,
        "duration_sec": round(duration, 3), "auto_width": auto_width,
    }


def video_info(video: str) -> dict:
    source = data.video_path(video)
    stat = source.stat()
    info = dict(_read_info(str(source.resolve()), stat.st_size, stat.st_mtime_ns))
    info["video"] = video
    info["auto_height"] = output_height(info, info["auto_width"])
    cache = data.find_cache(video)
    if cache:
        analysis = cache / "analysis.mp4"
        if analysis.is_file():
            try:
                info["cached_analysis_width"] = int(_read_info(
                    str(analysis.resolve()), analysis.stat().st_size,
                    analysis.stat().st_mtime_ns)["width"])
            except (ValueError, RuntimeError, KeyError):
                pass
    return info


def output_height(info: dict, target_width: int) -> int:
    return max(2, int(round(target_width * info["height"] / info["width"] / 2)) * 2)


def target_width(info: dict, requested) -> int:
    if requested is None or requested == "" or requested == "auto":
        return int(info["auto_width"])
    try:
        width = int(requested)
    except (TypeError, ValueError):
        raise ValueError("Ширина анализа: целое число либо 'auto'") from None
    if not (320 <= width <= 1920 and width % 2 == 0):
        raise ValueError("Ручная ширина: чётное число 320–1920 пикселей")
    return width


def preview_frame(video: str, time_sec, mode: str = "source", width="auto") -> Path:
    info = video_info(video)
    if mode not in ("source", "analysis"):
        raise ValueError("Неверный режим предпросмотра")
    try:
        timestamp = float(time_sec)
    except (ValueError, TypeError):
        raise ValueError("Неверное время кадра") from None
    if not math.isfinite(timestamp) or not 0 <= timestamp <= max(0, info["duration_sec"]):
        raise ValueError("Время кадра вне длительности видео")
    selected_width = target_width(info, width)
    # Distinct widths are actually fed into ffmpeg's scale filter.
    frame_width = min(info["width"], 1920) if mode == "source" else selected_width
    frame_width = max(2, frame_width // 2 * 2)
    source = data.video_path(video)
    stamp = source.stat()
    identifier = hashlib.sha256(
        f"{source.name}:{stamp.st_size}:{stamp.st_mtime_ns}".encode()
    ).hexdigest()[:20]
    # Frame-accurate cache key at 30 FPS (same timeline as pipeline).
    frame_index = int(round(timestamp * 30))
    root = data.RESULTS / "ui_previews" / identifier
    target = root / f"{mode}_{frame_width}_{frame_index}.jpg"
    if target.is_file() and target.stat().st_size > 0:
        return target
    with _PREVIEW_LOCK:
        if target.is_file() and target.stat().st_size > 0:
            return target
        # Frame preview is an approximation of the normalization path:
        # same scale=width:-2, automatic orientation, pixel aspect and fps.
        filters = (f"fps=30,scale={frame_width}:-2,setsar=1" if mode == "analysis"
                   else f"scale={frame_width}:-2,setsar=1")
        command = [
            "ffmpeg", "-nostdin", "-hide_banner", "-loglevel", "error",
            "-ss", f"{frame_index/30:.4f}", "-i", str(source),
            "-vf", filters, "-frames:v", "1", "-an",
            "-c:v", "mjpeg", "-q:v", "4", "-f", "image2pipe", "pipe:1",
        ]
        result = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                timeout=60, check=False)
        if result.returncode or not result.stdout.startswith(b"\xff\xd8"):
            raise RuntimeError("Не удалось создать предпросмотр кадра: " +
                               result.stderr.decode(errors="replace")[-350:])
        if len(result.stdout) > 9_000_000:
            raise RuntimeError("Слишком большой кадр предпросмотра")
        root.mkdir(parents=True, exist_ok=True)
        tmp = target.with_suffix(".tmp")
        try:
            tmp.write_bytes(result.stdout)
            os.replace(tmp, target)
        finally:
            tmp.unlink(missing_ok=True)
    return target
