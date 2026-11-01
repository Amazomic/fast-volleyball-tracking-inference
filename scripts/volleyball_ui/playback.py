"""On-demand browser-compatible video clips. No whole-video proxies, project-independent.

Clips are small, bounded, content-addressed by the ORIGINAL source signature.
FFmpeg seeks to the requested position in the input and only processes 18 sec.
"""
from __future__ import annotations

import hashlib
import json
import math
import os
import subprocess
import threading
import time
from pathlib import Path

from . import data

SEGMENT_SECONDS = 18
STREAM_WIDTH = 480  # display oriented width, no upscaling
MAX_CACHE_BYTES = 160 * 1024 * 1024
MAX_CLIP_BYTES = 18 * 1024 * 1024
_LOCK = threading.RLock()
_ENCODERS = threading.BoundedSemaphore(value=2)
_PENDING: dict[str, threading.Event] = {}


def segment_path(video: str, start_sec: int) -> Path:
    if not isinstance(start_sec, int) or start_sec < 0:
        raise ValueError("Неверная позиция воспроизведения")
    source = data.video_path(video)
    stamp = source.stat()
    signature = hashlib.sha256(json.dumps([
        str(source.resolve()), stamp.st_size, stamp.st_mtime_ns,
        STREAM_WIDTH, SEGMENT_SECONDS, "mp4-h264-v1"
    ]).encode()).hexdigest()[:24]
    directory = data.RESULTS / "ui_stream" / signature
    return directory / f"{start_sec:07d}.mp4"


def _cleanup(directory: Path, except_path: Path) -> None:
    """Evict oldest disposable clips globally, including old revisions."""
    root = directory.parent
    with _LOCK:
        clips = []
        for item in root.glob("*/*.mp4"):
            if item == except_path or not item.is_file():
                continue
            try:
                stat = item.stat()
            except OSError:
                continue
            clips.append((stat.st_mtime, item, stat.st_size))
        clips.sort(reverse=True)
        used = except_path.stat().st_size if except_path.exists() else 0
        for _, item, size in clips:
            if used + size <= MAX_CACHE_BYTES:
                used += size
                continue
            try:
                item.unlink()
            except OSError:
                pass


def get_segment(video: str, start_sec: int, duration: float) -> Path:
    """One exact aligned segment; validate input and avoid duplicate encodes."""
    if not math.isfinite(duration) or duration <= 0:
        raise ValueError("Некорректная длительность видео")
    if not isinstance(start_sec, int) or start_sec < 0 or start_sec >= duration:
        raise ValueError("Позиция вне длительности видео")
    if start_sec % SEGMENT_SECONDS:
        raise ValueError("Позиция должна совпадать с началом фрагмента")
    source = data.video_path(video)
    target = segment_path(video, start_sec)
    if target.is_file() and target.stat().st_size > 1024:
        return target
    identifier = str(target)
    with _LOCK:
        pending = _PENDING.get(identifier)
        if pending is None:
            pending = threading.Event()
            _PENDING[identifier] = pending
            leader = True
        else:
            leader = False
    if not leader:
        if not pending.wait(timeout=75):
            raise RuntimeError("Таймаут загрузки фрагмента")
        if target.is_file() and target.stat().st_size > 1024:
            return target
        raise RuntimeError("Не удалось загрузить видео")
    try:
        target.parent.mkdir(parents=True, exist_ok=True)
        tmp = target.with_suffix(".partial")
        length = min(SEGMENT_SECONDS, duration - start_sec)
        command = [
            "ffmpeg", "-nostdin", "-hide_banner", "-loglevel", "error",
            "-ss", str(start_sec), "-i", str(source),
            "-t", f"{length:.3f}",
            "-map", "0:v:0", "-map", "0:a:0?",
            "-vf", f"fps=30,scale={STREAM_WIDTH}:-2:flags=fast_bilinear,setsar=1",
            "-c:v", "libx264", "-preset", "ultrafast", "-crf", "29",
            "-pix_fmt", "yuv420p", "-threads", "4",
            "-c:a", "aac", "-b:a", "96k", "-ac", "2",
            "-movflags", "+faststart", "-avoid_negative_ts", "make_zero",
            "-y", "-f", "mp4", str(tmp),
        ]
        try:
            # Limit competing HEVC decoders on a workstation. The request
            # is fast only if several user seeks cannot saturate the machine.
            with _ENCODERS:
                result = subprocess.run(command, capture_output=True, timeout=65, check=False)
            if result.returncode or not tmp.exists() or tmp.stat().st_size < 1024:
                raise RuntimeError("Ошибка кодирования участка для браузера: "+
                                   result.stderr.decode(errors="replace")[-400:])
            if tmp.stat().st_size > MAX_CLIP_BYTES:
                raise RuntimeError("Слишком большой фрагмент видео")
            os.replace(tmp, target)
        finally:
            tmp.unlink(missing_ok=True)
        _cleanup(target.parent, target)
        return target
    finally:
        with _LOCK:
            event = _PENDING.pop(identifier, None)
            if event:
                event.set()
