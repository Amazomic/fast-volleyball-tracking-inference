"""Project/video/inference-scoped appearance grouping; never enrolls real people automatically.

The saved groups are suggestions, not identity labels. A distinct manual operation
links the whole group to a verified global player via data.assign.
"""
from __future__ import annotations

import hashlib
import math
import os
import threading
from datetime import datetime, timezone
from pathlib import Path
from urllib.request import urlopen

from . import data
from .analysis_runs import cache_for
from .projects import assert_project_video, project_dir

VERSION = 1
MODEL_NAME = "person-reidentification-retail-0287"
MODEL_ROOT = Path(__file__).resolve().parents[2] / "ov"
MODEL_BASE = (
    "https://storage.openvinotoolkit.org/repositories/open_model_zoo/"
    "2022.3/models_bin/1/person-reidentification-retail-0287/FP16/"
)
_LOCK = threading.RLock()
_JOBS: dict[str, dict] = {}


def _context(project_id: str, video: str, cache_id: str | None):
    assert_project_video(project_id, video)
    canonical = data.neural_cache_id(cache_id)
    if not canonical:
        raise ValueError("Сначала выберите завершённый запуск анализа")
    cache = cache_for(video, project_id, canonical)
    if not cache:
        raise FileNotFoundError("Результаты инференса игроков не найдены")
    if cache.parent.name == "postprocessing":
        cache = cache.parent.parent
    source = cache / "players.json"
    if not source.is_file():
        raise FileNotFoundError("В этом анализе нет результатов детекции игроков RAVEL")
    key = hashlib.sha256((video + "\0" + canonical).encode("utf-8")).hexdigest()[:24]
    target = project_dir(project_id) / "player_identity_groups" / (key + ".json")
    return cache, source, target


def _stamp(source: Path) -> dict:
    stat = source.stat()
    return {"mtime_ns": stat.st_mtime_ns, "size": stat.st_size}


def _read(target: Path, source: Path):
    doc = data.read_json(target, None)
    if (isinstance(doc, dict) and doc.get("schema") == VERSION
            and doc.get("stamp") == _stamp(source)):
        return doc
    return None


def _job_key(path: Path) -> str:
    return str(path)


def _update(key: str, **values):
    with _LOCK:
        if key in _JOBS:
            _JOBS[key].update(values)


def overview(project_id: str, video: str, cache_id: str | None,
             *, auto_start: bool = True) -> dict:
    cache, source, target = _context(project_id, video, cache_id)
    with _LOCK:
        doc = _read(target, source)
        state = _JOBS.get(_job_key(target), {})
        from .player_reference_matching import reference_stamp
        settings = data.read_json(_settings_path(target), {})
        if not isinstance(settings, dict):
            settings = {}
        stale = (not doc or doc.get("similarity_schema") != SIMILARITY_SCHEMA
                 or doc.get("reference_stamp") != reference_stamp(project_id)
                 or doc.get("identity_mode", "standard") != settings.get("mode", "standard")
                 or doc.get("auto_assign_enabled", False) != settings.get("auto_assign_enabled", False)
                 or doc.get("auto_assign_threshold_percent", 97) !=
                    settings.get("auto_assign_threshold_percent", 97))
        if stale and not state.get("running") and not state.get("error") and auto_start:
            _start(project_id, video, data.neural_cache_id(cache_id), cache, source, target,
                   preserve=bool(doc))
            state = _JOBS.get(_job_key(target), {})
        return {"status": ("running" if state.get("running") else "error" if state.get("error") else "ready" if doc else "empty"),
                "progress": state.get("progress", 0),
                "stage": state.get("stage", ""),
                "error": state.get("error", ""),
                "threshold_percent": (state.get("threshold_percent")
                                      or doc.get("threshold_percent", DEFAULT_THRESHOLD) if doc
                                      else DEFAULT_THRESHOLD),
                "score_type": "cosine_similarity_not_probability",
                "embedded_count": doc.get("embedded_count", 0) if doc else 0,
                "total_tracks": doc.get("total_tracks", 0) if doc else 0,
                "candidate_people": doc.get("candidate_people") if doc else None,
                "retracking": doc.get("retracking") if doc else None,
                "reference_counts": doc.get("reference_counts", {}) if doc else {},
                "player_matches": doc.get("player_matches", {}) if doc else {},
                "nearest_tracks": doc.get("nearest_tracks", {}) if doc else {},
                "missing_visual_ids": doc.get("missing_visual_ids", []) if doc else [],
                "groups": doc.get("groups", []) if doc else [],
                "unmatched_ids": doc.get("unmatched_ids", []) if doc else [],
                "suggestions": doc.get("suggestions", []) if doc else [],
                "method": doc.get("method") if doc else None,
                "identity_mode": doc.get("identity_mode", "standard") if doc else "standard",
                "auto_assign_threshold_percent": doc.get("auto_assign_threshold_percent", 97) if doc else 97,
                "auto_assign_enabled": doc.get("auto_assign_enabled", False) if doc else False,
                "auto_links": doc.get("auto_links", {}) if doc else {},
                "generated_at": doc.get("generated_at") if doc else None}


