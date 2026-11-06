from collections import Counter
from statistics import median

from . import data
from .timeline import ACTION_TYPES, timeline


RECOGNIZED_ACTIONS = ("attack", "block", "serve", "dig", "overhead_pass")


def summarize_actions(actions: list, owners: dict, excluded: set) -> dict:
    contacts = {}
    for index, action in enumerate(actions):
        kind = action.get("type")
        if kind not in ACTION_TYPES:
            continue
        track = action.get("player_id")
        track = str(track) if track is not None else None
        if track in excluded:
            continue
        owner = owners.get(track)
        when = action.get("time")
        identity = ("player", owner) if owner else ("track", track)
        key = ((identity, when) if track is not None and when is not None
               else ("event", action.get("id") or index))
        if track is None and action.get("contact_id"):
            key = ("contact", action["contact_id"])
        previous = contacts.get(key)
        rank = (kind in RECOGNIZED_ACTIONS, float(action.get("confidence") or 0))
        if previous is None or rank > previous[0]:
            contacts[key] = (rank, kind, owner)
    counts = Counter()
    players = {}
    unassigned = 0
    for _, kind, owner in contacts.values():
        counts[kind] += 1
        if owner is None:
            unassigned += 1
            continue
        row = players.setdefault(owner, {"touches": 0, "recognized_touches": 0,
                                         "actions": Counter()})
        row["touches"] += 1
        row["recognized_touches"] += int(kind in RECOGNIZED_ACTIONS)
        row["actions"][kind] += 1
    return {"total_touches": len(contacts), "unassigned_touches": unassigned,
            "counts": dict(counts), "players": players}


def apply_activity(players: list) -> float:
    baseline = median(p["touches"] for p in players) if players else 0
    for player in players:
        player["activity"] = player["touches"] / baseline if baseline else None
    return baseline


def video_statistics(video: str, project_id: str, cache_id: str | None, report: dict) -> dict:
    if not report.get("postprocessed", report.get("analyzed")):
        return {"analyzed": False, "rallies": 0, **summarize_actions([], {}, set())}
    from .analysis_runs import cache_for
    cache = cache_for(video, project_id, cache_id) if cache_id else data.find_cache(video, project_id)
    duration = None
    if cache is not None and (cache / "analysis.mp4").is_file():
        from .preview import _read_info
        path = cache / "analysis.mp4"
        stat = path.stat()
        duration = _read_info(str(path), stat.st_size, stat.st_mtime_ns)["duration_sec"]
    view = timeline(video, project_id, cache_id=cache_id, duration_sec=duration)
    owners = {str(row["player_track_id"]): row["player_id"]
              for row in report.get("raw", []) if row.get("player_id")}
    for group in report.get("groups", []):
        for track in group.get("track_ids", []):
            owners.setdefault(str(track), group["id"])
    result = summarize_actions(view.get("actions", []), owners,
                               set(map(str, report.get("excluded_track_ids", []))))
    return {**result, "analyzed": bool(view.get("analyzed")),
            "rallies": len(view.get("rallies", []))}
