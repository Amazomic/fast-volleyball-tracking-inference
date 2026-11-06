"""Private storage for one-click reels. No ordinary project is created."""
from __future__ import annotations
import json
import re
import shutil
import time
import uuid
from pathlib import Path
from . import data

QUICK_ROOT = data.RESULTS / "quick_reels"
_ID = re.compile(r"[a-f0-9]{32}")


def quick_dir(reel_id: str) -> Path:
    if not isinstance(reel_id, str) or not _ID.fullmatch(reel_id):
        raise ValueError("Недопустимый идентификатор быстрого рилса")
    return QUICK_ROOT / reel_id


def create_quick_reel(videos: list[str], audio: dict) -> tuple[str, Path]:
    """Store file references, not project settings or analysis profiles."""
    if not isinstance(videos, list) or not 1 <= len(videos) <= 30:
        raise ValueError("Для быстрого рилса нужно от 1 до 30 видео")
    if len(videos) != len(set(videos)):
        raise ValueError("Одинаковые исходники выбраны повторно")
    from .quick_reel_selection import selection_for
    entries = []
    seen_sources = set()
    seen_names = set()
    for token in videos:
        picked = selection_for(token)
        file = Path(picked["source"])
        if file in seen_sources:
            raise ValueError("Один и тот же исходник выбран дважды")
        seen_sources.add(file)
        name = picked["name"]
        if name in seen_names:
            name = file.stem + " [" + token[:6] + "]" + file.suffix
        seen_names.add(name)
        entries.append({"video": name, "selection_id": token,
                        "source": str(file), "size": picked["size"],
                        "mtime_ns": picked["mtime_ns"]})
    reel_id = uuid.uuid4().hex
    directory = quick_dir(reel_id)
    directory.mkdir(parents=True, exist_ok=False)
    request = {
        "id": reel_id, "videos": entries,
        "audio_path": audio["path"], "duration": audio["duration"],
        "created_at": time.time(), "default_analysis": True,
    }
    data.save_json(directory / "request.json", request)
    data.save_json(directory / "status.json", {
        "id": reel_id, "status": "queued", "stage": "Ожидание подготовки",
        "stage_index": 0, "stage_total": 3, "progress": 0,
        "videos": [{"video": row["video"], "state": "queued", "progress": 0}
                   for row in entries], "updated_at": time.time(),
    })
    return reel_id, directory


def latest_reel_id() -> str | None:
    entry = data.read_json(QUICK_ROOT / "latest.json", {})
    reel_id = entry.get("id") if isinstance(entry, dict) else None
    if isinstance(reel_id, str) and _ID.fullmatch(reel_id) and \
            (quick_dir(reel_id) / "request.json").is_file():
        return reel_id
    # Legacy reels predate latest.json. Recover the most recent standalone
    # request without changing its running analysis or montage process.
    if not QUICK_ROOT.is_dir():
        return None
    candidates = (folder / "request.json" for folder in QUICK_ROOT.iterdir()
                  if folder.is_dir() and _ID.fullmatch(folder.name))
    existing = [path for path in candidates if path.is_file()]
    return max(existing, key=lambda path: path.stat().st_mtime_ns).parent.name \
        if existing else None


def _archive_root() -> Path:
    # Separate durable MP4 outputs from the single latest scratch run.
    return QUICK_ROOT / "completed"


def archived_result_for(reel_id: str) -> tuple[dict, Path] | None:
    quick_dir(reel_id)  # Validate the external ID before using it in a filename.
    root = _archive_root()
    manifest = data.read_json(root / (reel_id + ".json"), {})
    output = root / (reel_id + ".mp4")
    if not isinstance(manifest, dict) or manifest.get("id") != reel_id:
        return None
    if not output.is_file() or output.stat().st_size <= 0:
        return None
    return manifest, output


def archive_completed(reel_id: str) -> bool:
    """Move a finished MP4 into the compact archive before deleting scratch.

    Return False when preservation fails; callers must leave the old folder
    untouched rather than silently discard an export.
    """
    result = result_for(reel_id)
    if result is None:
        return True  # Failed/cancelled or never completed: no ready reel to save.
    manifest, video = result
    if archived_result_for(reel_id) is not None:
        return True
    archive = _archive_root()
    archive.mkdir(parents=True, exist_ok=True)
    target = archive / (reel_id + ".mp4")
    if target.is_file():
        return False  # Do not overwrite unknown or partial archive content.
    compact = dict(manifest)
    request = data.read_json(quick_dir(reel_id) / "request.json", {})
    compact["created_at"] = request.get("created_at", time.time())
    compact["video"] = reel_id + ".mp4"
    metadata_tmp = archive / (reel_id + ".json.tmp")
    metadata_target = archive / (reel_id + ".json")
    # Prepare metadata first. If any filesystem operation fails, restore the
    # MP4 to its old location and leave the old analysis folder intact.
    try:
        data.save_json(metadata_tmp, compact)
        video.replace(target)  # Same filesystem: no large MP4 copy.
        metadata_tmp.replace(metadata_target)
    except OSError:
        if target.is_file() and not video.exists():
            target.replace(video)
        metadata_tmp.unlink(missing_ok=True)
        return False
    return archived_result_for(reel_id) is not None


