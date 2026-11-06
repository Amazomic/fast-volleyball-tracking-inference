"""Reusable analysis video inputs, independent of detector settings.

Project videos are normalized at project creation. A selected subrange needs
one accurate FFmpeg trim/encode, shared by all profiles using that range.
Cache files are immutable; per-analysis directories get hardlinks, not copies.
"""
from __future__ import annotations

import json
import os
from pathlib import Path
import shutil

from .common import identity, key, run, save


def _link(source: Path, destination: Path) -> None:
    if destination.exists() or destination.is_symlink():
        destination.unlink()
    try:
        os.link(source, destination)
    except OSError:
        try:
            destination.symlink_to(source)
        except OSError:
            shutil.copyfile(source, destination)


def _valid(path: Path, marker: Path, signature: str) -> bool:
    if not path.is_file() or not marker.is_file():
        return False
    try:
        saved = json.loads(marker.read_text(encoding="utf-8"))
        stat = path.stat()
        return (saved.get("signature") == signature
                and saved.get("size") == stat.st_size and stat.st_size > 0
                and saved.get("mtime_ns") == stat.st_mtime_ns)
    except (ValueError, TypeError, OSError):
        return False


def cached_source_identity(source: Path, prepared: Path | None) -> list:
    """Compute the full source SHA only once for an unchanged project video."""
    source=Path(source).resolve()
    if not prepared:
        return identity(source)
    directory=Path(prepared).resolve().parent
    stat=source.stat()
    signature=key([str(source),stat.st_size,stat.st_mtime_ns])
    marker=directory / ("source-id-" + signature + ".json")
    if marker.is_file():
        try:
            value=json.loads(marker.read_text(encoding="utf-8"))
            stored=value.get("identity")
            if isinstance(stored,list) and len(stored)==4 and (
                stored[:3]==[str(source),stat.st_size,stat.st_mtime_ns]
            ) and isinstance(stored[3],str) and len(stored[3])==64:
                print("VIDEO SOURCE: original SHA reused", flush=True)
                return stored
        except (ValueError,TypeError,OSError):
            pass
    value=identity(source)
    save(marker, {"identity":value})
    return value


def _check_segment_disk_space(input_path: Path, directory: Path, *,
                              start: float, seconds: float | None,
                              width: int) -> None:
    """Reject large FFmpeg outputs early when a near-full disk will fail.

    libx264 ultrafast CRF18 can be much larger than the H.264 source. Use an
    intentionally conservative planning estimate, not a predicted output size.
    Only runs for cache misses: linked prepared sources and cached segments
    don't need this disk reservation.
    """
    import math
    from .common import probe
    stats = shutil.disk_usage(directory)
    info = probe(input_path)
    try:
        duration = float(info.get("format", {}).get("duration", 0))
    except (TypeError, ValueError):
        duration = 0.0
    if not math.isfinite(duration) or duration <= 0:
        duration = 0.0
    remaining = max(0.0, duration - max(0.0, start)) if duration else 0.0
    length = min(remaining, seconds) if seconds and remaining else (
        seconds if seconds else remaining)
    ratio = min(1.0, length / duration) if duration else 1.0
    video = next((x for x in info.get("streams", [])
                  if x.get("codec_type") == "video"), {})
    source_width = int(video.get("width") or width)
    scale = min(4.0, max(0.25, (width/max(1,source_width))**2))
    # Conservative estimate for an intra-heavy fast encode plus a safety
    # margin left for JSON, RAVEL, ball model and subsequent job output.
    estimated_output = max(80 * 2**20,
                           input_path.stat().st_size * ratio * scale * 3.0)
    recommended = estimated_output + 512 * 2**20
    if stats.free < recommended:
        end = start + length if length else duration
        raise RuntimeError(
            "Недостаточно места для создания фрагмента анализа: "
            f"диапазон {start:.1f}–{end:.1f} с; "
            f"свободно {stats.free/2**30:.2f} ГиБ, "
            f"рекомендуется ≥{recommended/2**30:.2f} ГиБ "
            "(оценка с запасом для H.264 и дальнейшего анализа). "
            "Выберите короткий фрагмент 30–90 секунд "
            "или освободите место на диске. "
            "Нейросети ещё не запущены; сначала создаётся MP4.")


