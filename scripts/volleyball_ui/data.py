"""Read cached volleyball analyses and maintain user-reviewed athlete identities."""
from __future__ import annotations

import json
import os
import threading
import uuid
from collections import Counter, defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PROJECT = ROOT / "project"
RESULTS = ROOT / "results"
IDS_FILE = RESULTS / "player_identities.json"
ACTIONS = ("attack", "block", "serve", "dig", "overhead_pass", "low_touch")
_LOCK = threading.RLock()


def read_json(path: Path, default=None):
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return default


def save_json(path: Path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(path.name + ".tmp")
    tmp.write_text(json.dumps(value, ensure_ascii=False, indent=2), encoding="utf-8")
    os.replace(tmp, path)


def video_path(name: str) -> Path:
    if not isinstance(name, str) or name != Path(name).name or not name:
        raise ValueError("Недопустимое имя видео")
    source = PROJECT / name
    if source.suffix.lower() not in (".mp4", ".mov", ".mkv", ".avi", ".webm", ".m4v", ".mts") or not source.is_file():
        raise ValueError("Видео не найдено в project")
    return source


def _scan_analysis(base: Path, name: str) -> Path | None:
    if not base.is_dir():
        return None
    matches = []
    for path in base.glob("*/analysis/events.json"):
        payload = read_json(path)
        if not isinstance(payload, dict):
            continue
        if Path(str(payload.get("source", "")).replace(chr(92), "/")).name != name:
            continue
        matches.append((path.stat().st_mtime, path.parent.parent))
    return max(matches, default=(0, None))[1]


def find_cache(name: str, project_id=None) -> Path | None:
    if not project_id:
        return _scan_analysis(RESULTS / "analysis", name)
    from .projects import assert_project_video, project_cache_root, project_dir
    assert_project_video(project_id, name)
    scoped = _scan_analysis(project_cache_root(project_id), name)
    if scoped:
        return scoped
    # Backward compatible, explicitly imported legacy sources only.
    legacy = read_json(project_dir(project_id) / "legacy_analysis.json", {})
    if not isinstance(legacy, dict) or name not in legacy:
        return None
    cache_id = legacy[name]
    if not isinstance(cache_id, str) or not cache_id.isalnum():
        return None
    candidate = RESULTS / "analysis" / cache_id
    event_file = candidate / "analysis" / "events.json"
    info = read_json(event_file, {})
    if not isinstance(info, dict):
        return None
    source = Path(str(info.get("source", "")).replace(chr(92), "/")).name
    return candidate if source == name else None


def list_videos():
    if not PROJECT.is_dir():
        return []
    out = []
    for path in sorted(PROJECT.iterdir()):
        if not path.is_file() or path.suffix.lower() not in (".mp4", ".mov", ".mkv", ".avi", ".webm", ".m4v", ".mts"):
            continue
        cache = find_cache(path.name)
        out.append({
            "name": path.name, "size_gb": round(path.stat().st_size / 1e9, 2),
            "analyzed": cache is not None, "cache_id": cache.name if cache else None,
        })
    return out


def identity_file(project_id=None):
    if project_id is None or project_id == "":
        return IDS_FILE
    from .projects import get_project, project_dir
    get_project(project_id)
    return project_dir(project_id) / "player_identities.json"


def _identities(project_id=None):
    data = read_json(identity_file(project_id), {"schema": 1, "videos": {}})
    if not isinstance(data, dict) or not isinstance(data.get("videos"), dict):
        return {"schema": 1, "videos": {}}
    return data


def groups_for(name: str, project_id=None) -> list:
    data = _identities(project_id)
    return data["videos"].get(name, [])


def assign(name: str, track_ids: list, display_name: str, project_id=None) -> list:
    video_path(name)
    if project_id:
        from .projects import assert_project_video
        assert_project_video(project_id, name)
    if not isinstance(track_ids, list) or not track_ids or len(track_ids) > 200:
        raise ValueError("Выберите один или несколько ID (до 200)")
    if not isinstance(display_name, str) or not display_name.strip() or len(display_name.strip()) > 80:
        raise ValueError("Имя должно содержать от 1 до 80 символов")
    ids = list(dict.fromkeys(str(x) for x in track_ids))
    if any(not x.isdigit() or len(x) > 12 for x in ids):
        raise ValueError("ID игрока должен быть числом")
    display_name = display_name.strip()
    with _LOCK:
        data = _identities(project_id)
        groups = data["videos"].setdefault(name, [])
        chosen = next((g for g in groups if g["name"].casefold() == display_name.casefold()), None)
        if chosen is None:
            chosen = {"id": uuid.uuid4().hex[:12], "name": display_name, "track_ids": []}
            groups.append(chosen)
        for g in groups:
            g["track_ids"] = [tid for tid in g["track_ids"] if tid not in ids]
        chosen["track_ids"] = sorted(set(chosen["track_ids"] + ids), key=int)
        groups[:] = [g for g in groups if g["track_ids"]]
        save_json(identity_file(project_id), data)
        return groups


def unassign(name: str, ids: list, project_id=None) -> list:
    video_path(name)
    if project_id:
        from .projects import assert_project_video
        assert_project_video(project_id, name)
    if not isinstance(ids, list) or not ids or any(not str(x).isdigit() for x in ids):
        raise ValueError("Укажите ID для открепления")
    with _LOCK:
        data = _identities(project_id)
        groups = data["videos"].setdefault(name, [])
        keys = set(map(str, ids))
        for g in groups:
            g["track_ids"] = [x for x in g["track_ids"] if x not in keys]
        groups[:] = [g for g in groups if g["track_ids"]]
        save_json(identity_file(project_id), data)
        return groups


def _raw_stats(name: str, cache: Path, events: list, project_id=None) -> list:
    # Read only statistics co-located with the matched analysis.
    # The old global stats are never imported into a different project.
    if project_id:
        from .projects import project_dir
        path = project_dir(project_id) / "reports" / (name + ".player_statistics.json")
    else:
        path = cache / "player_statistics.json"
    data = read_json(path, {})
    source = str(data.get("source", "")).replace("\\", "/")
    if Path(source).name == "players.json" and cache.name in source and isinstance(data.get("players"), list):
        return data["players"]
    # Efficient fallback for newly analyzed video: all IDs with classified actions.
    stats = defaultdict(lambda: Counter())
    for event in events:
        track_id = event.get("player_track_id")
        typ = event.get("type")
        if track_id is not None and typ in ACTIONS:
            stats[str(track_id)][typ] += 1
    records = []
    for tid, counter in stats.items():
        row = {"player_track_id": tid, "detected_frames": None}
        row.update({t: counter[t] for t in ACTIONS})
        row["total_actions"] = sum(counter.values())
        records.append(row)
    return sorted(records, key=lambda r: -r["total_actions"])


def report(name: str, project_id=None) -> dict:
    video_path(name)
    cache = find_cache(name, project_id)
    if cache is None:
        return {"analyzed": False, "source": name, "counts": {}, "raw": [], "groups": [], "rallies": 0}
    info = read_json(cache / "analysis/events.json", {})
    events = info.get("events", [])
    if not isinstance(events, list):
        events = []
    counts = Counter(e.get("type") for e in events if isinstance(e, dict))
    raw = _raw_stats(name, cache, events, project_id)
    identities = groups_for(name, project_id)
    ids_to_stat = {str(x["player_track_id"]): x for x in raw}
    named = []
    assigned = set()
    for g in identities:
        sums = Counter()
        for tid in g["track_ids"]:
            assigned.add(tid)
            item = ids_to_stat.get(tid, {})
            for action in ACTIONS:
                sums[action] += int(item.get(action) or 0)
        named.append({"id": g["id"], "name": g["name"], "track_ids": g["track_ids"],
                      "actions": dict(sums), "total_actions": sum(sums.values())})
    named.sort(key=lambda p: (-p["total_actions"], p["name"]))
    return {
        "analyzed": True, "source": name, "cache_id": cache.name,
        "court_available": bool(info.get("court_available")),
        "counts": {k: v for k, v in counts.items() if k in ACTIONS},
        "rallies": counts.get("rally_start", 0),  # heuristic track fragments
        "track_count": len(raw), "groups": named, "raw": raw,
        "unassigned_count": sum(str(x["player_track_id"]) not in assigned for x in raw),
        "events_path": str(cache / "analysis/events.json"),
        "analysis_path": str(cache),
        "notes": "Track ID != identity. Counts are model estimates; zero does not prove absence.",
    }


def video_renders() -> list:
    """Return only finished, viewable video paths inside the results folder."""
    if not RESULTS.exists():
        return []
    found = []
    for path in sorted(RESULTS.glob("*.mp4"), key=lambda p: p.stat().st_mtime, reverse=True):
        if path.is_file() and path.stat().st_size > 0:
            found.append({"name": path.name, "size_mb": round(path.stat().st_size / 1e6, 1)})
    return found[:30]