def _migrate_legacy_results() -> None:
    """Import finished reels from the older per-run layout into one folder.

    Never touch incomplete or currently running tasks; migration moves only the
    completed MP4 and its small manifest, not source videos or scratch caches.
    """
    if not QUICK_ROOT.is_dir():
        return
    for folder in QUICK_ROOT.iterdir():
        if not folder.is_dir() or not _ID.fullmatch(folder.name):
            continue
        if status_for(folder.name).get("status") != "complete":
            continue
        if archived_result_for(folder.name) is not None:
            continue
        if not archive_completed(folder.name):
            print("QUICK REEL HISTORY: failed to archive:", folder.name, flush=True)


def completed_history() -> list[dict]:
    """Read completed reels from a single, durable MP4+JSON archive folder."""
    _migrate_legacy_results()
    archive = _archive_root()
    if not archive.is_dir():
        return []
    entries = []
    for record in archive.glob("*.json"):
        ident = record.stem
        if not _ID.fullmatch(ident):
            continue
        result = archived_result_for(ident)
        if result is None:
            continue
        manifest, video = result
        names = manifest.get("videos", [])
        entries.append({
            "id": ident,
            "created_at": float(manifest.get("created_at") or video.stat().st_mtime),
            "duration": manifest.get("duration"),
            "videos": names if isinstance(names, list) else [],
            "url": "/api/quick-reel/media?id=" + ident,
            "download_url": "/api/quick-reel/download?id=" + ident,
        })
    return sorted(entries, key=lambda row: (row["created_at"], row["id"]),
                  reverse=True)


def delete_completed(reel_id: str) -> dict:
    """Permanently delete only the selected completed export, not originals."""
    quick_dir(reel_id)  # Strict 32-character lowercase hex ID validation.
    if (reel_id == latest_reel_id() and
            status_for(reel_id).get("status") in ("queued", "running")):
        raise ValueError("Нельзя удалить рилс до завершения обработки")
    archived = archived_result_for(reel_id)
    if archived is None:
        raise ValueError("Готовый рилс не найден")
    manifest = _archive_root() / (reel_id + ".json")
    video = _archive_root() / (reel_id + ".mp4")
    # Remove the large file first. If unlink fails, metadata remains and can
    # still be retried; orphan JSON is ignored by completed_history.
    video.unlink()
    manifest.unlink(missing_ok=True)
    return {"deleted": reel_id}


def publish_latest(reel_id: str, audio_path: str) -> None:
    """Keep scratch data for the newest run, archive finished MP4 outputs."""
    directory = quick_dir(reel_id)
    if not (directory / "request.json").is_file():
        raise ValueError("Запуск быстрого рилса не найден")
    # Publish the active ID first. Cleanup is best effort and cannot invalidate
    # a running reel if a legacy result cannot be removed immediately.
    temporary = QUICK_ROOT / "latest.json.tmp"
    data.save_json(temporary, {"id": reel_id})
    temporary.replace(QUICK_ROOT / "latest.json")
    for item in QUICK_ROOT.iterdir():
        if item.is_dir() and _ID.fullmatch(item.name) and item.name != reel_id:
            try:
                if archive_completed(item.name):
                    shutil.rmtree(item)
                else:
                    print("QUICK REEL ARCHIVE: old folder retained:", item, flush=True)
            except OSError as exc:
                print("QUICK REEL CLEANUP:", item, exc, flush=True)
    selections = QUICK_ROOT / "selections"
    selected = {row.get("selection_id") for row in request_for(reel_id)["videos"]}
    if selections.is_dir():
        for item in selections.glob("*.json"):
            if item.stem not in selected:
                try:
                    item.unlink()
                except OSError as exc:
                    print("QUICK REEL SELECTION CLEANUP:", item, exc, flush=True)
        if not selected:
            try:
                selections.rmdir()
            except OSError:
                pass
    audio_roots = [QUICK_ROOT / "audio"]
    # Remove obsolete uploads stored by the old layout, too. This path is
    # managed by Quick Reel, never a folder with user's original files.
    if QUICK_ROOT.resolve() == (data.RESULTS / "quick_reels").resolve():
        audio_roots.append(data.RESULTS / "quick_reel_audio")
    for audio in audio_roots:
        if not audio.is_dir():
            continue
        for item in audio.iterdir():
            if item.is_file() and item.resolve() != Path(audio_path).resolve():
                try:
                    item.unlink()
                except OSError as exc:
                    print("QUICK REEL AUDIO CLEANUP:", item, exc, flush=True)
        try:
            audio.rmdir()  # Only if it is empty; do not remove the active track.
        except OSError:
            pass


def request_for(reel_id: str) -> dict:
    request = data.read_json(quick_dir(reel_id) / "request.json")
    if not isinstance(request, dict) or request.get("id") != reel_id:
        raise ValueError("Задача быстрого рилса не найдена")
    return request


def status_for(reel_id: str) -> dict:
    value = data.read_json(quick_dir(reel_id) / "status.json", {})
    return value if isinstance(value, dict) else {}


def result_for(reel_id: str) -> tuple[dict, Path] | None:
    directory = quick_dir(reel_id).resolve()
    manifest = data.read_json(directory / "result.json", {})
    if not isinstance(manifest, dict) or manifest.get("id") != reel_id:
        return archived_result_for(reel_id)
    relative = manifest.get("video")
    if not isinstance(relative, str) or not relative:
        return None
    output = (directory / relative).resolve()
    if not output.is_relative_to(directory) or output.suffix.lower() != ".mp4" \
            or not output.is_file() or output.stat().st_size == 0:
        return archived_result_for(reel_id)
    return manifest, output
