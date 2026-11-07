"""Replay postprocessing from cached network predictions WITHOUT model inference.

Usage:
  python -m scripts.volleyball_studio.reprocess --cache <analysis-cache> --mode baseline
Modes: baseline, ravel-touch, ravel-geometry, combined.
Every output is stored in <cache>/postprocessing/<version-id>, immutable by
configuration and source hashes. Legacy analysis files remain untouched.
"""
from __future__ import annotations

import argparse
import csv
import json
import math
import os
import sys
from pathlib import Path

from .common import ROOT, acquire_lock, cached, digest, key, run, save
from .inference_cache import InferenceCacheError, load
from .touch_peaks import select_touch_peaks, associate_touch_players
from .replay_player_tracking import reconstruct_players

MODES = ("baseline", "ravel-touch", "ravel-geometry", "combined")


def _validate_mode(manifest: dict, mode: str, touch_threshold: float,
                   ball_threshold: float | None,
                   player_threshold: float | None) -> None:
    if mode not in MODES:
        raise ValueError(f"Unknown replay mode: {mode}")
    if not math.isfinite(touch_threshold) or not 0 <= touch_threshold <= 1:
        raise ValueError("Invalid touch threshold")
    if mode != "baseline" and manifest.get("player_model") != "RAVEL-VB-013-9f":
        raise InferenceCacheError("RAVEL-013 outputs required for this mode")
    has_proposals = "players.raw.v2.jsonl" in manifest["outputs"]
    if mode in ("ravel-touch", "combined") and not has_proposals:
        floor = manifest.get("player_action_threshold")
        if floor is None or touch_threshold < float(floor):
            raise InferenceCacheError(
                "Touch threshold below stored decoder cutoff; rerun RAVEL GPU inference")
    if ball_threshold is not None:
        if not math.isfinite(ball_threshold) or not 0 <= ball_threshold <= 1:
            raise ValueError("Invalid ball threshold")
        if mode in ("baseline", "combined"):
            if (ball_threshold != float(manifest["ball_threshold"]) and
                    not {"ball/analysis/ball.raw.csv", "ball/analysis/ball.raw.json"}
                    <= set(manifest["outputs"])):
                raise InferenceCacheError(
                    "VballNet sidecar raw peaks missing: run GPU inference before changing "
                    "the ball threshold. Saved peaks allow approximate replay only.")
        else:
            if (ball_threshold < float(manifest["player_ball_threshold"])
                    and not has_proposals):
                raise InferenceCacheError(
                    "RAVEL ball threshold below stored decoder cutoff; "
                    "rerun GPU inference")
    if player_threshold is not None:
        if not math.isfinite(player_threshold) or not .02 <= player_threshold <= .95:
            raise ValueError("RAVEL player threshold must be 0.02..0.95")
        if manifest.get("player_model") != "RAVEL-VB-013-9f":
            raise InferenceCacheError("RAVEL-013 outputs required for player threshold replay")
        baseline = float(manifest.get("player_score_threshold") or 0)
        raw_floor = float(manifest.get("player_candidate_floor") or .05)
        if player_threshold < raw_floor:
            raise InferenceCacheError(
                "Requested player threshold is below the retained RAVEL raw floor; "
                "a new GPU capture is required")
        if (player_threshold < baseline and
                "players.raw.v2.jsonl" not in manifest["outputs"]):
            raise InferenceCacheError(
                "Player threshold below original decoder cutoff; "
                "new RAVEL GPU inference is required")


