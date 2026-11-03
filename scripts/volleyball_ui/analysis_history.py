"""All project analysis executions, with profile and per-stage elapsed time.

Canonical multi-video analyses and isolated partial analyses have independent
storage. They are merged here without changing or overwriting old results.
"""
from __future__ import annotations

from datetime import datetime, timezone

from . import data
from .projects import get_project, project_dir, project_run_root


def _seconds(value):
    try:
        seconds = float(value)
        return round(seconds, 2) if 0 <= seconds < 365 * 24 * 3600 else None
    except (TypeError, ValueError, OverflowError):
        return None


def _timestamp(value):
    try:
        return datetime.fromisoformat(value).timestamp()
    except (TypeError, ValueError, OverflowError):
        return 0.0


def _stages(rows):
    if not isinstance(rows, list):
        return []
    return [
        {
            "stage_index": row.get("stage_index"),
            "stage": row.get("stage"),
            "duration_sec": _seconds(row.get("duration_sec")),
            "fps": row.get("fps"),
        }
        for row in rows if isinstance(row, dict)
    ]


def _valid_stage_timings(rows: object) -> bool:
    """Require the structured stage measurements written by recent runners."""
    return isinstance(rows, list) and bool(rows) and all(
        isinstance(row, dict) and isinstance(row.get("stage_index"), int)
        and isinstance(row.get("stage"), str) and bool(row["stage"].strip())
        and _seconds(row.get("duration_sec")) is not None
        for row in rows
    )


def _named_profile(item: dict) -> str | None:
    """Legacy results without a recorded profile are not selectable history."""
    name = item.get("profile_name")
    return name.strip() if isinstance(name, str) and name.strip() else None


def list_history(project_id: str) -> list[dict]:
    """Show only fully successful invocations; interrupted attempts stay private."""
    project = get_project(project_id)
    allowed = set(project["videos"])
    records = []

    timings = project_run_root(project_id) / "analysis_timings"
    if timings.is_dir():
        for file in timings.glob("*.json"):
            item = data.read_json(file, {})
            if not isinstance(item, dict) or item.get("status") != "complete":
                continue
            profile = _named_profile(item)
            expected_stages = item.get("stage_names")
            # Old-format entries lacked profile, settings or complete stage
            # timings. Hide them, without deleting their original files.
            if (profile is None or not isinstance(item.get("settings"), dict)
                    or not isinstance(expected_stages, list) or not expected_stages):
                continue
            # A project launch is complete only when every intended video is.
            source_videos = item.get("videos")
            if (not isinstance(source_videos,list) or not source_videos or
                len(source_videos) != item.get("videos_total",len(source_videos)) or
                any(not isinstance(row,dict) or
                    row.get("status")!="complete" or
                    row.get("video") not in allowed
                    or not _valid_stage_timings(row.get("stages"))
                    or len(row["stages"]) != len(expected_stages)
                    for row in source_videos)):
                continue
            videos = []
            for row in item.get("videos", []):
                if not isinstance(row, dict) or row.get("video") not in allowed:
                    continue
                videos.append({
                    "video": row["video"],
                    "status": row.get("status"),
                    "duration_sec": _seconds(row.get("duration_sec")),
                    "stages": _stages(row.get("stages")),
                })
            records.append({
                "id": "run_" + file.stem,
                "kind": "project",
                "profile": profile,
                "profile_id": item.get("profile_id") or "legacy",
                "started_at": item.get("started_at"),
                "finished_at": item.get("finished_at"),
                "status": item.get("status", "unknown"),
                "duration_sec": _seconds(item.get("duration_sec")),
                "videos_total": item.get("videos_total", len(videos)),
                "stage_names": item.get("stage_names", []),
                "videos": videos,
                "error": item.get("error"),
            })

    experiments = project_dir(project_id) / "experiments" / "runs"
    if experiments.is_dir():
        from scripts.run_analysis_experiment import MODES, STAGES
        for file in experiments.glob("*.json"):
            item = data.read_json(file, {})
            if (not isinstance(item, dict) or item.get("video") not in allowed
                    or item.get("status") != "complete"):
                continue
            profile = _named_profile(item)
            if (profile is None or not isinstance(item.get("settings"), dict)
                    or not _valid_stage_timings(item.get("stages"))):
                continue
            mode = item.get("mode")
            stage_names = STAGES.get(mode, [])
            stages = _stages(item.get("stages"))
            recorded_names = [s["stage"] for s in stages if s.get("stage")]
            if recorded_names:
                stage_names = recorded_names
            records.append({
                "id": "experiment_" + file.stem,
                "kind": "fragment",
                "mode": mode,
                "profile": profile,
                "started_at": item.get("started_at"),
                "finished_at": item.get("finished_at"),
                "status": item.get("status", "unknown"),
                "duration_sec": _seconds(item.get("elapsed_sec")),
                "videos_total": 1,
                "stage_names": stage_names,
                "videos": [{
                    "video": item["video"],
                    "status": item.get("status"),
                    "duration_sec": _seconds(item.get("elapsed_sec")),
                    "start": item.get("start"),
                    "seconds": item.get("seconds"),
                    "stages": stages,
                }],
                "mode_label": MODES.get(mode, "Фрагмент"),
                "error": item.get("error"),
            })

    records.sort(key=lambda row: _timestamp(row.get("started_at")), reverse=True)
    return records
