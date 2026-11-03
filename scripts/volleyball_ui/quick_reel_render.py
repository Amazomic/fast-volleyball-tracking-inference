"""Independent quick-reel montage using shared clip utilities, not project runs."""
from __future__ import annotations

import json
import shutil
import subprocess
import sys
from pathlib import Path

from . import data
from .montage_project import plan_clips, compose_clips
from .quick_reel_jobs import quick_dir, request_for
from scripts.volleyball_studio.advanced import validated


def sources_for_reel(reel_id: str) -> list[dict]:
    root = quick_dir(reel_id).resolve()
    request = request_for(reel_id)
    sources = []
    for index, row in enumerate(request["videos"], 1):
        file = root / "analysis" / f"video_{index:02d}" / "result.json"
        result = data.read_json(file, {})
        if result.get("video") != row["video"] or result.get("source") != row["source"]:
            raise ValueError("Быстрый рилс: нет независимого результата анализа " + row["video"])
        cache = Path(result["cache"]).resolve()
        if not cache.is_relative_to(root) or not (cache/"analysis/events.json").is_file():
            raise ValueError("Анализ отсутствует в каталоге быстрого рилса")
        source = Path(row["source"]).resolve(strict=True)
        if str(source) != row["source"]:
            raise ValueError("Исходник быстрого рилса изменился")
        sources.append({
            "video":row["video"], "cache":cache, "video_path":source,
            "analysis_offset_sec":0, "fps":30.0,
        })
    return sources


def quick_render_defaults(audio: Path, duration: float) -> dict:
    """Use the ordinary montage engine's built-in defaults, no project settings."""
    return {
        "mode":"montage", "follow_ball":True,
        "selection_strategy":"highlights", "rally_selection_mode":"auto",
        "min_rally":1.5, "min_confidence":0.5,
        "context_sec":1.5, "gap_sec":3.0, "rally_min_sec":3.0,
        "min_action_counts":{}, "selected_rally_ids":None,
        "track_show_ball":False, "advanced":validated({}),
        "audio_path":str(audio), "audio_volume":40,
    }


def render_reel(reel_id: str, audio: Path, seconds: float, progress=None) -> Path:
    config = quick_render_defaults(audio, seconds)
    destination = quick_dir(reel_id) / "montage"
    destination.mkdir(parents=True,exist_ok=True)
    mode = config["mode"]
    def emit_stage(index, *, done=False):
        if progress:
            progress(index, done)
    emit_stage(2)
    # All project videos contribute candidates; only one combined output is
    # produced. Source-by-source montage is not an independent render mode.
    sources = sources_for_reel(reel_id)
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

    return output
