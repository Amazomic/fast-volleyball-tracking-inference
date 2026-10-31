"""Per-project volleyball court calibration (8 image-space keypoints).

Coordinates are in the source video's display orientation, not CSS pixels.
One static calibration applies to the selected video; camera movement requires
additional per-time calibration (not supported yet).
"""
from __future__ import annotations

import math
from . import data
from .preview import video_info
from .projects import assert_project_video, get_project, project_dir, project_ranges

NAMES = [
    "1_back_left", "2_back_left", "3_back_right", "4_back_right",
    "5_center_left", "6_center_right", "7_net_left", "8_net_right",
]


def court_path(project_id: str, video: str):
    assert_project_video(project_id, video)
    return project_dir(project_id) / "courts" / f"{video}_court.json"


def get_court(project_id: str, video: str):
    path = court_path(project_id, video)
    return data.read_json(path) if path.is_file() else None


def validate_court(project_id: str, video: str, payload: dict):
    if not isinstance(payload, dict):
        raise ValueError("Разметка должна быть объектом")
    info = video_info(video)
    width, height = int(info["width"]), int(info["height"])
    if payload.get("frame_width") != width or payload.get("frame_height") != height:
        raise ValueError("Размеры разметки не совпадают с исходным видео")
    pts = payload.get("keypoints")
    if not isinstance(pts, list) or len(pts) != 8:
        raise ValueError("Для разметки нужны все восемь точек")
    checked = []
    for i, pt in enumerate(pts):
        if not isinstance(pt, dict) or pt.get("id") != i or pt.get("name") != NAMES[i]:
            raise ValueError("Неправильный порядок точек площадки")
        try:
            x, y = float(pt["x"]), float(pt["y"])
        except (KeyError, ValueError, TypeError, OverflowError) as exc:
            raise ValueError("Некорректные координаты точки") from exc
        if not (math.isfinite(x) and math.isfinite(y) and 0 <= x <= width and 0 <= y <= height):
            raise ValueError(f"Точка {i+1} за пределами кадра")
        checked.append({"id": i, "name": NAMES[i], "x": round(x, 3),
                        "y": round(y, 3), "visible": True})
    # The first four are ordered near-left, far-left, far-right, near-right;
    # refuse essentially collinear or collapsed polygon.
    corners = checked[:4]
    signed_twice_area = sum(
        corners[i]["x"]*corners[(i+1)%4]["y"] -
        corners[(i+1)%4]["x"]*corners[i]["y"]
        for i in range(4))
    if abs(signed_twice_area) < width*height*0.0002:
        raise ValueError("Четыре угла площадки образуют слишком маленький или вырожденный контур")
    turns = []
    for i in range(4):
        a, b, c = (corners[(i+j)%4] for j in range(3))
        cross = ((b["x"]-a["x"])*(c["y"]-b["y"])
                 -(b["y"]-a["y"])*(c["x"]-b["x"]))
        turns.append(cross)
    if min(abs(t) for t in turns) < width*height*0.00002 or not (all(t>0 for t in turns) or all(t<0 for t in turns)):
        raise ValueError("Углы площадки должны образовывать выпуклый контур без пересечений")
    t = payload.get("frame_sec")
    try:
        t = float(t)
    except (TypeError, ValueError, OverflowError) as exc:
        raise ValueError("Неверная временная отметка разметки") from exc
    if not math.isfinite(t) or not 0 <= t <= info["duration_sec"]:
        raise ValueError("Время разметки вне исходного видео")
    return {
        "video_path": str(data.video_path(video)),
        "frame_sec": round(t, 3),
        "frame_index": int(round(t*30)),
        "frame_width": width, "frame_height": height,
        "keypoints": checked,
    }


def save_context(project_id: str, video: str, range_data: dict, court=None, clear_court=False):
    # Fully validate first to avoid saving range after a failed court validation.
    existing = get_project(project_id)
    assert_project_video(project_id, video)
    from .projects import validate_ranges, update_project
    combined = {**project_ranges(project_id), video: range_data}
    validated_ranges = validate_ranges(existing["videos"], combined)
    validated_court = validate_court(project_id, video, court) if court is not None else None
    if court is not None and clear_court:
        raise ValueError("Нельзя одновременно сохранить и очистить разметку")
    if validated_court is not None:
        data.save_json(court_path(project_id, video), validated_court)
    elif clear_court:
        court_path(project_id, video).unlink(missing_ok=True)
    updated = update_project(project_id, ranges=validated_ranges)
    return {"video": video, "range": updated["ranges"][video],
            "court": get_court(project_id, video),
            "project_id": project_id}


def project_context(project_id: str):
    project = get_project(project_id)
    ranges = project_ranges(project_id)
    return {"project_id": project_id, "videos": [
        {"video": name, "range": ranges[name],
         "court_saved": court_path(project_id, name).is_file()}
        for name in project["videos"]
    ]}