def prepare_proxy(*, source: Path, prepared: Path | None, proxy: Path,
                  start: float, seconds: float | None, width: int,
                  threads: int, prepared_input: bool = False) -> str:
    """Return 'reused' or 'created'; never re-encode the same prepared range.

    Full prepared source is linked directly. For a clip, the first run encodes
    it once to a shared segments/ directory, future runs only link the file.
    """
    source = Path(source).resolve()
    prepared = Path(prepared).resolve() if prepared else None
    proxy = Path(proxy)
    proxy.parent.mkdir(parents=True, exist_ok=True)
    input_path = prepared or source
    if not input_path.is_file():
        raise FileNotFoundError(input_path)
    clip_start = float(start or 0)
    clip_seconds = float(seconds) if seconds else None
    # A shared normalized MP4 can be linked directly only when the requested
    # model input dimensions match. Otherwise derive the one-time resized
    # variant FROM the prepared cache, never from the original source.
    prepared_width = None
    if prepared and not prepared_input:
        from .common import probe
        prepared_width = next(
            (int(stream["width"]) for stream in probe(prepared)["streams"]
             if stream.get("codec_type") == "video"), None)
    native_input = bool(prepared_input or (
        prepared and clip_start == 0 and not clip_seconds and
        prepared_width == int(width)))
    stat = input_path.stat()
    spec = ["video-input-v2", str(input_path), stat.st_size, stat.st_mtime_ns,
            clip_start, clip_seconds, int(width), bool(native_input)]
    signature = key(spec)
    local_marker = proxy.parent / "normalize.complete.json"
    if _valid(proxy, local_marker, signature):
        save(proxy.parent / "normalize.status.json",
             {"stage": "normalize", "status": "cached", "signature": signature})
        print("VIDEO INPUT: cached for this analysis", flush=True)
        return "reused"

    if native_input:
        shared = input_path
        operation = "reused"
    else:
        # Shared even when model/threshold/tracker options or analysis code change.
        directory = (prepared.parent if prepared else proxy.parent.parent) / "segments"
        directory.mkdir(parents=True, exist_ok=True)
        shared = directory / (signature + ".mp4")
        manifest = directory / (signature + ".json")
        if _valid(shared, manifest, signature):
            operation = "reused"
        else:
            _check_segment_disk_space(input_path, directory,
                                      start=clip_start, seconds=clip_seconds,
                                      width=width)
            temp = directory / (signature + "." + str(os.getpid()) + ".part.mp4")
            try:
                if temp.exists():
                    temp.unlink()
                command = ["ffmpeg", "-v", "error", "-nostdin", "-y",
                           "-ss", str(clip_start), "-i", str(input_path)]
                if clip_seconds:
                    command += ["-t", str(clip_seconds)]
                command += ["-vf", f"fps=30,scale={width}:-2,setsar=1",
                            "-an", "-c:v", "libx264", "-preset", "ultrafast",
                            "-crf", "18", "-threads", str(threads), str(temp)]
                run(command, proxy.parent / "normalize.log")
                if not temp.is_file() or not temp.stat().st_size:
                    raise RuntimeError("FFmpeg не создал фрагмент для анализа")
                temp.replace(shared)
                stat = shared.stat()
                save(manifest, {"signature": signature, "size": stat.st_size,
                                "mtime_ns": stat.st_mtime_ns})
                operation = "created"
            finally:
                temp.unlink(missing_ok=True)
    # Input video is linked from shared cache rather than copied or transcoded.
    if shared != proxy:
        _link(shared, proxy)
    stat = proxy.stat()
    save(local_marker, {"signature": signature, "size": stat.st_size,
                        "mtime_ns": stat.st_mtime_ns})
    save(proxy.parent / "normalize.status.json",
         {"stage": "normalize", "status": "cached" if operation == "reused" else "complete",
          "signature": signature})
    print("VIDEO INPUT: " + ("prepared input reused; no encoding" if operation == "reused"
                              else "encoded selected range once and cached"), flush=True)
    return operation