def start(project_id: str, video: str, cache_id: str | None) -> dict:
    cache, source, target = _context(project_id, video, cache_id)
    with _LOCK:
        if _JOBS.get(_job_key(target), {}).get("running"):
            return overview(project_id, video, cache_id, auto_start=False)
        _start(project_id, video, data.neural_cache_id(cache_id), cache, source, target,
               preserve=False)
    return overview(project_id, video, cache_id, auto_start=False)


def set_threshold(project_id: str, video: str, cache_id: str, threshold_percent: int) -> dict:
    threshold = validate_threshold(threshold_percent)
    cache, source, target = _context(project_id, video, cache_id)
    with _LOCK:
        if _JOBS.get(_job_key(target), {}).get("running"):
            raise ValueError("Дождитесь завершения группировки")
        doc = _read(target, source)
        if not doc:
            raise ValueError("Сначала дождитесь первой группировки")
        if doc.get("threshold_percent", DEFAULT_THRESHOLD) == threshold and doc.get("similarity_schema") == SIMILARITY_SCHEMA:
            return overview(project_id, video, cache_id, auto_start=False)
        doc["threshold_percent"] = threshold
        data.save_json(target, doc)
        settings = data.read_json(_settings_path(target), {})
        if isinstance(settings, dict) and settings:
            settings["threshold_percent"] = threshold
            data.save_json(_settings_path(target), settings)
        _start(project_id, video, data.neural_cache_id(cache_id), cache, source, target,
               preserve=True)
    return overview(project_id, video, cache_id, auto_start=False)


def _settings_path(target: Path) -> Path:
    return target.with_suffix(".settings.json")


def configure_identity(project_id: str, video: str, cache_id: str,
                       *, mode: str = "amateur", threshold_percent: int = 88,
                       auto_assign_threshold_percent: int = 97,
                       auto_assign_enabled: bool = True,
                       session_gap_sec: int = 120) -> dict:
    """Store per-inference settings; GPU cache contents remain immutable."""
    _cache, source, target = _context(project_id, video, cache_id)
    if mode not in ("amateur", "standard"):
        raise ValueError("Неизвестный режим ReID")
    threshold = validate_threshold(threshold_percent)
    auto_threshold = validate_threshold(auto_assign_threshold_percent)
    if not isinstance(auto_assign_enabled, bool):
        raise ValueError("Автопривязка должна быть да/нет")
    if not 30 <= int(session_gap_sec) <= 600:
        raise ValueError("Интервал сессии должен быть от 30 до 600 секунд")
    settings = {"mode": mode, "threshold_percent": threshold,
                "auto_assign_threshold_percent": auto_threshold,
                "auto_assign_enabled": auto_assign_enabled,
                "session_gap_sec": int(session_gap_sec)}
    with _LOCK:
        path = _settings_path(target)
        path.parent.mkdir(parents=True, exist_ok=True)
        data.save_json(path, settings)
        doc = _read(target, source)
        if doc:
            doc["threshold_percent"] = threshold
            data.save_json(target, doc)
    return settings


def set_auto_assign(project_id: str, video: str, cache_id: str,
                    threshold_percent: int, enabled: bool) -> dict:
    """Change automatic linking policy without rerunning GPU inference."""
    if not isinstance(enabled, bool):
        raise ValueError("Укажите да/нет")
    _cache, source, target = _context(project_id, video, cache_id)
    old = data.read_json(_settings_path(target), {})
    settings = configure_identity(project_id, video, cache_id,
        mode="amateur", threshold_percent=old.get("threshold_percent", DEFAULT_THRESHOLD),
        auto_assign_threshold_percent=threshold_percent, auto_assign_enabled=enabled,
        session_gap_sec=old.get("session_gap_sec", 120))
    with _LOCK:
        if _JOBS.get(_job_key(target), {}).get("running"):
            return overview(project_id, video, cache_id, auto_start=False)
        cache, _source, _target = _context(project_id, video, cache_id)
        _start(project_id, video, data.neural_cache_id(cache_id),
               cache, source, target, preserve=True)
    return overview(project_id, video, cache_id, auto_start=False)


