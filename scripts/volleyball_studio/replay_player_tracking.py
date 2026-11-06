"""CPU-only reconstruction of stable player tracks from immutable RAVEL proposals.

Keep original GPU IDs whenever supported by overlap evidence. Never rewrite the
neural cache; newly recovered tracklets get fresh, higher local ID numbers.
"""
from __future__ import annotations

import json
import math
from collections import Counter, defaultdict
from pathlib import Path


def reconstruct_players(cache: Path, width: int, height: int,
                        threshold: float, original: list[dict]) -> list[dict]:
    from src.inference_player_ball_openvino import PlayerHysteresis

    # Original GPU IDs anchor the identity space across CPU replay versions.
    originals: dict[int, list[dict]] = defaultdict(list)
    original_ids = set()
    for item in original:
        if item.get("class_name") != "player" or item.get("track_id") is None:
            continue
        try:
            identifier = int(item["track_id"])
            frame = int(item["frame_index"])
        except (TypeError, ValueError):
            continue
        original_ids.add(identifier)
        if item.get("interpolated"):
            continue
        box = item.get("bbox_xyxy")
        if not isinstance(box, list) or len(box) != 4:
            continue
        originals[frame].append((identifier, [
            float(box[0])/width, float(box[1])/height,
            float(box[2])/width, float(box[3])/height]))

    def iou(a, b):
        left, top = max(a[0],b[0]),max(a[1],b[1])
        right, bottom = min(a[2],b[2]),min(a[3],b[3])
        inter = max(0,right-left)*max(0,bottom-top)
        aa = max(0,a[2]-a[0])*max(0,a[3]-a[1])
        bb = max(0,b[2]-b[0])*max(0,b[3]-b[1])
        return inter / max(1e-9,aa+bb-inter)

    # Low-confidence boxes can extend a previously recognized player,
    # but do not start a new ID until they cross the user's open threshold.
    tracker = PlayerHysteresis(
        open_threshold=threshold, close_threshold=min(threshold,.05),
        hold_frames=90, match_iou=.05, max_center_distance=.14)
    frames = []
    votes = Counter()
    observed = Counter()
    with (cache / "players.raw.v2.jsonl").open(encoding="utf8") as src:
        for line in src:
            row = json.loads(line)
            frame = int(row["frame"])
            candidates = []
            for item in row["predictions"]:
                if item.get("class_name") != "player" or (
                    float(item.get("score") or 0) < tracker.close_threshold):
                    continue
                box = item.get("bbox_xyxy_norm")
                if not isinstance(box,list) or len(box)!=4:
                    continue
                if not all(math.isfinite(float(x)) for x in box) or (
                    box[2]<=box[0] or box[3]<=box[1]):
                    continue
                candidates.append(dict(item))
            candidates.sort(key=lambda x:float(x["score"]),reverse=True)
            selected=[]
            for candidate in candidates:
                if any(iou(candidate["bbox_xyxy_norm"],other["bbox_xyxy_norm"])>.65
                       for other in selected):
                    continue
                selected.append(candidate)
                if len(selected)>=24:
                    break
            for item in tracker.update(frame,selected):
                if item.get("class_name")!="player" or item.get("interpolated"):
                    continue
                box=item["bbox_xyxy_norm"]
                number=int(item["track_id"])
                observed[number]+=1
                if originals.get(frame):
                    old,overlap=max(
                        ((tid,iou(box,old_box)) for tid,old_box in originals[frame]),
                        key=lambda candidate:candidate[1])
                    if overlap>=.35:
                        votes[(number,old)]+=1
                frames.append((frame,number,item))
    # Deterministic one-to-one assignment preserves original confirmed IDs.
    mapping={}
    claimed=set()
    for (local,old),count in sorted(votes.items(),
                                   key=lambda entry:(-entry[1],-entry[1]/observed[entry[0][0]],
                                                     entry[0][0],entry[0][1])):
        if local in mapping or old in claimed:
            continue
        if count<2 and observed[local]>=3:
            continue
        if count/observed[local]<.35:
            continue
        mapping[local]=old
        claimed.add(old)
    next_id=max(original_ids,default=-1)+1
    for _frame,local,_item in frames:
        if local not in mapping:
            mapping[local]=next_id
            next_id+=1
    result=[]
    for frame,local,item in frames:
        x1,y1,x2,y2=(float(v) for v in item["bbox_xyxy_norm"])
        record={"frame_index":frame,"time_sec":round(frame/30.,4),
                "class_name":"player", "score":float(item["score"]),
                "track_id":mapping[local],
                "bbox_xyxy":[round(x1*width,2),round(y1*height,2),
                             round(x2*width,2),round(y2*height,2)]}
        if item.get("action"):
            record["action"]=item["action"]
            record["action_score"]=item.get("action_score")
        result.append(record)
    # Do not lose a previously verified RAVEL observation just because the
    # proposal decoder or subsequent CPU NMS produced no matching candidate.
    # Never add a second box on top of one already reconstructed in that frame.
    by_frame=defaultdict(list)
    for row in result:
        by_frame[int(row["frame_index"])].append(row)
    for old in original:
        if (old.get("class_name")!="player" or old.get("interpolated")
                or old.get("track_id") is None
                or float(old.get("score") or 0)<threshold):
            continue
        frame=int(old["frame_index"])
        box=old.get("bbox_xyxy")
        if not isinstance(box,list) or len(box)!=4:
            continue
        if any(item["track_id"]==old["track_id"] or
               iou([coord/(width if index%2==0 else height)
                    for index,coord in enumerate(item["bbox_xyxy"])],
                   [coord/(width if index%2==0 else height)
                    for index,coord in enumerate(box)])>.45
               for item in by_frame[frame]):
            continue
        preserved=dict(old)
        preserved["interpolated"]=False
        by_frame[frame].append(preserved)
        result.append(preserved)
    result.sort(key=lambda row:(int(row["frame_index"]),int(row["track_id"])))
    return result
