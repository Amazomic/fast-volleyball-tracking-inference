"""Single-workflow montage runner with exact-duration exports and stage events."""
from __future__ import annotations

import argparse
import json
import math
import shutil
import subprocess
import sys
from pathlib import Path

from . import data
from .projects import project_dir, project_output
from .montage_project import plan_clips, compose_clips
from scripts.volleyball_studio.advanced import validated, as_cli

STAGES = (
    "Проверка параметров и результатов анализа",
    "Отбор игровых эпизодов",
    "Монтаж исходных фрагментов",
    "Кодирование и сохранение видео",
)


def emit_stage(index: int, *, done: bool = False) -> None:
    """The runner, not nested CLI percentage logs, owns montage progress."""
    completed = index if done else index - 1
    print(json.dumps({
        "event": "UI_PROGRESS", "kind": "highlights", "origin": "montage_runner",
        "stage": STAGES[index - 1],
        "stage_index": index, "stage_total": len(STAGES),
        "completed_stages": completed, "total_stages": len(STAGES),
        "overall_percent": round(100 * completed / len(STAGES), 2),
    }, ensure_ascii=False), flush=True)


def validate_request(raw: dict) -> dict:
    if not isinstance(raw, dict):
        raise ValueError("Неправильные параметры монтажа")
    project_id = raw.get("project_id")
    from .projects import get_project
    project = get_project(project_id)
    mode = raw.get("mode", "custom")
    if mode not in ("montage", "tracks", "vertical", "custom"):
        raise ValueError("Неизвестный режим монтажа")
    from .montage_project import SELECTION_STRATEGIES
    strategy=raw.get("selection_strategy","highlights")
    if strategy not in SELECTION_STRATEGIES:
        raise ValueError("Неизвестный критерий отбора розыгрышей")
    selection_mode=raw.get("rally_selection_mode","auto")
    if selection_mode not in ("auto","manual"):
        raise ValueError("Неизвестный режим выбора розыгрышей")
    selected_ids=raw.get("selected_rally_ids",[])
    if selected_ids is None and selection_mode=="auto":
        selected_ids=[]
    if not isinstance(selected_ids,list) or len(selected_ids)>500 or any(
            not isinstance(rid,str) or len(rid)!=24 or
            any(c not in "0123456789abcdef" for c in rid)
            for rid in selected_ids) or len(set(selected_ids))!=len(selected_ids):
        raise ValueError("Некорректный список выбранных розыгрышей")
    follow_ball=raw.get("follow_ball",True if mode=="montage" else mode=="vertical")
    if not isinstance(follow_ball,bool):
        raise ValueError("Следить за мячом: ожидается да/нет")
    min_action_counts={}
    for kind in ("actions","attack","block","dig","overhead_pass","serve"):
        key="min_"+kind
        value=raw.get(key,0)
        try:
            number=float(value)
            if not math.isfinite(number) or not number.is_integer() or not 0<=number<=100:
                raise ValueError()
        except (ValueError,TypeError,OverflowError) as exc:
            raise ValueError(key+": целое число от 0 до 100") from exc
        min_action_counts[kind]=int(number)
    try:
        duration = float(raw.get("duration", 120))
        padding = float(raw.get("track_padding", 0.33))
        min_rally = float(raw.get("min_rally", 1.5))
        confidence = float(raw.get("min_confidence", 0.5))
    except (ValueError, TypeError, OverflowError) as exc:
        raise ValueError("Некорректная длительность или параметры") from exc
    min_duration = 10 if mode == "custom" else 1
    if not math.isfinite(duration) or not min_duration <= duration <= 600:
        raise ValueError(f"Длительность для режима должна быть от {min_duration} до 600 секунд")
    if selection_mode=="manual" and mode=="custom":
        raise ValueError("Ручной выбор доступен для монтажа розыгрышей")
    if not math.isfinite(padding) or not 0 <= padding <= 10:
        raise ValueError("Запас вокруг трека должен быть от 0 до 10 секунд")
    if (not math.isfinite(min_rally) or not math.isfinite(confidence) or
            not (1.5 <= min_rally <= 30) or not (0.05 <= confidence <= 1)):
        raise ValueError("Проверьте фильтры розыгрышей")

    from .montage_audio import resolve_audio, validate_volume
    soundtrack = resolve_audio(raw.get("audio_path"))
    audio_volume = validate_volume(raw.get("audio_volume", 40))
    videos = list(project["videos"])
    if not videos:
        raise ValueError("Добавьте исходные видео в проект")
    chosen_videos = raw.get("selected_videos", videos) if selection_mode=="manual" else videos
    if (not isinstance(chosen_videos,list) or
            len(chosen_videos)>len(videos) or
            any(not isinstance(video,str) or video not in videos
                for video in chosen_videos) or
            len(set(chosen_videos))!=len(chosen_videos)):
        raise ValueError("Выбраны видео, отсутствующие в проекте")
    # Always follow the project's canonical order, never the client order.
    chosen_videos=[video for video in videos if video in chosen_videos]
    from .rally_view import settings as rally_settings
    rally = rally_settings(raw.get("gap_sec", 3),
                           raw.get("rally_min_sec", 3),
                           raw.get("context_sec", 1.5))
    run_id = raw.get("run_id")
    caches = {}
    profile = None
    if run_id is not None:
        from .analysis_runs import list_runs, cache_for
        matches = [item for item in list_runs(project_id) if item["id"] == run_id]
        if len(matches) != 1:
            raise ValueError("Выбранный запуск анализа недоступен. Обновите список профилей.")
        run = matches[0]
        profile = run["profile"]
        for name in videos:
            cache_id = run["videos"].get(name)
            if not cache_id or not cache_for(name, project_id, cache_id):
                raise ValueError("В выбранном профиле отсутствует результат видео: " + name)
            caches[name] = cache_id
    else:
        # Compatibility with legacy callers; the browser always sends run_id.
        for name in videos:
            if not data.find_cache(name, project_id):
                raise ValueError("Сначала проанализируйте видео: " + name)
    return {
        "project_id": project_id, "mode": mode, "videos": videos,
        "duration": duration, "min_rally": min_rally, "min_confidence": confidence,
        "advanced": validated({**raw, **raw.get("advanced", {})}),
        "track_padding": padding,
        "track_include_not_rally": bool(raw.get("track_include_not_rally", False)),
        "track_show_ball": bool(raw.get("track_show_ball", False)),
        "request_id": raw.get("request_id"),
        "run_id": run_id, "profile": profile, "cache_ids": caches,
        "gap_sec": rally["gap_sec"], "rally_min_sec": rally["min_sec"],
        "context_sec": rally["context_sec"],
        "follow_ball": follow_ball, "selection_strategy": strategy,
        "rally_selection_mode": selection_mode,
        "selected_videos": chosen_videos if selection_mode=="manual" else videos,
        "selected_rally_ids": selected_ids if selection_mode=="manual" else [],
        "min_action_counts": min_action_counts,
        "audio_path": str(soundtrack) if soundtrack else "",
        "audio_volume": audio_volume,
    }


