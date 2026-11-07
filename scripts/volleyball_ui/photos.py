"""Lazy player portraits from saved tracker boxes and normalized analysis video.

Photos are evidence of what the tracker associated with a track_id at a frame.
They are NOT identity recognition. No inference is executed.
"""
from __future__ import annotations

import json
import math
import os
import threading
from pathlib import Path
from urllib.parse import quote

from . import data

INDEX_VERSION = 2
PHOTO_VERSION = 2
SAMPLES = 6
INDEX_LOCK = threading.Lock()
DECODE_LIMIT = threading.BoundedSemaphore(2)
_INDEX_MEMORY = {}


def validate_track_id(value) -> str:
    text = str(value)
    if not text.isascii() or not text.isdecimal() or len(text) > 12:
        raise ValueError("Некорректный track ID")
    return text


def _photo_base(cache: Path, project_id=None) -> Path:
    if project_id:
        from .projects import project_dir
        return project_dir(project_id) / "player_photos" / cache.name
    return data.RESULTS / "player_photos" / cache.name


def _index_path(cache: Path, project_id=None) -> Path:
    return _photo_base(cache, project_id) / "index.json"


def _photo_path(cache: Path, tid: str, sample: int, source_mtime: int, project_id=None) -> Path:
    return _photo_base(cache, project_id) / f"v{PHOTO_VERSION}_{source_mtime}_{tid}_{sample}.jpg"


def _analyzer_identity(cache: Path) -> dict:
    predictions = cache / "players.json"
    if not predictions.is_file():
        raise FileNotFoundError("Отсутствуют сохранённые детекции игроков")
    return {"version": INDEX_VERSION, "size": predictions.stat().st_size,
            "mtime_ns": predictions.stat().st_mtime_ns}


def _quality(item: dict) -> float:
    box = item["bbox_xyxy"]
    area = max(0, float(box[2])-float(box[0])) * max(0, float(box[3])-float(box[1]))
    # Favor high-confidence, visibly large, non-interpolated detections.
    return float(item.get("score", 0)) * min(1.0, (area / 22000) ** 0.35) * (
        0.6 if item.get("interpolated") else 1.0
    )