def _start(project_id, video, cache_id, cache, source, target, *, preserve=False):
    key = _job_key(target)
    _JOBS[key] = {"running": True, "progress": 0, "threshold_percent":
                  (_read(target, source) or {}).get("threshold_percent", DEFAULT_THRESHOLD),
                  "stage": "Подготовка признаков", "error": ""}
    thread = threading.Thread(target=_run_safe, args=(key, cache, source, target, project_id, video, cache_id, preserve),
                              daemon=True, name="player-appearance-groups")
    thread.start()


def _download_model():
    """Keep model weights out of git; fetch official Open Model Zoo FP16 if absent."""
    MODEL_ROOT.mkdir(exist_ok=True, parents=True)
    for ext in ("xml", "bin"):
        path = MODEL_ROOT / (MODEL_NAME + "." + ext)
        if path.is_file() and path.stat().st_size > 1000:
            continue
        temp = path.with_name(path.name + ".download")
        try:
            with urlopen(MODEL_BASE + MODEL_NAME + "." + ext, timeout=25) as response, temp.open("wb") as output:
                import shutil
                shutil.copyfileobj(response, output)
            if temp.stat().st_size < 1000:
                raise RuntimeError("Получены неполные веса ReID")
            os.replace(temp, path)
        finally:
            temp.unlink(missing_ok=True)
    return MODEL_ROOT / (MODEL_NAME + ".xml")


def _encoder():
    import cv2
    import numpy as np
    try:
        import openvino as ov
        model_path = _download_model()
        core = ov.Core()
        model = core.read_model(str(model_path))
        model.reshape({model.input(0): [-1, 3, 256, 128]})
        compiled = core.compile_model(model, "CPU")

        def embed(crops):
            if not crops:
                return np.zeros((0, 256), dtype=np.float32)
            batch = np.stack([cv2.resize(c, (128, 256)) for c in crops]).astype(np.float32)
            out = compiled(np.ascontiguousarray(batch.transpose(0, 3, 1, 2)))[0]
            out = np.asarray(out, dtype=np.float32).reshape(len(crops), -1)
            return out / np.maximum(np.linalg.norm(out, axis=1, keepdims=True), 1e-6)
        return "openvino_reid", embed
    except Exception:  # Model download/inference unavailable: conservative fallback only.
        # Only short, geometrically continuous tracks can be auto-grouped without a CNN.
        def embed(crops):
            vectors = []
            for crop in crops:
                hsv = cv2.cvtColor(cv2.resize(crop, (64, 128)), cv2.COLOR_BGR2HSV)
                # Shirt and shorts separately, excluding background-heavy margins.
                middle = hsv[:, 12:52]
                parts = (middle[14:67], middle[68:111])
                features = []
                for part in parts:
                    hist = cv2.calcHist([part], [0, 1], None, [18, 8], [0, 180, 0, 256]).flatten()
                    hist /= max(float(hist.sum()), 1e-6)
                    features.extend(np.sqrt(hist))
                vec = np.asarray(features, dtype=np.float32)
                vectors.append(vec / max(np.linalg.norm(vec), 1e-6))
            return np.stack(vectors) if vectors else np.zeros((0, 288), dtype=np.float32)
        return "color_fallback", embed


