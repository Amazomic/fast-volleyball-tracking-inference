"""Persistent projects for one or many source recordings.

Source videos remain in project/; analysis caches and immutable montage
branches live under results/projects/<id>/, while normalized preview videos
are shared by projects under results/prepared/.
"""
from __future__ import annotations

try:
    import fcntl  # POSIX file locking
except ImportError:
    fcntl = None  # Windows uses msvcrt.locking instead
import os
import threading
import uuid
from contextlib import contextmanager
import math
import unicodedata
from pathlib import Path

from . import data

PROJECTS_DIR = data.RESULTS / "projects"
REGISTER = PROJECTS_DIR / "projects.json"
_LOCK = threading.RLock()


@contextmanager
def _registry_write_lock():
    """Serialize the active project registry across both Volley UI servers.

    _LOCK protects threads only; without flock, simultaneous POSTs on ports
    8765 and 8766 can both read the same list and silently lose one project.
    """
    PROJECTS_DIR.mkdir(parents=True, exist_ok=True)
    with (PROJECTS_DIR / ".registry.lock").open("a+b") as handle:
        if fcntl is not None:
            fcntl.flock(handle.fileno(), fcntl.LOCK_EX)
        else:
            import msvcrt
            handle.seek(0)
            handle.write(b"0")
            handle.flush()
            handle.seek(0)
            msvcrt.locking(handle.fileno(), msvcrt.LK_LOCK, 1)
        try:
            yield
        finally:
            if fcntl is not None:
                fcntl.flock(handle.fileno(), fcntl.LOCK_UN)
            else:
                handle.seek(0)
                msvcrt.locking(handle.fileno(), msvcrt.LK_UNLCK, 1)


def _load() -> list:
    info = data.read_json(REGISTER, {"schema": 1, "projects": []})
    items = info.get("projects", []) if isinstance(info, dict) else []
    return items if isinstance(items, list) else []


def all_projects() -> list:
    with _LOCK:
        return [dict(p) for p in _load()]


def deletion_requested(identifier: str) -> bool:
    """Cross-process tombstone blocks work for a project being archived."""
    return (isinstance(identifier, str) and bool(identifier)
            and (PROJECTS_DIR / identifier / ".delete-requested").is_file())


def get_project(identifier: str) -> dict:
    if deletion_requested(identifier):
        raise ValueError("Проект удаляется: запуск новых задач запрещён")
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


def normalized_name(name: str) -> str:
    if not isinstance(name, str):
        raise ValueError("Название проекта должно быть строкой")
    clean = " ".join(unicodedata.normalize("NFKC", name).strip().split())
    if not 1 <= len(clean) <= 100:
        raise ValueError("Название проекта должно содержать от 1 до 100 символов")
    return clean


def _unique_name(projects: list, name: str, exclude_id: str | None = None) -> str:
    clean = normalized_name(name)
    if any(p.get("id") != exclude_id and normalized_name(p.get("name", "")).casefold() == clean.casefold()
           for p in projects):
        raise ValueError(f"Проект с названием «{clean}» уже существует. Укажите другое название.")
    return clean


def _restore_recorded_source(project: dict, video: str) -> None:
    """Repair a vanished import symlink only using this project's saved path.

    Never guess an original from the basename, overwrite a different source,
    or create a link if the source itself no longer exists.
    """
    recorded = project.get("source_paths", {})
    if not isinstance(recorded, dict) or not isinstance(recorded.get(video), str):
        return
    source = Path(recorded[video])
    link = data.PROJECT / video
    if not source.is_absolute() or not source.is_file() or source == link:
        return
    if link.exists() or link.is_symlink():
        return
    data.PROJECT.mkdir(parents=True, exist_ok=True)
    try:
        os.symlink(source, link)
    except FileExistsError:
        pass


def assert_project_video(project_id: str, video: str) -> None:
    project = get_project(project_id)
    if video not in project["videos"]:
        raise ValueError("Это видео не входит в выбранный проект")
    _restore_recorded_source(project, video)
    recorded = project.get("source_paths", {})
    if isinstance(recorded, dict) and video in recorded:
        try:
            if data.video_path(video).resolve(strict=True) != Path(recorded[video]).resolve(strict=True):
                raise ValueError("Источник видео изменился после создания проекта: " + video)
        except (OSError, RuntimeError):
            raise ValueError("Исходный файл видео недоступен: " + video) from None


