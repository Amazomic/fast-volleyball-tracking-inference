"""Read saved RAVEL-013 ball detections without CPU interpolation or re-tracking.

Returns bounded local-time windows, with detection scores and the original
GPU threshold. Uses an mtime-keyed in-memory index for large match outputs.
"""
from __future__ import annotations

from bisect import bisect_left, bisect_right
from functools import lru_cache
import json
import math
from pathlib import Path

from .analysis_runs import cache_for
from .projects import assert_project_video


@lru_cache(maxsize=2)
def _ball_index(path: str, mtime_ns: int, file_size: int):
    obj = json.loads(Path(path).read_text(encoding="utf-8"))
    fps = float(obj.get("fps") or 30)
    size = obj.get("source_size") or {}
    width = int(size.get("width") or 0)
    height = int(size.get("height") or 0)
    stored_floor = float(obj.get("ball_threshold") or 0)
    if not (1 <= fps <= 240 and width > 0 and height > 0
            and math.isfinite(stored_floor) and 0 <= stored_floor <= 1):
        raise ValueError("Некорректные параметры обнаружений мяча RAVEL")
    frames = {}
    for item in obj.get("predictions", []):
        if not isinstance(item, dict) or item.get("class_name") != "ball":
            continue
        if item.get("interpolated"):
            continue
        try:
            frame = int(item["frame_index"])
            score = float(item["score"])
            x1, y1, x2, y2 = map(float, item["bbox_xyxy"])
        except (KeyError, ValueError, TypeError, OverflowError):
            continue
        if (frame < 0 or not all(math.isfinite(v) for v in
                                 (score, x1, y1, x2, y2))
                or not 0.5 < score <= 1 or x2 <= x1 or y2 <= y1):
            continue
        x = (x1 + x2) / 2
        y = (y1 + y2) / 2
        if not 0 <= x <= width or not 0 <= y <= height:
            continue
        frames.setdefault(frame, []).append([round(x, 1), round(y, 1),
                                              round(score, 3)])
    for row in frames.values():
        row.sort(key=lambda point: point[2], reverse=True)
        del row[4:]
    return fps, width, height, stored_floor, sorted(frames), frames


def ravel_ball_window(project_id: str, video: str, start, length=8,
                      cache_id=None, threshold=None) -> dict:
    assert_project_video(project_id, video)
    try:
        start, length = float(start), float(length)
        threshold = None if threshold is None else float(threshold)
    except (TypeError, ValueError, OverflowError) as exc:
        raise ValueError("Недопустимый интервал или порог RAVEL") from exc
    if (not math.isfinite(start) or start < 0
            or not math.isfinite(length) or not 0 < length <= 12
            or threshold is not None and
            (not math.isfinite(threshold) or not 0 <= threshold <= 1)):
        raise ValueError("Недопустимый интервал или порог RAVEL")
    cache = cache_for(video, project_id, cache_id)
    if cache is None:
        return {"available": False, "reason": "no_inference", "frames": []}
    path = cache / "players.json"
    if not path.is_file():
        return {"available": False, "reason": "no_ravel_ball", "frames": []}
    stat = path.stat()
    fps, width, height, floor, indices, frames = _ball_index(
        str(path), stat.st_mtime_ns, stat.st_size)
    threshold = max(0.5, floor, threshold if threshold is not None else floor)
    left = bisect_left(indices, math.floor(start * fps))
    right = bisect_right(indices, math.ceil((start + length) * fps))
    rows = [
        [frame, [point for point in frames[frame] if point[2] >= threshold]]
        for frame in indices[left:right]
    ]
    rows = [row for row in rows if row[1]]
    return {
        "available": True,
        "source": "RAVEL-013",
        "kind": "saved_neural_detections",
        "fps": fps,
        "width": width,
        "height": height,
        "stored_threshold": floor,
        "effective_threshold": threshold,
        "start": start,
        "length": length,
        "frames": rows,
    }