def _samples(cache, source, project_id, key):
    """Use the same sampled tracker bounding boxes as the photo gallery."""
    import cv2
    from .photos import get_index
    index = get_index(cache, project_id)
    tracks = index.get("tracks", {})
    requests = []
    for tid, items in tracks.items():
        if not items:
            continue
        for entry in items[:3]:
            requests.append((int(entry["frame"]), str(tid), entry["bbox"]))
    requests.sort(key=lambda item: item[0])
    video = cache / "analysis.mp4"
    if not video.is_file():
        raise FileNotFoundError("Не найдено видео analysis.mp4 для извлечения изображений")
    cap = cv2.VideoCapture(str(video))
    if not cap.isOpened():
        raise RuntimeError("Не удалось открыть видео для ReID")
    grouped = {}
    try:
        for idx, (frame, tid, bbox) in enumerate(requests):
            if idx % 25 == 0:
                _update(key, progress=round(idx / max(1, len(requests)) * 74),
                        stage=f"Извлечение кадров {idx}/{len(requests)}")
            cap.set(cv2.CAP_PROP_POS_FRAMES, frame)
            ok, image = cap.read()
            if not ok:
                continue
            h, w = image.shape[:2]
            x0, y0, x1, y1 = (float(x) for x in bbox)
            bw, bh = x1 - x0, y1 - y0
            if bw < 12 or bh < 25:
                continue
            left, right = max(0, int(x0 - bw * .06)), min(w, int(x1 + bw * .06))
            top, bottom = max(0, int(y0 - bh * .04)), min(h, int(y1 + bh * .04))
            if right - left < 12 or bottom - top < 25:
                continue
            grouped.setdefault(tid, []).append(image[top:bottom, left:right].copy())
    finally:
        cap.release()
    return grouped


def _features(crops_by_id, key):
    import numpy as np
    mode, encoder = _encoder()
    result = {}
    ids = sorted(crops_by_id, key=lambda x: int(x))
    for index in range(0, len(ids), 24):
        part = ids[index:index + 24]
        crops = [image for tid in part for image in crops_by_id[tid]]
        if not crops:
            continue
        embedded = encoder(crops)
        start = 0
        for tid in part:
            amount = len(crops_by_id[tid])
            views = embedded[start:start + amount]
            start += amount
            # Keep short/unstable IDs available for matching. Prefer the
            # medoid view and discard incompatible outlier frames rather than
            # dropping the whole ID from the appearance index.
            pairwise = views @ views.T
            representative = views[int(np.argmax(pairwise.mean(axis=1)))]
            clean = views[(views @ representative) >= (.60 if mode == "openvino_reid" else .80)]
            centroid = clean.mean(axis=0) if len(clean) else representative
            norm = float(np.linalg.norm(centroid))
            if norm < 1e-6:
                continue
            result[tid] = centroid / norm
        _update(key, progress=75 + round(17 * (index + len(part)) / max(len(ids), 1)),
                stage=f"ReID: {index + len(part)}/{len(ids)} треков")
    return mode, result


def _compatible(a, b, tracks, owners):
    if owners.get(a) and owners.get(b) and owners[a] != owners[b]:
        return False, None
    left, right = tracks[a], tracks[b]
    if left["first_frame"] <= right["last_frame"] and right["first_frame"] <= left["last_frame"]:
        return False, None
    prev, following = (left, right) if left["last_frame"] < right["first_frame"] else (right, left)
    gap = following["first_frame"] - prev["last_frame"]
    distance = math.dist(prev["end_center"], following["start_center"])
    return True, (gap, distance)


def validate_threshold(value) -> int:
    """ReID cosine similarity percentage, not calibrated identification probability."""
    try:
        if isinstance(value, bool):
            raise ValueError
        number = int(value)
        if str(number) != str(value).strip():
            raise ValueError
    except (ValueError, TypeError) as exc:
        raise ValueError("Укажите целое значение сходства от 55 до 99%") from exc
    if not 55 <= number <= 99:
        raise ValueError("Сходство должно быть от 55 до 99%")
    return number


DEFAULT_THRESHOLD = 85
SIMILARITY_SCHEMA = 5


