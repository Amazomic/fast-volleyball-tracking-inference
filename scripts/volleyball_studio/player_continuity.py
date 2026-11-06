"""Conservative, unverified continuity proposals between RAVEL player tracklets.

This is CPU-only geometry, not ReID. A candidate group must NEVER be
presented as a confirmed real-world player identity. Human-approved
track -> player assignments live separately in project identity metadata.
"""
from __future__ import annotations

import math
from collections import defaultdict


def group_tracklets(predictions: list[dict], width: float, height: float,
                    fps: float = 30.0) -> dict:
    tracks: dict[str, dict] = {}
    for p in predictions:
        if (p.get("class_name") != "player" or p.get("track_id") is None
                or p.get("interpolated") or float(p.get("score") or 0) < .3):
            continue
        box = p.get("bbox_xyxy")
        if not isinstance(box, (list, tuple)) or len(box) != 4:
            continue
        try:
            frame = int(p["frame_index"])
            x1,y1,x2,y2 = map(float,box)
            tid = str(int(p["track_id"]))
        except (ValueError, TypeError, KeyError, OverflowError):
            continue
        if not all(map(math.isfinite,(x1,y1,x2,y2))) or x2<=x1 or y2<=y1:
            continue
        center = ((x1+x2)/(2*width),(y1+y2)/(2*height))
        body_height = (y2-y1)/height
        current = tracks.get(tid)
        if current is None:
            tracks[tid] = dict(id=tid,first=frame,last=frame,
                               first_center=center,last_center=center,
                               first_height=body_height,last_height=body_height,
                               frames=1)
            continue
        current["frames"]+=1
        if frame<current["first"]:
            current["first"]=frame
            current["first_center"]=center
            current["first_height"]=body_height
        if frame>current["last"]:
            current["last"]=frame
            current["last_center"]=center
            current["last_height"]=body_height

    # Only short, non-overlapping temporal gaps with close bounding boxes.
    # High-scoring ambiguous candidates are intentionally NOT merged.
    usable = sorted((t for t in tracks.values() if t["frames"]>=5),
                    key=lambda t:(t["first"],t["last"],t["id"]))
    out_edges=defaultdict(list)
    in_edges=defaultdict(list)
    for i, first in enumerate(usable):
        for second in usable[i+1:]:
            gap=second["first"]-first["last"]
            if gap<=0 or gap>min(20,int(round(fps*.7))):
                continue
            h1,h2=first["last_height"],second["first_height"]
            if min(h1,h2)<=0:
                continue
            ratio=max(h1,h2)/min(h1,h2)
            if ratio>1.18:
                continue
            distance=math.dist(first["last_center"],second["first_center"])
            if distance>.040:
                continue
            score=.70*(1-distance/.040)+.20*(1-gap/21)+.10*(1-(ratio-1)/.18)
            if score<.75:
                continue
            out_edges[first["id"]].append((score,second["id"]))
            in_edges[second["id"]].append((score,first["id"]))
    # Match only reciprocally unique endpoints: two people near the same
    # position must not be merged just because their detector IDs changed.
    edges=[]
    for source, options in out_edges.items():
        outgoing=sorted(options,reverse=True)
        score,target=outgoing[0]
        if len(outgoing)>1 and score-outgoing[1][0]<.12:
            continue
        incoming=sorted(in_edges[target],reverse=True)
        if incoming[0][1]!=source or (
                len(incoming)>1 and score-incoming[1][0]<.12):
            continue
        edges.append((source,target,round(score,3)))
    previous={target:source for source,target,_ in edges}
    next_id={source:target for source,target,_ in edges}
    memberships={}
    groups=[]
    for track in usable:
        tid=track["id"]
        if tid in memberships:
            continue
        ancestor=tid
        while ancestor in previous:
            ancestor=previous[ancestor]
        if ancestor in memberships:
            continue
        members=[ancestor]
        while members[-1] in next_id:
            nxt=next_id[members[-1]]
            if nxt in members:
                break
            members.append(nxt)
        gid="candidate-"+ancestor
        groups.append({"id":gid,"track_ids":members,
                       "confirmed":False,"evidence":"temporal_geometry_only"})
        for member in members:
            memberships[member]=gid
    for tid in sorted(tracks,key=lambda t:int(t)):
        if tid not in memberships:
            gid="candidate-"+tid
            memberships[tid]=gid
            groups.append({"id":gid,"track_ids":[tid],
                           "confirmed":False,"evidence":"single_tracklet"})
    return {
        "schema":"volley-player-continuity/1",
        "confirmed":False,
        "method":"short_gap_position_size_reciprocal_unambiguous",
        "raw_track_ids":len(tracks),
        "candidate_groups":len(groups),
        "proposed_links":[{"from_track_id":a,"to_track_id":b,"score":score}
                          for a,b,score in edges],
        "track_to_candidate":memberships,
        "groups":groups,
        "warning":"Only provisional geometric continuity; not unique human identities.",
    }
