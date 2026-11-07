"""Project analysis sessions and safe immutable cache references.

A session references content-addressed analysis caches; unchanged inputs may
reuse the same cache, while each launch still owns a separate timing record.
"""
from __future__ import annotations

from datetime import datetime
from pathlib import Path
import hashlib
import json
import os
import re
import uuid
import threading
from .projects import _registry_write_lock

from . import data
from .projects import assert_project_video, get_project, project_cache_root, project_run_root

CACHE_ID = re.compile(r"^[a-f0-9]{16,64}(?:\.[a-f0-9]{16,64})?$")

_ENTRY_LOCK = threading.RLock()
_RUN_ID = re.compile(r"^run_[a-zA-Z0-9_-]{6,100}$")


def _entries_path(project_id: str) -> Path:
    return project_run_root(project_id) / "header_entries.json"


def _entries(project_id: str) -> dict:
    value = data.read_json(_entries_path(project_id), {})
    if not isinstance(value, dict):
        return {"names": {}, "deleted": []}
    names = value.get("names", {})
    deleted = value.get("deleted", [])
    return {
        "names": names if isinstance(names, dict) else {},
        "deleted": deleted if isinstance(deleted, list) else [],
    }


def _write_entries(project_id: str, entries: dict) -> None:
    path = _entries_path(project_id)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + "." + uuid.uuid4().hex + ".tmp")
    try:
        temporary.write_text(
            json.dumps(entries, ensure_ascii=False, indent=2), encoding="utf-8")
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def _modify_entry(project_id: str, run_id: str,
                  name: str | None = None, *, delete: bool = False,
                  kind: str | None = None) -> dict:
    """Change the selected run entry, never a global profile or shared cache."""
    if not isinstance(run_id, str) or not _RUN_ID.fullmatch(run_id):
        raise ValueError("Некорректный идентификатор запуска")
    if delete is False:
        if not isinstance(name, str) or not name.strip() or len(name.strip()) > 100:
            raise ValueError("Название должно содержать от 1 до 100 символов")
        name = name.strip()
    with _ENTRY_LOCK, _registry_write_lock():
        match = next((row for row in list_runs(project_id, target_run_id=run_id)
                      if row["id"] == run_id), None)
        if match is None:
            raise ValueError("Запись анализа не найдена или уже удалена")
        is_cpu_version = bool(match.get("parent_run_id"))
        if kind is not None:
            if kind not in ("inference", "postprocess"):
                raise ValueError("Некорректный тип записи анализа")
            if kind == "postprocess" and not is_cpu_version:
                raise ValueError(
                    "Исходный результат — это первичный анализ. "
                    "Его можно изменить только в списке «Первичный анализ».")
            if kind == "inference" and is_cpu_version:
                raise ValueError(
                    "CPU-версию можно изменить только в списке «Обработка результатов».")
        entries = _entries(project_id)
        if delete:
            entries["deleted"] = list(dict.fromkeys([*entries["deleted"], run_id]))
            entries["names"].pop(run_id, None)
        else:
            entries["names"][run_id] = name
        _write_entries(project_id, entries)
        return {"id": run_id, "deleted": delete, "name": name if not delete else None,
                "child_versions_hidden": bool(delete and not match.get("parent_run_id")),
                "shared_cache_preserved": True}


def rename_run(project_id: str, run_id: str, name: str,
               kind: str | None = None) -> dict:
    return _modify_entry(project_id, run_id, name, kind=kind)


def delete_run(project_id: str, run_id: str,
               kind: str | None = None) -> dict:
    return _modify_entry(project_id, run_id, delete=True, kind=kind)


def restore_postprocessing_version(project_id: str, parent_id: str,
                                   settings: dict) -> None:
    """A fresh successful CPU replay can recreate a previously deleted entry."""
    settings_json = json.dumps(settings, ensure_ascii=False, sort_keys=True)
    version_id = parent_id + "_pp_" + hashlib.sha256(
        settings_json.encode("utf-8")).hexdigest()[:12]
    with _ENTRY_LOCK, _registry_write_lock():
        entries = _entries(project_id)
        if version_id in entries["deleted"]:
            entries["deleted"] = [item for item in entries["deleted"] if item != version_id]
            _write_entries(project_id, entries)



