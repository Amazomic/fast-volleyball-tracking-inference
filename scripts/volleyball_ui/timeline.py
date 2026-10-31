"""Compact source-time timeline for Volley Studio browser UI.

Rally boundaries are heuristic and may be split when the ball tracker loses the
ball. Event timestamps are measured in analysis-frame time and shifted by the
analysis offset to align with the ORIGINAL video.
"""
from __future__ import annotations

import math

from . import data

ACTION_TYPES = {"attack", "block", "serve", "dig", "overhead_pass", "low_touch"}


def event_time(e: dict, fps: float, offset: float) -> float | None:
    try:
        value = e.get("time_sec")
        if value is None:
            value = int(e["frame"]) / fps
        result = float(value) + offset
        return round(result, 3) if math.isfinite(result) and result >= 0 else None
    except (TypeError, ValueError, KeyError, ZeroDivisionError):
        return None


def timeline(video: str, project_id=None) -> dict:
    data.video_path(video)
    cache = data.find_cache(video, project_id)
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
    # Group start and end by analyzer track ID, not sequence-only pairing.
    groups = {}
    actions = []
    for e in source_events:
        if not isinstance(e, dict):
            continue
        when = event_time(e, fps, offset)
        if when is None:
            continue
        kind = e.get("type")
        if kind in ACTION_TYPES:
            actions.append({
                "id": str(e.get("id") or ""),
                "type": kind, "time": when,
                "confidence": round(float(e.get("confidence") or 0), 3),
                "player_id": str(e["player_track_id"]) if e.get("player_track_id") is not None else None,
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
    return {
        "analyzed": True, "cache_id": cache.name,
        "analysis_offset_sec": offset,
        "rallies": rallies, "actions": actions,
        "notes": "Цветные метки — предполагаемые действия; блоки розыгрышей — приблизительные границы.",
    }
