"""Persistent projects for one or many source recordings.

Source videos remain in project/; generated project output is stored under
results/projects/<uuid> and inference caches remain under results/analysis.
"""
from __future__ import annotations

import threading
import uuid
import math
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


def validate_ranges(videos: list[str], ranges=None) -> dict:
    """Persist absolute start/end seconds per recording; absence means full video."""
    if ranges is None:
        ranges = {}
    if not isinstance(ranges, dict) or any(k not in videos for k in ranges):
        raise ValueError("Диапазоны должны относиться к выбранным видео")
    from .preview import video_info
    normalized = {}
    for name in videos:
        duration = video_info(name)["duration_sec"]
        segment = ranges.get(name)
        if segment is None:
            normalized[name] = {"start": 0.0, "end": duration}
            continue
        if not isinstance(segment, dict) or set(segment) != {"start", "end"}:
            raise ValueError("Неправильный диапазон: " + name)
        try:
            a, b = float(segment["start"]), float(segment["end"])
        except (TypeError, ValueError, OverflowError) as exc:
            raise ValueError("Некорректные границы для " + name) from exc
        if not all(map(math.isfinite, (a, b))) or not 0 <= a < b <= duration + 0.01 or b - a < 1:
            raise ValueError("Диапазон должен быть в пределах видео (не менее 1 с): " + name)
        normalized[name] = {"start": round(a, 3), "end": round(min(b, duration), 3)}
    return normalized


def project_ranges(identifier: str) -> dict:
    record = get_project(identifier)
    return validate_ranges(record["videos"], record.get("ranges", {}))


def create_project(name: str, videos: list[str] | None = None, ranges: dict | None = None) -> dict:
    if not isinstance(name, str) or not 1 <= len(name.strip()) <= 100:
        raise ValueError("Название проекта должно содержать от 1 до 100 символов")
    videos = videos if videos is not None else []
    checked = validate_videos(videos)
    if not checked:
        raise ValueError("Выберите хотя бы одно видео для проекта")
    checked_ranges = validate_ranges(checked, ranges)
    with _LOCK:
        projects = _load()
        record = {"id": uuid.uuid4().hex[:12], "name": name.strip(), "videos": checked, "ranges": checked_ranges}
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


def update_project(identifier: str, name=None, videos=None, ranges=None) -> dict:
    with _LOCK:
        projects = _load()
        found = next((p for p in projects if p.get("id") == identifier), None)
        if found is None:
            raise ValueError("Проект не найден")
        if name is not None:
            if not isinstance(name, str) or not 1 <= len(name.strip()) <= 100:
                raise ValueError("Название проекта: от 1 до 100 символов")
            found["name"] = name.strip()
        if videos is not None or ranges is not None:
            selected = validate_videos(videos) if videos is not None else found["videos"]
            if not selected:
                raise ValueError("У проекта должно быть хотя бы одно видео")
            existing = {key: value for key, value in found.get("ranges", {}).items() if key in selected}
            found["ranges"] = validate_ranges(selected, ranges if ranges is not None else existing)
            found["videos"] = selected
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
    return {**p, "ranges": project_ranges(identifier), "analyses": [
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