def sources_for(config: dict) -> list[dict]:
    """Resolve exactly the analyzed inputs selected by the UI, never latest cache."""
    from .analysis_runs import cache_for
    from .montage_source import analysis_offset
    sources = []
    available = (config.get("selected_videos",config["videos"])
                 if config.get("rally_selection_mode")=="manual" else config["videos"])
    for video in available:
        cache_id = config["cache_ids"].get(video)
        cache = (cache_for(video, config["project_id"], cache_id)
                 if cache_id else data.find_cache(video, config["project_id"]))
        if not cache:
            raise ValueError("Нет результатов анализа для: " + video)
        original = data.video_path(video)
        if not original.is_file():
            raise ValueError("Исходное видео для монтажа отсутствует: " + video)
        sources.append({
            "video": video, "cache": cache,
            "video_path": original,  # NEVER cache/analysis.mp4 or prepared/...
            "analysis_offset_sec": analysis_offset(cache),
            "fps": 30.0,
        })
    return sources


def selection_for(config: dict) -> list[dict]:
    """The preview and actual exporters use the same eligible ranked full rallies."""
    from .montage_project import plan_rallies
    return plan_rallies(
        sources_for(config), config["duration"],
        min_rally=config["min_rally"], min_confidence=config["min_confidence"],
        padding=config["context_sec"],
        gap_sec=config["gap_sec"], rally_min_sec=config["rally_min_sec"],
        selection_strategy=config["selection_strategy"],
        min_action_counts=config["min_action_counts"],
        selected_rally_ids=(config["selected_rally_ids"]
            if config["rally_selection_mode"]=="manual" else None))