def _ball_motion_change(observations: list[tuple[int, float, float]],
                        frame: int, window_frames: int = 4) -> dict | None:
    """Validate an action peak using an actual change in ball flight.

    The velocity fits and gravity correction intentionally match the
    geometry detector in src/player_contacts.py. Merely passing through
    a player box must not qualify as a Touch.
    """
    if os.environ.get("VOLLEY_RUST_MOTION") == "1":
        from .rust_core import rust_ball_motion_changes
        return rust_ball_motion_changes(observations, [frame], window_frames)[0]

    from src.player_contacts import (
        VELOCITY_WINDOW, MIN_VELOCITY_SAMPLES, MAX_SEGMENT_GAP_FRAMES,
        MIN_SPEED_PX_PER_FRAME, MIN_VELOCITY_RESIDUAL_PX_FRAME,
        RELATIVE_VELOCITY_RESIDUAL, DEFAULT_GRAVITY_PX_PER_FRAME2,
    )

    def fit(index: int, step: int):
        segment = []
        last_frame = observations[index][0]
        while 0 <= index < len(observations) and len(segment) < VELOCITY_WINDOW:
            item = observations[index]
            if abs(item[0] - last_frame) > MAX_SEGMENT_GAP_FRAMES:
                break
            segment.append(item)
            last_frame = item[0]
            index += step
        if (len(segment) < MIN_VELOCITY_SAMPLES
                or max(p[0] for p in segment) - min(p[0] for p in segment) < 1):
            return None
        center_t = sum(p[0] for p in segment) / len(segment)
        variance = sum((p[0] - center_t) ** 2 for p in segment)
        vx = sum((p[0] - center_t) * p[1] for p in segment) / variance
        vy = sum((p[0] - center_t) * p[2] for p in segment) / variance
        return vx, vy, center_t

    from bisect import bisect_left
    strongest = None
    start = bisect_left(observations, (frame - window_frames, -math.inf, -math.inf))
    for index in range(start, len(observations)):
        observed_frame = observations[index][0]
        if observed_frame > frame + window_frames:
            break
        before, after = fit(index, -1), fit(index, +1)
        if before is None or after is None:
            continue
        bx, by, bt = before
        ax, ay, at = after
        speed_before, speed_after = math.hypot(bx, by), math.hypot(ax, ay)
        top_speed = max(speed_before, speed_after)
        if top_speed < MIN_SPEED_PX_PER_FRAME:
            continue
        residual = math.hypot(
            ax - bx, ay - by - DEFAULT_GRAVITY_PX_PER_FRAME2 * max(at - bt, 0))
        residual_threshold = max(
            MIN_VELOCITY_RESIDUAL_PX_FRAME, RELATIVE_VELOCITY_RESIDUAL * top_speed)
        if residual < residual_threshold:
            continue
        cosine = (bx * ax + by * ay) / max(speed_before * speed_after, 1e-9)
        angle = math.degrees(math.acos(max(-1., min(1., cosine))))
        speed_delta = abs(speed_after - speed_before)
        # Gravity alone can rotate a trajectory near its apex. The corrected
        # residual must be strong AND either heading or speed must jump.
        if angle < 25. and speed_delta < max(4., .30 * top_speed):
            continue
        evidence = {
            "motion_frame": observed_frame,
            "speed_before_px_frame": round(speed_before, 3),
            "speed_after_px_frame": round(speed_after, 3),
            "direction_change_deg": round(angle, 2),
            "velocity_residual_px_frame": round(residual, 3),
        }
        if strongest is None or residual > strongest[0]:
            strongest = residual, evidence
    return strongest[1] if strongest else None


def _ravel_contacts(track: dict, predictions: list[dict], threshold: float,
                    min_interval_sec: float = .35,
                    max_reach_ratio: float = .35, *,
                    selected_peaks: list[dict] | None = None,
                    smooth: int = 5, min_gap: int = 10) -> list[dict]:
    """Require player reach AND a gravity-corrected velocity change for Touch.

    RAVEL probabilities propose event times; the ball trajectory confirms the
    event. A straight flight crossing a player box is not a contact.
    Original model probabilities are retained for raw preview/CPU replay.
    """
    from bisect import bisect_left

    observations = sorted(
        (int(row["frame"]), float(row["x"]), float(row["y"]))
        for row in track.get("frame_observations", [])
        if isinstance(row, dict) and {"frame", "x", "y"} <= row.keys()
        and all(math.isfinite(float(row[k])) for k in ("x", "y"))
    )
    if not observations:
        return []
    frames = [row[0] for row in observations]
    earliest, latest = int(track["start_frame"]), int(track["last_frame"])
    candidates = []
    peaks = (selected_peaks if selected_peaks is not None else
             select_touch_peaks(predictions, threshold, smooth, min_gap))
    # Transfer the trajectory once for all candidate model peaks, rather
    # than invoking the Rust C ABI separately for each proposed touch.
    rust_motions = None
    if os.environ.get("VOLLEY_RUST_MOTION") == "1" and peaks:
        from .rust_core import rust_ball_motion_changes
        candidate_frames = list(dict.fromkeys(
            int(peak["frame_index"]) for peak in peaks
            if peak.get("class_name") == "player" and peak.get("action") == "Touch"
            and not peak.get("interpolated")
            and float(peak.get("action_score") or 0) >= threshold
        ))
        rust_motions = dict(zip(candidate_frames,
                                rust_ball_motion_changes(observations, candidate_frames)))
    for p in peaks:
        if (p.get("class_name") != "player" or p.get("action") != "Touch"
                or p.get("interpolated")
                or float(p.get("action_score") or 0) < threshold):
            continue
        frame = int(p["frame_index"])
        if frame < earliest or frame > latest:
            continue
        box = p.get("bbox_xyxy")
        if not isinstance(box, (list, tuple)) or len(box) != 4:
            continue
        try:
            x1,y1,x2,y2 = map(float, box)
        except (TypeError, ValueError):
            continue
        height = y2-y1
        if height <= 0 or x2 <= x1:
            continue
        idx=bisect_left(frames,frame)
        neighbors = [observations[i] for i in (idx-1,idx)
                     if 0 <= i < len(observations)
                     and abs(observations[i][0]-frame)<=2]
        if not neighbors:
            continue
        _,x,y = min(neighbors,key=lambda row: abs(row[0]-frame))
        # Reject a passing flight far from the predicted hitter. Bboxes are
        # image-space pixels, so compare extra reach to player height.
        outside_x = max(x1-x, x-x2, 0.)
        outside_y = max(y1-y, y-y2, 0.)
        if max(outside_x,outside_y) > max_reach_ratio * height:
            continue
        motion = (rust_motions.get(frame) if rust_motions is not None
                  else _ball_motion_change(observations, frame))
        if motion is None:
            continue
        candidates.append({
            "frame": frame, "ball_x": x, "ball_y": y,
            "contact_type": "unknown_touch", "confidence": float(p["action_score"]),
            "player_track_id": p.get("track_id"),
            **motion,
            "side": "unknown", "origin": "ravel_touch",
        })
    candidates.sort(key=lambda item: item["frame"])
    # Consecutive per-frame positives are one action, even when multiple
    # model queries (or player IDs) claim the same ball.
    gap_frames=max(1,round(30*min_interval_sec))
    clusters=[]
    last_frame=-10**10
    for item in candidates:
        if clusters and item["frame"]-last_frame<=gap_frames:
            if item["confidence"]>clusters[-1]["confidence"]:
                clusters[-1]=item
        else:
            clusters.append(item)
        last_frame=item["frame"]
    return clusters


