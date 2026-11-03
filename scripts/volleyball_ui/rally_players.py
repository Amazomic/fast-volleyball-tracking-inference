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

    # ijson streams massive (>1 GB) RAVEL predictions without allocating
    # an entire predictions array. Falling back to events is safer than loading
    # the raw JSON into memory when the optional dependency is missing.
    try:
        import ijson
    except ImportError:
        return {"schema": SCHEMA, "available": False, "frames": {}}

    frames: dict[int, set[str]] = defaultdict(set)
    with file.open("rb") as handle:
        for row in ijson.items(handle, "predictions.item"):
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
        frequency = Counter(identifier for ids in observations for identifier in ids)
        lasting = sorted(tid for tid, hits in frequency.items() if hits >=
                         (2 if len(observations)>=3 else 1))
        result.append({
            "player_count": occupancy[pick],
            "player_track_ids": lasting,
            "linked_player_ids": [],
        })
    return result
