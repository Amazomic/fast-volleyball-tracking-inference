"""Unmodified RAVEL-013 Touch predictions for the preview (not heuristic events).

All timestamps are local to the prepared analysis video. They are intentionally
not limited to detected rallies and are never fed back into event classification.
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
def _touch_index(filename: str, mtime_ns: int, filesize: int):
    payload = json.loads(Path(filename).read_text(encoding="utf-8"))
    model = Path(str(payload.get("model", ""))).stem
    if model != "RAVEL-VB-013-9f":
        return None
    fps = float(payload.get("fps") or 30)
    size = payload.get("source_size") or {}
    width, height = int(size.get("width") or 0), int(size.get("height") or 0)
    floor = float(payload.get("action_threshold") or 0)
    if not (0 < fps <= 240 and width > 0 and height > 0 and 0 <= floor <= 1):
        raise ValueError("Invalid RAVEL Touch metadata")
    events = []
    last_frame = 0
    for item in payload.get("predictions", []):
        if not isinstance(item, dict):
            continue
        try:
            frame = int(item["frame_index"])
            if frame >= 0:
                last_frame = max(last_frame, frame)
            if (item.get("class_name") != "player" or
                    item.get("interpolated") or item.get("action") != "Touch"):
                continue
            score = float(item["action_score"])
            track = int(item["track_id"]) if item.get("track_id") is not None else None
            box = [float(v) for v in item["bbox_xyxy"]]
            if (frame < 0 or not math.isfinite(score) or not 0 <= score <= 1 or
                    not all(map(math.isfinite, box)) or len(box) != 4 or
                    box[2] <= box[0] or box[3] <= box[1]):
                continue
        except (KeyError, TypeError, ValueError, OverflowError):
            continue
        events.append({"frame": frame, "time": round(frame / fps, 4),
                       "score": round(score, 3), "player_id": track,
                       "box": [round(v, 1) for v in box]})
    events.sort(key=lambda x: (x["frame"], x["player_id"] or -1))
    return (fps, width, height, floor, max(1, last_frame / fps),
            events, [item["time"] for item in events])


def touch_predictions(project_id: str, video: str, cache_id: str | None,
                      *, threshold: float = 0.5, start: float | None = None,
                      length: float = 8.0) -> dict:
    assert_project_video(project_id, video)
    try:
        threshold = float(threshold)
        if start is not None:
            start = float(start)
        length = float(length)
    except (ValueError, TypeError, OverflowError) as exc:
        raise ValueError("Invalid RAVEL Touch query") from exc
    if (not math.isfinite(threshold) or not 0 <= threshold <= 1 or
            (start is not None and (not math.isfinite(start) or start < 0)) or
            not math.isfinite(length) or not 0 < length <= 12):
        raise ValueError("Invalid RAVEL Touch query")
    cache = cache_for(video, project_id, cache_id)
    path = (cache / "players.json") if cache else None
    if cache and (cache / "players.touch-preview.json").is_file():
        # A replay may expose valid model Touch below the original decoder floor.
        path = cache / "players.touch-preview.json"
    if not path or not path.is_file():
        return {"available": False, "reason": "missing"}
    stat = path.stat()
    indexed = _touch_index(str(path), stat.st_mtime_ns, stat.st_size)
    if indexed is None:
        return {"available": False, "reason": "different_model"}
    fps, width, height, floor, duration, rows, times = indexed
    effective = max(threshold, floor)
    chosen = [row for row in rows if row["score"] >= effective]
    bins = [0] * 360
    for row in chosen:
        index = min(len(bins)-1, int(row["time"] / duration * len(bins)))
        bins[index] += 1
    answer = {"available": True, "model": "RAVEL-VB-013",
              "threshold": effective, "stored_threshold": floor,
              "count": len(chosen), "fps": fps, "width": width, "height": height,
              "duration": duration, "bins": bins,
              "note": "Raw player-frame Touch classifications; not unique contacts or rallies."}
    if start is not None:
        lo = bisect_left(times, start)
        hi = bisect_right(times, start + length)
        matched = [row for row in rows[lo:hi] if row["score"] >= effective]
        answer.update({"start": start, "length": length,
                       "window_total": len(matched), "truncated": len(matched) > 2500,
                       "events": matched[:2500]})
    return answer