def _merge_contacts(geometry: list[dict], neural: list[dict],
                    min_interval_sec: float = .35) -> list[dict]:
    """Prefer geometrically supported actions over nearby model positives."""
    result = [dict(item, origin=item.get("origin", "geometry")) for item in geometry]
    gap_frames = max(3, round(30*min_interval_sec/2))
    for item in neural:
        # The model peak can be a few frames later than the physical impact.
        # Deduplicate against the actual trajectory break, not just peak time.
        motion_frame = int(item.get("motion_frame", item["frame"]))
        if not any(abs(int(old["frame"]) - motion_frame) <= gap_frames
                   for old in result):
            result.append(item)
    return sorted(result, key=lambda item: item["frame"])


def _proposal_records(cache: Path, manifest: dict, width: int, height: int,
                      touch_threshold: float, ball_threshold: float, *,
                      include_touches: bool = True,
                      include_balls: bool = True) -> tuple[list[dict], list[dict]]:
    """Stream bounded decoded proposals; no neural model is loaded.

    This format retains action probabilities and RAVEL ball maxima, but the
    initial decoder still applies a .05 player candidate floor and model NMS.
    """
    if "players.raw.v2.jsonl" not in manifest["outputs"]:
        return [], []
    touches, best_ball = [], {}
    with (cache / "players.raw.v2.jsonl").open(encoding="utf-8") as stream:
        for line in stream:
            sample = json.loads(line)
            frame = int(sample["frame"])
            if frame < 0:
                raise InferenceCacheError("Negative frame index in RAVEL proposals")
            for record in sample["predictions"]:
                if include_touches and record.get("class_name") == "player":
                    scores = record.get("action_scores") or {}
                    # Retain subthreshold Touch probabilities: the moving
                    # average must see the full signal before peak thresholding.
                    if ("Touch" in scores and math.isfinite(float(scores["Touch"]))
                            and float(scores["Touch"]) > 0):
                        box = record["bbox_xyxy_norm"]
                        touches.append({
                            "frame_index": frame, "class_name": "player",
                            "action": "Touch", "action_score": float(scores["Touch"]),
                            "score": float(record.get("score") or 0),
                            "track_id": record.get("track_id"),
                            "bbox_xyxy": [box[0]*width, box[1]*height,
                                          box[2]*width, box[3]*height],
                        })
                elif include_balls and record.get("class_name") == "ball":
                    score = float(record["score"])
                    if not score > 0.5 or score < ball_threshold or score <= best_ball.get(frame, {}).get("score", -1):
                        continue
                    box = record["bbox_xyxy_norm"]
                    best_ball[frame] = {
                        "frame_index": frame, "class_name": "ball", "score": score,
                        "bbox_xyxy": [box[0]*width, box[1]*height,
                                      box[2]*width, box[3]*height],
                    }
    return touches, list(best_ball.values())


