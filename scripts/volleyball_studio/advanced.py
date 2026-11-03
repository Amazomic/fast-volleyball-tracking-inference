"""Validated advanced options used by the web UI and the studio CLI.

The options describe real flags of the pipeline's detection, tracking and
export scripts. File paths are intentionally managed internally.
"""
from __future__ import annotations

from argparse import ArgumentParser
import math

# name: (type, default, lower, upper, choices)
SPECS = {
    "track_max_distance": (float, 200.0, 1, 2000, None),
    "track_min_duration": (float, 0.5, 0.05, 30, None),
    "track_player_threshold": (float, 0.35, 0, 1, None),
    "track_ball_threshold": (float, 0.35, 0, 1, None),
    "track_no_ball_fill": (bool, False, None, None, None),
    "player_model": (str, "RAVEL-VB-012-9f", None, None, ("RAVEL-VB-012-9f", "RAVEL-VB-011-9f", "RAVEL-VB-013-9f")),
    "player_device": (str, "CPU", None, None, ("CPU", "GPU", "AUTO")),
    "player_backend": (str, "openvino", None, None, ("openvino", "onnx-cuda", "onnx-cpu")),
    "player_score_threshold": (float, 0.42, 0, 1, None),
    "player_close_threshold": (float, 0.20, 0, 1, None),
    "player_hysteresis_frames": (int, 18, 0, 30, None),
    "player_match_center_distance": (float, 0.11, 0.02, 0.30, None),
    "player_match_iou": (float, 0.08, 0, 1, None),
    "player_ball_threshold": (float, 0.35, 0, 1, None),
    "player_head_threshold": (float, 0.30, 0, 1, None),
    "player_action_threshold": (float, 0.50, 0, 1, None),
    "player_nms_iou": (float, 0.5, 0, 1, None),
    "player_stride": (int, 9, 1, 36, None),
    "player_frame_step": (int, 1, 1, 4, None),
    "player_warmup_runs": (int, 3, 0, 20, None),
    "player_performance_hint": (str, "LATENCY", None, None, ("LATENCY", "THROUGHPUT")),
    "player_include_features": (bool, False, None, None, None),
    "player_output_video": (bool, False, None, None, None),
    "ball_output_video": (bool, False, None, None, None),
    "tracks_export_mode": (str, "none", None, None, ("none", "combined", "split", "both")),
    "tracks_include_not_rally": (bool, False, None, None, None),
    "tracks_mark_ball": (bool, True, None, None, None),
    "tracks_debug": (bool, False, None, None, None),
    "tracks_padding": (float, 0.33, 0, 10, None),
    "show_rally_snapshot": (bool, False, None, None, None),
    "show_rally_track": (int, 0, 0, 99999, None),
    "montage_padding_sec": (float, 1.5, 0, 10, None),
    "reels_export": (bool, False, None, None, None),
    "reels_limit": (int, 10, 1, 40, None),
    "reels_smoothing": (str, "moving_avg", None, None, ("none", "moving_avg", "savitzky_golay", "kalman")),
    "reels_interpolation": (str, "hold", None, None, ("hold", "linear")),
    "reels_margin": (float, 0, -1000, 1000, None),
    "reels_padding": (float, 0.33, 0, 10, None),
    "reels_smooth_window": (int, 15, 3, 101, None),
    "reels_smooth_polyorder": (int, 2, 1, 5, None),
    "reels_fade_transition": (bool, False, None, None, None),
}


def validate_one(name, raw):
    kind, default, minimum, maximum, choices = SPECS[name]
    if kind is bool:
        if isinstance(raw, bool):
            value = raw
        elif raw in ("true", "on", "1", 1):
            value = True
        elif raw in ("false", "off", "0", 0, None, ""):
            value = False
        else:
            raise ValueError(f"{name}: ожидается да/нет")
    elif kind is int:
        if isinstance(raw, bool):
            raise ValueError(f"{name}: ожидается целое число")
        numeric = float(raw)
        if not math.isfinite(numeric) or not numeric.is_integer():
            raise ValueError(f"{name}: ожидается целое число")
        value = int(numeric)
    elif kind is float:
        value = float(raw)
        if not math.isfinite(value):
            raise ValueError(f"{name}: значение должно быть конечным")
    else:
        value = str(raw)
    if choices is not None and value not in choices:
        raise ValueError(f"{name}: допустимые значения: {', '.join(choices)}")
    if minimum is not None and value < minimum or maximum is not None and value > maximum:
        raise ValueError(f"{name}: допустимый диапазон {minimum}–{maximum}")
    return value


def validated(raw: dict):
    values = {name: validate_one(name, raw.get(name, spec[1]))
              for name, spec in SPECS.items()}
    if values["reels_smooth_window"] % 2 != 1:
        raise ValueError("Окно сглаживания должно быть нечётным")
    if values["reels_smooth_polyorder"] >= values["reels_smooth_window"]:
        raise ValueError("Порядок полинома должен быть меньше окна сглаживания")
    return values


def add_arguments(parser: ArgumentParser):
    for name, (kind, default, minimum, maximum, choices) in SPECS.items():
        flag = "--" + name.replace("_", "-")
        if kind is bool:
            # BooleanOptionalAction supports both --flag and --no-flag.
            from argparse import BooleanOptionalAction
            parser.add_argument(flag, action=BooleanOptionalAction, default=default)
        else:
            parser.add_argument(flag, type=kind, default=default, choices=choices)


def as_cli(values: dict):
    result = []
    for name, value in values.items():
        kind = SPECS[name][0]
        flag = "--" + name.replace("_", "-")
        if kind is bool:
            result.append(flag if value else "--no-" + name.replace("_", "-"))
        else:
            result.extend([flag, str(value)])
    return result


def from_namespace(args):
    return validated({name: getattr(args, name, spec[1]) for name, spec in SPECS.items()})
