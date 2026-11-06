"""Reconstruct conservative rally groups from saved ball tracks + contact events.

These are inferred *playing intervals*, not ground-truth serve-to-point labels.
A reliable technical-return track or another serve is a hard boundary.
Only explicit serve events anchor a start, never speculative attacks.
"""
from __future__ import annotations

import hashlib
import json
import math
from typing import Iterable

FPS = 30
DEFAULT_GAP_FRAMES = 90


def rally_selection_id(video: str, cache_id: str, group: dict) -> str:
    """Stable shared identifier for a reconstructed interval in Results and Montage."""
    identity = json.dumps([
        video, cache_id, group["source_start_frame"], group["source_end_frame"],
        sorted(group.get("track_ids", [])),
    ], ensure_ascii=False)
    return hashlib.sha256(identity.encode("utf-8")).hexdigest()[:24]


def _classification(track):
    return track.get("rally_classification") or {}


def _valid_track(track):
    try:
        a,b=int(track["start_frame"]),int(track["last_frame"])
    except (TypeError,ValueError,KeyError):
        return None
    return (a,b) if 0<=a<=b else None


def _strong_barrier(track):
    classification=_classification(track)
    if classification.get("label")=="rally" or classification.get("is_rally") is True:
        return False
    if classification.get("not_rally_reason") != "technical_return":
        return False
    strength=classification.get("technical_return_confidence")
    return strength is None or float(strength)>=.75


def _usable_serves(events):
    serves=set()
    for event in events or []:
        if not isinstance(event,dict) or event.get("type")!="serve":
            continue
        try:
            confidence=float(event.get("confidence") or 0)
            frame=int(event["frame"])
        except (ValueError,TypeError,KeyError):
            continue
        if confidence>=.7 and frame>=0:
            serves.add(frame)
    return sorted(serves)


def rebuild(tracks: Iterable[dict], *, events=None, fps=FPS,
            max_gap_frames=DEFAULT_GAP_FRAMES, padding_frames=45,
            duration_frames=None, min_frames=0):
    if not (fps>0 and max_gap_frames>=0 and padding_frames>=0):
        raise ValueError("Invalid rally grouping options")
    valid=[]
    blockers=[]
    for track in tracks:
        if not isinstance(track,dict):continue
        bounds=_valid_track(track)
        if not bounds:continue
        a,b=bounds
        if _classification(track).get("label")=="rally" or _classification(track).get("is_rally") is True:
            valid.append((a,b,track))
        elif _strong_barrier(track):
            blockers.append((a,b))
    valid.sort(key=lambda entry:(entry[0],entry[1]))
    blockers.sort()
    serves=_usable_serves(events)
    contact_frames=[]
    for event in events or []:
        if not isinstance(event,dict) or "contact" not in event:
            continue
        try:
            frame=int(event["frame"])
            confidence=float(event.get("confidence") or 0)
        except (KeyError,TypeError,ValueError,OverflowError):
            continue
        if frame>=0 and math.isfinite(confidence) and confidence>=.5:
            contact_frames.append(frame)
    grouped=[]
    def barrier_between(start,end):
        if end<start:return False
        return any(a<=end and b>=start for a,b in blockers)
    def serve_between(start,end):
        return any(start<s<=end for s in serves)
    for a,b,track in valid:
        label=_classification(track)
        if grouped:
            g=grouped[-1]
            gap=a-g["detected_end_frame"]-1
            hard_barrier=barrier_between(g["detected_end_frame"]+1,a-1)
            # A new serve is not part of a previous point, even at zero gap.
            new_serve=serve_between(g["detected_end_frame"],a)
            if gap<=max_gap_frames and not hard_barrier and not new_serve:
                g["detected_end_frame"]=max(g["detected_end_frame"],b)
                g["tracks"].append(track)
                g["confidence"]=max(g["confidence"],float(label.get("rally_confidence") or 0))
                continue
        grouped.append({"detected_start_frame":a,"detected_end_frame":b,
                        "tracks":[track],
                        "confidence":float(label.get("rally_confidence") or 0)})
    # Drop *only* after merging so valid short-ball segments contribute to full rallies.
    grouped=[g for g in grouped
             if g["detected_end_frame"]-g["detected_start_frame"]+1>=min_frames]
    end_of_video=(max([g["detected_end_frame"] for g in grouped],default=0)+1
                  if duration_frames is None else max(1,int(duration_frames)))
    for i,g in enumerate(grouped):
        a,b=g["detected_start_frame"],g["detected_end_frame"]
        prev=grouped[i-1]["detected_end_frame"] if i else -1
        nex=grouped[i+1]["detected_start_frame"] if i+1<len(grouped) else end_of_video
        # Only a documented serve may anchor a rally; avoid inventing serves.
        preceding=[f for f in serves if a-round(4*fps)<=f<=a and f>prev
                   and not barrier_between(f,a-1)]
        if preceding:
            a=min(preceding)
            g["serve_frame"]=a
            g["start_evidence"]="detected_serve"
        else:
            g["serve_frame"]=None
            g["start_evidence"]="first_rally_ball_track"
        # The last tracked ball is NOT proof that the point ended.
        g["end_evidence"]="last_rally_ball_track"
        lower=max(0,(prev+a+1)//2)
        upper=min(end_of_video-1,(b+nex-1)//2)
        start=max(lower,a-padding_frames)
        stop=min(upper,b+padding_frames)
        last_touch=max((f for f in contact_frames if b-round(.5*fps)<=f<=b),
                       default=None)
        for x,y in blockers:
            # Do not include a confirmed return ball in context.
            if y>=start and x<a:
                start=max(start,y+1)
            if x<=stop and x>b:
                boundary=x-1
                if last_touch is not None and x-b<=round(.15*fps):
                    boundary=max(boundary,last_touch+round(.75*fps))
                stop=min(stop,boundary)
        later_serves=[f for f in serves if b<f<=stop]
        if later_serves:stop=min(stop,min(later_serves)-1)
        g["source_start_frame"]=min(max(0,start),a)
        g["source_end_frame"]=max(b,stop)
        g["joined_parts"]=len(g["tracks"])
        g["track_ids"]=[t.get("track_id") for t in g["tracks"]]
        g["boundary_method"]="reconstructed_ball_tracks; serve/point not verified"
        g["context_before_frames"]=a-g["source_start_frame"]
        g["context_after_frames"]=g["source_end_frame"]-b
        g["duration_sec"]=round((g["source_end_frame"]-g["source_start_frame"]+1)/fps,4)
    return grouped
