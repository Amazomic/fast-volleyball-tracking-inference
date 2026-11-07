import json
import math
from pathlib import Path


def analysis_capabilities(cache: Path) -> dict:
    original = cache.parent.parent if cache.parent.name == "postprocessing" else cache
    try:
        manifest = json.loads((original / "inference.manifest.json").read_text(encoding="utf-8"))
        if not isinstance(manifest, dict):
            manifest = {}
    except (OSError, ValueError):
        manifest = {}
    outputs = manifest.get("outputs") or {}
    if not isinstance(outputs, dict):
        outputs = {}
    complete = bool(outputs) and manifest.get("schema") == "volley-inference/1" and all(
        (original / name).is_file() for name in outputs
    ) and {"players.json", "ball/analysis/ball.csv"} <= outputs.keys()
    touch = manifest.get("player_model") == "RAVEL-VB-013-9f"
    floor = manifest.get("player_action_threshold")
    if "players.raw.v2.jsonl" in outputs:
        floor = 0
    try:
        floor = float(floor)
        if not math.isfinite(floor) or not 0 <= floor <= 1:
            floor = None
    except (TypeError, ValueError):
        floor = None
    modes = ["baseline"] if complete else []
    if complete and touch:
        modes += (["ravel-touch"] if floor is not None else []) + ["ravel-geometry"]
        if floor is not None:
            modes.append("combined")
    return {
        "reprocess_modes": modes, "touch_floor": floor,
        "raw_sources": {
            "vballnet": (original / "ball/analysis/ball.csv").is_file(),
            "ravel": (original / "players.json").is_file(),
        },
        "model_touch": touch,
    }
