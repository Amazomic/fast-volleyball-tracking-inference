"""Cached appearance references for project players; suggestions never create identities.

Reference sources: user-confirmed track IDs in this inference and photos
in the global player album, from previously confirmed assignments.
"""
from __future__ import annotations

import hashlib
import os
from collections import defaultdict
from pathlib import Path

from . import data

MAX_ALBUM_PHOTOS = 36


def reference_stamp(project_id: str) -> str:
    """Any roster/album/verified-identity change invalidates suggested owners."""
    from .projects import project_dir
    roster = sorted(data.project_player_ids(project_id))
    identity = data.identity_file(project_id)
    album_stamps = []
    for player_id in roster:
        folder = data.RESULTS / "players" / player_id
        manifest = folder / "index.json"
        stat = manifest.stat() if manifest.is_file() else None
        album_stamps.append((player_id, stat.st_mtime_ns if stat else 0,
                             stat.st_size if stat else 0))
    # Automatic links are suggestions, not new trusted training examples.
    # Their writes must not invalidate/re-trigger ReID indefinitely.
    raw = data.read_json(identity, {})
    verified = []
    for video_key, groups in (raw.get("videos", {}) if isinstance(raw, dict) else {}).items():
        for group in groups:
            metadata = group.get("link_meta", {})
            manually_reviewed = [str(tid) for tid in group.get("track_ids", [])
                if metadata.get(str(tid), {}).get("reviewed", True)]
            if manually_reviewed:
                verified.append((video_key, group.get("id"), tuple(sorted(manually_reviewed))))
    payload = repr((roster, album_stamps, sorted(verified),
                    raw.get("auto_rejected", {}) if isinstance(raw, dict) else {}))
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()[:24]


def _album_candidates(player_id: str) -> tuple[Path, str, list[dict]]:
    folder = data.RESULTS / "players" / player_id
    manifest = folder / "index.json"
    raw = data.read_json(manifest, {})
    entries = raw.get("photos", []) if isinstance(raw, dict) else []
    valid = []
    seen = set()
    for entry in reversed(entries) if isinstance(entries, list) else []:
        if not isinstance(entry, dict) or not entry.get("track_assignment_reviewed"):
            continue
        filename = entry.get("filename")
        if (not isinstance(filename, str) or not filename.endswith(".jpg")
                or Path(filename).name != filename):
            continue
        path = folder / "photos" / filename
        if not path.is_file():
            continue
        # Diversify across matches and tracklets before adding other views.
        key = (entry.get("project_id"), entry.get("video"),
               entry.get("cache_id"), entry.get("track_id"))
        if key in seen:
            continue
        seen.add(key)
        valid.append((path, entry))
        if len(valid) >= MAX_ALBUM_PHOTOS:
            break
    stat = manifest.stat() if manifest.is_file() else None
    stamp = hashlib.sha256(repr((stat.st_mtime_ns if stat else 0,
                                 stat.st_size if stat else 0,
                                 [p.name for p, _ in valid])).encode("utf-8")).hexdigest()[:24]
    return folder, stamp, valid


