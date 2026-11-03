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
    # Concurrent UI servers must never share a temporary filename.
    tmp = path.with_name(path.name + "." + uuid.uuid4().hex + ".tmp")
    try:
        tmp.write_text(json.dumps(value, ensure_ascii=False, indent=2), encoding="utf-8")
        os.replace(tmp, path)
    finally:
        tmp.unlink(missing_ok=True)


def video_path(name: str) -> Path:
    if not isinstance(name, str) or name != Path(name).name or not name:
        raise ValueError("Недопустимое имя видео")
    source = PROJECT / name
    if source.suffix.lower() not in (".mp4", ".mov", ".mkv", ".avi", ".webm", ".m4v", ".mts") or not source.is_file():
        raise ValueError("Видео не найдено в project")
    return source


def source_matches_video(recorded_source: str, name: str) -> bool:
    """Match a cache's real source to a project video, including local symlinks.

    The studio resolves symlinks before writing events.json. Comparing only
    basenames loses projects whose imported files received " (2)" aliases.
    Require an identical real file, never a fuzzy filename/stem match.
    """
    if not isinstance(recorded_source, str) or not recorded_source.strip():
        return False
    try:
        source = Path(recorded_source.replace(chr(92), "/"))
        return source.resolve(strict=True) == video_path(name).resolve(strict=True)
    except (OSError, RuntimeError, ValueError):
        return False


def _scan_analysis(base: Path, name: str, *, resolve_source: bool = False) -> Path | None:
    if not base.is_dir():
        return None
    matches = []
    for path in base.glob("*/analysis/events.json"):
        payload = read_json(path)
        if not isinstance(payload, dict):
            continue
        source = payload.get("source", "")
        if resolve_source:
            if not source_matches_video(source, name):
                continue
        elif Path(str(source).replace(chr(92), "/")).name != name:
            continue
        matches.append((path.stat().st_mtime, path.parent.parent))
    return max(matches, default=(0, None))[1]


def find_cache(name: str, project_id=None) -> Path | None:
    if not project_id:
        return _scan_analysis(RESULTS / "analysis", name)
    from .projects import assert_project_video, project_cache_root, project_dir
    assert_project_video(project_id, name)
    scoped = _scan_analysis(project_cache_root(project_id), name, resolve_source=True)
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
    return candidate if source_matches_video(info.get("source", ""), name) else None


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
    """Video-scoped track assignments, retaining compatibility with old files."""
    info = read_json(identity_file(project_id), {"schema": 2, "videos": {}})
    if not isinstance(info, dict) or not isinstance(info.get("videos"), dict):
        info = {"schema": 2, "videos": {}}
    return info


def _catalog():
    """Global player registry. Adopt legacy project identities without dropping track links."""
    registry = RESULTS / "players" / "registry.json"
    first_load = not registry.is_file()
    catalog = read_json(registry, {"schema": 1, "players": []})
    if not isinstance(catalog, dict) or not isinstance(catalog.get("players"), list):
        catalog = {"schema": 1, "players": []}
    scanned = set(catalog.get("imported_projects", []))
    known = {p["name"].casefold(): p for p in catalog["players"]
             if isinstance(p, dict) and isinstance(p.get("name"), str)}
    changed = False
    from .projects import all_projects, project_dir
    for project in all_projects():
        if project["id"] in scanned:
            continue
        source = project_dir(project["id"]) / "player_identities.json"
        if not source.is_file():
            continue
        stored = read_json(source, {})
        if not isinstance(stored, dict):
            continue
        scanned.add(project["id"])
        changed = True
        candidates = list(stored.get("players") or [])
        for groups in (stored.get("videos") or {}).values():
            if isinstance(groups, list):
                candidates.extend(groups)
        for entry in candidates:
            if not isinstance(entry, dict) or not isinstance(entry.get("name"), str):
                continue
            key = entry["name"].casefold()
            if key not in known:
                athlete = {"id": uuid.uuid4().hex[:12], "name": entry["name"],
                           "created_by": "legacy_import"}
                catalog["players"].append(athlete)
                known[key] = athlete
                changed = True
        # Normalize legacy group IDs to the new global IDs before the first save.
        normalized = False
        for groups in (stored.get("videos") or {}).values():
            if not isinstance(groups, list):
                continue
            for group in groups:
                if not isinstance(group, dict):
                    continue
                entry = known.get(str(group.get("name", "")).casefold())
                if entry and group.get("id") != entry["id"]:
                    group["id"] = entry["id"]
                    normalized = True
        if normalized:
            save_json(project_dir(project["id"]) / "player_identities.json", stored)
    if changed or first_load:
        catalog["imported_projects"] = sorted(scanned)
        save_json(registry, catalog)
    for person in catalog["players"]:
        pid = person.get("id")
        if isinstance(pid, str) and len(pid) == 12 and all(c in "0123456789abcdef" for c in pid):
            (RESULTS / "players" / pid / "photos").mkdir(parents=True, exist_ok=True)
    return catalog


def global_players() -> list:
    with _LOCK:
        return [dict(p) for p in _catalog()["players"]]


