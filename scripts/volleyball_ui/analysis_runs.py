"""Project analysis sessions and safe immutable cache references.

A session references content-addressed analysis caches; unchanged inputs may
reuse the same cache, while each launch still owns a separate timing record.
"""
from __future__ import annotations

from datetime import datetime
from pathlib import Path
import json
import os
import re
import uuid

from . import data
from .projects import assert_project_video, get_project, project_cache_root, project_run_root

CACHE_ID = re.compile(r"^[a-f0-9]{16,64}$")


def cache_for(video: str, project_id: str, cache_id: str | None = None) -> Path | None:
    assert_project_video(project_id, video)
    if not cache_id:
        return data.find_cache(video, project_id)
    if not isinstance(cache_id, str) or not CACHE_ID.fullmatch(cache_id):
        raise ValueError("Некорректный идентификатор анализа")
    path = project_cache_root(project_id) / cache_id
    event = data.read_json(path / "analysis/events.json", {})
    if not isinstance(event, dict) or not data.source_matches_video(event.get("source"), video):
        raise ValueError("Результат анализа не принадлежит этому видео")
    return path


def list_runs(project_id: str) -> list[dict]:
    project = get_project(project_id)
    history = project_run_root(project_id) / "analysis_timings"
    runs = []
    if history.is_dir():
        for file in sorted(history.glob("*.json"), reverse=True):
            entry = data.read_json(file, {})
            if not isinstance(entry, dict) or entry.get("status") != "complete":
                continue
            profile = entry.get("profile_name")
            # Only actual profile-bound results are selectable. Legacy caches
            # without a recorded profile stay on disk, not in this dropdown.
            if not isinstance(profile, str) or not profile.strip():
                continue
            profile = profile.strip()
            # Never expose a partly finished run, even when its parent was
            # incorrectly stamped complete by an old version of the runner.
            entries=entry.get("videos",[])
            if (not isinstance(entries,list) or len(entries)!=len(project["videos"]) or
                    any(not isinstance(row,dict) or row.get("status")!="complete"
                        for row in entries)):
                continue
            expected_stages=entry.get("stage_names")
            if isinstance(expected_stages,list) and expected_stages:
                # A complete status alone is not enough when the modern runner
                # records stage timings: all stages must have finished.
                if any(not isinstance(row.get("stages"),list) or
                       len(row["stages"])!=len(expected_stages) or
                       any(not isinstance(stage,dict) or
                           stage.get("stage_index")!=index
                           for index,stage in enumerate(row["stages"],1))
                       for row in entries):
                    continue
            by_video = {}
            for row in entries:
                video, ident = row.get("video"), row.get("cache_id")
                if video not in project["videos"] or not isinstance(ident, str):
                    continue
                try:
                    cache_for(video, project_id, ident)
                except ValueError:
                    continue
                by_video[video] = ident
            if len(by_video)==len(project["videos"]):
                run = {
                    "id": "run_" + file.stem, "profile": profile,
                    "profile_id": entry.get("profile_id") or "legacy",
                    "started_at": entry.get("started_at"), "videos": by_video,
                    "status": "complete",
                }
                runs.append(run)
                # Materialize the project -> analysis-run branch for both new
                # and pre-existing successful analyses, without rewriting caches.
                branch = project_run_root(project_id) / "branches" / run["id"]
                manifest = branch / "analysis.json"
                previous = data.read_json(manifest, {})
                if (not isinstance(previous, dict) or
                        previous.get("project_id") != project_id or
                        "profile_id" not in previous or
                        "settings" not in previous):
                    branch.mkdir(parents=True, exist_ok=True)
                    # Two UI servers may discover the same historical branch
                    # concurrently. Distinct temp files keep publication atomic.
                    temporary = branch / ("analysis." + uuid.uuid4().hex + ".tmp")
                    try:
                        temporary.write_text(json.dumps({
                            "project_id": project_id, **run, "timing_file": file.name,
                            # Snapshot actual settings, not a mutable global profile.
                            "settings": entry.get("settings", {}),
                        }, ensure_ascii=False, indent=2), encoding="utf-8")
                        os.replace(temporary, manifest)
                    finally:
                        temporary.unlink(missing_ok=True)
    # Unnamed historical caches are preserved, but not offered as profiles.
    def started(item):
        try:
            return datetime.fromisoformat(item["started_at"]).timestamp()
        except (TypeError, ValueError, KeyError):
            return 0.0

    runs.sort(key=started, reverse=True)
    # Each successful invocation is its own immutable project branch.
    # The profile is a configuration label, not the identity of an analysis.
    return runs