def preview_selection(raw: dict) -> dict:
    """Return the whole analyzed rally catalogue and a separate export plan.

    Both montage modes display the same project/profile-bound rally IDs.
    Only manual mode lets the browser choose IDs. Auto mode highlights its
    ranked subset without hiding any other discovered rally.
    """
    from .montage_project import plan_rallies
    config = validate_request(raw)
    sources = sources_for(config)
    options = dict(
        min_rally=config["min_rally"], min_confidence=config["min_confidence"],
        padding=config["context_sec"], gap_sec=config["gap_sec"],
        rally_min_sec=config["rally_min_sec"],
        selection_strategy=config["selection_strategy"],
        min_action_counts=config["min_action_counts"])
    candidates = plan_rallies(
        sources, config["duration"], **options, list_candidates=True)
    catalog_ids = {row["rally_id"] for row in candidates}
    selection_error = ""
    if config["rally_selection_mode"] == "manual":
        invalid = set(config["selected_rally_ids"]) - catalog_ids
        if invalid:
            raise ValueError("Список розыгрышей изменился. Выберите их повторно.")
        chosen = set(config["selected_rally_ids"])
        selected = [row for row in candidates if row["rally_id"] in chosen]
    else:
        try:
            selected = plan_rallies(sources, config["duration"], **options)
        except (ValueError, RuntimeError, FileNotFoundError) as exc:
            # Missing scores or a short time budget must not hide the catalogue.
            selected = []
            selection_error = str(exc)
    selected_ids = {row["rally_id"] for row in selected}
    offsets = {row["video"]: row["analysis_offset_sec"] for row in sources}
    result = []
    for i, group in enumerate(candidates, 1):
        offset = float(offsets[group["video"]])
        result.append({
            "video": group["video"], "cache_id": group["cache_id"],
            "rally_id": group["rally_id"], "number": i,
            "start": round(offset + group["source_start_frame"] / 30, 3),
            "end": round(offset + (group["source_end_frame"] + 1) / 30, 3),
            "detected_start": round(offset + group["detected_start_frame"] / 30, 3),
            "detected_end": round(offset + (group["detected_end_frame"] + 1) / 30, 3),
            "duration": group["duration_sec"],
            "score": round(group["ranking_score"], 3),
            "confidence": round(group["confidence"], 3),
            "joined_parts": group["joined_parts"],
            "player_count": group.get("player_count"),
            "player_track_ids": group.get("player_track_ids", []),
            "linked_player_ids": group.get("linked_player_ids", []),
            "actions_total": group.get("actions_total", 0),
            "action_counts": group.get("action_counts", {}),
            "selected": group["rally_id"] in selected_ids,
        })
    by_id = {row["rally_id"]: row for row in result}
    selected_rows = [by_id[row["rally_id"]] for row in selected]
    selected_duration = round(sum(row["duration"] for row in selected_rows), 3)
    return {"run_id": config["run_id"], "profile": config["profile"],
            "selection_strategy": config["selection_strategy"],
            "min_action_counts": config["min_action_counts"],
            "follow_ball": config["follow_ball"],
            "settings": {k: config[k] for k in ("gap_sec", "rally_min_sec", "context_sec")},
            "rally_selection_mode": config["rally_selection_mode"],
            "candidates": result, "candidate_count": len(result),
            "selected": selected_rows, "selection_error": selection_error,
            "selected_duration": selected_duration,
            "requested_duration": (selected_duration if
                config["rally_selection_mode"] == "manual" else config["duration"]),
            "source_count": len(sources), "selected_videos": [s["video"] for s in sources]}


