"""Bounded player occupancy snapshots for reconstructed rallies.

A player count is the typical number of simultaneously detected players, not
the number of track IDs (a single athlete may get multiple temporary IDs).
The track IDs are retained for a future user-approved athlete association.
No face recognition or identity inference is performed.
"""
from __future__ import annotations

from collections import Counter, defaultdict
from functools import lru_cache
from pathlib import Path
import math
import statistics

from .data import read_json, save_json

SCHEMA = 1
STEP = 15  # 2 observations per second at a 30 FPS analysis rate.


@lru_cache(maxsize=5)
def _index(path: str, mtime_ns: int, byte_size: int) -> dict:
    file = Path(path)
    sidecar = file.with_name("player-occupancy.json")
    previous = read_json(sidecar, {})
    if (isinstance(previous, dict) and previous.get("schema") == SCHEMA
            and previous.get("mtime_ns") == mtime_ns
            and previous.get("byte_size") == byte_size):
        return previous

    # Stream full matches with ijson; if unavailable, load only small files.
    # A large RAVEL payload must never be materialized just for rally counts.
    try:
        import ijson
    except ImportError:
        ijson = None
        if byte_size > 8 * 1024 * 1024:
            return {"schema": SCHEMA, "available": False, "frames": {}}

    def observations():
        if ijson is None:
            yield from read_json(file, {}).get("predictions", [])
        else:
            with file.open("rb") as handle:
                yield from ijson.items(handle, "predictions.item")

    frames: dict[int, set[str]] = defaultdict(set)
    for row in observations():
        if (not isinstance(row, dict) or row.get("class_name") != "player"
                or row.get("interpolated") or row.get("track_id") is None):
            continue
        try:
            frame = int(row["frame_index"])
            score = float(row.get("score") or 0)
            track = str(int(row["track_id"]))
        except (KeyError, ValueError, TypeError, OverflowError):
            continue
        if frame < 0 or frame % STEP or not math.isfinite(score) or score < .3:
            continue
        frames[frame].add(track)

    result = {"schema": SCHEMA, "available": True, "mtime_ns": mtime_ns,
              "byte_size": byte_size,
              "frames": {str(frame): sorted(identifiers)
                         for frame, identifiers in sorted(frames.items())}}
    save_json(sidecar, result)
    return result


def player_info(cache: Path, ranges: list[tuple[int, int]], *, fps: float = 30) -> list[dict]:
    """Return aligned occupancy and track-id metadata for frame ranges."""
    unknown = {"player_count": None, "player_track_ids": [], "linked_player_ids": []}
    # A CPU replay may filter player detections independently of the original
    # RAVEL output. Rally occupancy must follow the selected result version.
    path = cache / "players.replay.json"
    if not path.is_file():
        if cache.parent.name == "postprocessing":
            cache = cache.parent.parent
        path = cache / "players.json"
    if not path.is_file() or not ranges:
        return [unknown.copy() for _ in ranges]
    stat = path.stat()
    indexed = _index(str(path), stat.st_mtime_ns, stat.st_size)
    if not indexed.get("available"):
        return [unknown.copy() for _ in ranges]
    # Index is sampled in source RAVEL frames. The analyzer normalizes
    # its ball-tracking intervals to 30 FPS as well.
    snapshots = [(int(frame), set(ids)) for frame, ids in indexed["frames"].items()]
    mapping = {}
    sidecar_cache = cache.parent.parent if cache.parent.name == "postprocessing" else cache
    continuity = read_json(sidecar_cache / "player-continuity.json", {})
    if isinstance(continuity, dict) and continuity.get("schema") == 2:
        mapping = continuity.get("track_to_candidate", {})
        if not isinstance(mapping, dict):
            mapping = {}
    result = []
    for begin, end in ranges:
        observations = [ids for frame, ids in snapshots if begin <= frame <= end]
        if not observations:
            result.append(unknown.copy())
            continue
        occupancy = sorted(len(ids) for ids in observations)
        # 75th percentile discounts missed detections but avoids one-frame
        # false-positive spikes (which a raw maximum would count).
        pick = min(len(occupancy)-1, math.floor(.75*(len(occupancy)-1)))
        raw_count = occupancy[pick]
        stable_count = raw_count
        estimated = False
        if mapping:
            # Each candidate must be seen on BOTH sides of any inferred gap.
            # A single false-positive observation cannot create an athlete.
            per_identity = defaultdict(list)
            for frame, ids in snapshots:
                if begin <= frame <= end:
                    for tid in ids:
                        per_identity[mapping.get(tid, "track-"+tid)].append(frame)
            presence = []
            for frame, ids in snapshots:
                if not begin <= frame <= end:
                    continue
                visible = {mapping.get(tid,"track-"+tid) for tid in ids}
                for identity, seen in per_identity.items():
                    if identity in visible or len(seen)<2:
                        continue
                    before = max((f for f in seen if f<frame),default=None)
                    after = min((f for f in seen if f>frame),default=None)
                    if before is not None and after is not None and after-before <= fps*2:
                        visible.add(identity)
                presence.append(len(visible))
            if presence:
                smoothed=sorted(presence)
                candidate=smoothed[min(len(smoothed)-1,math.floor(.75*(len(smoothed)-1)))]
                if candidate>raw_count:
                    stable_count=candidate
                    estimated=True
        frequency = Counter(identifier for ids in observations for identifier in ids)
        lasting = sorted(tid for tid, hits in frequency.items() if hits >=
                         (2 if len(observations)>=3 else 1))
        result.append({
            "player_count": stable_count,
            "observed_player_count": raw_count,
            "player_count_interpolated": estimated,
            "player_track_ids": lasting,
            "linked_player_ids": [],
        })
    return result
