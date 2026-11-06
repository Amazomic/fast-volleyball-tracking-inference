"""Verified athlete photo albums for a future supervised re-identification model.

Only manually assigned video-scoped tracks are enrolled. Each image keeps source
provenance in index.json. This module does not train or identify people.
"""
from __future__ import annotations

import hashlib
import re
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
            seen = set()
            for key in keys:
                cache_id = data.neural_cache_id(
                    key.split("::", 1)[1] if "::" in key else None)
                canonical = video + ("::" + cache_id if cache_id else "")
                if canonical in seen:
                    continue
                seen.add(canonical)
                groups = data.groups_for(video, project["id"], cache_id)
                ids = [tid for group in groups if group["id"] == player_id
                       for tid in group.get("track_ids", [])]
                for start in range(0, len(ids), 200):
                    outcome = sync_photos(
                        project["id"], video, ids[start:start + 200], cache_id)
                    total += outcome["copied"]
                    errors.extend(outcome["errors"])
    return {"copied": total, "errors": errors}


def save_player_photos(project_id: str, video: str, player_id: str, cache_id=None) -> dict:
    album_dir(player_id)
    ids = [str(tid) for group in data.groups_for(video, project_id, cache_id)
           if group["id"] == player_id for tid in group.get("track_ids", [])
           if group.get("link_meta", {}).get(str(tid), {}).get("reviewed", True)]
    result = sync_photos(project_id, video, ids, cache_id)
    return {**result, "count": album_photos(player_id)["count"]}


def sync_photos(project_id: str, video: str, track_ids: list[str], cache_id=None) -> dict:
    """Refresh photos for selected IDs based on their *current* manual labels.

    Removes stale photos when an ID is moved or unassigned. The analysis is
    reused; the source video is never modified.
    """
    cache_id = data.neural_cache_id(cache_id)
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
            if group.get("link_meta", {}).get(str(tid), {}).get("reviewed", True)
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
                data.save_json(manifest, {**index, "schema": 1, "photos": kept})
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
                if filename in existing or filename in set(index.get("rejected", [])):
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
            data.save_json(manifest, {**index, "schema": 1, "photos": rows})
    return {"copied": copied, "errors": errors}


# Albums are global to the person; project removal never touches these files.
_SAFE_PHOTO = re.compile(r"[0-9a-f]{24}\.jpg\Z")


def _album_index(player_id: str):
    folder = album_dir(player_id)
    document = data.read_json(folder / "index.json", {"schema": 1, "photos": []})
    if not isinstance(document, dict):
        return {"schema": 1, "photos": []}
    return document


def album_photos(player_id: str) -> dict:
    """Read only saved photo rows, not project-scoped temporary crops."""
    with _ALBUM_LOCK:
        folder = album_dir(player_id)
        rows = _album_index(player_id).get("photos", [])
        photos = []
        for row in rows if isinstance(rows, list) else []:
            filename = row.get("filename") if isinstance(row, dict) else None
            if (not isinstance(filename, str) or not _SAFE_PHOTO.fullmatch(filename)
                    or not (folder / "photos" / filename).is_file()):
                continue
            photos.append({
                "filename": filename, "frame": row.get("frame"),
                "track_id": row.get("track_id"), "video": row.get("video"),
                "project_id": row.get("project_id"), "saved_at": row.get("saved_at"),
            })
        photos.sort(key=lambda row: (str(row.get("saved_at") or ""), row["filename"]), reverse=True)
        return {"player_id": player_id, "photos": photos, "count": len(photos)}


def album_photo_path(player_id: str, filename: str):
    with _ALBUM_LOCK:
        folder = album_dir(player_id)
        if not isinstance(filename, str) or not _SAFE_PHOTO.fullmatch(filename):
            raise ValueError("Некорректный файл фотографии")
        if not any(isinstance(r, dict) and r.get("filename") == filename
                   for r in _album_index(player_id).get("photos", [])):
            raise FileNotFoundError("Фотография не найдена в базе игрока")
        path = folder / "photos" / filename
        if not path.is_file():
            raise FileNotFoundError("Файл фотографии не найден")
        return path


def remove_album_photo(player_id: str, filename: str) -> dict:
    """Reject this sample permanently, including after future photo sync."""
    with _ALBUM_LOCK:
        folder = album_dir(player_id)
        if not isinstance(filename, str) or not _SAFE_PHOTO.fullmatch(filename):
            raise ValueError("Некорректный файл фотографии")
        index = _album_index(player_id)
        rows = index.get("photos", [])
        if not isinstance(rows, list) or not any(
                isinstance(r, dict) and r.get("filename") == filename for r in rows):
            raise FileNotFoundError("Фотография не найдена в базе игрока")
        index["photos"] = [r for r in rows if r.get("filename") != filename]
        index["rejected"] = sorted(set(index.get("rejected", [])) | {filename})
        data.save_json(folder / "index.json", index)
        (folder / "photos" / filename).unlink(missing_ok=True)
        return {"player_id": player_id, "removed": filename, "count": len(index["photos"])}