def _record_selection_source(destination: Path, config: dict) -> None:
    """Persist the exact source run and reconstruction settings with the export."""
    path = destination / "selection.json"
    payload = json.loads(path.read_text(encoding="utf-8"))
    payload.update({
        "analysis_run_id": config["run_id"],
        "analysis_profile": config["profile"],
        "analysis_cache_ids": config["cache_ids"],
        "rally_settings": {
            "gap_sec": config["gap_sec"],
            "min_sec": config["rally_min_sec"],
            "context_sec": config["context_sec"],
        },
        "selection_filters": {
            "min_rally_sec": config["min_rally"],
            "min_confidence": config["min_confidence"],
            "strategy": config["selection_strategy"],
            "mode": config["rally_selection_mode"],
            "selected_rally_ids": config.get("selected_rally_ids"),
            "selected_videos": config.get("selected_videos",config["videos"]),
            "min_action_counts": config["min_action_counts"],
        },
        "follow_ball": config["follow_ball"],
        "soundtrack": {
            "path": config["audio_path"],
            "volume_percent": config["audio_volume"],
        } if config["audio_path"] else None,
    })
    path.write_text(json.dumps(payload, ensure_ascii=False, indent=2),
                    encoding="utf-8")


def _finalize_mp4(source: Path, output: Path, duration: float) -> None:
    """Produces a browser-compatible MP4 capped to the requested duration."""
    if not source.is_file() or source.stat().st_size == 0:
        raise RuntimeError("Инструмент не создал исходный видеоролик")
    output.parent.mkdir(parents=True, exist_ok=True)
    # build_project_highlights has already encoded native original clips.
    # Stream-copy the finished MP4 rather than losing quality again at CRF 23.
    cmd = [
        "ffmpeg", "-nostdin", "-hide_banner", "-loglevel", "error", "-y",
        "-i", str(source), "-t", f"{duration:.3f}",
        "-map", "0:v:0", "-map", "0:a:0?", "-c", "copy",
        "-movflags", "+faststart", str(output),
    ]
    subprocess.run(cmd, check=True)
    probe = subprocess.run(
        ["ffprobe", "-v", "error", "-show_entries", "format=duration",
         "-of", "default=nw=1:nk=1", str(output)],
        text=True, capture_output=True, check=True, timeout=15)
    try:
        actual = float(probe.stdout.strip())
    except ValueError as exc:
        raise RuntimeError("Не удалось измерить длительность готового MP4") from exc
    if actual <= 0 or actual > duration + .15:
        raise RuntimeError(f"Некорректная длительность монтажа: {actual:.2f} с")
    print(f"OUTPUT DURATION: {actual:.2f} / {duration:.2f} seconds", flush=True)


