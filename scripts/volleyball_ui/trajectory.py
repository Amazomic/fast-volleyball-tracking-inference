"""Read bounded, cached trajectory windows from the local detector CSV.

Times in this endpoint refer to the normalized analysis video, not absolute
original-video timestamps. No inference or external services are used.
"""
from __future__ import annotations

import csv
import json
import math
from bisect import bisect_left, bisect_right
from functools import lru_cache
from pathlib import Path

from .data import find_cache, read_json
from .projects import assert_project_video

MAX_WINDOW_SECONDS = 12.0


@lru_cache(maxsize=4)
def _load_points(path: str, mtime_ns: int, size: int):
    frames = []
    points = []
    with Path(path).open(newline="", encoding="utf-8") as stream:
        for row in csv.DictReader(stream):
            try:
                if float(row["Visibility"]) <= 0:
                    continue
                frame = int(row["Frame"])
                x, y = float(row["X"]), float(row["Y"])
            except (TypeError, ValueError, KeyError):
                continue
            if frame < 0 or not (math.isfinite(x) and math.isfinite(y)) or x < 0 or y < 0:
                continue
            frames.append(frame)
            points.append([frame, round(x, 2), round(y, 2)])
    if frames != sorted(frames):
        points.sort(key=lambda p: p[0])
        frames = [p[0] for p in points]
    return frames, points


@lru_cache(maxsize=4)
def _load_raw_points(path: str, mtime_ns: int, size: int):
    """Read pre-threshold ball proposals; preserve scores for later filtering."""
    points = []
    with Path(path).open(newline="", encoding="utf-8") as stream:
        for row in csv.DictReader(stream):
            try:
                frame = int(row["Frame"])
                score = float(row["Confidence"])
                x, y = float(row["X"]), float(row["Y"])
            except (TypeError, ValueError, KeyError):
                continue
            if (frame < 0 or not all(math.isfinite(v) for v in (score, x, y))
                    or not 0 <= score <= 1 or x < 0 or y < 0):
                continue
            points.append([frame, round(x, 2), round(y, 2), score])
    points.sort(key=lambda p: p[0])
    return points


def ball_detection_capability(cache: Path) -> dict:
    """Metadata only; do not scan the raw CSV when opening a preview."""
    replay = read_json(cache / "postprocess.manifest.json", {})
    mode = (replay.get("settings") or {}).get("mode") if isinstance(replay, dict) else None
    if mode in ("ravel-touch", "ravel-geometry"):
        return {"available": False, "reason": "ravel_ball_source"}
    path = cache / "ball" / "analysis" / "ball.raw.csv"
    metadata = cache / "ball" / "analysis" / "ball.raw.json"
    if not path.is_file() or not metadata.is_file():
        return {"available": False, "reason": "legacy"}
    try:
        info = json.loads(metadata.read_text(encoding="utf-8"))
        threshold = float(info["analysis_threshold"])
        if info.get("format") != "ball-raw-peak-v2" or not 0.05 <= threshold <= .95:
            raise ValueError("Invalid ball raw metadata")
    except (OSError, ValueError, KeyError, TypeError):
        return {"available": False, "reason": "invalid"}
    if mode in ("baseline", "combined"):
        modified = (replay.get("settings") or {}).get("ball_threshold")
        if modified is not None:
            threshold = float(modified)
    return {"available": True, "analysis_threshold": threshold,
            "candidate": "maximum_heatmap_response"}


def trajectory_window(project_id: str, video: str, start: float,
                      length: float = 10.0, cache_id=None, threshold=None,
                      include_scores: bool = False):
    """Return [frame,x,y] in local analysis pixels, no more than 12 seconds."""
    assert_project_video(project_id, video)
    try:
        start = float(start)
        length = float(length)
        if threshold is not None:
            threshold = float(threshold)
    except (TypeError, ValueError, OverflowError) as exc:
        raise ValueError("Некорректные границы траектории или порог мяча") from exc
    if (not math.isfinite(start) or not math.isfinite(length) or start < 0
            or not 0 < length <= MAX_WINDOW_SECONDS
            or (threshold is not None and (
                not math.isfinite(threshold) or not .05 <= threshold <= .95))):
        raise ValueError("Недопустимый диапазон запроса траектории или порог мяча")

    from .analysis_runs import cache_for
    cache = cache_for(video,project_id,cache_id) if cache_id else find_cache(video,project_id)
    if cache is None:
        return {"available": False, "points": []}
    info = read_json(cache / "analysis" / "events.json", {})
    if not isinstance(info, dict):
        info = {}
    # GPU-only primary runs intentionally have no CPU events.json.
    neural = read_json(cache / "inference.manifest.json", {})
    if not isinstance(neural, dict):
        neural = {}
    fps = float(info.get("fps") or neural.get("fps") or 30)
    if not math.isfinite(fps) or not 1 <= fps <= 240:
        raise ValueError("Некорректный FPS в результатах анализа")
    coordinate_space = info.get("coordinate_space") or {}
    if not isinstance(coordinate_space, dict):
        coordinate_space = {}
    width = float(coordinate_space.get("width") or neural.get("width") or 0)
    height = float(coordinate_space.get("height") or neural.get("height") or 0)
    if not all(math.isfinite(n) and n > 0 for n in (width, height)):
        # Old caches may not contain coordinate-space metadata.
        return {"available": False, "points": []}

    csv_path = cache / "ball" / "analysis" / "ball.csv"
    if not csv_path.is_file():
        return {"available": False, "points": []}
    capability = ball_detection_capability(cache)
    if threshold is not None:
        if not capability["available"]:
            return {"available": False, "reason": "no_raw_candidates",
                    "supports_threshold": False, "points": []}
        raw_path = csv_path.with_name("ball.raw.csv")
        stat = raw_path.stat()
        # The saved confidence is not re-estimated from the legacy binary CSV.
        candidates = _load_raw_points(str(raw_path), stat.st_mtime_ns, stat.st_size)
        points = [[frame, x, y, round(score, 3)] if include_scores else
                  [frame, x, y] for frame, x, y, score in candidates
                  if score >= threshold]
        frames = [p[0] for p in points]
    else:
        stat = csv_path.stat()
        frames, points = _load_points(str(csv_path), stat.st_mtime_ns, stat.st_size)
    lo = bisect_left(frames, max(0, math.floor(start * fps)))
    hi = bisect_right(frames, math.ceil((start + length) * fps))
    window = [p for p in points[lo:hi] if p[1] <= width and p[2] <= height]
    # Give the UI an honest way to navigate sparse detections instead of
    # appearing broken when the first visible frame has no ball at all.
    valid = [p[0] for p in points if p[1] <= width and p[2] <= height]
    next_index = bisect_left(valid, math.floor(start * fps))
    return {
        "available": True,
        "supports_threshold": capability["available"],
        "effective_threshold": threshold,
        "analysis_threshold": capability.get("analysis_threshold"),
        "fps": fps,
        "width": int(width),
        "height": int(height),
        "points": window,
        "first_frame": valid[0] if valid else None,
        "next_frame": valid[next_index] if next_index < len(valid) else None,
        "next_after_window": (
            valid[bisect_right(valid, math.ceil((start + length) * fps))]
            if bisect_right(valid, math.ceil((start + length) * fps)) < len(valid)
            else None
        ),
        "detections_total": len(valid),
    }