def project_cache_root(project_id: str) -> Path:
    return project_dir(project_id) / "analysis"


def project_run_root(project_id: str) -> Path:
    return project_dir(project_id) / "runs"


def project_logs_root(project_id: str) -> Path:
    return project_dir(project_id) / "logs"


def _validate_player_ids(player_ids) -> list[str]:
    if not isinstance(player_ids, list) or len(player_ids) > 200:
        raise ValueError("Выберите не более 200 игроков")
    chosen = list(dict.fromkeys(player_ids))
    if not chosen:
        return []
    catalog = {p["id"] for p in data.global_players()}
    if any(not isinstance(pid, str) or pid not in catalog for pid in chosen):
        raise ValueError("Один из выбранных игроков не найден в общей базе")
    return chosen


def create_project(name: str, videos: list[str] | None = None, ranges: dict | None = None,
                   player_ids: list | None = None) -> dict:
    clean_name = normalized_name(name)
    videos = videos if videos is not None else []
    checked = validate_videos(videos)
    if not checked:
        raise ValueError("Выберите хотя бы одно видео для проекта")
    checked_ranges = validate_ranges(checked, ranges)
    sources = {video: str(data.video_path(video).resolve(strict=True)) for video in checked}
    selected_players = _validate_player_ids(player_ids or [])
    with _LOCK, _registry_write_lock():
        projects = _load()
        _unique_name(projects, clean_name)
        record = {"id": uuid.uuid4().hex[:12], "name": clean_name,
                  "videos": checked, "ranges": checked_ranges,
                  "source_paths": sources, "player_ids": selected_players}
        folder = PROJECTS_DIR / record["id"]
        folder.mkdir(parents=True, exist_ok=False)
        try:
            data.save_json(folder / "project.json", record)
            projects.append(record)
            data.save_json(REGISTER, {"schema": 1, "projects": projects})
        except Exception:
            (folder / "project.json").unlink(missing_ok=True)
            folder.rmdir()
            raise
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


def update_project(identifier: str, name=None, videos=None, ranges=None,
                   player_ids=None) -> dict:
    with _LOCK, _registry_write_lock():
        projects = _load()
        found = next((p for p in projects if p.get("id") == identifier), None)
        if found is None:
            raise ValueError("Проект не найден")
        if name is not None:
            found["name"] = _unique_name(projects, name, exclude_id=identifier)
        if player_ids is not None:
            found["player_ids"] = _validate_player_ids(player_ids)
        if videos is not None or ranges is not None:
            selected = validate_videos(videos) if videos is not None else found["videos"]
            if not selected:
                raise ValueError("У проекта должно быть хотя бы одно видео")
            existing = {key: value for key, value in found.get("ranges", {}).items() if key in selected}
            found["ranges"] = validate_ranges(selected, ranges if ranges is not None else existing)
            found["videos"] = selected
            found["source_paths"] = {
                name: str(data.video_path(name).resolve(strict=True)) for name in selected
            }
        data.save_json(project_dir(identifier) / "project.json", found)
        data.save_json(REGISTER, {"schema": 1, "projects": projects})
        return dict(found)


def archive_project(identifier: str) -> dict:
    """Remove a project from the active registry, preserving its directory in .trash.

    Never unlink source recordings from project/ or a shared legacy cache.
    The project-specific analysis, settings and renders are recoverable.
    """
    with _LOCK, _registry_write_lock():
        projects = _load()
        original = next((p for p in projects if p.get("id") == identifier), None)
        if original is None:
            raise ValueError("Проект не найден")
        directory = PROJECTS_DIR / identifier
        trash = PROJECTS_DIR / ".trash"
        trash.mkdir(parents=True, exist_ok=True)
        target = trash / identifier
        if target.exists():
            raise ValueError("Архив проекта уже существует; операция отменена")
        if directory.is_dir():
            directory.rename(target)
        try:
            data.save_json(REGISTER, {"schema": 1, "projects": [
                p for p in projects if p.get("id") != identifier
            ]})
        except BaseException:
            if target.exists() and not directory.exists():
                target.rename(directory)
            raise
        return {"id": identifier, "name": original["name"], "archived": target.is_dir()}


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
    """Project selection must survive missing/unlinked source recordings.

    A missing video is a project-level warning, never a reason to hide every
    project from the picker. Stored ranges were validated when registered.
    """
    p = get_project(identifier)
    output = project_output(identifier)
    analyses = []
    missing = []
    for name in p["videos"]:
        _restore_recorded_source(p, name)
        try:
            assert_project_video(identifier, name)
            data.video_path(name)
        except (ValueError, OSError):
            missing.append(name)
            analyses.append({"video": name, "available": False, "analyzed": False})
            continue
        try:
            analyzed = data.find_cache(name, identifier) is not None
        except (ValueError, OSError):
            analyzed = False
        analyses.append({"video": name, "available": True, "analyzed": analyzed})
    return {**p, "ranges": p.get("ranges", {}), "analyses": analyses,
            "missing_videos": missing,
            "highlight_ready": output.is_file() and output.stat().st_size > 0}


