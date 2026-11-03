"""Frame-exact montage clips cut from ORIGINAL recordings, not analysis proxies.

Neural inference runs at 30 fps in a possibly trimmed/resized analysis MP4.
The cut position therefore maps the selected analysis frame to original time:
  original_sec = analysis_offset_sec + analysis_frame / 30
All rendering happens on the native source's rotated display frame.
"""
from __future__ import annotations

import json
import math
import subprocess
from fractions import Fraction
from pathlib import Path

FPS = 30


def analysis_offset(cache: Path) -> float:
    metadata = cache / "analysis" / "events.json"
    payload = json.loads(metadata.read_text(encoding="utf-8")) if metadata.is_file() else {}
    value = float(payload.get("analysis_offset_sec") or 0)
    if not math.isfinite(value) or value < 0:
        raise ValueError("Некорректное смещение исходного видео в анализе")
    return value


def source_frame_rate(source: Path) -> tuple[float, str]:
    """Return native nominal video FPS (e.g., 60/1, 30000/1001)."""
    result = subprocess.run(
        ["ffprobe", "-v", "error", "-select_streams", "v:0",
         "-show_entries", "stream=r_frame_rate,avg_frame_rate",
         "-of", "json", str(source)],
        capture_output=True, text=True, check=True, timeout=20)
    rows = json.loads(result.stdout).get("streams") or []
    if not rows:
        raise ValueError("Исходник не содержит видеопотока")
    for name in ("avg_frame_rate", "r_frame_rate"):
        raw = rows[0].get(name, "0/0")
        try:
            rate = Fraction(raw)
            if 1 <= float(rate) <= 240:
                # Variable frame rate is approximately 60 fps in mobile files;
                # use the nominal exact rational when it is close enough.
                nominal = Fraction(rows[0].get("r_frame_rate", raw))
                if nominal and abs(float(nominal) - float(rate)) < .2:
                    rate = nominal
                return float(rate), f"{rate.numerator}/{rate.denominator}"
        except (ZeroDivisionError, ValueError, TypeError):
            continue
    raise ValueError("Не удалось определить кадровую частоту исходного видео")


def clip_from_original(source: Path, target: Path, *, start_frame: int,
                       end_frame: int, offset: float) -> dict:
    """Extract one selected rally using native source resolution and rotation.

    FPS is normalized to match tracking, but pixel resolution is NEVER scaled.
    """
    source = Path(source).resolve(strict=True)
    if not source.is_file() or not isinstance(start_frame, int) or not isinstance(end_frame, int):
        raise ValueError("Неверный исходник или границы выбранного эпизода")
    if start_frame < 0 or end_frame < start_frame or not math.isfinite(offset) or offset < 0:
        raise ValueError("Недопустимые границы исходного эпизода")
    seconds = (end_frame - start_frame + 1) / FPS
    native_fps, native_fps_rate = source_frame_rate(source)
    count = max(1, round(seconds * native_fps))
    absolute = offset + start_frame / FPS
    target.parent.mkdir(parents=True, exist_ok=True)
    from .montage_encoding import encode_ffmpeg
    def command(flags):
        return ["ffmpeg", "-nostdin", "-hide_banner", "-loglevel", "error", "-y",
                "-ss", f"{absolute:.6f}", "-i", str(source),
                "-map", "0:v:0", "-an",
                "-vf", f"fps={native_fps_rate},setsar=1",
                "-frames:v", str(count), *flags,
                "-movflags", "+faststart", str(target)]
    print(f"MONTAGE ORIGINAL: {source.name} @ {absolute:.3f}s, "
          f"{count} frames @ {native_fps_rate}, native resolution", flush=True)
    encode_ffmpeg(command, seconds)
    if not target.is_file() or not target.stat().st_size:
        raise RuntimeError("Не удалось вырезать эпизод из оригинального видео")
    return {"source": str(source), "offset_sec": offset,
            "start_sec": round(absolute, 6), "frames": count,
            "duration_sec": seconds, "fps": native_fps,
            "fps_rate": native_fps_rate, "original_resolution": True}


def relative_track(source_track: Path, target_dir: Path, *,
                   start_frame: int, native_fps: float = FPS,
                   native_count: int | None = None) -> Path:
    """Rebase full-source 30 fps indexes onto a locally extracted 30 fps clip."""
    payload = json.loads(source_track.read_text(encoding="utf-8"))
    end_frame = int(payload["last_frame"])
    if int(payload["start_frame"]) != start_frame or end_frame < start_frame:
        raise ValueError("Несовместимые границы трека и исходного эпизода")
    count = (native_count if native_count is not None
             else max(1, round((end_frame - start_frame + 1) * native_fps / FPS)))
    payload["start_frame"] = 0
    payload["last_frame"] = count - 1
    payload["positions"] = [
        [xy, round((int(frame) - start_frame) * native_fps / FPS)]
        for xy, frame in payload.get("positions", [])
        if start_frame <= int(frame) <= end_frame
    ]
    payload["fps"] = native_fps
    target_dir.mkdir(parents=True, exist_ok=True)
    path = target_dir / source_track.name
    path.write_text(json.dumps(payload, ensure_ascii=False), encoding="utf-8")
    return path