def project_player_ids(project_id: str) -> list[str]:
    from .projects import get_project
    record = get_project(project_id)
    stored = _identities(project_id)
    if "player_ids" in record and not (
        not record["player_ids"] and stored.get("schema") == 1 and
        any(stored.get("videos", {}).values())
    ):
        return list(record["player_ids"])
    # Legacy projects had no explicit roster; infer it from their old assignments.
    names = set()
    ids = set()
    stored = _identities(project_id)
    for entry in stored.get("players") or []:
        if isinstance(entry, dict) and isinstance(entry.get("name"), str):
            names.add(entry["name"].casefold())
            ids.add(entry.get("id"))
    for groups in stored["videos"].values():
        if isinstance(groups, list):
            names.update(g["name"].casefold() for g in groups
                         if isinstance(g, dict) and isinstance(g.get("name"), str))
            ids.update(g.get("id") for g in groups if isinstance(g, dict))
    return [p["id"] for p in global_players()
            if p["id"] in ids or p["name"].casefold() in names]


def players_for(project_id=None) -> list:
    """Global people, optionally narrowed to a project's selected roster."""
    catalog = global_players()
    if not project_id:
        return [{**p, "tracks_by_video": {}, "total_track_ids": 0} for p in catalog]
    membership = set(project_player_ids(project_id))
    links = defaultdict(dict)
    stored = _identities(project_id)
    id_by_name = {p["name"].casefold(): p["id"] for p in catalog}
    known_ids = {p["id"] for p in catalog}
    for video, groups in stored["videos"].items():
        if not isinstance(groups, list):
            continue
        for group in groups:
            if not isinstance(group, dict):
                continue
            identifier = (group.get("id") if group.get("id") in known_ids else
                          id_by_name.get(str(group.get("name", "")).casefold()))
            if identifier in membership and group.get("track_ids"):
                links[identifier][video] = list(group["track_ids"])
    return [{**p, "tracks_by_video": links.get(p["id"], {}),
             "total_track_ids": sum(map(len, links.get(p["id"], {}).values()))}
            for p in catalog if p["id"] in membership]


def _player_name(value: str) -> str:
    if not isinstance(value, str) or not value.strip() or len(value.strip()) > 80:
        raise ValueError("Имя должно содержать от 1 до 80 символов")
    return value.strip()


def create_player(display_name: str, project_id: str | None = None) -> list:
    name = _player_name(display_name)
    with _LOCK:
        catalog = _catalog()
        if any(p["name"].casefold() == name.casefold() for p in catalog["players"]):
            raise ValueError("Игрок с таким именем уже существует")
        player = {"id": uuid.uuid4().hex[:12], "name": name, "created_by": "manual"}
        catalog["players"].append(player)
        save_json(RESULTS / "players" / "registry.json", catalog)
        (RESULTS / "players" / player["id"] / "photos").mkdir(parents=True, exist_ok=True)
    if project_id:
        attach_player(player["id"], project_id)
    return players_for(project_id)


def attach_player(player_id: str, project_id: str) -> list:
    from .projects import update_project
    if not any(p["id"] == player_id for p in global_players()):
        raise ValueError("Игрок не найден в общей базе")
    current = project_player_ids(project_id)
    if player_id not in current:
        update_project(project_id, player_ids=[*current, player_id])
    return players_for(project_id)


def rename_player(player_id: str, display_name: str, project_id: str | None = None) -> list:
    name = _player_name(display_name)
    with _LOCK:
        catalog = _catalog()
        athlete = next((p for p in catalog["players"] if p["id"] == player_id), None)
        if athlete is None:
            raise ValueError("Игрок не найден")
        if any(p["id"] != player_id and p["name"].casefold() == name.casefold()
               for p in catalog["players"]):
            raise ValueError("Игрок с таким именем уже существует")
        athlete["name"] = name
        save_json(RESULTS / "players" / "registry.json", catalog)
    return players_for(project_id)


def delete_player(player_id: str, project_id: str) -> list:
    """Remove a person from this project's roster without deleting the global person."""
    from .projects import update_project
    current = project_player_ids(project_id)
    if player_id not in current:
        raise ValueError("Игрок не входит в этот проект")
    update_project(project_id, player_ids=[pid for pid in current if pid != player_id])
    return players_for(project_id)


def groups_for(name: str, project_id=None, cache_id=None) -> list:
    stored = _identities(project_id)["videos"]
    groups = stored.get(name + "::" + cache_id, []) if cache_id else stored.get(name, [])
    if cache_id and not groups and name + "::" + cache_id not in stored:
        # Legacy assignments belong to the then-active cache only.
        latest = find_cache(name, project_id)
        if latest and latest.name == cache_id:
            groups = stored.get(name, [])
    if not project_id:
        return groups
    catalog = global_players()
    by_name = {p["name"].casefold(): p for p in catalog}
    by_id = {p["id"]: p for p in catalog}
    allowed = set(project_player_ids(project_id))
    normalized = []
    for group in groups:
        player = by_id.get(group.get("id")) or by_name.get(str(group.get("name", "")).casefold())
        if not player or player["id"] not in allowed:
            continue
        normalized.append({**group, "id": player["id"], "name": player["name"]})
    return normalized


