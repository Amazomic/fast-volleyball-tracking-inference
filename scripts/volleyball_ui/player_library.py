"""Verified athlete photo albums for a future supervised re-identification model.

Only manually assigned video-scoped tracks are enrolled. Each image keeps source
provenance in index.json. This module does not train or identify people.
"""
from __future__ import annotations

import hashlib
import shutil
import threading
from datetime import datetime, timezone

from . import data
from .photos import photo_path, photos_metadata

_ALBUM_LOCK = threading.RLock()


def album_dir(player_id: str):
    if not any(p["id"] == player_id for p in data.global_players()):
        raise ValueError("Игрок не найден в общей базе")
    return data.RESULTS / "players" / player_id


def photo_counts() -> dict[str, int]:
    result = {}
    for person in data.global_players():
        index = data.read_json(album_dir(person["id"]) / "index.json", {})
        rows = index.get("photos", []) if isinstance(index, dict) else []
        result[person["id"]] = len(rows)
    return result


def _key(project_id: str, video: str, track_id: str, frame: int, cache_id=None) -> str:
    raw = f"{project_id}\x00{video}\x00{track_id}\x00{frame}" + (f"\x00{cache_id}" if cache_id else "")
    return hashlib.sha256(raw.encode("utf-8")).hexdigest()[:24]


def collect_existing_player_photos(player_id: str) -> dict:
    """Backfill already verified tracks from all active projects."""
    album_dir(player_id)
    from .projects import all_projects
    total = 0
    errors = []
    for project in all_projects():
        stored = data._identities(project["id"])["videos"]
        for video in project["videos"]:
            keys = [key for key in stored
                    if key == video or key.startswith(video + "::")]
            for key in keys:
                cache_id = key.split("::", 1)[1] if "::" in key else None
                groups = data.groups_for(video, project["id"], cache_id)
                ids = [tid for group in groups if group["id"] == player_id
                       for tid in group.get("track_ids", [])]
                for start in range(0, len(ids), 200):
                    outcome = sync_photos(
                        project["id"], video, ids[start:start + 200], cache_id)
                    total += outcome["copied"]
                    errors.extend(outcome["errors"])
    return {"copied": total, "errors": errors}


def sync_photos(project_id: str, video: str, track_ids: list[str], cache_id=None) -> dict:
    """Refresh photos for selected IDs based on their *current* manual labels.

    Removes stale photos when an ID is moved or unassigned. The analysis is
    reused; the source video is never modified.
    """
    track_ids = [str(id_) for id_ in track_ids]
    if not track_ids:
        return {"copied": 0, "errors": []}
    copied, errors = 0, []
    with _ALBUM_LOCK:
        # Remove photos for these track keys from any previous owner's album.
        current = {
            str(tid): group["id"]
            for group in data.groups_for(video, project_id, cache_id)
            for tid in group.get("track_ids", [])
        }
        for person in data.global_players():
            folder = album_dir(person["id"])
            manifest = folder / "index.json"
            index = data.read_json(manifest, {"schema": 1, "photos": []})
            rows = index.get("photos", [])
            kept = []
            for row in rows:
                if (row.get("project_id") == project_id and
                    row.get("video") == video and row.get("cache_id") == cache_id and
                    str(row.get("track_id")) in track_ids and
                    current.get(str(row.get("track_id"))) != person["id"]):
                    (folder / "photos" / row["filename"]).unlink(missing_ok=True)
                else:
                    kept.append(row)
            if len(kept) != len(rows):
                data.save_json(manifest, {"schema": 1, "photos": kept})
        for tid in track_ids:
            player_id = current.get(tid)
            if player_id is None:
                continue
            folder = album_dir(player_id)
            manifest = folder / "index.json"
            index = data.read_json(manifest, {"schema": 1, "photos": []})
            rows = index.get("photos", [])
            existing = {r["filename"] for r in rows}
            try:
                samples = photos_metadata(video, tid, project_id, cache_id)["samples"]
            except (FileNotFoundError, ValueError, OSError) as error:
                errors.append(f"Track #{tid}: {error}")
                continue
            for sample in samples:
                frame = int(sample["frame"])
                filename = _key(project_id, video, tid, frame, cache_id) + ".jpg"
                if filename in existing:
                    continue
                try:
                    source = photo_path(video, tid, sample["index"], project_id, cache_id)
                    dest = folder / "photos" / filename
                    dest.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copyfile(source, dest)
                    rows.append({
                        "filename": filename, "project_id": project_id,
                        "video": video, "cache_id": cache_id, "track_id": tid, "frame": frame,
                        "confidence": sample.get("confidence"),
                        "source": "manual_track_assignment",
                        "track_assignment_reviewed": True,
                        "photo_reviewed": False,
                        "training_status": "candidate",
                        "saved_at": datetime.now(timezone.utc).isoformat(),
                    })
                    existing.add(filename)
                    copied += 1
                except (RuntimeError, FileNotFoundError, OSError) as error:
                    errors.append(f"Track #{tid}, frame {frame}: {error}")
            data.save_json(manifest, {"schema": 1, "photos": rows})
    return {"copied": copied, "errors": errors}
