#!/usr/bin/env python3
"""CPU-only replay of one completed analysis run for all project videos."""
from __future__ import annotations

import argparse
import json
import math
import sys
import hashlib
from datetime import datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from scripts.volleyball_studio.reprocess import MODES, reprocess
from scripts.volleyball_ui.analysis_runs import cache_for, list_runs


def process_project(project_id: str, run_id: str, mode: str | None = None,
                    touch_threshold: float = 0.5,
                    ball_threshold: float | None = None,
                    player_threshold: float | None = None,
                    touch_min_interval_sec: float = .35,
                    touch_max_reach_ratio: float = .35,
                    track_min_duration: float = .5,
                    track_max_distance: float = 200.,
                    track_player_threshold: float = .35,
                    track_ball_threshold: float = .35,
                    track_no_ball_fill: bool = False,
                    beach: bool = False,
                    player_hold_frames: int = 90,
                    player_match_distance: float = .14) -> dict:
    if (mode is not None and mode not in MODES or
            not math.isfinite(touch_threshold) or not 0 <= touch_threshold <= 1):
        raise ValueError("Unsupported postprocessing settings")
    started_at = datetime.now().astimezone().isoformat()
    existing_runs = list_runs(project_id)
    existing_ids = {run["id"] for run in existing_runs}
    matches = [run for run in existing_runs
               if run["id"] == run_id and not run.get("parent_run_id")]
    if len(matches) != 1:
        raise ValueError("Choose a completed original analysis run")
    original = matches[0]
    results = {}
    total = len(original["videos"])
    if not total:
        raise ValueError("Original analysis has no completed videos")
    published_settings = None
    stages_per_video = 5
    for index, (video, cache_id) in enumerate(original["videos"].items(), 1):
        base = cache_for(video, project_id, cache_id)
        last_step = -1
        def report(step: int, message: str):
            nonlocal last_step
            last_step = step
            completed = (index - 1)*stages_per_video + step
            print(json.dumps({
                "event": "UI_PROGRESS", "kind": "reprocess",
                "phase": "complete" if step == stages_per_video else "running",
                "stage": message, "video_name": video,
                "video_index": index, "videos_total": total,
                "stage_index": step, "stage_total": stages_per_video,
                "completed_stages": completed,
                "total_stages": total*stages_per_video,
                "overall_percent": round(100*completed/(total*stages_per_video), 1),
            }, ensure_ascii=False), flush=True)
        report(0, f"Видео {index}/{total}: проверяем кэш · {video}")
        from scripts.volleyball_ui.data import excluded_track_ids
        excluded = excluded_track_ids(video, project_id, cache_id)
        output = reprocess(base, mode=mode, touch_threshold=touch_threshold,
                           excluded_player_ids=excluded,
                           ball_threshold=ball_threshold,
                           player_threshold=player_threshold,
                           touch_min_interval_sec=touch_min_interval_sec,
                           touch_max_reach_ratio=touch_max_reach_ratio,
                           track_min_duration=track_min_duration,
                           track_max_distance=track_max_distance,
                           track_player_threshold=track_player_threshold,
                           track_ball_threshold=track_ball_threshold,
                           track_no_ball_fill=track_no_ball_fill, beach=beach,
                           player_hold_frames=player_hold_frames,
                           player_match_distance=player_match_distance,
                           on_progress=report)
        # Do not report a successful CPU version unless the worker published
        # its manifest and each video has the same effective settings.
        manifest_path = output / "postprocess.manifest.json"
        if not manifest_path.is_file():
            raise ValueError(f"CPU postprocessing incomplete: {video}")
        metadata = json.loads(manifest_path.read_text(encoding="utf-8"))
        settings = metadata.get("settings")
        if (metadata.get("schema") != "volley-postprocessing/1" or
                metadata.get("version_id") != output.name or
                not isinstance(settings, dict) or
                not (output / "ranking.json").is_file() or
                not (output / "analysis/events.json").is_file()):
            raise ValueError(f"CPU result manifest invalid: {video}")
        if published_settings is None:
            published_settings = settings
        elif settings != published_settings:
            raise ValueError("CPU settings differ between project videos")
        results[video] = cache_id + "." + output.name
        if last_step < stages_per_video:
            report(stages_per_video, "Версия анализа сохранена")
    # A version removed from the header may be regenerated with the same
    # settings. Publish it again only after every project video has completed.
    from scripts.volleyball_ui.analysis_runs import restore_postprocessing_version
    restore_postprocessing_version(project_id, run_id, published_settings)
    from scripts.volleyball_ui.analysis_runs import publish_postprocessing_snapshot
    settings_json = json.dumps(published_settings, ensure_ascii=False, sort_keys=True)
    legacy_id = run_id + "_pp_" + hashlib.sha256(
        settings_json.encode("utf-8")).hexdigest()[:12]
    snapshot = publish_postprocessing_snapshot(
        project_id, run_id, published_settings, results,
        started_at=started_at, suppress_legacy=legacy_id not in existing_ids)
    return {"status": "complete", "project_id": project_id, "parent_run_id": run_id,
            "mode": published_settings["mode"], "videos": results,
            "snapshot_id": snapshot["id"]}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project-id", required=True)
    parser.add_argument("--run-id", required=True)
    # Missing --mode means automatic: RAVEL-013 uses Touch peaks + geometry.
    parser.add_argument("--mode", choices=MODES, default=None)
    parser.add_argument("--touch-threshold", type=float, default=.5)
    parser.add_argument("--ball-threshold", type=float)
    parser.add_argument("--player-threshold", type=float)
    parser.add_argument("--touch-min-interval-sec", type=float, default=.35)
    parser.add_argument("--touch-max-reach-ratio", type=float, default=.35)
    parser.add_argument("--track-min-duration", type=float, default=.5)
    parser.add_argument("--track-max-distance", type=float, default=200.)
    parser.add_argument("--track-player-threshold", type=float, default=.35)
    parser.add_argument("--track-ball-threshold", type=float, default=.35)
    parser.add_argument("--player-hold-frames", type=int, default=90)
    parser.add_argument("--player-match-distance", type=float, default=.14)
    parser.add_argument("--track-no-ball-fill", action="store_true")
    parser.add_argument("--beach", action="store_true")
    args = parser.parse_args()
    print(json.dumps(process_project(
        args.project_id, args.run_id, args.mode, args.touch_threshold,
        args.ball_threshold, args.player_threshold,
        args.touch_min_interval_sec, args.touch_max_reach_ratio,
        args.track_min_duration, args.track_max_distance,
        args.track_player_threshold, args.track_ball_threshold,
        args.track_no_ball_fill, args.beach,
        args.player_hold_frames, args.player_match_distance),
        ensure_ascii=False), flush=True)


if __name__ == "__main__":
    main()
