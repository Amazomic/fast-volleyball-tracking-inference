"""Project-scoped, repeatable CPU projection of touches onto confirmed athletes.

Source events and inference detections are never modified. Each run keeps a
separate action index for subsequent per-player montage selection.
"""
from __future__ import annotations

from collections import Counter
from datetime import datetime, timezone
from pathlib import Path
import re

from . import data
from .analysis_runs import cache_for, list_runs
from .projects import project_dir
from .player_identity_groups import effective_track_identities, _context

_RUN_NAME = re.compile(r"^[A-Za-z0-9_-]{1,120}$")


def _run(project_id: str, run_id: str) -> dict:
    if not isinstance(run_id, str) or not _RUN_NAME.fullmatch(run_id):
        raise ValueError("Некорректный запуск анализа")
    run = next((item for item in list_runs(project_id, target_run_id=run_id)
                if item["id"] == run_id), None)
    if not run or not run.get("videos"):
        raise ValueError("Выберите существующую обработку проекта")
    return run


def _path(project_id: str, run_id: str) -> Path:
    return project_dir(project_id) / "reports" / "player_actions" / (run_id + ".json")


def _summary_path(project_id: str, run_id: str) -> Path:
    return _path(project_id, run_id).with_suffix(".summary.json")


def _signature(project_id: str, run: dict) -> dict:
    """Lightweight invalidation when events, IDs, ReID merges or roster change."""
    root = project_dir(project_id)
    paths = [root / "player_identities.json",
             data.RESULTS / "players" / "registry.json"]
    from .data import project_player_ids
    signatures = {"roster": sorted(project_player_ids(project_id))}
    for video, cache_id in sorted(run["videos"].items()):
        cache = cache_for(video, project_id, cache_id)
        if cache is None:
            raise ValueError("Обработка видео недоступна: " + video)
        events = cache / "analysis" / "events.json"
        if not events.is_file():
            raise ValueError("Сначала выполните CPU-обработку видео: " + video)
        paths.append(events)
        try:
            _cache, _source, groups = _context(project_id, video, cache_id)
            paths.append(groups)
        except (ValueError, FileNotFoundError):
            pass
    for path in paths:
        stat = path.stat() if path.is_file() else None
        signatures[str(path)] = ([stat.st_mtime_ns, stat.st_size] if stat else None)
    return signatures


def recalculate(project_id: str, run_id: str) -> dict:
    """Rebind all classified touches in a selected project run, without GPU."""
    run = _run(project_id, run_id)
    people = {p["id"]: p["name"] for p in data.players_for(project_id)}
    signature_before = _signature(project_id, run)
    totals = {pid: {"player_id": pid, "name": name, "touches": 0,
                    "actions": {action: 0 for action in data.ACTIONS}}
              for pid, name in people.items()}
    by_video = {}
    action_events = []
    total = assigned = 0

    for video, cache_id in run["videos"].items():
        cache = cache_for(video, project_id, cache_id)
        document = data.read_json(cache / "analysis" / "events.json")
        if not isinstance(document, dict) or not isinstance(document.get("events"), list):
            raise ValueError("Некорректные события CPU-обработки: " + video)
        owners = effective_track_identities(project_id, video, cache_id)["players"]
        excluded = data.excluded_track_ids(video, project_id, cache_id)
        video_total = video_assigned = 0
        video_counts = Counter()
        rally_id = None
        for event in document["events"]:
            if not isinstance(event, dict):
                continue
            action = event.get("type")
            if action == "rally_start":
                rally_id = str(event.get("id") or "") or None
                continue
            if action == "rally_end":
                rally_id = None
                continue
            if action not in data.ACTIONS:
                continue
            track = event.get("player_track_id")
            track_id = str(track) if track is not None else None
            if track_id in excluded:
                continue
            owner = owners.get(track_id) if track_id is not None else None
            player_id = owner.get("id") if owner and owner.get("id") in people else None
            video_total += 1
            video_counts[action] += 1
            total += 1
            if player_id:
                assigned += 1
                video_assigned += 1
                totals[player_id]["touches"] += 1
                totals[player_id]["actions"][action] += 1
            action_events.append({
                "event_id": str(event.get("id") or ""),
                "video": video, "cache_id": cache_id,
                "frame": event.get("frame"), "time_sec": event.get("time_sec"),
                "action": action, "confidence": event.get("confidence"),
                "rally_id": rally_id, "player_track_id": track_id,
                "player_id": player_id,
                "identity_source": owner.get("source") if player_id else None,
            })
        by_video[video] = {"cache_id": cache_id, "touches": video_total,
                           "assigned_touches": video_assigned,
                           "unassigned_touches": video_total - video_assigned,
                           "actions": dict(video_counts)}

    if signature_before != _signature(project_id, run):
        raise ValueError("Привязки или данные изменились во время пересчёта. Повторите его.")
    result = {
        "schema": "volley-player-actions/1", "status": "ready",
        "project_id": project_id, "run_id": run_id, "videos": dict(run["videos"]),
        "calculated_at": datetime.now(timezone.utc).isoformat(),
        "signature": signature_before,
        "total_touches": total, "assigned_touches": assigned,
        "unassigned_touches": total - assigned,
        "players": sorted(totals.values(), key=lambda p: (-p["touches"], p["name"])),
        "by_video": by_video,
        "events": action_events,
    }
    data.save_json(_path(project_id, run_id), result)
    data.save_json(_summary_path(project_id, run_id),
                   {**_summary(result), "signature": signature_before})
    return _summary(result)


def _summary(doc: dict) -> dict:
    return {k: v for k, v in doc.items() if k not in ("events", "signature")}


def get_actions(project_id: str, run_id: str, *,
                player_id: str | None = None, action: str | None = None) -> dict:
    run = _run(project_id, run_id)
    summary_only = player_id is None and action is None
    path = _summary_path(project_id, run_id) if summary_only else _path(project_id, run_id)
    stored = data.read_json(path, {})
    if stored.get("schema") != "volley-player-actions/1":
        return {"status": "missing", "players": []}
    if stored.get("videos") != run["videos"] or stored.get("signature") != _signature(project_id, run):
        return {"status": "stale", "players": []}
    summary = _summary(stored)
    if player_id is not None or action is not None:
        if player_id is not None and (not isinstance(player_id, str) or
                                     not any(p["player_id"] == player_id for p in stored["players"])):
            raise ValueError("Игрок не найден в сохранённом пересчёте")
        if action is not None and action not in data.ACTIONS:
            raise ValueError("Неизвестный тип касания")
        summary["events"] = [
            event for event in stored["events"]
            if (player_id is None or event["player_id"] == player_id)
            and (action is None or event["action"] == action)
        ]
    return summary
