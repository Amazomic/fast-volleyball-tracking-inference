"""Immutable provenance and compatibility checks for saved neural outputs.

The manifest is published only after BOTH detectors successfully complete.
It is not a promise that thresholded/decoded outputs contain full tensors.
"""
from __future__ import annotations

import json
from pathlib import Path

from .common import digest, save

SCHEMA = "volley-inference/1"
FILES = (
    "ball/analysis/ball.csv",
    "ball/analysis/ball.raw.csv",
    "ball/analysis/ball.raw.json",
    "players.json",
    "players.raw.jsonl",
    "players.raw.v2.jsonl",
    "players.raw.json",
)


class InferenceCacheError(ValueError):
    """An incomplete or incompatible cache must be re-inferred."""


def publish(folder: Path, *, source: Path, source_id, model: str,
            ball_threshold: float, backend: str,
            analysis_offset_sec: float = 0.,
            court_present: bool = False,
            player_touch_smooth: int = 5,
            player_touch_min_gap: int = 10) -> Path:
    folder, source = Path(folder).resolve(), Path(source).resolve()
    entries = {}
    for name in FILES:
        path = folder / name
        if path.is_file():
            entries[name] = {"bytes": path.stat().st_size, "sha256": digest(path)}
    if not all(name in entries for name in ("players.json", "players.raw.jsonl",
                                            "players.raw.json", "ball/analysis/ball.csv")):
        raise InferenceCacheError("The neural outputs are incomplete")
    raw = folder / "ball/analysis/ball.raw.json"
    if backend != "openvino" and not raw.is_file():
        raise InferenceCacheError("VballNet raw sidecar is missing")
    player_data = json.loads((folder / "players.json").read_text(encoding="utf-8"))
    player_model = Path(str(player_data.get("model", ""))).stem
    data = {
        "schema": SCHEMA, "source": str(source), "source_identity": source_id,
        "input_sha256": digest(folder / "analysis.mp4"),
        "ball_model": model, "ball_backend": backend,
        "analysis_offset_sec": float(analysis_offset_sec),
        "court_present": bool(court_present),
        "ball_threshold": float(ball_threshold),
        "player_model": player_model,
        "player_score_threshold": player_data.get("score_threshold"),
        "player_candidate_floor": float(
            json.loads((folder / "players.raw.json").read_text(encoding="utf8"))
                .get("minimum_confidence", .05))
                if (folder / "players.raw.json").is_file() else .05,
        "player_ball_threshold": player_data.get("ball_threshold"),
        "player_action_threshold": player_data.get("action_threshold"),
        "player_touch_smooth": int(player_touch_smooth),
        "player_touch_min_gap": int(player_touch_min_gap),
        "frame_step": player_data.get("frame_step"),
        "width": player_data["source_size"]["width"],
        "height": player_data["source_size"]["height"],
        "fps": player_data["fps"], "outputs": entries,
        "limitations": [
            "VballNet heatmap/grid sidecar contains only one peak per frame",
            "RAVEL raw proposals are retained to the declared candidate floor; "
            "full logits and skipped frames are not stored",
            "Changing decoding thresholds can require new GPU inference",
        ],
    }
    target = folder / "inference.manifest.json"
    save(target, data)
    return target


def load(folder: Path, *, verify_hashes: bool = True) -> dict:
    folder = Path(folder).resolve()
    path = folder / "inference.manifest.json"
    if not path.is_file():
        raise InferenceCacheError(
            "Old or incomplete inference cache: no manifest. Run GPU inference again.")
    try:
        manifest = json.loads(path.read_text(encoding="utf-8"))
        if manifest.get("schema") != SCHEMA:
            raise InferenceCacheError("Unsupported inference cache version")
        outputs = manifest["outputs"]
        for name in ("players.json", "ball/analysis/ball.csv"):
            if name not in outputs:
                raise InferenceCacheError(f"Missing required inference output: {name}")
        for name, info in outputs.items():
            if name not in FILES:
                raise InferenceCacheError(f"Unexpected inference output path: {name}")
            candidate = folder / name
            if (not candidate.is_file() or candidate.stat().st_size != info["bytes"]
                    or (verify_hashes and digest(candidate) != info["sha256"])):
                raise InferenceCacheError(f"Damaged inference cache: {name}")
        return manifest
    except (OSError, KeyError, TypeError, json.JSONDecodeError) as exc:
        raise InferenceCacheError("Corrupted inference manifest") from exc