def execute(config: dict):
    emit_stage(1)
    config = validate_request(config)
    emit_stage(1, done=True)
    project_id, mode = config["project_id"], config["mode"]
    identifier = config.get("request_id") or "manual"
    if config["run_id"]:
        # Montage belongs to an exact completed analysis invocation, not just
        # its configuration name or the project's latest cache.
        from .projects import project_run_root
        destination = (project_run_root(project_id) / "branches"
                       / config["run_id"] / "montages" / identifier)
    else:
        # Legacy CLI callers without an analysis-run selection.
        destination = project_dir(project_id) / "montages" / identifier
    destination.mkdir(parents=True, exist_ok=True)
    seconds = config["duration"]
    if config["rally_selection_mode"]=="manual":
        chosen=selection_for(config)
        if not chosen:
            raise ValueError("Отметьте хотя бы один розыгрыш для монтажа")
        seconds=sum((int(g["source_end_frame"])-int(g["source_start_frame"])+1)/30
                    for g in chosen)
        if seconds>7200:
            raise ValueError("Выбранные розыгрыши превышают лимит 2 часа")
    print(f"MONTAGE MODE: {mode}; target: {seconds:.2f} seconds", flush=True)
    emit_stage(2)
    if mode == "custom":
        previous = project_output(project_id)
        if previous.is_file() and previous.stat().st_size:
            shutil.copy2(previous, destination / "previous_project_highlights.mp4")
        cmd = [
            sys.executable, str(data.ROOT / "scripts" / "build_project_highlights.py"),
            "--project-id", project_id, "--duration", str(seconds),
            "--min-rally", str(config["min_rally"]),
            "--min-confidence", str(config["min_confidence"]),
            "--rally-gap-sec", str(config["gap_sec"]),
            "--rally-min-sec", str(config["rally_min_sec"]),
            "--context-sec", str(config["context_sec"]),
            *([] if not config["run_id"] else ["--run-id", config["run_id"]]),
            *as_cli({k: v for k, v in config["advanced"].items()
                     if k.startswith(("reels_", "montage_"))}),
        ]
        emit_stage(2, done=True)
        emit_stage(3)
        subprocess.run(cmd, cwd=data.ROOT, check=True)
        manifest_file = project_dir(project_id) / "highlights_manifest.json"
        manifest = json.loads(manifest_file.read_text(encoding="utf-8"))
        clips = manifest.get("selected_rallies", [])
        (destination / "selection.json").write_text(json.dumps({
            "selection_method": "shared complete rallies: ranking + whole boundaries",
            "requested_duration": seconds,
            "selected_duration": manifest.get("total_seconds"),
            "source_count": len(config["videos"]),
            "selected": [{
                "index": index, "video": clip["video"],
                "track_ids": clip.get("track_ids", []),
                "start_frame": clip["source_start_frame"],
                "end_frame": clip["source_end_frame"],
                "detected_start_frame": clip["detected_start_frame"],
                "detected_end_frame": clip["detected_end_frame"],
                "score": clip["ranking_score"], "confidence": clip["confidence"],
                "source_duration": clip["duration_sec"],
            } for index, clip in enumerate(clips)],
        }, ensure_ascii=False, indent=2), encoding="utf-8")
        _record_selection_source(destination, config)
        emit_stage(3, done=True)
        emit_stage(4)
        final_video = destination / "best_moments.mp4"
        _finalize_mp4(project_output(project_id), final_video, seconds)
        if config["audio_path"]:
            from .montage_audio import overlay_music
            overlay_music(final_video, Path(config["audio_path"]), config["audio_volume"])
        emit_stage(4, done=True)
        return

    # All project videos contribute candidates; only one combined output is
    # produced. Source-by-source montage is not an independent render mode.
    sources = sources_for(config)
    for source in sources:
        if not (source["cache"] / "tracks/analysis/tracks").is_dir():
            raise ValueError("Отсутствуют треки анализа: " + source["video"])
    # Identical full-rally timeline in all modes; context belongs to selection.
    choices = plan_clips(
        sources, destination / "selected_tracks", seconds,
        mode=mode, padding=config["context_sec"],
        min_rally=config["min_rally"], min_confidence=config["min_confidence"],
        gap_sec=config["gap_sec"], rally_min_sec=config["rally_min_sec"],
        selection_strategy=config["selection_strategy"],
        min_action_counts=config["min_action_counts"],
        selected_rally_ids=(config["selected_rally_ids"]
            if config["rally_selection_mode"]=="manual" else None))
    _record_selection_source(destination, config)
    used = sorted(set(c["video"] for c in choices))
    print(f"SELECTED: {len(choices)} эпизодов из {len(used)}/{len(sources)} видео; "
          f"бюджет {seconds:.2f} с", flush=True)
    emit_stage(2, done=True)

    temp_root = destination / "_render_temp"
    temp_root.mkdir(parents=True, exist_ok=True)
    segments = []
    emit_stage(3)
    from .montage_source import clip_from_original, relative_track
    for clip in choices:
        index = clip["index"]
        video = clip["video"]
        source_video = Path(clip["video_path"])
        folder = temp_root / f"{index:03d}"
        folder.mkdir(parents=True, exist_ok=True)
        # Build this episode from its original recording. The analysis cache
        # supplies only frame boundaries and ball coordinates (at 30 FPS).
        original_clip = folder / "original_episode.mp4"
        provenance = clip_from_original(
            source_video, original_clip,
            start_frame=int(clip["start_frame"]),
            end_frame=int(clip["end_frame"]),
            offset=float(clip.get("analysis_offset_sec", 0)),
        )
        metadata = folder / "original_source.json"
        metadata.write_text(json.dumps(provenance, ensure_ascii=False, indent=2),
                            encoding="utf-8")
        use_tracking = config["follow_ball"] if mode == "montage" else mode == "vertical"
        print(f"EPISODE {index+1}/{len(choices)}: ORIGINAL {video} "
              f"at {provenance['start_sec']:.3f}s "
              f"({clip['source_duration']:.2f}s)", flush=True)
        if use_tracking or config["track_show_ball"]:
            local_track = relative_track(
                Path(clip["track_json"]), folder / "local_tracks",
                start_frame=int(clip["start_frame"]),
                native_fps=float(provenance["fps"]),
                native_count=int(provenance["frames"]))
        if not use_tracking:
            if not config["track_show_ball"]:
                # A camera-wide montage needs no OpenCV/mp4v intermediate.
                # Keep native source detail from the exact original clip.
                raw_file = original_clip
            else:
                raw_file = folder / "marked_clip.mp4"
                cmd = [
                    sys.executable, str(data.ROOT / "src" / "track_processor.py"),
                    "--video_path", str(original_clip),
                    "--json_dir", str(local_track.parent),
                    "--output_path", str(raw_file),
                    "--padding", "0",
                    "--quality-crf", "16",
                ]
                from .montage_encoding import encoder_flags
                preferred = ("gpu" if "h264_nvenc" in encoder_flags(
                    float(clip["source_duration"])) else "cpu")
                cmd.extend(["--quality-encoder", preferred])
                try:
                    subprocess.run(cmd, cwd=data.ROOT, check=True)
                except subprocess.CalledProcessError:
                    if preferred != "gpu":
                        raise
                    cmd[cmd.index("--quality-encoder")+1] = "cpu"
                    subprocess.run(cmd, cwd=data.ROOT, check=True)
        else:
            advanced = config["advanced"]
            from .montage_encoding import encoder_flags
            quality_encoder = (
                "gpu" if "h264_nvenc" in encoder_flags(
                    float(clip["source_duration"])) else "cpu")
            cmd = [
                sys.executable, str(data.ROOT / "src" / "make_reels.py"),
                "--video_path", str(original_clip),
                "--json_dir", str(local_track.parent),
                "--output_dir", str(folder / "cropped"),
                "--smoothing", advanced["reels_smoothing"],
                "--interpolation", advanced["reels_interpolation"],
                "--margin", str(advanced["reels_margin"]),
                "--padding", "0",
                "--smooth_window", str(advanced["reels_smooth_window"]),
                "--smooth_polyorder", str(advanced["reels_smooth_polyorder"]),
                "--quality-crf", "16",
                "--quality-encoder", quality_encoder,
            ]
            try:
                subprocess.run(cmd, cwd=data.ROOT, check=True)
            except subprocess.CalledProcessError:
                if quality_encoder != "gpu":
                    raise
                print("MONTAGE: GPU crop failed; retrying on CPU",flush=True)
                cmd[cmd.index("--quality-encoder") + 1] = "cpu"
                subprocess.run(cmd, cwd=data.ROOT, check=True)
            created = list((folder / "cropped").rglob("*.mp4"))
            if not created:
                raise RuntimeError(f"make_reels.py не создал эпизод {index+1}")
            raw_file = next(
                (file for file in created if "combined" in file.name),
                sorted(created)[0])
        if not raw_file.is_file() or raw_file.stat().st_size == 0:
            raise RuntimeError(f"Нет готового фрагмента {index+1} из {video}")
        segments.append(raw_file)
    emit_stage(3, done=True)

    emit_stage(4)
    track_camera = config["follow_ball"] if mode == "montage" else mode == "vertical"
    output_name = ("highlights_reel.mp4" if track_camera else "highlights_full.mp4") if mode=="montage" else (
        "project_tracks.mp4" if mode=="tracks" else "project_vertical.mp4")
    output = destination / output_name
    compose_clips(segments, output, seconds, "vertical" if track_camera else "tracks")
    if config["audio_path"]:
        from .montage_audio import overlay_music
        print("MONTAGE MUSIC:", Path(config["audio_path"]).name, flush=True)
        overlay_music(output, Path(config["audio_path"]), config["audio_volume"])
    shutil.rmtree(temp_root)
    print("MONTAGE COMPLETE:", output, flush=True)
    emit_stage(4, done=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--config", required=True)
    args = parser.parse_args()
    config = json.loads(Path(args.config).read_text(encoding="utf-8"))
    execute(config)


if __name__ == "__main__":
    main()
