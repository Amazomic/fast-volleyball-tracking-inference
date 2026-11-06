"""Bounded per-frame RAVEL player boxes from already saved players.json."""
from __future__ import annotations
from bisect import bisect_left, bisect_right
from functools import lru_cache
import json
import math
from pathlib import Path

from .data import find_cache
from .projects import assert_project_video


@lru_cache(maxsize=3)
def _index(path: str, mtime_ns: int, size: int):
    obj=json.loads(Path(path).read_text(encoding="utf-8"))
    fps=float(obj.get("fps") or 30)
    dimensions=obj.get("source_size") or {}
    width=int(dimensions.get("width") or 0)
    height=int(dimensions.get("height") or 0)
    if not (1<=fps<=240 and width>0 and height>0):
        raise ValueError("Неправильные размеры результатов RAVEL")
    frames={}
    for p in obj.get("predictions",[]):
        if not isinstance(p,dict) or p.get("class_name")!="player" or p.get("track_id") is None:
            continue
        if p.get("interpolated"):
            continue
        try:
            frame=int(p["frame_index"])
            track=int(p["track_id"])
            score=float(p.get("score") or 0)
            x1,y1,x2,y2=map(float,p["bbox_xyxy"])
        except (ValueError,KeyError,TypeError,OverflowError):
            continue
        if not all(map(math.isfinite,(x1,y1,x2,y2,score))) or frame<0 or track<0:
            continue
        if x2<=x1 or y2<=y1 or score<.02:
            continue
        x1,y1=max(0,x1),max(0,y1)
        x2,y2=min(width,x2),min(height,y2)
        if x2<=x1 or y2<=y1:
            continue
        frames.setdefault(frame,[]).append(
            [track,round(x1,1),round(y1,1),round(x2,1),round(y2,1),round(score,3)]
        )
    return fps,width,height,sorted(frames),frames


@lru_cache(maxsize=3)
def _raw_offsets(path: str, mtime_ns: int, size: int):
    """Sparse seek index; do not load an entire match into RAM."""
    result = []
    with Path(path).open("rb") as handle:
        while True:
            pos = handle.tell()
            line = handle.readline()
            if not line:
                break
            try:
                frame = int(line.split(b",", 1)[0].lstrip(b"["))
            except (ValueError, IndexError):
                continue
            if not result or frame >= result[-1][0] + 120:
                result.append((frame, pos))
    return result


def player_detection_capability(cache: Path) -> dict:
    raw = cache / "players.raw.jsonl"
    meta = cache / "players.raw.json"
    if not raw.is_file() or not meta.is_file():
        return {"available": False, "reason": "legacy"}
    try:
        obj = json.loads(meta.read_text(encoding="utf-8"))
        floor = float(obj["minimum_confidence"])
        baseline = float(obj["score_threshold"])
        replay = cache / "postprocess.manifest.json"
        if replay.is_file():
            settings = json.loads(replay.read_text(encoding="utf-8")).get("settings") or {}
            if settings.get("player_threshold") is not None:
                baseline = float(settings["player_threshold"])
        if obj.get("format") != "ravel-raw-players-v1" or not 0 <= floor <= baseline <= 1:
            raise ValueError("raw RAVEL metadata")
    except (ValueError, TypeError, KeyError, OSError):
        return {"available": False, "reason": "invalid"}
    return {"available": True, "minimum_confidence": floor,
            "analysis_threshold": baseline, "mode": "raw_pre_tracking"}


def _iou(a, b):
    x1, y1 = max(a[0],b[0]), max(a[1],b[1])
    x2, y2 = min(a[2],b[2]), min(a[3],b[3])
    overlap = max(0,x2-x1)*max(0,y2-y1)
    a_area = max(0,a[2]-a[0])*max(0,a[3]-a[1])
    b_area = max(0,b[2]-b[0])*max(0,b[3]-b[1])
    return overlap / max(1e-9, a_area+b_area-overlap)