def build_groups(features: dict, tracks: dict, owners: dict | None = None,
                 mode: str = "openvino_reid", threshold_percent: int = DEFAULT_THRESHOLD) -> dict:
    """Compare every available embedded Track ID to every other ID.

    Appearance scores are cosine similarities, not calibrated probabilities.
    Only grouping is constrained by simultaneous visibility and verified owners.
    """
    import numpy as np
    owners = owners or {}
    threshold = validate_threshold(threshold_percent) / 100
    ids = sorted((tid for tid in features if tid in tracks), key=int)
    result = {"groups": [], "unmatched_ids": sorted(tracks, key=int),
              "suggestions": [], "nearest_tracks": {str(tid): [] for tid in tracks},
              "embedded_count": len(ids), "total_tracks": len(tracks),
              "missing_visual_ids": sorted(set(tracks) - set(ids), key=int)}
    if not ids:
        return result
    vectors = np.stack([features[tid] for tid in ids]).astype(np.float32)
    vectors /= np.maximum(np.linalg.norm(vectors, axis=1, keepdims=True), 1e-6)
    similarities = np.clip(vectors @ vectors.T, -1., 1.)
    np.fill_diagonal(similarities, -1.)
    count = len(ids)
    starts = np.asarray([tracks[tid]["first_frame"] for tid in ids])
    ends = np.asarray([tracks[tid]["last_frame"] for tid in ids])
    coexisting = (starts[:, None] <= ends[None, :]) & (starts[None, :] <= ends[:, None])

    # Display nearest compatible IDs for EVERY track, including short tracks.
    # All identities participate in the matrix, without a global top-80 cutoff.
    for i, tid in enumerate(ids):
        if count == 1:
            continue
        order = np.argsort(-similarities[i])
        picked = []
        for j in order:
            other = ids[int(j)]
            compatible, _ = _compatible(tid, other, tracks, owners)
            if not compatible:
                continue
            picked.append({"track_id": other, "similarity": round(float(similarities[i, j]), 4)})
            if len(picked) == 3:
                break
        result["nearest_tracks"][tid] = picked

    # Vectorized candidate discovery covers all unique ID pairs; only pairs
    # above the user-defined threshold need costly temporal/cluster checks.
    valid = np.transpose(np.nonzero(np.triu(similarities >= threshold, k=1)))
    candidates = [(float(similarities[i, j]), int(i), int(j)) for i, j in valid]
    candidates.sort(reverse=True)
    parent = list(range(count))
    members = {i: {i} for i in range(count)}

    def find(index):
        while parent[index] != index:
            index = parent[index]
        return index

    def ambiguous(i, j, score):
        # Vectorized contradiction: another equally similar person was visible
        # at the same time as j, so i must not be assigned to j automatically.
        potential = (similarities[i] >= score - .018) & coexisting[j]
        potential[i] = False
        potential[j] = False
        return bool(np.any(potential))

    accepted = []
    for sim, i, j in candidates:
        a, b = ids[i], ids[j]
        compatible, continuity = _compatible(a, b, tracks, owners)
        if not compatible or ambiguous(i, j, sim) or ambiguous(j, i, sim):
            continue
        if mode == "color_fallback":
            gap, distance = continuity
            fps = max(float(tracks[a].get("fps", 30)), 1)
            if gap > 4 * fps or distance > .11 or sim < max(threshold, .985):
                continue
        # Very short tracklets have less reliable appearance measurements.
        if (min(tracks[a]["detected_frames"], tracks[b]["detected_frames"]) < 3
                and sim < min(.999, threshold + .07)):
            continue
        ra, rb = find(i), find(j)
        if ra == rb:
            continue
        # Complete-link prevents indirect A-B-C bridging and respects manual labels.
        if any(not _compatible(ids[x], ids[y], tracks, owners)[0]
               or similarities[x, y] < threshold
               for x in members[ra] for y in members[rb]):
            continue
        parent[rb] = ra
        members[ra].update(members.pop(rb))
        accepted.append((sim, a, b))
    for values in members.values():
        if len(values) < 2:
            continue
        ordered_indices = sorted(values, key=lambda i: int(ids[i]))
        ordered = [ids[i] for i in ordered_indices]
        minimum = min(float(similarities[a, b])
                      for offset, a in enumerate(ordered_indices)
                      for b in ordered_indices[offset+1:])
        result["groups"].append({"id": "group-" + ordered[0], "track_ids": ordered,
                                 "similarity": round(minimum, 4), "source": "automatic"})
    result["groups"].sort(key=lambda g: (-len(g["track_ids"]), int(g["track_ids"][0])))
    used = {tid for group in result["groups"] for tid in group["track_ids"]}
    result["unmatched_ids"] = sorted(set(tracks) - used, key=int)
    result["suggestions"] = [{"a": a, "b": b, "similarity": round(sim, 4)}
                             for sim, a, b in accepted[:80]]
    return result


