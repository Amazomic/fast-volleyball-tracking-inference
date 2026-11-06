"""Conservative multi-rally ReID associations for amateur volleyball.

Never rewrites GPU Track IDs and never assumes exactly N players are present.
The caller persists only supported associations, keeping manual links authoritative.
"""
from __future__ import annotations

import math
from collections import defaultdict

import numpy as np


def _conflict(a: str, b: str, tracks: dict, owners: dict) -> bool:
    if owners.get(a) and owners.get(b) and owners[a] != owners[b]:
        return True
    left, right = tracks[a], tracks[b]
    return (left["first_frame"] <= right["last_frame"]
            and right["first_frame"] <= left["last_frame"])


def stitch_rallies(features: dict, tracks: dict, groups: list[dict],
                   owners: dict | None = None, *, threshold_percent: int = 88,
                   session_gap_sec: int = 120, margin: float = .025) -> dict:
    """Join fragments across rallies using mutual-best appearance and complete-link.

    Sessions split only when every player is absent for a long uninterrupted
    interval. This is NOT proof of a change of sides or a new set.
    """
    owners = owners or {}
    threshold = threshold_percent / 100
    valid = {str(k): np.asarray(v, np.float32) for k, v in features.items()
             if str(k) in tracks}
    if not valid:
        return {"groups": [], "unmatched_ids": sorted(tracks, key=int),
                "session_boundaries": []}
    fps = max((float(row.get("fps") or 30) for row in tracks.values()), default=30.)
    timeline = sorted((int(t["first_frame"]), int(t["last_frame"])) for t in tracks.values())
    boundaries, last_end = [], -1
    for first, last in timeline:
        if last_end >= 0 and first - last_end > session_gap_sec * fps:
            boundaries.append(first)
        last_end = max(last_end, last)
    def segment(tid: str) -> int:
        return sum(int(tracks[tid]["first_frame"]) >= at for at in boundaries)
    similarities = {}
    def sim(a, b):
        key = tuple(sorted((a, b)))
        if key not in similarities:
            va, vb = valid[a], valid[b]
            na, nb = np.linalg.norm(va), np.linalg.norm(vb)
            similarities[key] = float(np.dot(va, vb) / (na * nb)) if na > 1e-6 and nb > 1e-6 else -1.
        return similarities[key]

    used, clusters = set(), {}
    for group in groups:
        if group.get("source") in ("manual", "automatic_retrack"):
            continue
        if group.get("similarity") is None or float(group["similarity"]) < threshold - .0001:
            continue
        ids = [str(x) for x in group.get("track_ids", []) if str(x) in valid and str(x) not in used]
        if (len(ids) < 2 or len({segment(x) for x in ids}) != 1
                or any(_conflict(a, b, tracks, owners) or sim(a, b) < threshold
                       for i, a in enumerate(ids) for b in ids[i+1:])):
            continue
        clusters[ids[0]] = ids
        used.update(ids)
    for tid in sorted(valid, key=int):
        if tid not in used:
            clusters[tid] = [tid]

    def compatible(first, second):
        if segment(first[0]) != segment(second[0]):
            return False
        return all(not _conflict(a, b, tracks, owners) and sim(a,b) >= threshold
                   for a in first for b in second)

    # Globally merge mutual-best cluster pairs, checking every cross-pair.
    # A second high-scoring candidate within the margin blocks a merge.
    while len(clusters) > 1:
        names = list(clusters)
        scores = defaultdict(list)
        for i, left in enumerate(names):
            for right in names[i+1:]:
                a, b = clusters[left], clusters[right]
                if not compatible(a,b):
                    continue
                score = sum(sim(x,y) for x in a for y in b) / (len(a)*len(b))
                scores[left].append((score, right))
                scores[right].append((score, left))
        best = {key: sorted(options, reverse=True) for key, options in scores.items()}
        choices = []
        for left, ranked in best.items():
            score,right = ranked[0]
            if left >= right or best.get(right, [(None,None)])[0][1] != left:
                continue
            if (len(ranked) > 1 and score - ranked[1][0] < margin) or (
                    len(best[right]) > 1 and score - best[right][1][0] < margin):
                continue
            choices.append((score, left, right))
        if not choices:
            break
        _score, left, right = max(choices)
        clusters[left] = sorted(clusters[left] + clusters.pop(right), key=int)

    result = []
    for ids in clusters.values():
        if len(ids) < 2:
            continue
        minimum = min(sim(a,b) for i,a in enumerate(ids) for b in ids[i+1:])
        result.append({"id": "group-" + ids[0], "track_ids": ids,
                       "similarity": round(minimum, 4), "source": "amateur_reid"})
    result.sort(key=lambda g:(-len(g["track_ids"]),int(g["track_ids"][0])))
    grouped = {tid for group in result for tid in group["track_ids"]}
    return {"groups":result,
            "unmatched_ids":sorted(set(tracks)-grouped,key=int),
            "session_boundaries":boundaries}


def auto_assignment_proposals(matches: dict, groups: list[dict], tracks: dict,
                              confirmed: dict, *, threshold_percent: int = 97,
                              margin: float = .04) -> dict[str, list[str]]:
    """Select candidate automatic player links without changing confirmed labels.

    Auto links never become confirmed ReID references. Simultaneous track IDs
    cannot be assigned to the same player, including existing manual tracks.
    """
    threshold = threshold_percent / 100
    assigned = dict(confirmed)
    result = defaultdict(list)
    def qualifies(tid):
        row = matches.get(tid) or {}
        similarity = row.get("similarity")
        runner = row.get("runner_up_similarity")
        if similarity is None or row.get("confirmed") or row.get("ambiguous"):
            return None
        if float(similarity) < threshold - .0001:
            return None
        if runner is not None and float(similarity)-float(runner) <= margin + 1e-6:
            return None
        if not row.get("player_id") or int(row.get("reference_count") or 0) < 2:
            return None
        return str(row["player_id"])
    grouped = set()
    candidates = []
    for group in groups:
        ids = [str(tid) for tid in group.get("track_ids", []) if str(tid) in tracks]
        if len(ids) < 2 or group.get("source") in ("manual","automatic_retrack"):
            continue
        people = [qualifies(tid) for tid in ids]
        strong = [p for p in people if p]
        if len(strong) < 2 or len(set(strong)) != 1:
            continue
        target = strong[0]
        # Weak members are not automatically assigned unless they have their
        # own sufficiently reliable evidence for the same person.
        good = [tid for tid in ids if qualifies(tid)==target]
        grouped.update(ids)
        candidates.extend((tid,target) for tid in good)
    for tid in tracks:
        if tid not in grouped and qualifies(tid) and int(tracks[tid].get("detected_frames") or 0)>=8:
            candidates.append((tid,qualifies(tid)))
    candidates.sort(key=lambda pair:(
        -float(matches[pair[0]].get("similarity") or 0), int(pair[0])))
    for tid, pid in candidates:
        if tid in assigned:
            continue
        if any(other != tid and owner == pid
               and _conflict(tid, other, tracks, {})
               for other,owner in assigned.items() if other in tracks):
            continue
        assigned[tid] = pid
        result[pid].append(tid)
    return dict(result)
