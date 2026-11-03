"""Compact source-time timeline for Volley Studio browser UI.

Rally boundaries are heuristic and may be split when the ball tracker loses the
ball. Event timestamps are measured in analysis-frame time and shifted by the
analysis offset to align with the ORIGINAL video.
"""
from __future__ import annotations

import math

from . import data

ACTION_TYPES = {"attack", "block", "serve", "dig", "overhead_pass", "low_touch", "unknown_touch"}


def event_time(e: dict, fps: float, offset: float, *, absolute_time: bool = False) -> float | None:
    """The studio v2 contract stores absolute source times, unlike legacy clips."""
    try:
        value = e.get("time_sec")
        if value is None:
            return round(int(e["frame"]) / fps + offset, 3)
        result = float(value) + (0 if absolute_time else offset)
        return round(result, 3) if math.isfinite(result) and result >= 0 else None
    except (TypeError, ValueError, KeyError, ZeroDivisionError):
        return None


def timeline(video: str, project_id=None, *, duration_sec: float | None = None,
             gap_sec: float = 3.0, min_sec: float = 3.0,
             context_sec: float = 1.5, cache_id: str | None = None) -> dict:
    data.video_path(video)
    from .analysis_runs import cache_for
    cache = cache_for(video,project_id,cache_id) if cache_id else data.find_cache(video,project_id)
    if not cache:
        return {"analyzed": False, "rallies": [], "actions": [], "notes": "Нет результатов анализа"}
    payload = data.read_json(cache / "analysis/events.json", {})
    if not isinstance(payload, dict):
        return {"analyzed": False, "rallies": [], "actions": []}
    source_events = payload.get("events", [])
    if not isinstance(source_events, list):
        source_events = []
    fps = float(payload.get("fps") or 30)
    offset = float(payload.get("analysis_offset_sec") or 0)
    if not 0 < fps <= 240 or not math.isfinite(offset):
        raise ValueError("В анализе некорректная частота кадров или смещение")
    # volleyball-events/2 already includes the trim offset in time_sec.
    absolute_time = payload.get("schema") == "volleyball-events/2"
    # Group start and end by analyzer track ID, not sequence-only pairing.
    groups = {}
    actions = []
    for e in source_events:
        if not isinstance(e, dict):
            continue
        when = event_time(e, fps, offset, absolute_time=absolute_time)
        if when is None:
            continue
        kind = e.get("type")
        if kind in ACTION_TYPES:
            ball = e.get("ball")
            point = None
            if isinstance(ball, dict):
                try:
                    x, y = float(ball["x"]), float(ball["y"])
                    if math.isfinite(x) and math.isfinite(y):
                        point = {"x": x, "y": y, "space": ball.get("space", "analysis_pixels")}
                except (KeyError, ValueError, TypeError):
                    pass
            actions.append({
                "id": str(e.get("id") or ""),
                "type": kind, "time": when,
                "confidence": round(float(e.get("confidence") or 0), 3),
                "player_id": str(e["player_track_id"]) if e.get("player_track_id") is not None else None,
                "ball": point,
            })
        if kind in ("rally_start", "rally_end"):
            track_id = str(e.get("track_id") or e.get("id") or "")
            row = groups.setdefault(track_id, {"id": track_id, "start": None, "end": None})
            if kind == "rally_start":
                row["start"] = min(row["start"], when) if row["start"] is not None else when
            else:
                row["end"] = max(row["end"], when) if row["end"] is not None else when
    rallies = [
        {"id": row["id"], "start": row["start"], "end": row["end"], "boundary": "heuristic"}
        for row in groups.values()
        if row["start"] is not None and row["end"] is not None and row["end"] > row["start"]
    ]
    rallies.sort(key=lambda r: r["start"])
    actions.sort(key=lambda a: a["time"])
    from .rally_view import from_tracks, settings
    cfg = settings(gap_sec, min_sec, context_sec)
    # Modern projects contain full tracked trajectories and classification.
    # Older legacy projects only have paired events and retain that fallback.
    if duration_sec is None:
        duration_sec = max([0.0] + [
            float(e.get("frame", 0)) / fps
            for e in source_events if isinstance(e, dict)
        ])
    merged = from_tracks(cache, offset, fps, duration_sec, **{
        "gap_sec": cfg["gap_sec"], "min_sec": cfg["min_sec"],
        "context_sec": cfg["context_sec"],
        "source_video": video,
    })
    if merged is not None:
        rallies = merged["rallies"]
        # Show contact markers only for reconstructed rally windows, not for
        # non-rally trajectories that were never accepted by the classifier.
        actions = [a for a in actions
                   if any(
                       r["detected_start"] <= a["time"] <= r["detected_end"] or
                       (a["type"]=="serve" and r.get("start_evidence")=="detected_serve"
                        and r["start"]<=a["time"]<=r["detected_start"])
                       for r in rallies)]
    return {
        "analyzed": True, "cache_id": cache.name,
        "analysis_offset_sec": offset,
        "coordinate_space": payload.get("coordinate_space"),
        "rally_summary": merged if merged is not None else {"settings": cfg},
        "rallies": rallies, "actions": actions,
        "notes": "Цветные метки — предполагаемые действия; блоки розыгрышей — приблизительные границы.",
    }
