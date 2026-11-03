"""Reconstruct plausible rally intervals from locally classified trajectory segments.

These are *heuristic* intervals, not serve/point ground truth. A ball track is
not an entire rally: only segments classified as rally are considered, and
short tracking gaps can be joined before optional context is applied.
"""
from __future__ import annotations

import math
from pathlib import Path

from .data import read_json

DEFAULT_GAP_SEC = 3.0
DEFAULT_MIN_SEC = 3.0
DEFAULT_CONTEXT_SEC = 1.5


def settings(gap=DEFAULT_GAP_SEC, minimum=DEFAULT_MIN_SEC, context=DEFAULT_CONTEXT_SEC):
    try:
        values = {
            "gap_sec": float(gap), "min_sec": float(minimum), "context_sec": float(context),
        }
    except (ValueError, TypeError, OverflowError) as exc:
        raise ValueError("Настройки розыгрышей должны быть числами") from exc
    limits = {"gap_sec": (0, 12), "min_sec": (0, 40), "context_sec": (0, 10)}
    for name, value in values.items():
        lo, hi = limits[name]
        if not math.isfinite(value) or not lo <= value <= hi:
            raise ValueError(f"{name}: допустимый диапазон {lo}–{hi} секунд")
    return values


def from_tracks(cache: Path, offset: float, fps: float, duration: float,
                *, gap_sec=DEFAULT_GAP_SEC, min_sec=DEFAULT_MIN_SEC,
                context_sec=DEFAULT_CONTEXT_SEC,
                source_video: str | None = None) -> dict | None:
    path = cache / "analysis" / "game-tracks.json"
    if not path.is_file():
        return None
    cfg = settings(gap_sec, min_sec, context_sec)
    items = read_json(path, [])
    if not isinstance(items, list):
        raise ValueError("Некорректная структура сохранённых треков")
    from scripts.rally_boundaries import rebuild, rally_selection_id
    ev=read_json(cache/"analysis/events.json",{})
    serves=ev.get("events",[]) if isinstance(ev,dict) else []
    valid=[t for t in items if isinstance(t,dict) and (
        (t.get("rally_classification") or {}).get("label")=="rally" or
        (t.get("rally_classification") or {}).get("is_rally") is True)]
    groups=rebuild(items,events=serves,fps=fps,
        max_gap_frames=round(cfg["gap_sec"]*fps),
        padding_frames=round(cfg["context_sec"]*fps),
        duration_frames=max(1,round(duration*fps)),
        min_frames=max(0,round(cfg["min_sec"]*fps)))
    from .rally_players import player_info
    occupancy=player_info(cache,[(g["detected_start_frame"],g["detected_end_frame"])
                                 for g in groups],fps=fps)
    output=[]
    for row, players in zip(groups,occupancy):
        a,b=row["detected_start_frame"],row["detected_end_frame"]
        start,end=row["source_start_frame"],row["source_end_frame"]
        output.append({
            "id":f"rally-{a}-{b}",
            "rally_id":rally_selection_id(source_video, cache.name, row) if source_video else None,
            "start":round(offset+start/fps,3),
            "end":round(offset+(end+1)/fps,3),
            "detected_start":round(offset+a/fps,3),
            "detected_end":round(offset+(b+1)/fps,3),
            "joined_parts":row["joined_parts"],
            **players,
            "confidence":round(row["confidence"],3),
            "start_evidence":row["start_evidence"],
            "end_evidence":row["end_evidence"],
            "boundary":row["boundary_method"],
        })
    return {
        "rallies": output,
        "raw_tracks": len(items),
        "classified_tracks": len(valid),
        "unclassified_tracks": len(items) - len(valid),
        "settings": cfg,
    }