def preserve_manual_groups(computed: dict, old: dict) -> dict:
    """Threshold changes never silently undo manual mergers or splits."""
    pinned = [dict(g, track_ids=list(g["track_ids"]))
              for g in old.get("groups", [])
              if g.get("source") == "manual" and len(g.get("track_ids", [])) >= 2]
    forbidden = set(map(str, old.get("manual_excluded_ids", [])))
    pinned_ids = {str(tid) for group in pinned for tid in group["track_ids"]}
    unavailable = forbidden | pinned_ids
    groups = []
    for group in computed["groups"]:
        remaining = [tid for tid in group["track_ids"] if tid not in unavailable]
        if len(remaining) >= 2:
            groups.append(dict(group, track_ids=remaining, id="group-" + remaining[0]))
    groups.extend(pinned)
    used = {tid for group in groups for tid in group["track_ids"]}
    computed["groups"] = sorted(groups, key=lambda g: -len(g["track_ids"]))
    computed["unmatched_ids"] = sorted((set(computed["unmatched_ids"]) | unavailable |
                                        {tid for g in computed["groups"] for tid in g["track_ids"]}) - used, key=int)
    return computed


def _embedding_fingerprint(source):
    model_files = (MODEL_ROOT / (MODEL_NAME + ".xml"), MODEL_ROOT / (MODEL_NAME + ".bin"))
    return hashlib.sha256(repr((VERSION, "samples-v3-medoid-all-tracks", _stamp(source),
        [(p.stat().st_mtime_ns, p.stat().st_size) if p.is_file() else None
         for p in model_files])).encode("utf-8")).hexdigest()


def _read_embeddings(source, target):
    import numpy as np
    path = target.with_suffix(".npz")
    if not path.is_file():
        return None
    try:
        with np.load(path, allow_pickle=False) as packed:
            if str(packed["fingerprint"].item()) != _embedding_fingerprint(source):
                return None
            ids = [str(tid) for tid in packed["ids"].tolist()]
            vectors = packed["vectors"].astype(np.float32)
            mode = str(packed["mode"].item())
            if len(ids) != len(vectors):
                return None
            return mode, dict(zip(ids, vectors))
    except (ValueError, KeyError, OSError):
        return None


def _save_embeddings(source, target, mode, features):
    import numpy as np
    target.parent.mkdir(parents=True, exist_ok=True)
    path = target.with_suffix(".npz")
    temp = path.with_suffix(".tmp.npz")
    ids = sorted(features, key=int)
    vectors = (np.stack([features[tid] for tid in ids]).astype(np.float16)
               if ids else np.empty((0, 0), np.float16))
    try:
        np.savez_compressed(temp, ids=np.asarray(ids), vectors=vectors,
                            mode=np.asarray(mode), fingerprint=np.asarray(_embedding_fingerprint(source)))
        os.replace(temp, path)
    finally:
        temp.unlink(missing_ok=True)


