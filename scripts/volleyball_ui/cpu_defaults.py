"""Version-controlled golden CPU settings shared by the UI and automatic inference replay.

Never promote changed settings just because a run completed: compare final
contact/rally/player quality on the same evaluation clips first.
"""
from __future__ import annotations

import json
from pathlib import Path

_DEFAULTS_FILE = Path(__file__).with_name("static") / "cpu-defaults.json"
_REQUIRED = frozenset({
    "mode", "touch_threshold", "ball_threshold", "player_threshold",
    "touch_min_interval_sec", "touch_max_reach_ratio",
    "track_min_duration", "track_max_distance", "player_hold_frames",
    "player_match_distance", "track_player_threshold",
    "track_ball_threshold", "track_no_ball_fill", "beach",
})


def golden_cpu_settings() -> dict:
    payload = json.loads(_DEFAULTS_FILE.read_text(encoding="utf-8"))
    if payload.get("schema") != "volley-cpu-golden-defaults/1":
        raise ValueError("Unexpected golden CPU config schema")
    settings = payload.get("settings")
    if not isinstance(settings, dict) or set(settings) != _REQUIRED:
        raise ValueError("Incomplete golden CPU settings")
    return dict(settings)


def automatic_cpu_settings(player_model: str) -> dict:
    """Only older player models need the geometry-only compatibility fallback."""
    settings = golden_cpu_settings()
    if player_model != "RAVEL-VB-013-9f":
        settings["mode"] = "baseline"
        settings["player_threshold"] = None
    return settings
