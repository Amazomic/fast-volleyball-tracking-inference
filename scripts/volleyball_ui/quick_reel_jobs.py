"""Private storage for one-click reels. No ordinary project is created."""
from __future__ import annotations
import json
import re
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
        return None
    relative = manifest.get("video")
    if not isinstance(relative, str) or not relative:
        return None
    output = (directory / relative).resolve()
    if not output.is_relative_to(directory) or output.suffix.lower() != ".mp4" \
            or not output.is_file() or output.stat().st_size == 0:
        return None
    return manifest, output