def _raw_window(path: Path, start_frame: int, end_frame: int, threshold: float,
                width: int, height: int, original: dict):
    stat = path.stat()
    offsets = _raw_offsets(str(path), stat.st_mtime_ns, stat.st_size)
    at = bisect_right([row[0] for row in offsets],start_frame)-1
    first = offsets[max(0,at)][1] if offsets else 0
    output = []
    with path.open("rb") as stream:
        stream.seek(first)
        for line in stream:
            try:
                frame, proposals = json.loads(line)
                frame = int(frame)
            except (TypeError, ValueError):
                continue
            if frame < start_frame:
                continue
            if frame > end_frame:
                break
            source = original.get(frame, [])
            used = set()
            boxes = []
            for p in proposals:
                if not isinstance(p,list) or len(p) != 5:
                    continue
                score, x1,y1,x2,y2 = p
                if (not all(isinstance(v,(int,float)) and math.isfinite(v) for v in p)
                        or score < threshold or x2 <= x1 or y2 <= y1):
                    continue
                box = [max(0,x1*width),max(0,y1*height),
                       min(width,x2*width),min(height,y2*height)]
                best_id, best_overlap = -1, .65
                for i,record in enumerate(source):
                    if i in used:
                        continue
                    overlap = _iou(box,record[1:5])
                    if overlap > best_overlap:
                        best_id,best_overlap = i,overlap
                if best_id >= 0:
                    used.add(best_id)
                    track = source[best_id][0]
                else:
                    track = -1  # Raw detection without a confirmed baseline Track ID.
                boxes.append([track,*[round(v,1) for v in box],round(score,3)])
            if boxes:
                output.append([frame,boxes[:16]])
    return output


def player_boxes_window(project_id: str, video: str, start, length=6,
                        cache_id=None, threshold=None):
    assert_project_video(project_id,video)
    try:
        start=float(start)
        length=float(length)
        threshold=None if threshold is None else float(threshold)
    except (TypeError,ValueError,OverflowError) as e:
        raise ValueError("Недопустимые границы видео или порог игроков") from e
    if (not math.isfinite(start) or not math.isfinite(length) or start<0
            or not 0<length<=10 or (threshold is not None and
            (not math.isfinite(threshold) or not .02<=threshold<=.95))):
        raise ValueError("Недопустимые границы рамок или порог игроков")
    from .analysis_runs import cache_for
    cache=cache_for(video,project_id,cache_id) if cache_id else find_cache(video,project_id)
    if not cache or not (cache/"players.json").is_file():
        return {"available":False,"frames":[]}
    replay_file=cache/"players.replay.json"
    file=replay_file if replay_file.is_file() else cache/"players.json"
    stat=file.stat()
    fps,width,height,keys,frames=_index(str(file),stat.st_mtime_ns,stat.st_size)
    lo=bisect_left(keys,math.floor(start*fps))
    hi=bisect_right(keys,math.ceil((start+length)*fps))
    capability=player_detection_capability(cache)
    if threshold is not None:
        if not capability["available"]:
            return {"available":False,"reason":"no_raw_players","frames":[]}
        if threshold < capability["minimum_confidence"]:
            raise ValueError("Порог ниже сохранённых сырых обнаружений RAVEL")
        rows=_raw_window(cache/"players.raw.jsonl",
            max(0,math.floor(start*fps)),math.ceil((start+length)*fps),
            threshold,width,height,frames)
    else:
        rows=[[frame,frames[frame][:16]] for frame in keys[lo:hi]]
    # Labels are resolved per request, not cached with GPU detections. Renames,
    # manual merges and linked players therefore appear without a CPU rerun.
    identity = {"candidates": {}, "players": {}}
    if project_id:
        from .player_identity_groups import effective_track_identities
        try:
            identity = effective_track_identities(project_id, video, cache_id or cache.name)
        except (FileNotFoundError, ValueError):
            # Legacy/unregistered analyses still display their original IDs.
            pass
    from .data import excluded_track_ids
    excluded = excluded_track_ids(video, project_id, cache_id or cache.name)
    if excluded:
        rows = [[frame, [box for box in boxes if str(box[0]) not in excluded]]
                for frame, boxes in rows]
    return {
        "available":True,"fps":fps,"width":width,"height":height,
        "frames":rows,"supports_threshold":capability["available"],
        "preview_track_ids":threshold is not None,
        "player_labels":identity["players"],
        "candidate_groups":identity["candidates"],
        "frame_step":int(max(1,round(fps*length/max(1,len(rows)))))
                      if rows else 1,
    }
