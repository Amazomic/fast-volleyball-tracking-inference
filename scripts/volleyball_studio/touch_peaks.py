"""Temporal peak picking for RAVEL-013 Touch probabilities.

Matches the upstream RAVEL diagnostic algorithm (5-frame moving average,
strongest-first non-maximum suppression, 10-frame minimum separation).
Keep this CPU-only: replay must not import the GPU inference runner.
"""
from __future__ import annotations

import math
import numpy as np


def select_touch_peaks(predictions: list[dict], threshold: float,
                       smooth: int = 5, min_gap: int = 10) -> list[dict]:
    """Return one strongest player candidate per temporal Touch peak.

    The score is the smoothed probability, as in upstream find_touch_events.
    Original framewise proposals remain unchanged in the inference cache.
    """
    if smooth < 1 or min_gap < 1:
        raise ValueError("Touch smoothing and minimum gap must be positive")
    best: dict[int, dict] = {}
    for item in predictions:
        if item.get("class_name") != "player" or item.get("interpolated"):
            continue
        score = item.get("action_score")
        if score is None and isinstance(item.get("action_scores"), dict):
            score = item["action_scores"].get("Touch")
        try:
            frame, score = int(item["frame_index"]), float(score)
            box = item["bbox_xyxy"]
            if (frame < 0 or not math.isfinite(score) or not 0 < score <= 1
                    or len(box) != 4 or any(not math.isfinite(float(v)) for v in box)):
                continue
        except (KeyError, TypeError, ValueError, OverflowError):
            continue
        if score > float(best.get(frame, {}).get("action_score", -1)):
            best[frame] = dict(item, action="Touch", action_score=score)
    if not best:
        return []
    curve = np.zeros(max(best) + 1, dtype=np.float32)
    for frame, item in best.items():
        curve[frame] = item["action_score"]
    # Identical peak ranking and exclusion rules to upstream find_touch_events.
    kernel = np.ones(smooth, np.float32) / smooth
    averaged = np.convolve(curve, kernel, mode="same")
    picked: list[int] = []
    result = []
    for pos in np.argsort(-averaged, kind="stable"):
        pos = int(pos)
        value = float(averaged[pos])
        if value < threshold:
            break
        if any(abs(pos - previous) < min_gap for previous in picked):
            continue
        # Moving averages can shift a peak to a frame lacking a detection.
        # Use the strongest nearby observed player to assign the actor.
        candidates = [frame for frame in range(max(0, pos - smooth // 2),
                                                min(len(curve), pos + smooth // 2 + 1))
                      if frame in best]
        if not candidates:
            continue
        actor_frame = max(candidates, key=lambda frame: (best[frame]["action_score"],
                                                         -abs(frame - pos)))
        item = dict(best[actor_frame])
        item["frame_index"] = pos
        item["action_score"] = value
        item["raw_action_score"] = best[actor_frame]["action_score"]
        picked.append(pos)
        result.append(item)
    return sorted(result, key=lambda item: item["frame_index"])


def associate_touch_players(peaks: list[dict], tracked: list[dict]) -> list[dict]:
    """Resolve raw RAVEL action proposals to the stored player Track IDs.

    The full raw proposal cache precedes tracking, so its track_id is usually
    absent. Match boxes to actual non-interpolated tracked detections in a
    narrow frame window. Never guess an identity below the IoU threshold.
    """
    if not peaks:
        return []
    needed = {int(peak["frame_index"]) + delta for peak in peaks
              for delta in (-2, -1, 0, 1, 2)}
    nearby: dict[int, list[dict]] = {}
    for item in tracked:
        if (item.get("class_name") == "player"
                and item.get("track_id") is not None
                and not item.get("interpolated")
                and int(item["frame_index"]) in needed):
            nearby.setdefault(int(item["frame_index"]), []).append(item)

    def iou(a: list, b: list) -> float:
        x1, y1 = max(a[0], b[0]), max(a[1], b[1])
        x2, y2 = min(a[2], b[2]), min(a[3], b[3])
        overlap = max(0.0, x2 - x1) * max(0.0, y2 - y1)
        area_a = max(0.0, a[2] - a[0]) * max(0.0, a[3] - a[1])
        area_b = max(0.0, b[2] - b[0]) * max(0.0, b[3] - b[1])
        return overlap / max(1e-9, area_a + area_b - overlap)

    result = []
    for peak in peaks:
        event = dict(peak)
        # Raw network association IDs are not the stable player Track IDs.
        # Never propagate an unverified raw ID into personal statistics.
        event["track_id"] = None
        frame = int(event["frame_index"])
        options = [(iou(event["bbox_xyxy"], item["bbox_xyxy"]),
                    -abs(frame - checked), item["track_id"])
                   for checked in range(frame - 2, frame + 3)
                   for item in nearby.get(checked, [])]
        if options:
            overlap, _, track_id = max(options)
            if overlap >= 0.3:
                event["track_id"] = track_id
        result.append(event)
    return result