def cache_for(video: str, project_id: str, cache_id: str | None = None) -> Path | None:
    assert_project_video(project_id, video)
    if not cache_id:
        return data.find_cache(video, project_id)
    if not isinstance(cache_id, str) or not CACHE_ID.fullmatch(cache_id):
        raise ValueError("Некорректный идентификатор анализа")
    base_id, separator, version_id = cache_id.partition(".")
    base = project_cache_root(project_id) / base_id
    path = base
    if separator:
        path = base / "postprocessing" / version_id
        metadata = data.read_json(path / "postprocess.manifest.json", {})
        if (not isinstance(metadata, dict) or
                metadata.get("schema") != "volley-postprocessing/1" or
                metadata.get("version_id") != version_id or
                Path(str(metadata.get("inference_cache", ""))).resolve() != base.resolve() or
                not (path / "ranking.json").is_file()):
            raise ValueError("Версия постобработки неполная или не принадлежит анализу")
    event = data.read_json(path / "analysis/events.json", {})
    if isinstance(event, dict) and data.source_matches_video(event.get("source"), video):
        return path
    if not separator:
        neural = data.read_json(path / "inference.manifest.json", {})
        if (isinstance(neural, dict)
                and neural.get("schema") == "volley-inference/1"
                and data.source_matches_video(neural.get("source"), video)
                and (path / "analysis.mp4").is_file()
                and (path / "players.json").is_file()
                and (path / "ball/analysis/ball.csv").is_file()):
            return path
    raise ValueError("Неполный результат анализа или неверная привязка к видео")


def publish_postprocessing_snapshot(project_id: str, parent_id: str,
                                    settings: dict, videos: dict, *,
                                    started_at: str | None = None,
                                    suppress_legacy: bool = False) -> dict:
    """Every successful launch gets a snapshot; expensive caches remain shared."""
    if not _RUN_ID.fullmatch(parent_id):
        raise ValueError("Invalid original analysis ID")
    if not isinstance(settings, dict) or not isinstance(videos, dict) or not videos:
        raise ValueError("Incomplete CPU snapshot")
    snapshot_id = parent_id + "_pp_" + uuid.uuid4().hex
    snapshot = {
        "schema": "volley-cpu-snapshot/1",
        "id": snapshot_id,
        "parent_run_id": parent_id,
        "started_at": started_at or datetime.now().astimezone().isoformat(),
        "postprocess_settings": dict(settings),
        "videos": dict(videos),
        "suppress_legacy": bool(suppress_legacy),
    }
    folder = project_run_root(project_id) / "cpu_snapshots" / parent_id
    folder.mkdir(parents=True, exist_ok=True)
    target = folder / (snapshot_id + ".json")
    temporary = folder / (snapshot_id + ".tmp")
    try:
        temporary.write_text(json.dumps(snapshot, ensure_ascii=False, indent=2),
                             encoding="utf-8")
        os.replace(temporary, target)
    finally:
        temporary.unlink(missing_ok=True)
    return snapshot


def _cpu_snapshots(project_id: str, run: dict) -> tuple[list[dict], set[str]]:
    """Read only valid snapshots with complete matching results for every video."""
    root = project_run_root(project_id) / "cpu_snapshots" / run["id"]
    if not root.is_dir():
        return [], set()
    snapshots, suppressed = [], set()
    for file in root.glob("*.json"):
        entry = data.read_json(file, {})
        if (not isinstance(entry, dict) or
                entry.get("schema") != "volley-cpu-snapshot/1" or
                entry.get("parent_run_id") != run["id"] or
                not isinstance(entry.get("id"), str) or
                not entry["id"].startswith(run["id"] + "_pp_") or
                not _RUN_ID.fullmatch(entry["id"]) or
                not isinstance(entry.get("postprocess_settings"), dict) or
                not isinstance(entry.get("videos"), dict) or
                set(entry["videos"]) != set(run["videos"])):
            continue
        if any(not isinstance(entry["videos"][video], str) or
               not entry["videos"][video].startswith(base_id + ".")
               for video, base_id in run["videos"].items()):
            continue
        try:
            for video, cache_id in entry["videos"].items():
                cache_for(video, project_id, cache_id)
        except (ValueError, OSError):
            continue
        config = entry["postprocess_settings"]
        snapshots.append({
            **run, "id": entry["id"],
            "profile": run["profile"] + " / " + str(config.get("mode", "CPU")),
            "videos": entry["videos"],
            "parent_run_id": run["id"],
            "postprocess_settings": config,
            "started_at": entry.get("started_at"),
            "cpu_snapshot": True,
        })
        if entry.get("suppress_legacy"):
            settings_json = json.dumps(config, ensure_ascii=False, sort_keys=True)
            suppressed.add(run["id"] + "_pp_" + hashlib.sha256(
                settings_json.encode("utf-8")).hexdigest()[:12])
    return snapshots, suppressed


