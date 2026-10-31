"""Whole-rally selection from cached game tracks. No neural-network inference.

Boundary status is heuristic: with no court annotation, exact serve/contact/point
events cannot be guaranteed. Adjacent rally tracks within 45 frames are treated
as one continuous rally so a brief ball-detector dropout does not cut the point.
"""
from __future__ import annotations

import json
from pathlib import Path

FPS = 30
MERGE_GAP_FRAMES = 45
LEAD_FRAMES = 60
TAIL_FRAMES = 45


def load_groups(cache: Path, ranking: list[dict] | None = None,
                merge_gap: int = MERGE_GAP_FRAMES) -> list[dict]:
    tracks_path = cache / "analysis" / "game-tracks.json"
    if not tracks_path.is_file():
        raise FileNotFoundError(f"Missing saved game tracks: {tracks_path}")
    tracks = sorted(json.loads(tracks_path.read_text(encoding="utf-8")),
                    key=lambda t: (int(t["start_frame"]), int(t["last_frame"])))
    ranked = {(int(r["start"]), int(r["end"])): r for r in ranking or []}
    groups = []
    current = []

    def flush() -> None:
        if not current:
            return
        start = int(current[0]["start_frame"])
        end = max(int(x["last_frame"]) for x in current)
        entries = [ranked[(int(x["start_frame"]), int(x["last_frame"]))]
                   for x in current
                   if (int(x["start_frame"]), int(x["last_frame"])) in ranked]
        groups.append({
            "detected_start_frame": start,
            "detected_end_frame": end,
            "source_start_frame": start,
            "source_end_frame": end,
            "track_ids": [x.get("track_id") for x in current],
            "joined_parts": len(current),
            "ranking_score": round(max([float(r["score"]) for r in entries], default=0.0)
                                   + 0.3 * (sum(float(r["score"]) for r in entries)
                                            - max([float(r["score"]) for r in entries], default=0.0)), 4),
            "ranked": bool(entries),
            "confidence": round(max((float(r.get("confidence", 0)) for r in entries), default=0), 4),
            "boundary_method": "merged_rally_tracks_with_context; serve/point not verified",
        })

    for t in tracks:
        classification = t.get("rally_classification") or {}
        if classification.get("label") != "rally":
            flush()
            current = []
            continue
        if current and int(t["start_frame"]) - int(current[-1]["last_frame"]) - 1 > merge_gap:
            flush()
            current = []
        current.append(t)
    flush()

    # Prevent context padding from entering a neighboring recorded segment.
    # The adjacent segments may be non-rallies, so use ALL track boundaries.
    for group in groups:
        a = group["detected_start_frame"]
        b = group["detected_end_frame"]
        prev_ends = [int(x["last_frame"]) for x in tracks if int(x["last_frame"]) < a]
        next_starts = [int(x["start_frame"]) for x in tracks if int(x["start_frame"]) > b]
        nearest_prev = max(prev_ends, default=-1)
        nearest_next = min(next_starts, default=10**10)
        group["source_start_frame"] = max(0, a - LEAD_FRAMES, nearest_prev + 1)
        group["source_end_frame"] = min(b + TAIL_FRAMES, nearest_next - 1)
        group["context_before_frames"] = a - group["source_start_frame"]
        group["context_after_frames"] = group["source_end_frame"] - b
        group["duration_sec"] = round((group["source_end_frame"] - group["source_start_frame"] + 1) / FPS, 4)
    return groups


def choose_whole_groups(groups: list[dict], target_sec: float, max_over_sec: float = 6.0) -> list[dict]:
    """Prioritize the highest-scored rallies; never trim a complete scene."""
    ranked = sorted((g for g in groups if g["ranked"]),
                    key=lambda g: (g["ranking_score"], g["duration_sec"]), reverse=True)
    if not ranked:
        raise RuntimeError("No ranked full rallies available")
    cap = round(FPS * (target_sec + max_over_sec))
    chosen, used = [], 0
    for group in ranked:
        duration_frames = group["source_end_frame"] - group["source_start_frame"] + 1
        if used + duration_frames <= cap:
            chosen.append(group.copy())
            used += duration_frames
    # A full rally is always preferred over filling exactly to the requested second.
    if not chosen:
        raise RuntimeError("No complete rally fits the duration budget")
    chosen.sort(key=lambda g: g["source_start_frame"])
    timeline_frames = 0
    for index, g in enumerate(chosen, 1):
        g["sequence"] = index
        g["montage_start_sec"] = round(timeline_frames / FPS, 4)
        timeline_frames += g["source_end_frame"] - g["source_start_frame"] + 1
    return chosen
