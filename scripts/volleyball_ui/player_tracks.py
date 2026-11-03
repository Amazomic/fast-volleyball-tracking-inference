"""Lightweight offline player-track summaries and cautious continuity candidates.

No identity recognition or automatic merging. Only geometric/time evidence from
RAVEL player detections; every proposed association must be approved by user.
"""
from __future__ import annotations

import json
import math
from functools import lru_cache
from pathlib import Path

from .data import find_cache
from .projects import assert_project_video


@lru_cache(maxsize=5)
def _compute(path: str, mtime_ns: int, size: int):
    payload = json.loads(Path(path).read_text(encoding="utf-8"))
    fps = float(payload.get("fps") or 30)
    dimensions = payload.get("source_size") or {}
    width, height = float(dimensions.get("width") or 1), float(dimensions.get("height") or 1)
    tracks = {}
    for item in payload.get("predictions", []):
        if not isinstance(item, dict) or item.get("class_name") != "player" or item.get("track_id") is None:
            continue
        box = item.get("bbox_xyxy")
        frame = item.get("frame_index")
        if not isinstance(box, list) or len(box) != 4 or not isinstance(frame, int):
            continue
        try:
            coords = [float(value) for value in box]
            tid = str(int(item["track_id"]))
            score = float(item.get("score") or 0)
        except (ValueError, TypeError, OverflowError):
            continue
        if not all(math.isfinite(n) for n in (*coords, score)) or coords[2] <= coords[0]:
            continue
        center = [(coords[0]+coords[2])/(2*width), (coords[1]+coords[3])/(2*height)]
        box_height = (coords[3]-coords[1])/height
        t = tracks.setdefault(tid, {"player_track_id":tid,"first_frame":frame,
            "last_frame":frame,"observed_frames":0,"score_sum":0.0,
            "start_center":center,"end_center":center,
            "start_height":box_height,"end_height":box_height})
        if frame <= t["first_frame"]:
            t["first_frame"] = frame
            t["start_center"] = center
            t["start_height"] = box_height
        if frame >= t["last_frame"]:
            t["last_frame"] = frame
            t["end_center"] = center
            t["end_height"] = box_height
        if not item.get("interpolated"):
            t["observed_frames"] += 1
            t["score_sum"] += score
    for t in tracks.values():
        observed=t["observed_frames"]
        t["detected_frames"] = observed
        t["duration_sec"] = round((t["last_frame"]-t["first_frame"]+1)/fps, 2)
        t["start_sec"] = round(t["first_frame"]/fps, 2)
        t["end_sec"] = round(t["last_frame"]/fps, 2)
        t["mean_score"] = round(t["score_sum"]/observed, 3) if observed else 0.0
        del t["score_sum"]
    return {"fps":fps,"tracks":tracks}


def tracks_for(name: str, project_id=None, cache_id=None) -> dict:
    if project_id:
        assert_project_video(project_id,name)
    from .analysis_runs import cache_for
    cache = cache_for(name,project_id,cache_id) if cache_id else find_cache(name,project_id)
    if not cache:
        return {"fps":30,"tracks":{}}
    path=cache/"players.json"
    if not path.is_file():
        return {"fps":30,"tracks":{}}
    stat=path.stat()
    return _compute(str(path),stat.st_mtime_ns,stat.st_size)


def suggest_continuations(name: str, project_id, track_id: str, limit: int = 8, cache_id=None) -> dict:
    if not str(track_id).isascii() or not str(track_id).isdecimal():
        raise ValueError("Некорректный ID игрока")
    data=tracks_for(name,project_id,cache_id)
    tracks=data["tracks"]
    origin=tracks.get(str(track_id))
    if not origin:
        raise FileNotFoundError("ID игрока не найден в сохранённом анализе")
    fps=data["fps"]
    choices=[]
    for tid,t in tracks.items():
        if tid == str(track_id) or t["detected_frames"] < 8:
            continue
        # Never propose overlapping active tracks as an identity match.
        if t["first_frame"] > origin["last_frame"]:
            gap = t["first_frame"]-origin["last_frame"]
            a,b = origin["end_center"],t["start_center"]
            sizes=(origin["end_height"],t["start_height"])
            direction="после"
        elif t["last_frame"] < origin["first_frame"]:
            gap = origin["first_frame"]-t["last_frame"]
            a,b = origin["start_center"],t["end_center"]
            sizes=(origin["start_height"],t["end_height"])
            direction="до"
        else:
            continue
        if not 1 <= gap <= fps*5 or min(sizes) <= 0:
            continue
        distance = math.dist(a,b)
        ratio=max(sizes)/min(sizes)
        if distance>.13 or ratio>1.9:
            continue
        confidence=max(0,1-distance/.13)*.72+max(0,1-gap/(fps*5))*.28
        choices.append({
            "track_id":tid,"gap_sec":round(max(0,gap)/fps,2),
            "distance":round(distance,3),"score":round(confidence,3),
            "direction":direction,"detected_frames":t["detected_frames"],
            "duration_sec":t["duration_sec"],
            "mean_score":t["mean_score"],
            "reason":"Близость рамок во времени и пространстве, не распознавание личности",
        })
    choices.sort(key=lambda t:(-t["score"],-t["detected_frames"]))
    return {"track_id":str(track_id),"candidates":choices[:min(limit,12)],
            "note":"Только гипотезы по координатам; разные игроки рядом могут дать ложное совпадение."}
