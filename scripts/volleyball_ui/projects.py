"""Persistent projects for one or many source recordings.

Source videos remain in project/; generated project output is stored under
results/projects/<uuid> and inference caches remain under results/analysis.
"""
from __future__ import annotations

import threading
import uuid
from pathlib import Path

from . import data

PROJECTS_DIR = data.RESULTS / "projects"
REGISTER = PROJECTS_DIR / "projects.json"
_LOCK = threading.RLock()


def _load() -> list:
    info = data.read_json(REGISTER, {"schema": 1, "projects": []})
    items = info.get("projects", []) if isinstance(info, dict) else []
    return items if isinstance(items, list) else []


def all_projects() -> list:
    with _LOCK:
        return [dict(p) for p in _load()]


def get_project(identifier: str) -> dict:
    for p in all_projects():
        if p["id"] == identifier:
            return p
    raise ValueError("Проект не найден")


def create_project(name: str, videos: list[str] | None = None) -> dict:
    if not isinstance(name, str) or not 1 <= len(name.strip()) <= 100:
        raise ValueError("Название проекта должно содержать от 1 до 100 символов")
    videos = videos if videos is not None else []
    checked = validate_videos(videos)
    with _LOCK:
        projects = _load()
        record = {"id": uuid.uuid4().hex[:12], "name": name.strip(), "videos": checked}
        projects.append(record)
        data.save_json(REGISTER, {"schema": 1, "projects": projects})
    return record


def validate_videos(videos: list) -> list[str]:
    if not isinstance(videos, list) or len(videos) > 30:
        raise ValueError("Выберите не более 30 видео")
    clean = []
    for name in videos:
        source = data.video_path(name)
        if source.name not in clean:
            clean.append(source.name)
    return clean


def update_project(identifier: str, name=None, videos=None) -> dict:
    with _LOCK:
        projects = _load()
        found = next((p for p in projects if p.get("id") == identifier), None)
        if found is None:
            raise ValueError("Проект не найден")
        if name is not None:
            if not isinstance(name, str) or not 1 <= len(name.strip()) <= 100:
                raise ValueError("Название проекта: от 1 до 100 символов")
            found["name"] = name.strip()
        if videos is not None:
            found["videos"] = validate_videos(videos)
        data.save_json(REGISTER, {"schema": 1, "projects": projects})
        return dict(found)


def project_dir(identifier: str) -> Path:
    get_project(identifier)
    return PROJECTS_DIR / identifier


def project_videos(identifier: str) -> list[Path]:
    p = get_project(identifier)
    if not p["videos"]:
        raise ValueError("Добавьте хотя бы одно видео в проект")
    return [data.video_path(name) for name in p["videos"]]


def project_output(identifier: str) -> Path:
    return project_dir(identifier) / "highlights.mp4"


def project_summary(identifier: str) -> dict:
    p = get_project(identifier)
    output = project_output(identifier)
    return {**p, "analyses": [
        {"video": name, "analyzed": data.find_cache(name) is not None}
        for name in p["videos"]
    ], "highlight_ready": output.is_file() and output.stat().st_size > 0}


def project_report(identifier: str) -> dict:
    """Aggregate event counts across the included source videos.

    Track IDs are video-scoped and are never treated as shared identities.
    Named players are merged only by user-supplied display name.
    """
    from collections import Counter
    p = get_project(identifier)
    totals = Counter()
    rallies = 0
    ids = 0
    participants = {}
    analyzed = 0
    sources = []
    for name in p["videos"]:
        report = data.report(name, identifier)
        sources.append({"name": name, "analyzed": report["analyzed"],
                        "rallies": report["rallies"], "counts": report["counts"]})
        if not report["analyzed"]:
            continue
        analyzed += 1
        totals.update(report["counts"])
        rallies += report["rallies"]
        ids += report.get("track_count", 0)
        for item in report.get("groups", []):
            key=item["name"].casefold()
            group=participants.setdefault(key,{
                "name": item["name"], "total_actions": 0,
                "track_ids": [], "actions": Counter()
            })
            group["total_actions"]+=item["total_actions"]
            group["actions"].update(item["actions"])
            group["track_ids"].extend(name+" #"+str(i) for i in item["track_ids"])
    return {
        "source": p["name"], "project_id": identifier,
        "analyzed": analyzed>0, "analyzed_videos": analyzed,
        "total_videos": len(p["videos"]), "counts": dict(totals),
        "rallies": rallies, "track_count": ids,
        "groups": sorted(({
            **item, "actions": dict(item["actions"])
        } for item in participants.values()),key=lambda x:-x["total_actions"]),
        "sources": sources
    }