def _run_safe(key, cache, source, target, project_id, video, cache_id, preserve=False):
    try:
        from .player_tracks import tracks_for
        from .photos import get_index
        from .player_reference_matching import match_players, reference_stamp
        initial_stamp = _stamp(source)
        previous = _read(target, source) or {}
        settings = data.read_json(_settings_path(target), {})
        if not isinstance(settings, dict):
            settings = {}
        identity_mode = settings.get("mode", "standard")
        threshold = validate_threshold(previous.get(
            "threshold_percent", settings.get("threshold_percent", DEFAULT_THRESHOLD)))
        auto_threshold = validate_threshold(settings.get("auto_assign_threshold_percent", 97))
        auto_enabled = settings.get("auto_assign_enabled", False) is True
        _update(key, threshold_percent=threshold, stage="Чтение ID из результатов RAVEL", progress=2)
        stats = tracks_for(video, project_id, cache_id)["tracks"]
        cached_features = _read_embeddings(source, target)
        if cached_features:
            _update(key, progress=93, stage="Повторное использование ReID-признаков")
            mode, embeddings = cached_features
        else:
            get_index(cache, project_id)
            crops = _samples(cache, source, project_id, key)
            mode, embeddings = _features(crops, key)
            _save_embeddings(source, target, mode, embeddings)
        assignments = data.groups_for(video, project_id, cache_id)
        occupied = {str(tid): group["id"] for group in assignments
                    for tid in group.get("track_ids", [])}
        # Only reviewed manual links are hard identity constraints. Auto links
        # are provisional and may be corrected by a later ReID recomputation.
        owners = {str(tid): group["id"] for group in assignments
                  for tid in group.get("track_ids", [])
                  if group.get("link_meta", {}).get(str(tid), {}).get("reviewed", True)}
        _update(key, stage="Сравнение всех Track ID", progress=94)
        computed = build_groups(embeddings, stats, owners, mode, threshold)
        if mode == "openvino_reid":
            from .player_retracking import propose_retracked_groups
            tuning = propose_retracked_groups(cache, embeddings,
                                              threshold_percent=threshold)
            if tuning and tuning["matched_source_ids"] >= max(3, int(tuning["source_track_ids"] * .7)):
                safe = []
                for group in tuning["groups"]:
                    confirmed = {owners[tid] for tid in group["track_ids"] if tid in owners}
                    if len(confirmed) <= 1:
                        safe.append(group)
                if identity_mode == "amateur":
                    # Never replace appearance evidence with geometry-only clusters.
                    from .player_amateur_match import stitch_rallies
                    secure = [g for g in safe
                        if g.get("similarity") is not None
                        and float(g["similarity"]) >= threshold / 100]
                    computed.update(stitch_rallies(embeddings, stats,
                        [*computed["groups"], *secure], owners,
                        threshold_percent=threshold,
                        session_gap_sec=int(settings.get("session_gap_sec", 120))))
                else:
                    consumed = {tid for group in safe for tid in group["track_ids"]}
                    computed["groups"] = safe
                    computed["unmatched_ids"] = sorted(set(stats)-consumed, key=int)
                computed["retracking"] = {
                    "profile": "60 frames · center 0.15 · OpenVINO ReID",
                    "settings": tuning["settings"],
                    "original_track_ids": tuning["source_track_ids"],
                    "retracked_ids": tuning["retracked_ids"],
                    "matched_source_ids": tuning["matched_source_ids"],
                    "appearance_links": tuning["appearance_links"],
                    "unreliable_track_ids": tuning["unreliable_track_ids"],
                    "confirmed": False}
        if identity_mode == "amateur" and mode == "openvino_reid" and "session_boundaries" not in computed:
            from .player_amateur_match import stitch_rallies
            computed.update(stitch_rallies(embeddings, stats, computed["groups"],
                owners, threshold_percent=threshold,
                session_gap_sec=int(settings.get("session_gap_sec", 120))))
        if identity_mode == "amateur":
            used = {tid for group in computed["groups"] for tid in group["track_ids"]}
            computed["candidate_people"] = len(computed["groups"]) + len(set(stats)-used)
        if preserve and previous:
            computed = preserve_manual_groups(computed, previous)
            if "retracking" in computed:
                ignored = computed["retracking"]["unreliable_track_ids"]
                computed["candidate_people"] = (
                    len(computed["groups"]) +
                    sum(tid not in ignored for tid in computed["unmatched_ids"]))
        _update(key, stage="Поиск игроков по подтверждённым образцам", progress=96)
        encoder_box = [None]
        def reusable_encoder():
            if encoder_box[0] is None:
                encoder_box[0] = _encoder()
            return encoder_box[0]
        player_matches, counts = match_players(
            embeddings, project_id, video, cache_id, mode, reusable_encoder,
            threshold_percent=threshold)
        if _stamp(source) != initial_stamp:
            raise ValueError("Инференс изменился во время группировки")
        auto_links = {}
        if identity_mode == "amateur" and auto_enabled and mode == "openvino_reid":
            from .player_amateur_match import auto_assignment_proposals
            # The existing assignment map includes auto links so no row is
            # silently relabelled; automatic links are never trusted references.
            proposed = auto_assignment_proposals(player_matches,
                computed["groups"], stats, occupied, threshold_percent=auto_threshold)
            storage_key = data._identity_storage_key(video, cache_id)
            rejected = set(data._identities(project_id).get(
                "auto_rejected", {}).get(storage_key, []))
            for pid, tids in proposed.items():
                fresh = [tid for tid in tids if tid not in occupied and tid not in rejected]
                for start in range(0, len(fresh), 200):
                    batch = fresh[start:start+200]
                    if batch:
                        data.assign(video, batch, project_id=project_id,
                            player_id=pid, cache_id=cache_id,
                            source="reid_auto", reviewed=False)
                        auto_links.setdefault(pid, []).extend(batch)
        doc = {
            "schema": VERSION, "similarity_schema": SIMILARITY_SCHEMA,
            "stamp": initial_stamp, "method": mode,
            "threshold_percent": threshold,
            "manual_excluded_ids": previous.get("manual_excluded_ids", []) if preserve else [],
            "reference_stamp": reference_stamp(project_id),
            "reference_counts": counts, "player_matches": player_matches,
            "identity_mode": identity_mode,
            "auto_assign_enabled": auto_enabled,
            "auto_assign_threshold_percent": auto_threshold,
            "auto_links": auto_links,
            "generated_at": datetime.now(timezone.utc).isoformat(),
            **computed,
        }
        target.parent.mkdir(parents=True, exist_ok=True)
        if identity_mode == "amateur" and mode == "openvino_reid":
            from .data import save_json
            mapping = {str(tid): group["id"] for group in computed["groups"]
                       for tid in group["track_ids"]}
            # Immutable neural detections + separately replaceable CPU sidecar.
            save_json(cache / "player-continuity.json", {
                "schema": 2, "source": "amateur_reid",
                "threshold_percent": threshold,
                "track_to_candidate": mapping,
                "session_boundaries": computed.get("session_boundaries", [])})
        data.save_json(target, doc)
        _update(key, running=False, progress=100, stage="Готово", error="")
    except Exception as exc:
        _update(key, running=False, error=str(exc), stage="Ошибка")
        import traceback
        traceback.print_exc()


