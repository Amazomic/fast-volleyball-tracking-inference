"""Conservatively detect originals already matching the analysis-ready MP4 format.

Used by both project preparation and standalone Quick Reel. An unsuitable or
uncertain source is transcoded as before; original bytes are never modified.
"""
from __future__ import annotations

from fractions import Fraction
from functools import lru_cache
from pathlib import Path

from .preview import _ffprobe


@lru_cache(maxsize=128)
def _compatible(path: str, size: int, mtime_ns: int, width: int) -> bool:
    source = Path(path)
    if source.suffix.lower() != ".mp4":
        return False
    meta = _ffprobe(source)
    if "mp4" not in str(meta.get("format", {}).get("format_name", "")).split(","):
        return False
    streams = meta.get("streams", [])
    videos = [s for s in streams if s.get("codec_type") == "video"]
    audio = [s for s in streams if s.get("codec_type") == "audio"]
    if len(videos) != 1 or len(audio) > 1 or len(videos) + len(audio) != len(streams):
        return False
    video = videos[0]
    if (video.get("codec_name") != "h264"
            or video.get("pix_fmt") != "yuv420p"
            or int(video.get("width") or 0) != width
            or int(video.get("height") or 0) % 2
            or video.get("sample_aspect_ratio") not in ("1:1", "1/1")
            or video.get("field_order", "progressive") not in ("progressive", "unknown")
            or any(s.get("codec_name") != "aac" for s in audio)):
        return False
    try:
        if (Fraction(str(video.get("r_frame_rate"))) != 30
                or Fraction(str(video.get("avg_frame_rate"))) != 30
                or abs(float(video.get("start_time", 0))) > 0.02):
            return False
        rotation = float(video.get("tags", {}).get("rotate", 0))
        for side in video.get("side_data_list", []):
            if side.get("rotation") is not None:
                rotation = float(side["rotation"])
                break
        return rotation % 360 == 0
    except (TypeError, ValueError, ZeroDivisionError):
        return False


def is_analysis_ready(source: str | Path, width: int) -> bool:
    source = Path(source).resolve(strict=True)
    stat = source.stat()
    return _compatible(str(source), stat.st_size, stat.st_mtime_ns, int(width))
