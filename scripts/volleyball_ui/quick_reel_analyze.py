"""Run full default analysis for a quick reel; always fresh, never project-bound."""
from __future__ import annotations

import argparse
import json
from pathlib import Path

from . import data
from .quick_reel_jobs import quick_dir, request_for
from .quick_reel_prepare import prepared_entry, ready_source_path


def default_arguments():
    from scripts.volleyball_studio.__main__ import parser
    args = parser().parse_args([])
    # The current recommended GPU defaults (not a saved user profile).
    args.backend = "onnx"
    args.player_model = "RAVEL-VB-013-9f"
    args.player_backend = "onnx-cuda"
    args.player_device = "GPU"
    args.gpu_device_id = 0
    args.gpu_memory_limit_mb = 0
    args.beach = True
    args.no_court_fallback = True
    return args


def analyze_one(reel_id: str, video_index: int) -> dict:
    request = request_for(reel_id)
    rows = request["videos"]
    if not 1 <= video_index <= len(rows):
        raise ValueError("Неверный номер исходного видео")
    row = rows[video_index-1]
    source = Path(row["source"]).resolve(strict=True)
    stat = source.stat()
    if (str(source) != row["source"] or stat.st_size != row["size"]
            or stat.st_mtime_ns != row["mtime_ns"]):
        raise ValueError("Исходное видео изменилось после выбора: " + row["video"])
    prepared = ready_source_path(source)
    if prepared is None:
        raise RuntimeError("Сначала подготовьте исходное видео: " + row["video"])
    args = default_arguments()
    args.input = source
    args.prepared_video = prepared
    args.prepared_input = True
    args.analysis_width = int(prepared_entry(source)[4]["width"])
    args.cache_salt = reel_id + "-" + str(video_index)
    args.analysis_start = 0
    args.analysis_seconds = None
    # Each quick reel has its own analysis directory and source-specific work.
    # No search of existing results/projects, no shared inference caches.
    work = quick_dir(reel_id) / "analysis" / f"video_{video_index:02d}"
    work.mkdir(parents=True, exist_ok=True)
    from scripts.volleyball_studio.analysis import analyze
    from scripts.volleyball_studio.advanced import from_namespace
    from_namespace(args)
    _, warnings, cache = analyze(source, work, args)
    if not (cache/"ranking.json").is_file() or not (cache/"analysis/events.json").is_file():
        raise RuntimeError("Анализ не сохранил полный рейтинг и события")
    result = {
        "video": row["video"], "source": str(source), "cache": str(cache),
        "prepared": str(prepared), "warnings": warnings,
        "analysis_settings": {
            "ball_model": args.ball_model, "ball_threshold": args.ball_threshold,
            "player_model": args.player_model, "player_backend": args.player_backend,
            "player_device": args.player_device,
            "player_score_threshold": args.player_score_threshold,
            "player_action_threshold": args.player_action_threshold,
            "analysis_width": args.analysis_width,
            "gpu_memory_limit_mb": args.gpu_memory_limit_mb,
        },
    }
    data.save_json(work/"result.json", result)
    return result


def main():
    arg = argparse.ArgumentParser()
    arg.add_argument("--id", required=True)
    arg.add_argument("--video-index", type=int, required=True)
    options = arg.parse_args()
    result = analyze_one(options.id, options.video_index)
    print(json.dumps({"event": "QUICK_ANALYSIS_COMPLETE",
                      "video_index": options.video_index,
                      "cache": result["cache"]}, ensure_ascii=False), flush=True)


if __name__ == "__main__":
    main()