def modify(project_id, video, cache_id, action, ids):
    if not isinstance(ids, list) or not 1 <= len(ids) <= 5000:
        raise ValueError("Выберите ID для изменения групп")
    ids = list(dict.fromkeys(str(x) for x in ids))
    if any(not x.isascii() or not x.isdecimal() or len(x) > 12 for x in ids):
        raise ValueError("Некорректные Track ID")
    cache, source, target = _context(project_id, video, cache_id)
    with _LOCK:
        if _JOBS.get(_job_key(target), {}).get("running"):
            raise ValueError("Дождитесь завершения текущей группировки")
        doc = _read(target, source)
        if not doc:
            raise ValueError("Дождитесь завершения группировки")
        groups = doc["groups"]
        all_ids = {tid for group in groups for tid in group["track_ids"]} | set(doc["unmatched_ids"])
        if any(tid not in all_ids for tid in ids):
            raise ValueError("ID отсутствует в выбранном анализе")
        if action == "split":
            doc["manual_excluded_ids"] = sorted(
                set(doc.get("manual_excluded_ids", [])) | set(ids), key=int)
            for group in groups:
                group["track_ids"] = [tid for tid in group["track_ids"] if tid not in ids]
                if len(group["track_ids"]) >= 2:
                    group["source"] = "manual"
            groups[:] = [group for group in groups if len(group["track_ids"]) >= 2]
        elif action == "merge" and len(ids) >= 2:
            doc["manual_excluded_ids"] = [
                tid for tid in doc.get("manual_excluded_ids", []) if tid not in ids]
            selected = set(ids)
            # Never silently drag unselected members of another cluster into an edit.
            for group in groups:
                group["track_ids"] = [tid for tid in group["track_ids"] if tid not in selected]
            groups[:] = [group for group in groups if len(group["track_ids"]) >= 2]
            groups.append({"id": "manual-" + hashlib.sha256(",".join(sorted(ids, key=int)).encode()).hexdigest()[:12],
                           "track_ids": sorted(selected, key=int),
                           "similarity": None, "source": "manual"})
        else:
            raise ValueError("Неизвестное действие с группами")
        used = {tid for g in groups for tid in g["track_ids"]}
        doc["unmatched_ids"] = sorted(all_ids - used, key=int)
        data.save_json(target, doc)
    return overview(project_id, video, cache_id, auto_start=False)


def link_group(project_id, video, cache_id, group_id, player_id):
    _cache, source, target = _context(project_id, video, cache_id)
    with _LOCK:
        if _JOBS.get(_job_key(target), {}).get("running"):
            raise ValueError("Дождитесь завершения текущей группировки")
        doc = _read(target, source)
        if not doc:
            raise ValueError("Группировка ещё не готова")
        group = next((g for g in doc["groups"] if g["id"] == group_id), None)
        if not group:
            raise ValueError("Группа не найдена")
        ids = group["track_ids"]
        for index in range(0, len(ids), 200):
            data.assign(video, ids[index:index + 200], project_id=project_id,
                        player_id=player_id, cache_id=cache_id)
    return {"linked": len(ids), "ids": ids, "group_id": group_id}