def assign(name: str, track_ids: list, display_name: str | None = None,
           project_id=None, player_id: str | None = None, cache_id=None) -> list:
    video_path(name)
    if project_id:
        from .projects import assert_project_video
        assert_project_video(project_id, name)
    if not isinstance(track_ids, list) or not track_ids or len(track_ids) > 200:
        raise ValueError("Выберите один или несколько ID (до 200)")
    ids = list(dict.fromkeys(str(x) for x in track_ids))
    if any(not x.isdigit() or len(x) > 12 for x in ids):
        raise ValueError("ID игрока должен быть числом")
    with _LOCK:
        catalog = global_players()
        if player_id:
            player = next((p for p in catalog if p["id"] == player_id), None)
            if player is None:
                raise ValueError("Выбранный игрок не найден")
        else:
            display_name = _player_name(display_name)
            player = next((p for p in catalog
                           if p["name"].casefold() == display_name.casefold()), None)
            if player is None:
                create_player(display_name)
                player = next(p for p in global_players()
                              if p["name"].casefold() == display_name.casefold())
        if project_id:
            attach_player(player["id"], project_id)
        data = _identities(project_id)
        from .analysis_runs import cache_for
        if cache_id:
            cache_for(name, project_id, cache_id)
        storage_key = name + "::" + cache_id if cache_id else name
        if storage_key not in data["videos"] and cache_id:
            data["videos"][storage_key] = [dict(g, track_ids=list(g["track_ids"]))
                for g in groups_for(name, project_id, cache_id)]
        groups = data["videos"].setdefault(storage_key, [])
        chosen = next((g for g in groups
                       if g.get("id") == player["id"] or
                       g.get("name", "").casefold() == player["name"].casefold()), None)
        if chosen is None:
            chosen = {"id": player["id"], "name": player["name"], "track_ids": []}
            groups.append(chosen)
        chosen["id"] = player["id"]
        chosen["name"] = player["name"]
        for g in groups:
            g["track_ids"] = [tid for tid in g["track_ids"] if tid not in ids]
            if isinstance(g.get("link_meta"), dict):
                for tid in ids:
                    g["link_meta"].pop(tid, None)
        chosen["track_ids"] = sorted(set(chosen["track_ids"] + ids), key=int)
        metadata = chosen.setdefault("link_meta", {})
        for tid in ids:
            metadata[tid] = {"source": "manual", "reviewed": True}
        groups[:] = [g for g in groups if g["track_ids"]]
        data["schema"] = 2
        save_json(identity_file(project_id), data)
        return groups


def unassign(name: str, ids: list, project_id=None, cache_id=None) -> list:
    video_path(name)
    if project_id:
        from .projects import assert_project_video
        assert_project_video(project_id, name)
    if not isinstance(ids, list) or not ids or any(not str(x).isdigit() for x in ids):
        raise ValueError("Укажите ID для открепления")
    with _LOCK:
        data = _identities(project_id)
        from .analysis_runs import cache_for
        if cache_id:
            cache_for(name, project_id, cache_id)
        storage_key = name + "::" + cache_id if cache_id else name
        if storage_key not in data["videos"] and cache_id:
            data["videos"][storage_key] = [dict(g, track_ids=list(g["track_ids"]))
                for g in groups_for(name, project_id, cache_id)]
        groups = data["videos"].setdefault(storage_key, [])
        keys = set(map(str, ids))
        for g in groups:
            g["track_ids"] = [x for x in g["track_ids"] if x not in keys]
            if isinstance(g.get("link_meta"), dict):
                for tid in keys:
                    g["link_meta"].pop(tid, None)
        groups[:] = [g for g in groups if g["track_ids"]]
        data["schema"] = 2
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


def report(name: str, project_id=None, cache_id: str | None = None) -> dict:
    video_path(name)
    from .analysis_runs import cache_for
    cache = cache_for(name, project_id, cache_id) if cache_id else find_cache(name, project_id)
    if cache is None:
        return {"analyzed": False, "source": name, "counts": {}, "raw": [], "groups": [], "rallies": 0}
    info = read_json(cache / "analysis/events.json", {})
    events = info.get("events", [])
    if not isinstance(events, list):
        events = []
    counts = Counter(e.get("type") for e in events if isinstance(e, dict))
    raw = _raw_stats(name, cache, events, project_id)
    # The action-only table omitted most real tracklets. Include EVERY
    # RAVEL track with an observed duration so UI can prioritize stable IDs.
    from .player_tracks import tracks_for
    track_data = tracks_for(name, project_id, cache_id)
    counted = {str(t["player_track_id"]):dict(t) for t in raw}
    for tid, stats in track_data["tracks"].items():
        existing = counted.setdefault(tid, {"player_track_id":tid,"total_actions":0})
        existing.update(stats)
    raw = sorted(counted.values(), key=lambda x:(
        -int(x.get("detected_frames") or 0),
        -int(x.get("total_actions") or 0),int(x["player_track_id"])))
    identities = groups_for(name, project_id, cache_id)
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
