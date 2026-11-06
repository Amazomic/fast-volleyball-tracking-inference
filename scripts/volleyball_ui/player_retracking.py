"""Recover provisional player continuity from cached RAVEL detection proposals.

This is a CPU-only read of immutable neural predictions. The original Track IDs
remain unchanged; this module only proposes groups of them and never enrolls
real player_id identities. It is deliberately bounded to short clips.
"""
from __future__ import annotations

from collections import Counter, defaultdict
import json
import math
from pathlib import Path

MAX_RAW_BYTES = 15 * 1024 * 1024
MAX_FRAMES = 3600
DEFAULT_TRACKER = {
    "open_threshold": .42,
    "close_threshold": .20,
    "hold_frames": 60,
    "max_center_distance": .15,
    "match_iou": .08,
}
MAX_VISUAL_GAP_FRAMES = 8 * 30
MAX_VISUAL_DISTANCE = .12
MIN_REID_SIMILARITY = .82
MIN_REID_MARGIN = .08


def _normalized_center(box):
    return ((float(box[0])+float(box[2]))/2,
            (float(box[1])+float(box[3]))/2)


def propose_retracked_groups(cache: Path, features: dict,
                             *, threshold_percent: int = 85,
                             settings: dict | None = None) -> dict | None:
    """Re-track short clip proposals and stitch non-overlapping appearance IDs.

    Returns no suggestions if raw decoder proposals are unavailable. The caller
    must keep persistent, manually confirmed identities strictly separate.
    """
    import numpy as np
    from src.inference_player_ball_openvino import PlayerHysteresis

    cache = Path(cache)
    path = cache / "players.raw.jsonl"
    players_path = cache / "players.json"
    if not path.is_file() or not players_path.is_file():
        return None
    if path.stat().st_size > MAX_RAW_BYTES:
        return None
    player_data = json.loads(players_path.read_text(encoding="utf8"))
    predictions = player_data.get("predictions") or []
    fps = float(player_data.get("fps") or 30)
    source_size = player_data.get("source_size") or {}
    width = float(source_size.get("width") or 0)
    height = float(source_size.get("height") or 0)
    if not (width>0 and height>0 and 1<=fps<=120):
        return None
    original = defaultdict(list)
    original_tracks = defaultdict(list)
    for p in predictions:
        if (not isinstance(p,dict) or p.get("class_name")!="player"
                or p.get("track_id") is None or p.get("interpolated")):
            continue
        box = p.get("bbox_xyxy")
        if not isinstance(box,list) or len(box)!=4:
            continue
        try:
            cx,cy = _normalized_center([
                box[0]/width,box[1]/height,box[2]/width,box[3]/height])
            oid=str(int(p["track_id"]))
            frame=int(p["frame_index"])
            original[frame].append((oid,cx,cy,float(p.get("score") or 0)))
            original_tracks[oid].append((frame,cx,cy))
        except (ValueError,TypeError,ZeroDivisionError):
            continue

    cfg = {**DEFAULT_TRACKER, **(settings or {})}
    tracker = PlayerHysteresis(cfg["open_threshold"],cfg["close_threshold"],
        cfg["hold_frames"],cfg["match_iou"],cfg["max_center_distance"])
    votes = defaultdict(Counter)
    observed = defaultdict(list)
    frame_count = 0
    with path.open(encoding="utf8") as stream:
        for line in stream:
            frame, candidates = json.loads(line)
            frame=int(frame)
            frame_count=max(frame_count,frame+1)
            if frame_count>MAX_FRAMES:
                return None
            detections=[
                {"class_name":"player","frame_index":frame,
                 "score":float(row[0]),"bbox_xyxy_norm":row[1:]}
                for row in candidates if len(row)==5]
            output=tracker.update(frame,detections)
            for p in output:
                if p.get("class_name")!="player" or p.get("interpolated"):
                    continue
                rid=str(p["track_id"])
                cx,cy=_normalized_center(p["bbox_xyxy_norm"])
                observed[rid].append((frame,cx,cy,float(p["score"])))
                # The model proposals and original detections are the SAME
                # boxes. Match them spatially without using appearance labels.
                refs=original.get(frame,())
                if refs:
                    nearest=min(refs,key=lambda a:math.hypot(cx-a[1],cy-a[2]))
                    if math.hypot(cx-nearest[1],cy-nearest[2])<=.018:
                        votes[nearest[0]][rid]+=1
    if not observed:
        return None

    original_to_retrack = {}
    for original_id, counter in votes.items():
        rid, amount=counter.most_common(1)[0]
        if amount>=3 and amount/sum(counter.values())>=.70:
            original_to_retrack[original_id]=rid
    # Some real detections fall below the tracker's 20% closing floor.
    # If their *entire* short fragment is geometrically bracketed by the
    # same persistent track, fill that identity gap without manufacturing
    # observations or assigning a real person_id.
    for oid, fragment in original_tracks.items():
        if oid in original_to_retrack or len(fragment)>45:
            continue
        fragment.sort()
        first,last=fragment[0],fragment[-1]
        candidates=[]
        for rid, track in observed.items():
            before=next((row for row in reversed(track) if row[0]<first[0]),None)
            after=next((row for row in track if row[0]>last[0]),None)
            if before is None or after is None:
                continue
            if first[0]-before[0]>60 or after[0]-last[0]>60:
                continue
            d1=math.hypot(first[1]-before[1],first[2]-before[2])
            d2=math.hypot(last[1]-after[1],last[2]-after[2])
            if max(d1,d2)<=.055:
                candidates.append((d1+d2,rid))
        candidates.sort()
        if candidates and (len(candidates)==1 or
                           candidates[1][0]-candidates[0][0]>=.04):
            rid=candidates[0][1]
            original_to_retrack[oid]=rid
            votes[oid][rid]=len(fragment)
    members=defaultdict(list)
    for original_id, rid in original_to_retrack.items():
        members[rid].append(original_id)

    # Aggregate the already computed OpenVINO ReID embeddings of original IDs.
    # A long, stable fragment should outweigh a few blurry short frames.
    vectors={}
    for rid,ids in members.items():
        entries=[(features[oid],min(votes[oid][rid],150))
                 for oid in ids if oid in features and votes[oid][rid]>0]
        if not entries:
            continue
        arr=np.stack([v for v,_ in entries]).astype(np.float32)
        weights=np.asarray([w for _,w in entries],np.float32)
        center=np.average(arr,axis=0,weights=weights)
        norm=float(np.linalg.norm(center))
        if norm>1e-6:vectors[rid]=center/norm

    # Strong appearance evidence is required for long-gap stitching.
    threshold=max(MIN_REID_SIMILARITY, float(threshold_percent)/100.)
    pair_scores=[]
    ids=sorted(vectors,key=int)
    for i,a in enumerate(ids):
        for b in ids[i+1:]:
            left,right=observed[a],observed[b]
            start_a,end_a=left[0][0],left[-1][0]
            start_b,end_b=right[0][0],right[-1][0]
            if start_a<=end_b and start_b<=end_a:
                continue
            prev,nxt=(left,right) if end_a<start_b else (right,left)
            gap=nxt[0][0]-prev[-1][0]
            if not 0<gap<=max(1,round(MAX_VISUAL_GAP_FRAMES*fps/30)):
                continue
            distance=math.hypot(prev[-1][1]-nxt[0][1],
                                prev[-1][2]-nxt[0][2])
            if distance>MAX_VISUAL_DISTANCE:
                continue
            sim=float(vectors[a]@vectors[b])
            if sim>=threshold:
                pair_scores.append((sim,a,b,gap,distance))
    pair_scores.sort(reverse=True)

    # Mutual-best with a margin; do not bridge multiple uncertain identities.
    selected=[]
    used=set()
    for score,a,b,gap,distance in pair_scores:
        if a in used or b in used:
            continue
        alternative_a=max((s for s,x,y,_,_ in pair_scores
                           if x==a and y!=b or y==a and x!=b),default=-1.)
        alternative_b=max((s for s,x,y,_,_ in pair_scores
                           if x==b and y!=a or y==b and x!=a),default=-1.)
        if score-max(alternative_a,alternative_b)<MIN_REID_MARGIN:
            continue
        selected.append((a,b,score,gap,distance))
        used.update((a,b))

    def _distinct_people_not_combined(ids):
        chosen=set(ids)
        for detections in original.values():
            simultaneous=[p for p in detections
                          if p[0] in chosen and p[3]>=.35]
            for i,left in enumerate(simultaneous):
                for right in simultaneous[i+1:]:
                    if left[0]!=right[0] and math.hypot(
                            left[1]-right[1],left[2]-right[2])>.04:
                        return False
        return True

    result_groups=[]
    all_original=set(original_to_retrack)
    for a,b,score,gap,distance in selected:
        tids=sorted(set(members[a]+members[b]),key=int)
        if len(tids)<2 or not _distinct_people_not_combined(tids):
            continue
        result_groups.append({
            "id":"group-"+tids[0],"track_ids":tids,
            "similarity":round(score,4),"source":"automatic_retrack_reid",
            "evidence":{"gap_frames":gap,
                        "center_distance":round(distance,4)}})
    for rid,oids in members.items():
        if rid in used:continue
        tids=sorted(set(oids),key=int)
        if len(tids)<2 or not _distinct_people_not_combined(tids):
            continue
        result_groups.append({
            "id":"group-"+tids[0],"track_ids":tids,
            "similarity":None,"source":"automatic_retrack"})
    result_groups.sort(key=lambda g:(-len(g["track_ids"]),int(g["track_ids"][0])))
    grouped={tid for g in result_groups for tid in g["track_ids"]}
    total_ids={str(p["track_id"]) for p in predictions
               if isinstance(p,dict) and p.get("class_name")=="player"
               and p.get("track_id") is not None}
    unassigned=sorted(total_ids-grouped,key=int)
    observed_counts=Counter(str(p["track_id"]) for p in predictions
        if isinstance(p,dict) and p.get("class_name")=="player"
        and p.get("track_id") is not None and not p.get("interpolated"))
    # One or two observations do not establish a distinct human identity.
    # Keep those technical IDs visible and unassigned, but exclude them from
    # the *estimate* of stable players.
    unreliable=sorted((tid for tid in unassigned
                       if observed_counts[tid]<3),key=int)
    return {
        "groups":result_groups,"unmatched_ids":unassigned,
        "candidate_people":len(result_groups)+len(unassigned)-len(unreliable),
        "unreliable_track_ids":unreliable,
        "settings":cfg,"source_track_ids":len(total_ids),
        "retracked_ids":len(observed),
        "matched_source_ids":len(all_original),
        "appearance_links":[{"a":a,"b":b,"similarity":round(s,4),
                             "gap_frames":gap,"distance":round(dist,4)}
                            for a,b,s,gap,dist in selected],
        "method":"cached_raw_retrack_and_openvino_reid",
        "confirmed":False,
        "warning":"Candidate identities, not manually verified players.",
    }