def _album_features(player_id: str, mode: str, get_encoder):
    import numpy as np
    folder, stamp, entries = _album_candidates(player_id)
    from .player_identity_groups import MODEL_NAME, MODEL_ROOT
    model_files = [MODEL_ROOT / (MODEL_NAME + ext) for ext in (".xml", ".bin")]
    model_stamp = [(p.stat().st_size, p.stat().st_mtime_ns) if p.is_file() else None
                   for p in model_files]
    stamp = hashlib.sha256(repr((stamp, model_stamp, "uniform-crop-v2")).encode("utf-8")).hexdigest()[:24]
    destination = folder / ("reid-reference-" + mode + ".npz")
    if destination.is_file():
        try:
            with np.load(destination, allow_pickle=False) as cached:
                if str(cached["stamp"].item()) == stamp:
                    return [(arr.astype(np.float32), tuple(origin))
                            for arr, origin in zip(cached["vectors"], cached["origins"].tolist())]
        except (OSError, ValueError, KeyError):
            pass
    if not entries:
        return []
    import cv2
    crops, origins = [], []
    for path, entry in entries:
        photo = cv2.imread(str(path))
        if photo is None or min(photo.shape[:2]) < 12:
            continue
        # Gallery photos add a 22% horizontal/18% vertical margin around
        # RAVEL's bbox. Match the tighter ~6%/4% crops used for analysis.
        height, width = photo.shape[:2]
        cropped = photo[int(height * .10):max(int(height * .10)+1, int(height * .90)),
                        int(width * .11):max(int(width * .11)+1, int(width * .89))]
        if min(cropped.shape[:2]) < 12:
            continue
        crops.append(cropped)
        origins.append((str(entry.get("project_id") or ""),
                        str(entry.get("video") or ""),
                        str(data.neural_cache_id(entry.get("cache_id")) or ""),
                        str(entry.get("track_id") or "")))
    if not crops:
        return []
    encoder_mode, encoder = get_encoder()
    if encoder_mode != mode:
        # The same embedding space must be used for tracks and references.
        return []
    vectors = []
    for index in range(0, len(crops), 16):
        vectors.extend(encoder(crops[index:index+16]))
    array = np.asarray(vectors, np.float32)
    target = destination.with_suffix(".tmp.npz")
    try:
        np.savez_compressed(target, stamp=np.asarray(stamp),
                            vectors=array.astype(np.float16), origins=np.asarray(origins))
        os.replace(target, destination)
    finally:
        target.unlink(missing_ok=True)
    return list(zip(array, origins))


def match_players(features: dict, project_id: str, video: str, cache_id: str,
                  mode: str, get_encoder, *, threshold_percent: int = 80) -> tuple[dict, dict]:
    """Rank only members of this project's roster. Explicit labels stay untouched.

    Scores are cosine similarities, *not* calibrated probabilities.
    """
    import numpy as np
    roster = {p["id"]: p for p in data.players_for(project_id)}
    if not roster:
        return {}, {}
    canonical = str(data.neural_cache_id(cache_id) or "")
    verified = {}
    for row in data.groups_for(video, project_id, canonical):
        pid = row.get("id")
        if pid not in roster:
            continue
        for tid in row.get("track_ids", []):
            # Legacy links are manually verified; newly proposed links aren't.
            if row.get("link_meta", {}).get(str(tid), {}).get("reviewed", True):
                verified[str(tid)] = pid
    ref_by_player: dict[str, list] = defaultdict(list)
    for tid, pid in verified.items():
        if tid in features:
            ref_by_player[pid].append((
                features[tid], (project_id, video, canonical, tid)))
    for pid in roster:
        ref_by_player[pid].extend(_album_features(pid, mode, get_encoder))

    # All references from the same source track are excluded for that ID to
    # avoid a tautological 100% match to its own manually enrolled image.
    threshold = threshold_percent / 100
    matches = {}
    counts = {pid: len(ref_by_player[pid]) for pid in roster}
    for tid, vector in features.items():
        if tid in verified:
            pid = verified[tid]
            matches[str(tid)] = {
                "player_id": pid, "name": roster[pid]["name"],
                "similarity": None, "above_threshold": True,
                "confirmed": True, "ambiguous": False,
                "reference_count": counts[pid], "runner_up_similarity": None,
                "source": "manual_verified",
            }
            continue
        candidates = []
        for pid, refs in ref_by_player.items():
            others = [(r, origin) for r, origin in refs
                      if tuple(origin) != (project_id, video, canonical, tid)]
            if not others:
                continue
            sims = [float(np.dot(vector, r)) for r, _origin in others
                    if len(r) == len(vector)]
            if not sims:
                continue
            similarity = max(-1, min(1, max(sims)))
            candidates.append((similarity, pid, len(sims)))
        if not candidates:
            continue
        candidates.sort(reverse=True)
        score, pid, evidence = candidates[0]
        second = candidates[1][0] if len(candidates) > 1 else None
        matches[str(tid)] = {
            "player_id": pid, "name": roster[pid]["name"],
            "similarity": round(score, 4),
            "above_threshold": score >= threshold,
            "ambiguous": bool(second is not None and score == second),
            "reference_count": evidence,
            "runner_up_similarity": round(second, 4) if second is not None else None,
            "source": "confirmed_track_and_album",
        }
    return matches, counts