def _build_index(cache: Path, stamp: dict) -> dict:
    source_file = cache / "players.json"
    # Streaming keeps a large RAVEL analysis from allocating gigabytes merely
    # to prepare three appearance samples per Track ID.
    if source_file.stat().st_size > 8 * 1024 * 1024:
        try:
            import ijson
        except ImportError:
            payload = json.loads(source_file.read_text(encoding="utf-8"))
            predictions = payload.get("predictions", [])
            fps, video_source = payload.get("fps") or 30, payload.get("video") or ""
        else:
            with source_file.open("rb") as source:
                fps = next(ijson.items(source, "fps"), 30)
            with source_file.open("rb") as source:
                video_source = next(ijson.items(source, "video"), "")
            def stream_predictions():
                with source_file.open("rb") as source:
                    yield from ijson.items(source, "predictions.item", use_float=True)
            predictions = stream_predictions()
    else:
        payload = json.loads(source_file.read_text(encoding="utf-8"))
        predictions = payload.get("predictions", [])
        fps, video_source = payload.get("fps") or 30, payload.get("video") or ""
    if not isinstance(predictions, list) and not hasattr(predictions, "__next__"):
        raise ValueError("Неверный формат players.json")
    buckets = {}
    # Half-second windows retain candidates for six distinct moments on short tracks.
    bucket_frames = max(1, int(round(float(fps or 30) / 2)))
    for record in predictions:
        if record.get("class_name") != "player" or record.get("track_id") is None:
            continue
        frame = record.get("frame_index")
        box = record.get("bbox_xyxy")
        if not isinstance(frame, int) or frame < 0 or not isinstance(box, list) or len(box) != 4:
            continue
        if not all(isinstance(x, (int, float)) and math.isfinite(x) for x in box):
            continue
        if box[2] <= box[0] or box[3] <= box[1]:
            continue
        try:
            tid = validate_track_id(record["track_id"])
        except ValueError:
            continue  # Unassigned/invalid tracker IDs have no stable identity.
        # Keep the clearest detection in each half-second window.
        bucket_key = (tid, frame // bucket_frames)
        score = _quality(record)
        old = buckets.get(bucket_key)
        if old is None or score > old["quality"]:
            buckets[bucket_key] = {
                "frame": frame, "bbox": list(map(float, box)),
                "quality": round(score, 4), "confidence": round(float(record.get("score", 0)), 3),
            }
    grouped = {}
    for (tid, _), item in buckets.items():
        grouped.setdefault(tid, []).append(item)
    best = {}
    for tid, records in grouped.items():
        records.sort(key=lambda r: r["frame"])
        # Sample the full observed timespan, not six neighboring detection frames.
        # If an interval has no observation, pick its nearest still-unused sample.
        # Sparse tracks may legitimately have fewer than six distinct photos.
        start, end = records[0]["frame"], records[-1]["frame"]
        if len(records) <= SAMPLES:
            best[tid] = records
            continue
        available = records.copy()
        selected = []
        for index in range(SAMPLES):
            low = start + (end - start) * index / SAMPLES
            high = start + (end - start) * (index + 1) / SAMPLES
            target = (low + high) / 2
            in_zone = [r for r in available if low <= r["frame"] <= high]
            # Prefer temporal coverage; quality breaks ties at similar moments.
            pick = min(in_zone or available,
                       key=lambda r: (abs(r["frame"] - target), -r["quality"]))
            selected.append(pick)
            available.remove(pick)
        best[tid] = sorted(selected, key=lambda r: r["frame"])
    return {"stamp": stamp, "fps": float(fps),
            "source": str(video_source), "tracks": best}


def get_index(cache: Path, project_id=None) -> dict:
    stamp = _analyzer_identity(cache)
    index_path = _index_path(cache, project_id)
    with INDEX_LOCK:
        existing = _INDEX_MEMORY.get(str(index_path))
        if not (isinstance(existing, dict) and existing.get("stamp") == stamp):
            existing = data.read_json(index_path)
        if isinstance(existing, dict) and existing.get("stamp") == stamp:
            _INDEX_MEMORY[str(index_path)] = existing
            return existing
        index = _build_index(cache, stamp)
        data.save_json(index_path, index)
        _INDEX_MEMORY[str(index_path)] = index
        return index


def photos_metadata(video: str, track_id, project_id=None, cache_id=None) -> dict:
    data.video_path(video)
    from .analysis_runs import cache_for
    cache = cache_for(video,project_id,data.neural_cache_id(cache_id)) if cache_id else data.find_cache(video, project_id)
    if cache is None:
        raise FileNotFoundError("Видео ещё не проанализировано")
    tid = validate_track_id(track_id)
    index = get_index(cache, project_id)
    records = index["tracks"].get(tid, [])
    return {"track_id": tid, "video": video, "samples": [
        {"index": i, "frame": entry["frame"],
         "time_sec": round(entry["frame"]/index["fps"], 2),
         "confidence": entry["confidence"],
          "url": f"/api/player-photo?video={quote(video, safe='')}&id={tid}&sample={i}" + (f"&project_id={project_id}" if project_id else "") + (f"&cache_id={cache_id}" if cache_id else "")}
        for i, entry in enumerate(records)
    ]}


def photo_path(video: str, track_id, sample: int = 0, project_id=None, cache_id=None) -> Path:
    data.video_path(video)
    from .analysis_runs import cache_for
    cache = cache_for(video,project_id,data.neural_cache_id(cache_id)) if cache_id else data.find_cache(video, project_id)
    if cache is None:
        raise FileNotFoundError("Видео ещё не проанализировано")
    tid = validate_track_id(track_id)
    if not isinstance(sample, int) or not 0 <= sample < SAMPLES:
        raise ValueError("Неверный номер кадра")
    index = get_index(cache, project_id)
    samples = index["tracks"].get(tid, [])
    if sample >= len(samples):
        raise FileNotFoundError("Для этого ID нет достаточно качественных кадров")
    target = _photo_path(cache, tid, sample, index["stamp"]["mtime_ns"], project_id)
    if target.is_file() and target.stat().st_size > 0:
        return target
    source = cache / "analysis.mp4"
    if not source.is_file():
        raise FileNotFoundError("Нормализованное видео analysis.mp4 не найдено")
    with DECODE_LIMIT:
        if target.is_file() and target.stat().st_size > 0:
            return target
        # Imported inside request so the plain UI itself doesn't require OpenCV.
        import cv2
        cv2.setNumThreads(1)
        cap = cv2.VideoCapture(str(source))
        try:
            entry = samples[sample]
            cap.set(cv2.CAP_PROP_POS_FRAMES, int(entry["frame"]))
            ok, frame = cap.read()
            if not ok:
                raise RuntimeError(f"Не удалось получить кадр {entry['frame']}")
            height, width = frame.shape[:2]
            x1, y1, x2, y2 = entry["bbox"]
            box_w = x2-x1
            box_h = y2-y1
            margin_x, margin_y = box_w*0.22, box_h*0.18
            left = max(0, int(x1-margin_x))
            top = max(0, int(y1-margin_y))
            right = min(width, int(x2+margin_x))
            bottom = min(height, int(y2+margin_y))
            if right-left < 10 or bottom-top < 15:
                raise RuntimeError("Слишком маленький bbox игрока")
            crop = frame[top:bottom, left:right]
            max_width, max_height = 360, 540
            resize = min(1.0, max_width/crop.shape[1], max_height/crop.shape[0])
            if resize < 1.0:
                crop = cv2.resize(crop, None, fx=resize, fy=resize, interpolation=cv2.INTER_AREA)
            target.parent.mkdir(parents=True, exist_ok=True)
            tmp = target.with_name(target.stem + ".tmp.jpg")
            if not cv2.imwrite(str(tmp), crop, [cv2.IMWRITE_JPEG_QUALITY, 87]):
                raise RuntimeError("Не удалось сохранить фото")
            os.replace(tmp, target)
        finally:
            cap.release()
    return target