def _player_proposals(cache: Path, width: int, height: int,
                      threshold: float) -> list[dict]:
    """Rebuild player detections from saved decoded RAVEL proposals.

    The proposal cache is after neural decoding/NMS, not full network tensors.
    Framewise deduplication is approximate and IDs may differ from the original.
    """
    result = []
    with (cache / "players.raw.v2.jsonl").open(encoding="utf-8") as stream:
        for line in stream:
            sample = json.loads(line)
            frame = int(sample["frame"])
            candidates = [p for p in sample["predictions"]
                          if p.get("class_name") == "player"
                          and float(p.get("score") or 0) >= threshold]
            candidates.sort(key=lambda p: float(p["score"]), reverse=True)
            kept = []
            for p in candidates:
                box = p["bbox_xyxy_norm"]
                if len(box) != 4 or any(not math.isfinite(float(x)) for x in box):
                    continue
                if box[2] <= box[0] or box[3] <= box[1]:
                    continue
                # Reject multiple queries representing one person.
                if any(_box_iou(box, q["bbox_xyxy_norm"]) >= .75 for q in kept):
                    continue
                kept.append(p)
                if len(kept) >= 24:
                    break
            for p in kept:
                box = p["bbox_xyxy_norm"]
                record = {
                    "frame_index": frame, "time_sec": round(frame/30., 4),
                    "class_name": "player", "score": float(p["score"]),
                    "track_id": p.get("track_id"),
                    "bbox_xyxy": [round(box[0]*width, 2), round(box[1]*height, 2),
                                  round(box[2]*width, 2), round(box[3]*height, 2)],
                }
                if p.get("action"):
                    record["action"] = p["action"]
                    record["action_score"] = p.get("action_score")
                result.append(record)
    return result


def _box_iou(a: list, b: list) -> float:
    x1, y1 = max(a[0], b[0]), max(a[1], b[1])
    x2, y2 = min(a[2], b[2]), min(a[3], b[3])
    intersection = max(0, x2-x1)*max(0, y2-y1)
    aa = max(0, a[2]-a[0])*max(0, a[3]-a[1])
    bb = max(0, b[2]-b[0])*max(0, b[3]-b[1])
    return intersection/max(1e-9, aa+bb-intersection)


def _inspect_source(cache: Path, manifest: dict) -> tuple[Path, dict, dict]:
    source = Path(manifest["source"])
    if not source.is_file():
        raise InferenceCacheError("Original source video is missing")
    player_data = json.loads((cache / "players.json").read_text(encoding="utf-8"))
    if Path(str(player_data.get("model", ""))).stem != manifest["player_model"]:
        raise InferenceCacheError("RAVEL model metadata mismatch")
    source_probe = json.loads((cache / "source.probe.json").read_text(encoding="utf-8"))
    return source, player_data, source_probe


def _link_original(cache: Path, output: Path, relative: str) -> None:
    """Hard-link, never copy the large shared proxy into a version directory."""
    source = cache / relative
    if not source.is_file():
        return
    destination = output / relative
    destination.parent.mkdir(parents=True, exist_ok=True)
    if destination.exists():
        if destination.samefile(source):
            return
        raise InferenceCacheError(f"Conflicting version file: {relative}")
    try:
        os.link(source, destination)
    except OSError as exc:
        raise InferenceCacheError(
            f"Cannot hard-link prepared input: {relative}; refusing to duplicate video"
        ) from exc


