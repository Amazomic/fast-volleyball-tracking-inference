"""Whole-rally selection from cached game tracks. No neural-network inference.

Boundary status is heuristic: exact serve-to-point events are not guaranteed.
Both montage and Results share scripts/rally_boundaries.py. Nearby rally tracks
within 90 frames may be joined, except across high-confidence technical returns
or a new detected serve.
"""
from __future__ import annotations

import json
import subprocess
from functools import lru_cache
from pathlib import Path

FPS = 30
MERGE_GAP_FRAMES = 90
LEAD_FRAMES = 60
TAIL_FRAMES = 45


@lru_cache(maxsize=24)
def _video_duration_frames(file: str, mtime_ns: int, size: int) -> int | None:
    # Same normalized analysis duration used by Results, including final-rally context.
    try:
        probe = subprocess.run(
            ["ffprobe", "-v", "error", "-show_entries", "format=duration",
             "-of", "default=noprint_wrappers=1:nokey=1", file],
            capture_output=True, text=True, check=True, timeout=6)
        value = float(probe.stdout.strip())
        return max(1, round(value * FPS)) if 0 < value < 86400 else None
    except (OSError, ValueError, subprocess.SubprocessError):
        return None


def load_groups(cache: Path, ranking: list[dict] | None = None,
                merge_gap: int = MERGE_GAP_FRAMES,
                context_padding: float | None = None,
                min_sec: float = 0, duration_frames: int | None = None) -> list[dict]:
    """Use the SAME boundaries as results, with ranking of every merged part."""
    from scripts.rally_boundaries import rebuild
    tracks_path=cache/"analysis/game-tracks.json"
    if not tracks_path.is_file():
        raise FileNotFoundError(f"Missing saved game tracks: {tracks_path}")
    tracks=json.loads(tracks_path.read_text(encoding="utf-8"))
    event_path=cache/"analysis/events.json"
    payload=json.loads(event_path.read_text(encoding="utf-8")) if event_path.is_file() else {}
    ranked={(int(r["start"]),int(r["end"])):r for r in ranking or []}
    if duration_frames is None:
        media = cache / "analysis.mp4"
        if media.is_file():
            stat = media.stat()
            duration_frames = _video_duration_frames(str(media), stat.st_mtime_ns, stat.st_size)
    pad=max(0,round(context_padding*FPS)) if context_padding is not None else LEAD_FRAMES
    groups=rebuild(tracks,events=payload.get("events",[]),fps=FPS,
                   max_gap_frames=merge_gap,padding_frames=pad,
                   min_frames=round(min_sec*FPS),
                   duration_frames=duration_frames)
    from scripts.volleyball_ui.rally_players import player_info
    occupancy=player_info(cache,[(g["detected_start_frame"],g["detected_end_frame"])
                                 for g in groups])
    for g, players in zip(groups,occupancy):
        g.update(players)
        entries=[ranked[(int(t["start_frame"]),int(t["last_frame"]))]
                 for t in g["tracks"]
                 if (int(t["start_frame"]),int(t["last_frame"])) in ranked]
        scores=[float(e["score"]) for e in entries]
        g["ranking_score"]=round(max(scores,default=0)+
                                 .3*(sum(scores)-max(scores,default=0)),4)
        g["ranked"]=bool(entries)
        g["confidence"]=round(max(
            [g["confidence"]]+[float(e.get("confidence",0)) for e in entries]),4)
        # Reuse classified analysis events for montage selection. The intervals
        # are the same reconstructed ranges shown in Results.
        action_types=("attack", "block", "dig", "overhead_pass", "serve", "low_touch")
        counts={name:0 for name in action_types}
        observed=set()
        for event in payload.get("events") or []:
            if not isinstance(event,dict) or event.get("type") not in counts:
                continue
            try:
                frame=int(event["frame"])
            except (KeyError,TypeError,ValueError):
                continue
            inside=(g["detected_start_frame"]<=frame<=g["detected_end_frame"])
            if event["type"]=="serve" and g.get("serve_frame")==frame:
                inside=True
            if not inside:continue
            identity=event.get("id") or (frame,event["type"],event.get("player_track_id"))
            if identity in observed:continue
            observed.add(identity)
            counts[event["type"]]+=1
        # Older analyses may only have per-track aggregate action features.
        # Use them only when no classified event matches this rally.
        if not any(counts.values()):
            for track in g["tracks"]:
                features=track.get("rally_features") or {}
                for kind in action_types:
                    key={"dig":"dig_count", "overhead_pass":"overhead_count",
                         "low_touch":"low_touch_count"}.get(kind,kind+"_count")
                    counts[kind]+=max(0,int(features.get(key,0) or 0))
        g["action_counts"]=counts
        g["actions_total"]=sum(counts.values())
        # Internal heavy track payloads are not needed by montage selection.
        del g["tracks"]
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