def _postprocessing_runs(project_id: str, run: dict) -> list[dict]:
    """Present only complete, matching configurations for EVERY run video.

    A replay gets a different run id and cache-id pair; saved montage edits
    remain bound to their original result version.
    """
    snapshots, suppressed = _cpu_snapshots(project_id, run)
    available = {}
    for video, base_id in run["videos"].items():
        versions = {}
        root = project_cache_root(project_id) / base_id / "postprocessing"
        if not root.is_dir():
            return snapshots
        for folder in root.iterdir():
            if not folder.is_dir() or not CACHE_ID.fullmatch(folder.name):
                continue
            manifest_path = folder / "postprocess.manifest.json"
            meta = data.read_json(manifest_path, {})
            if not isinstance(meta, dict) or not isinstance(meta.get("settings"), dict):
                continue
            composite = base_id + "." + folder.name
            try:
                cache_for(video, project_id, composite)
            except ValueError:
                continue
            settings = json.dumps(meta["settings"], ensure_ascii=False, sort_keys=True)
            versions[settings] = (composite, manifest_path.stat().st_mtime)
        if not versions:
            return snapshots
        available[video] = versions
    common = set.intersection(*(set(x) for x in available.values())) if available else set()
    results = []
    for settings_json in sorted(common):
        mappings = {video: available[video][settings_json][0] for video in run["videos"]}
        setting_hash = hashlib.sha256(settings_json.encode("utf-8")).hexdigest()[:12]
        if run["id"] + "_pp_" + setting_hash in suppressed:
            continue
        config = json.loads(settings_json)
        results.append({
            **run,
            "id": run["id"] + "_pp_" + setting_hash,
            "profile": run["profile"] + " / " + config["mode"],
            "videos": mappings,
            "parent_run_id": run["id"],
            "postprocess_settings": config,
            "started_at": datetime.fromtimestamp(
                max(available[video][settings_json][1] for video in available)
            ).astimezone().isoformat(),
        })
    return snapshots + results


def list_runs(project_id: str, *, target_run_id: str | None = None) -> list[dict]:
    project = get_project(project_id)
    history = project_run_root(project_id) / "analysis_timings"
    runs = []
    if history.is_dir():
        # A single rename/delete must not rediscover all GPU runs and every
        # postprocessing directory in the project. The ID encodes its timing
        # file; CPU versions share the primary run's timing file.
        if target_run_id is None:
            timing_files = sorted(history.glob("*.json"), reverse=True)
        else:
            original_id = target_run_id.partition("_pp_")[0]
            if not _RUN_ID.fullmatch(original_id):
                raise ValueError("Некорректный идентификатор запуска")
            timing_file = history / (original_id.removeprefix("run_") + ".json")
            timing_files = (timing_file,) if timing_file.is_file() else ()
        for file in timing_files:
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
            video_ranges = {}
            for row in entries:
                video, ident = row.get("video"), row.get("cache_id")
                if video not in project["videos"] or not isinstance(ident, str):
                    continue
                try:
                    cache_for(video, project_id, ident)
                except ValueError:
                    continue
                by_video[video] = ident
                if row.get("range_start") is not None:
                    video_ranges[video] = {
                        "start": row["range_start"], "end": row.get("range_end", 0)}
                else:
                    # Old timing records have no range snapshot; use the
                    # recorded GPU cache's immutable offset, never the current
                    # mutable project range.
                    raw = data.read_json(
                        project_cache_root(project_id) / ident /
                        "inference.manifest.json", {})
                    if isinstance(raw, dict) and raw.get("analysis_offset_sec") is not None:
                        video_ranges[video] = {
                            "start": raw["analysis_offset_sec"], "end": None}
            if len(by_video)==len(project["videos"]):
                run = {
                    "id": "run_" + file.stem, "profile": profile,
                    "profile_id": entry.get("profile_id") or "legacy",
                    "started_at": entry.get("started_at"), "videos": by_video,
                    "video_ranges": video_ranges,
                    "postprocessed_videos": [video for video, ident in by_video.items()
                        if (project_cache_root(project_id) / ident / "analysis/events.json").is_file()],
                    "status": "complete",
                    "inference_only": bool(entry.get("inference_only")
                         or (entry.get("settings") or {}).get("inference_only")),
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
    # Each CPU replay is selectable without replacing its parent GPU result.
    for original in tuple(runs):
        # Direct lookup of a primary result needs no CPU-version enumeration.
        if target_run_id is None or "_pp_" in target_run_id:
            runs.extend(_postprocessing_runs(project_id, original))
    # Unnamed historical caches are preserved, but not offered as profiles.
    def started(item):
        try:
            return datetime.fromisoformat(item["started_at"]).timestamp()
        except (TypeError, ValueError, KeyError):
            return 0.0

    # Header metadata is project-scoped. Removing an original run hides its
    # derived CPU versions, without unlinking expensive shared inference files.
    entry_meta = _entries(project_id)
    deleted = set(entry_meta["deleted"])
    runs = [run for run in runs
            if run["id"] not in deleted and run.get("parent_run_id") not in deleted]
    for run in runs:
        if run.get("parent_run_id"):
            # CPU names describe the original launch time, never the profile or
            # mutable processing settings. Preserve the timestamp's timezone;
            # browser-local Date formatting can shift the displayed hour.
            try:
                run["auto_name"] = datetime.fromisoformat(
                    run["started_at"].replace("Z", "+00:00")
                ).strftime("%d.%m.%y %H:%M")
            except (TypeError, ValueError, KeyError, AttributeError):
                pass
        label = entry_meta["names"].get(run["id"])
        if isinstance(label, str) and label.strip():
            run["display_name"] = label.strip()
    runs.sort(key=started, reverse=True)
    # Each successful invocation is its own immutable project branch.
    # The profile is a configuration label, not the identity of an analysis.
    return runs