def project_report(identifier: str, run_id: str | None = None) -> dict:
    from collections import Counter
    from .report_statistics import apply_activity, video_statistics, RECOGNIZED_ACTIONS
    from .player_library import album_photos
    p = get_project(identifier)
    totals = Counter()
    rallies = 0
    ids = 0
    total_touches = unassigned_touches = 0
    participants = {
        player["id"]: {"id": player["id"], "name": player["name"],
                       "touches": 0, "recognized_touches": 0, "total_actions": 0,
                       "track_ids": [], "actions": Counter()}
        for player in data.players_for(identifier)
    }
    analyzed = 0
    sources = []
    session = None
    if run_id:
        from .analysis_runs import list_runs
        session = next((r for r in list_runs(identifier) if r["id"] == run_id), None)
        if session is None:
            raise ValueError("Запуск анализа не найден")
    for name in p["videos"]:
        if session and name not in session["videos"]:
            continue
        try:
            cache_id = session["videos"][name] if session else None
            report = data.report(name, identifier,
                                 cache_id)
            stats = video_statistics(name, identifier, cache_id, report)
        except (ValueError, OSError):
            report = {"analyzed": False, "rallies": 0, "counts": {}, "groups": []}
            stats = {"analyzed": False, "rallies": 0, "counts": {},
                     "total_touches": 0, "unassigned_touches": 0, "players": {}}
        sources.append({"name": name, "analyzed": stats["analyzed"],
                        "rallies": stats["rallies"], "counts": stats["counts"],
                        "total_touches": stats["total_touches"]})
        if not stats["analyzed"]:
            continue
        analyzed += 1
        totals.update(stats["counts"])
        rallies += stats["rallies"]
        total_touches += stats["total_touches"]
        unassigned_touches += stats["unassigned_touches"]
        ids += report.get("track_count", 0)
        for item in report.get("groups", []):
            if item["id"] in participants:
                participants[item["id"]]["track_ids"].extend(
                    name+" #"+str(i) for i in item["track_ids"])
        for player_id, row in stats["players"].items():
            group = participants.get(player_id)
            if group is None:
                unassigned_touches += row["touches"]
                continue
            group["touches"] += row["touches"]
            group["total_actions"] += row["touches"]
            group["recognized_touches"] += row["recognized_touches"]
            group["actions"].update(row["actions"])
    players = list(participants.values())
    baseline = apply_activity(players)
    for player in players:
        player["actions"] = dict(player["actions"])
        photos = album_photos(player["id"])["photos"]
        player["photo"] = photos[0]["filename"] if photos else None
        if not analyzed:
            player["activity"] = None
    result = {
        "source": p["name"], "project_id": identifier,
        "analyzed": analyzed>0, "analyzed_videos": analyzed,
        "total_videos": len(p["videos"]), "counts": dict(totals),
        "total_touches": total_touches, "unassigned_touches": unassigned_touches,
        "recognized_touches": sum(totals[k] for k in RECOGNIZED_ACTIONS),
        "median_touches": baseline, "player_count": len(players),
        "rallies": rallies, "track_count": ids,
        "groups": players,
        "sources": sources
    }
    if run_id:
        data.save_json(project_dir(identifier) / "reports" / ("summary." + run_id + ".json"), result)
    else:
        data.save_json(project_dir(identifier) / "reports" / "summary.json", result)
    return result