def reprocess(cache: Path, *, mode: str | None = None,
              touch_threshold: float = .5, ball_threshold: float | None = None,
              player_threshold: float | None = None,
              track_min_duration: float = .5, track_max_distance: float = 200.,
              track_player_threshold: float = .35, track_ball_threshold: float = .35,
              track_no_ball_fill: bool = False, beach: bool = False,
              touch_min_interval_sec: float = .35,
              touch_max_reach_ratio: float = .35,
              player_hold_frames: int = 90,
              player_match_distance: float = .14,
              excluded_player_ids=(),
              on_progress=None) -> Path:
    """Create/reuse a CPU-only postprocessing version; return its directory."""
    cache = Path(cache).resolve()
    excluded = {str(tid) for tid in excluded_player_ids}
    if any(not tid.isascii() or not tid.isdecimal() or len(tid) > 12
           for tid in excluded):
        raise ValueError("Invalid excluded player Track ID")
    manifest = load(cache)
    # RAVEL-013 defaults to temporal model Touch + geometry; older models
    # lack an action head and keep the original geometry-only pipeline.
    if mode is None:
        mode = ("combined" if manifest.get("player_model") == "RAVEL-VB-013-9f"
                else "baseline")
    _validate_mode(manifest, mode, touch_threshold, ball_threshold, player_threshold)
    if (isinstance(player_hold_frames, bool) or
            not isinstance(player_hold_frames, int) or
            not 0 <= player_hold_frames <= 180):
        raise ValueError("CPU player ID retention must be an integer from 0 to 180 frames")
    if (not math.isfinite(player_match_distance) or
            not .02 <= player_match_distance <= .30):
        raise ValueError("CPU player match distance must be 0.02..0.30")
    for value in (track_min_duration, track_max_distance, track_player_threshold,
                  track_ball_threshold):
        if not math.isfinite(value) or value < 0:
            raise ValueError("Invalid tracking parameter")
    if not 0 <= track_player_threshold <= 1 or not 0 <= track_ball_threshold <= 1:
        raise ValueError("Invalid tracking confidence")
    if track_min_duration == 0 or track_max_distance == 0:
        raise ValueError("Invalid tracking duration or distance")
    if (not math.isfinite(touch_min_interval_sec)
            or not .10 <= touch_min_interval_sec <= 1.5
            or not math.isfinite(touch_max_reach_ratio)
            or not .1 <= touch_max_reach_ratio <= 1.0):
        raise ValueError("Touch interval must be 0.10..1.5 s and reach 0.1..1.0")
    source, player_data, source_probe = _inspect_source(cache, manifest)
    original = cache / "analysis.mp4"
    if not original.is_file() or digest(original) != manifest["input_sha256"]:
        raise InferenceCacheError("Analysis video changed; GPU cache invalid")
    code = [ROOT / "src/track_calculator_with_court.py",
            ROOT / "scripts/volleyball_studio/player_continuity.py",
            ROOT / "scripts/volleyball_studio/replay_player_tracking.py",
            ROOT / "src/player_contacts.py",
            ROOT / "scripts/volleyball_studio/events.py",
            ROOT / "scripts/volleyball_studio/analysis.py",
            ROOT / "scripts/volleyball_studio/touch_peaks.py",
            Path(__file__)]
    options = dict(
        mode=mode, touch_threshold=touch_threshold, ball_threshold=ball_threshold,
        player_threshold=player_threshold,
        player_hold_frames=player_hold_frames,
        player_match_distance=player_match_distance,
        track_min_duration=track_min_duration,
        track_max_distance=track_max_distance,
        track_player_threshold=track_player_threshold,
        track_ball_threshold=track_ball_threshold,
        track_no_ball_fill=track_no_ball_fill, beach=beach,
        touch_min_interval_sec=touch_min_interval_sec,
        touch_max_reach_ratio=touch_max_reach_ratio)
    if excluded:
        options["excluded_player_ids"] = sorted(excluded, key=int)
    court = cache / "court.json"
    court_enabled = bool(manifest.get("court_present", court.is_file()))
    if court_enabled and not court.is_file():
        raise InferenceCacheError("Court calibration is missing; cannot replay")
    version_id = key([manifest["input_sha256"], manifest["outputs"], options,
                      [digest(path) for path in code],
                      digest(court) if court_enabled else None])
    folder = cache / "postprocessing" / version_id
    folder.mkdir(parents=True, exist_ok=True)
    lock = acquire_lock(folder)
    def report(step: int, message: str):
        if on_progress is not None:
            on_progress(step, message)
    try:
        completed = folder / "postprocess.manifest.json"
        if completed.is_file():
            old = json.loads(completed.read_text(encoding="utf-8"))
            output_hashes = old.get("files") if isinstance(old, dict) else None
            required = {"ranking.json", "analysis/events.json",
                        "analysis/game-tracks.json", "track-index.json"}
            if (old.get("version_id") == version_id
                    and isinstance(output_hashes, dict)
                    and required.issubset(output_hashes)
                    and all((folder / name).is_file()
                            and digest(folder / name) == value
                            for name, value in output_hashes.items())):
                report(5, "Готовая версия найдена в кэше")
                return folder
        report(1, "Сохранённые предсказания проверены · фильтруем обнаружения")
        width, height = int(manifest["width"]), int(manifest["height"])
        if abs(float(manifest["fps"]) - 30.) > .2:
            raise InferenceCacheError("Expected 30 FPS analysis proxy")
        player_path = cache / "players.json"
        ball_csv = cache / "ball/analysis/ball.csv"
        active_ball_csv = ball_csv
        if (ball_threshold is not None and mode in ("baseline", "combined")
                and ball_threshold != float(manifest["ball_threshold"])):
            # Peak filtering is an approximation; original heatmap contour
            # decoding cannot be recovered from this sidecar.
            active_ball_csv = folder / "ball/analysis/ball.csv"
            active_ball_csv.parent.mkdir(parents=True, exist_ok=True)
            with (cache / "ball/analysis/ball.raw.csv").open(
                    newline="", encoding="utf-8") as src, active_ball_csv.open(
                    "w", newline="", encoding="utf-8") as dst:
                writer = csv.writer(dst)
                writer.writerow(("Frame", "Visibility", "X", "Y", "Radius"))
                for row in csv.DictReader(src):
                    if float(row["Confidence"]) >= ball_threshold:
                        writer.writerow((row["Frame"], 1, row["X"],
                                         row["Y"], row["Radius"]))
        wants_touches = mode in ("ravel-touch", "combined")
        wants_ravel_ball = mode in ("ravel-touch", "ravel-geometry")
        touch_proposals, ball_proposals = (_proposal_records(
            cache, manifest, width, height, touch_threshold,
            ball_threshold if ball_threshold is not None
            else float(manifest["player_ball_threshold"] or 0),
            include_touches=wants_touches, include_balls=wants_ravel_ball)
            if wants_touches or wants_ravel_ball else ([], []))
        if excluded or player_threshold is not None or (
                mode in ("ravel-touch", "ravel-geometry") and ball_proposals):
            replay_payload = dict(player_data)
            predictions = list(player_data["predictions"])
            if player_threshold is not None:
                original_floor = float(manifest.get("player_score_threshold") or 0)
                if player_threshold < original_floor:
                    predictions = [
                        p for p in predictions if p.get("class_name") != "player"
                    ] + reconstruct_players(
                        cache, width, height, player_threshold,
                        player_data["predictions"],
                        hold_frames=player_hold_frames,
                        max_center_distance=player_match_distance)
                else:
                    predictions = [
                        p for p in predictions if p.get("class_name") != "player"
                        or float(p.get("score") or 0) >= player_threshold]
                replay_payload["score_threshold"] = player_threshold
            if mode in ("ravel-touch", "ravel-geometry") and ball_proposals:
                predictions = [
                    p for p in predictions if p.get("class_name") != "ball"
                ] + ball_proposals
            if excluded:
                predictions = [p for p in predictions
                               if p.get("class_name") != "player"
                               or str(p.get("track_id")) not in excluded]
            replay_payload["predictions"] = predictions
            player_path = folder / "players.replay.json"
            save(player_path, replay_payload)
        report(2, "Обнаружения подготовлены · строим траектории мяча")
        track_dir = folder / "tracks/analysis/tracks"
        track_dir.mkdir(parents=True, exist_ok=True)
        command = [sys.executable, ROOT / "src/track_calculator_with_court.py",
                   "--players_json_path", player_path, "--fps", "30",
                   "--video_width", str(width), "--video_height", str(height),
                   "--min_duration_sec", str(track_min_duration),
                   "--max_distance", str(track_max_distance),
                   "--player_score_threshold", str(
                       player_threshold if player_threshold is not None
                       else track_player_threshold),
                   "--ball_score_threshold", str(
                       ball_threshold if ball_threshold is not None and
                       mode in ("ravel-touch", "ravel-geometry")
                       else track_ball_threshold),
                   "--output_dir", folder / "tracks",
                   # The tracker otherwise derives its output folder from the
                   # players JSON stem when --csv_path is omitted. That is
                   # "players.replay/tracks", not "analysis/tracks": RAVEL-only
                   # tracks existed, but replay looked in the wrong directory!
                   "--tracks_dir", track_dir]
        if mode in ("baseline", "combined"):
            command += ["--csv_path", active_ball_csv]
        if mode != "combined" and track_no_ball_fill:
            command.append("--no_ball_fill")
        if court_enabled:
            command += ["--court_json_path", court]
        if beach:
            command.append("--beach")

        def track_action():
            for old in track_dir.glob("track_*.json"):
                old.unlink()
            run(command, folder / "tracks.log")
            paths = sorted(track_dir.glob("track_*.json"))
            index = folder / "track-index.json"
            print(f"CPU REPLAY {mode}: {len(paths)} ball tracks in {track_dir}",
                  flush=True)
            save(index, [str(path) for path in paths])
            return [index, *paths]
        paths = cached(folder, "tracks", version_id, track_action)
        report(3, "Траектории построены · вычисляем касания и события")

        from .analysis import (image_contact_detector, uncalibrated_contacts,
                               player_motion, score_track)
        from .events import event_timeline, contacts_for

        ball_rows = {}
        if mode in ("baseline", "combined"):
            with active_ball_csv.open(newline="", encoding="utf-8") as stream:
                for row in csv.DictReader(stream):
                    if float(row["Visibility"]) > 0:
                        ball_rows[int(row["Frame"])] = (float(row["X"]), float(row["Y"]))
        player_predictions = (json.loads(player_path.read_text(encoding="utf-8"))
                              ["predictions"] if player_path != cache / "players.json"
                              else player_data["predictions"])
        if excluded:
            player_predictions = [p for p in player_predictions
                                  if p.get("class_name") != "player"
                                  or str(p.get("track_id")) not in excluded]
        # CPU-only, independently versioned proposed continuity of player
        # tracklets. This is NOT an automatic player identity assignment.
        from .player_continuity import group_tracklets
        save(folder / "player-continuity.json", group_tracklets(
            player_predictions, width, height, float(manifest["fps"])))
        touch_predictions = (touch_proposals if "players.raw.v2.jsonl" in manifest["outputs"]
                             else player_data["predictions"])
        if excluded:
            touch_predictions = [p for p in touch_predictions
                                 if p.get("class_name") != "player"
                                 or str(p.get("track_id")) not in excluded]
        if player_threshold is not None:
            touch_predictions = [p for p in touch_predictions
                                 if p.get("class_name") != "player"
                                 or float(p.get("score") or 0) >= player_threshold]
        # Compute temporal peaks once for the whole match, before splitting into
        # ball tracks; avoid rescanning millions of predictions per rally.
        peaks = (associate_touch_players(select_touch_peaks(
            touch_predictions, touch_threshold,
            int(manifest.get("player_touch_smooth") or 5),
            int(manifest.get("player_touch_min_gap") or 10)), player_predictions)
            if mode in ("ravel-touch", "combined") else [])
        balls, players_by_frame = {}, {}
        for p in player_predictions:
            frame = int(p["frame_index"])
            if p.get("class_name") == "ball":
                if not float(p["score"]) > 0.5:
                    continue
                box = p["bbox_xyxy"]
                ball = dict(x=(box[0]+box[2])/2, y=(box[1]+box[3])/2,
                            confidence=float(p["score"]))
                if ball["confidence"] > balls.get(frame, {}).get("confidence", 0):
                    balls[frame] = ball
                if mode in ("ravel-touch", "ravel-geometry"):
                    ball_rows[frame] = (ball["x"], ball["y"])
            elif (p.get("class_name") == "player" and float(p["score"]) >=
                  (player_threshold if player_threshold is not None else .5)):
                players_by_frame.setdefault(frame, []).append(p)
        offset = float(manifest.get("analysis_offset_sec") or 0)
        legacy_events = cache / "analysis/events.json"
        if legacy_events.is_file():
            offset = float(json.loads(legacy_events.read_text(encoding="utf-8"))
                           .get("analysis_offset_sec") or 0)
        event_path = folder / "analysis/events.json"
        enriched_path = folder / "analysis/game-tracks.json"

        def events_action():
            tracks = [json.loads(path.read_text(encoding="utf-8"))
                      for path in paths[1:]]
            detector = (image_contact_detector(player_path, width, height)
                        if not court_enabled else None)
            for track in tracks:
                # Flight fragments and technical returns are not actual game
                # rallies; never turn every ball pass into a touch event.
                classification=track.get("rally_classification") or {}
                accepted=(classification.get("is_rally") is True
                          or classification.get("label") == "rally")
                if not accepted:
                    track.setdefault("player_interaction", {})["contacts"]=[]
                    continue
                if detector:
                    track["player_interaction"] = uncalibrated_contacts(track, detector)
                if mode in ("ravel-touch", "combined"):
                    neural = _ravel_contacts(track, touch_predictions, touch_threshold,
                                             touch_min_interval_sec,
                                             touch_max_reach_ratio,
                                             selected_peaks=peaks)
                    geometric = (track.get("player_interaction") or {}).get("contacts", [])
                    contacts = (neural if mode == "ravel-touch"
                                else _merge_contacts(geometric, neural,
                                                     touch_min_interval_sec))
                    track.setdefault("player_interaction", {})["contacts"] = contacts
            timeline = event_timeline(tracks, source, offset, width, height,
                                      court_enabled)
            for event in timeline["events"]:
                if (event.get("contact") or {}).get("origin") == "ravel_touch":
                    event["status"] = "model_prediction"
            save(event_path, timeline)
            save(enriched_path, tracks)
            return [event_path, enriched_path]
        cached(folder, "events", version_id, events_action)
        report(4, "Касания рассчитаны · формируем розыгрыши")

        all_tracks = json.loads(enriched_path.read_text(encoding="utf-8"))
        timeline = json.loads(event_path.read_text(encoding="utf-8"))
        candidates = []
        total_frames = int(player_data.get("benchmark", {}).get("processed_frames") or 0)
        for track in all_tracks:
            track.setdefault("player_interaction", {})["contacts"] = contacts_for(
                timeline, track)
            candidate = score_track(track, balls, width, .5, .05, court_enabled)
            if candidate is None:
                continue
            start, end = candidate["start"], candidate["end"]
            candidate.update(
                id=key([str(source), start, end, version_id]),
                source=str(source), source_content=manifest["source_identity"][-1],
                analysis_offset=offset, analysis_width=width, analysis_height=height,
                track=track, ball=balls, players=players_by_frame,
                ball_rows=ball_rows, cache=str(folder), events=str(event_path),
                source_duration=float(source_probe["format"]["duration"]))
            candidate["components"]["player_motion"] = player_motion(
                players_by_frame, start, end, width)
            candidate["safe_context_start"] = max(
                [t["last_frame"] + 1 for t in all_tracks
                 if t["last_frame"] < start], default=0)
            candidate["safe_context_end"] = min(
                [t["start_frame"] - 1 for t in all_tracks
                 if t["start_frame"] > end], default=max(0, total_frames-1))
            candidate["score"] = round(
                sum(candidate["components"].values()) * candidate["confidence"], 4)
            candidates.append({k: v for k, v in candidate.items()
                               if k not in ("ball", "players", "ball_rows", "track")})
        save(folder / "ranking.json", candidates)
        # Make each version consumable by existing preview/montage readers.
        # NTFS hardlinks share bytes with the inference cache, not disk space.
        for name in ("analysis.mp4", "source.probe.json", "players.json",
                     "players.raw.json", "players.raw.jsonl",
                     "players.raw.v2.jsonl", "inference.manifest.json",
                     "ball/analysis/ball.raw.json",
                     "ball/analysis/ball.raw.csv", "court.json"):
            _link_original(cache, folder, name)
        if mode in ("baseline", "combined"):
            if active_ball_csv == ball_csv:
                _link_original(cache, folder, "ball/analysis/ball.csv")
        else:
            # Results overlay reads the selected model's coordinate source.
            replay_csv = folder / "ball/analysis/ball.csv"
            replay_csv.parent.mkdir(parents=True, exist_ok=True)
            with replay_csv.open("w", newline="", encoding="utf-8") as stream:
                writer = csv.writer(stream)
                writer.writerow(("Frame", "Visibility", "X", "Y", "Radius"))
                for frame, (x, y) in sorted(ball_rows.items()):
                    writer.writerow((frame, 1, x, y, 0))
        files = ["ranking.json", "analysis/events.json",
                 "analysis/game-tracks.json", "track-index.json",
                 "player-continuity.json"]
        if active_ball_csv != ball_csv:
            files.append("ball/analysis/ball.csv")
        if mode in ("ravel-touch", "combined") and "players.raw.v2.jsonl" in manifest["outputs"]:
            save(folder / "players.touch-preview.json", {
                "model": str(player_data["model"]), "fps": manifest["fps"],
                "source_size": {"width": width, "height": height},
                "action_threshold": touch_threshold,
                "predictions": touch_proposals,
            })
            files.append("players.touch-preview.json")
        if player_path != cache / "players.json":
            files.append("players.replay.json")
        save(completed, {
            "schema": "volley-postprocessing/1",
            "version_id": version_id, "inference_cache": str(cache),
            "source": str(source), "settings": options,
            "reused_neural_inference": True,
            "files": {name: digest(folder / name) for name in files},
            "track_count": len(paths)-1, "ranked_count": len(candidates),
            "limitations": manifest["limitations"],
        })
        report(5, "Сохранены розыгрыши и новая версия анализа")
        return folder
    finally:
        lock.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cache", type=Path, required=True)
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
    parser.add_argument("--track-no-ball-fill", action="store_true")
    parser.add_argument("--beach", action="store_true")
    args = parser.parse_args()
    output = reprocess(
        args.cache, mode=args.mode, touch_threshold=args.touch_threshold,
        ball_threshold=args.ball_threshold, player_threshold=args.player_threshold,
        track_min_duration=args.track_min_duration,
        track_max_distance=args.track_max_distance,
        track_player_threshold=args.track_player_threshold,
        track_ball_threshold=args.track_ball_threshold,
        track_no_ball_fill=args.track_no_ball_fill, beach=args.beach,
        touch_min_interval_sec=args.touch_min_interval_sec,
        touch_max_reach_ratio=args.touch_max_reach_ratio)
    print(json.dumps({"status": "complete", "postprocess": str(output),
                      "version_id": output.name}, ensure_ascii=False))


if __name__ == "__main__":
    main()
