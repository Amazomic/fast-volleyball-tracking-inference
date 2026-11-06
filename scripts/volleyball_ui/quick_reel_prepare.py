"""Project-independent preparation using only the shared results/prepared cache."""
from __future__ import annotations

import fcntl
import hashlib
import json
import os
import subprocess
import time
from pathlib import Path

from . import data
from .preview import _read_info
from .video_compat import is_analysis_ready


def prepared_entry(value: str | Path):
    source = Path(value).resolve(strict=True)
    stat = source.stat()
    info = _read_info(str(source),stat.st_size,stat.st_mtime_ns)
    width = int(info["auto_width"])
    identity = {
        "source": str(source), "size": stat.st_size,
        "mtime_ns": stat.st_mtime_ns, "width": width,
        "fps": 30, "schema": 2,
    }
    key = hashlib.sha256(json.dumps(
        identity, sort_keys=True).encode("utf-8")).hexdigest()[:28]
    base = data.RESULTS / "prepared"
    return (source, info, base / (key + ".mp4"),
            base / (key + ".json"), identity)


def _valid(output: Path, marker: Path, identity: dict) -> bool:
    saved = data.read_json(marker, {})
    return (isinstance(saved, dict) and saved.get("identity") == identity
            and saved.get("status") == "ready"
            and output.is_file() and output.stat().st_size > 0)


def ready_source_path(video: str | Path) -> Path | None:
    source, _, output, marker, identity = prepared_entry(video)
    if is_analysis_ready(source, identity["width"]):
        return source
    return output if _valid(output, marker, identity) else None


def prepare_source(video: str, progress=None) -> tuple[Path, bool]:
    """Return (video, reused). No ordinary project references or results."""
    source, info, output, marker, identity = prepared_entry(video)
    if is_analysis_ready(source, identity["width"]):
        if progress:
            progress(100, True)
        return source, True
    output.parent.mkdir(parents=True, exist_ok=True)
    if _valid(output, marker, identity):
        if progress:
            progress(100, True)
        return output, True
    # Same lock used by ordinary project preparation, but not its queue.
    handle = output.with_suffix(".prepare.lock").open("a+b")
    try:
        fcntl.flock(handle.fileno(), fcntl.LOCK_EX)
        if _valid(output, marker, identity):
            if progress:
                progress(100, True)
            return output, True
        temp = output.with_name(output.stem + "." + str(os.getpid()) + ".partial.mp4")
        error_log = output.with_suffix(".ffmpeg.log")
        total = max(.1, float(info["duration_sec"]))
        data.save_json(marker, {"identity": identity, "status": "preparing",
                                "progress": 0})
        try:
            cmd = [
                "ffmpeg", "-hide_banner", "-loglevel", "error", "-nostdin",
                "-y", "-i", str(source), "-vf",
                f"fps=30,scale={identity['width']}:-2,setsar=1",
                "-map", "0:v:0", "-map", "0:a:0?",
                "-c:a", "aac", "-b:a", "128k",
                "-c:v", "libx264", "-preset", "veryfast",
                "-crf", "20", "-threads", "4",
                "-movflags", "+faststart",
                "-progress", "pipe:1", "-nostats", str(temp),
            ]
            last = 0.
            with error_log.open("wb") as errors:
                with subprocess.Popen(
                    cmd, stdout=subprocess.PIPE, stderr=errors,
                    stdin=subprocess.DEVNULL, text=True, errors="replace"
                ) as process:
                    for line in process.stdout:
                        if line.startswith("out_time="):
                            try:
                                h, m, seconds = line.split("=", 1)[1].strip().split(":")
                                percent = min(99.5,
                                    (int(h)*3600+int(m)*60+float(seconds))/total*100)
                            except (ValueError, IndexError):
                                continue
                            if time.monotonic()-last >= .5:
                                data.save_json(marker, {
                                    "identity": identity, "status": "preparing",
                                    "progress": round(percent, 1),
                                })
                                if progress:
                                    progress(percent, False)
                                last = time.monotonic()
                    code = process.wait()
            if code:
                raise RuntimeError("Ошибка подготовки видео FFmpeg: "
                                   + error_log.read_text(
                                       encoding="utf-8", errors="replace")[-400:])
            if not temp.is_file() or temp.stat().st_size <= 0:
                raise RuntimeError("FFmpeg не создал подготовленное видео")
            if prepared_entry(video)[4] != identity:
                raise RuntimeError("Исходный файл изменился во время подготовки")
            temp.replace(output)
            data.save_json(marker, {"identity": identity,
                                    "status": "ready", "progress": 100})
            if progress:
                progress(100, False)
            return output, False
        except Exception as error:
            data.save_json(marker, {"identity": identity, "status": "failed",
                                    "progress": 0, "error": str(error)[:500]})
            raise
        finally:
            temp.unlink(missing_ok=True)
    finally:
        fcntl.flock(handle.fileno(), fcntl.LOCK_UN)
        handle.close()
