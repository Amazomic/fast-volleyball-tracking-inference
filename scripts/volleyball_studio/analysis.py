import csv
import json
import math
import shutil
from pathlib import Path
import sys
import time

import numpy as np

from .common import ANALYSIS_INPUT_VERSION, ROOT, cached, digest, identity, key, probe, run, save
from .events import contacts_for, event_timeline
from .ball_models import DEFAULT_BALL_MODEL, resolve_ball_model


def image_contact_detector(player_path, width, height):
    sys.path.insert(0, str(ROOT / "src"))
    from player_contacts import ContactDetector, PlayerStore

    class ImageContacts(ContactDetector):
        def _side_of(self, box, court_x):
            return "unknown"

        def _ball_above_net(self, sample, radius):
            return False

    store = PlayerStore(str(player_path), source_width=width, source_height=height)
    return ImageContacts(store, None, 30)


def uncalibrated_contacts(track, detector):
    from player_contacts import BallSample
    observations = track.get("frame_observations", [])
    samples = [BallSample(frame=p["frame"], x=p["x"], y=p["y"], radius_px=p.get("radius_med6_px") or p.get("radius_px")) for p in observations]
    contacts = [choice.best.to_dict() for choice in detector._detect_choices(samples)]
    return {"enabled": True, "method": "image_space_without_court", "contacts": contacts}


def analyze(source, work, args):
    from .advanced import from_namespace
    from .analysis_stages import StageReporter
    stages = StageReporter()
    advanced = from_namespace(args)
    model, openvino_model = resolve_ball_model(getattr(args, "ball_model", DEFAULT_BALL_MODEL))
    player_format = "onnx" if advanced["player_backend"].startswith("onnx") else "xml"
    players_model = ROOT / ("models" if player_format=="onnx" else "ov") / (advanced["player_model"] + "." + player_format)
    if not players_model.is_file():
        raise FileNotFoundError(f"Модель игроков не найдена: {players_model}")
    # Identity policy runs after expensive neural inference. Never invalidate
    # the GPU cache merely because ReID/auto-link thresholds changed.
    identity_keys = {"player_identity_mode", "player_reid_threshold_percent",
                     "player_auto_assign", "player_auto_assign_threshold_percent",
                     "player_session_gap_sec"}
    player_cfg = {k:v for k,v in advanced.items()
                  if k.startswith("player_")
                  and k not in identity_keys | {"player_output_video"}}
    track_cfg = {k:v for k,v in advanced.items() if k.startswith("track_")}
    court_in_results = ROOT / "results" / (source.stem + "_court.json")
    if getattr(args, "court_json", None):
        court = args.court_json.resolve()
        if not court.is_file():
            raise ValueError("Разметка площадки для этого видео не найдена")
    elif getattr(args, "no_court_fallback", False):
        court = work / "__no_saved_court__.json"
    else:
        court = court_in_results if court_in_results.is_file() else source.with_name(source.stem + "_court.json")
    from .video_inputs import cached_source_identity
    source_id = cached_source_identity(source, getattr(args, "prepared_video", None))
    # Inference cache keys depend ONLY on neural decoding code. Touch
    # heuristics, CPU tracking, video montage and rally classifiers must not
    # invalidate an expensive inference artifact on code changes.
    neural_sources = (
        "inference_onnx_seq_gray_v2.py",
        "inference_openvino_seq_gray_v2.py",
        "inference_player_ball_openvino.py",
        "inference_player_ball_onnx.py",
        "predictions_io.py", "constants.py", "models.py",
    )
    code = {name: digest(ROOT / "src" / name)
            for name in neural_sources if (ROOT / "src" / name).is_file()}
    signature_parts = ["neural-source-scope-v2", ANALYSIS_INPUT_VERSION,
                       source_id, args.analysis_seconds,
                       args.analysis_start, args.analysis_width, model.name, code,
                       player_cfg, track_cfg if not getattr(args, "inference_only", False) else None]
    # Keep existing successful cache keys unchanged. Only a retry after an
    # interrupted run receives a new isolated content address.
    if getattr(args, "cache_salt", ""):
        signature_parts.append(args.cache_salt)
    signature = key(signature_parts)
    folder = work / signature
    folder.mkdir(parents=True, exist_ok=True)
    source_probe = probe(source)
    save(folder / "source.probe.json", source_probe)
    proxy = folder / "analysis.mp4"
    prepared = getattr(args, "prepared_video", None)
    if prepared is not None:
        prepared = prepared.resolve()
        if not prepared.is_file():
            raise ValueError("Подготовленный файл отсутствует")
        # Original source is retained for signatures, player/court identities.
        # Proxy is only the decoded input to subsequent neural networks.
    from .video_inputs import prepare_proxy
    stages.begin(1)
    prepare_proxy(
        source=source, prepared=prepared, proxy=proxy,
        start=args.analysis_start, seconds=args.analysis_seconds,
        width=args.analysis_width, threads=args.threads,
        prepared_input=getattr(args, "prepared_input", False),
    )
    stages.end()
    inference_input = proxy
    if getattr(args, "scratch_dir", None):
        scratch = args.scratch_dir.resolve() / signature
        scratch.mkdir(parents=True, exist_ok=True)
        inference_input = scratch / "analysis.mp4"
        cached(scratch, "proxy", key([digest(proxy)]), lambda: (shutil.copyfile(proxy, inference_input), [inference_input])[1])
    video = next(s for s in probe(proxy)["streams"] if s["codec_type"] == "video")
    width, height = video["width"], video["height"]
    csv_path = folder / "ball/analysis/ball.csv"
    gpu_config = (getattr(args, "gpu_device_id", 0),
                  getattr(args, "gpu_memory_limit_mb", 0))
    ball_key = key(["raw-ball-candidates-v2", signature, identity(model), identity(openvino_model), identity(openvino_model.with_suffix(".bin")) if openvino_model.suffix == ".xml" else None, args.backend, args.threads if args.backend in ("onnx-cpu","openvino") else None, gpu_config if args.backend=="onnx" else None, args.ball_threshold, advanced["ball_output_video"]])

    print(f"BALL DETECTOR: model={model.name}; backend={args.backend}; gpu_device_id={gpu_config[0]}; gpu_memory_limit_mb={gpu_config[1]}; CPU threads={args.threads}; threshold={args.ball_threshold}", flush=True)
    def ball():
        csv_path.parent.mkdir(parents=True, exist_ok=True)
        if csv_path.exists():
            csv_path.unlink()
        script = "inference_openvino_seq_gray_v2.py" if args.backend == "openvino" else "inference_onnx_seq_gray_v2.py"
        command = [sys.executable, ROOT / "src" / script, "--video_path", inference_input, "--output_dir", folder / "ball"]
        if not advanced["ball_output_video"]:
            command.append("--only_csv")
        command += (["--model_xml", openvino_model,
                     "--device", "CPU", "--num-threads", str(args.threads),
                     "--threshold", str(args.ball_threshold)] if args.backend == "openvino" else
                    ["--model_path", model,
                     "--device", "cpu" if args.backend=="onnx-cpu" else "cuda",
                     "--num-threads", str(args.threads),
                     "--gpu-device-id", str(gpu_config[0]),
                     "--gpu-memory-limit-mb", str(gpu_config[1]),
                     "--confidence_threshold", str(args.ball_threshold)])
        run(command, folder / "ball.log")
        if args.backend == "openvino":
            shutil.copyfile(folder / "ball/analysis_predict_ball.csv", csv_path)
        raw_path = csv_path.with_name("ball.raw.csv")
        raw_meta = csv_path.with_name("ball.raw.json")
        if args.backend != "openvino" and (not raw_path.is_file() or not raw_meta.is_file()):
            raise RuntimeError("VballNet не сохранил сырые оценки мяча")
        return [csv_path, raw_path, raw_meta] if args.backend != "openvino" else [csv_path]

    stages.begin(2)
    cached(folder, "ball", ball_key, ball)
    stages.end()
    player_path = folder / "players.json"
    player_key = key(["raw-ravel-players-v2", signature, identity(players_model), identity(players_model.with_suffix(".bin")) if players_model.suffix==".xml" else None, identity(players_model.with_suffix(".json")), player_cfg, advanced["player_output_video"], args.threads, gpu_config if advanced["player_backend"]=="onnx-cuda" else None])

    print(f"PLAYER DETECTOR: model={players_model.name}, backend={advanced['player_backend']}, device={advanced['player_device']}, gpu_device_id={gpu_config[0]}, gpu_memory_limit_mb={gpu_config[1]}, threads={args.threads}, frame_step={advanced['player_frame_step']}", flush=True)
    def players():
        cmd = [sys.executable, ROOT / "src/inference_player_ball_openvino.py",
               inference_input, "--model", players_model, "--output", player_path,
               "--backend", advanced["player_backend"],
               "--device", advanced["player_device"], "--num-threads", str(args.threads),
               "--gpu-device-id", str(gpu_config[0]),
               "--gpu-memory-limit-mb", str(gpu_config[1]),
               "--score-threshold", str(advanced["player_score_threshold"]),
               "--close-threshold", str(advanced["player_close_threshold"]),
               "--hysteresis-frames", str(advanced["player_hysteresis_frames"]),
               "--match-center-distance", str(advanced["player_match_center_distance"]),
               "--match-iou", str(advanced["player_match_iou"]),
               "--ball-threshold", str(advanced["player_ball_threshold"]),
               "--head-threshold", str(advanced["player_head_threshold"]),
               "--action-threshold", str(advanced["player_action_threshold"]),
               "--nms-iou", str(advanced["player_nms_iou"]),
               "--stride", str(advanced["player_stride"]),
               "--frame-step", str(advanced["player_frame_step"]),
               "--warmup-runs", str(advanced["player_warmup_runs"]),
               "--performance-hint", advanced["player_performance_hint"],
               "--touch-smooth", str(advanced["player_touch_smooth"]),
               "--touch-min-gap", str(advanced["player_touch_min_gap"]),
               "--touch-hold", str(advanced["player_touch_hold"]),
               "--strip-seconds", str(advanced["player_strip_seconds"]),
               "--reader", advanced["player_reader"],
               "--decode-hwaccel", advanced["player_decode_hwaccel"],
               "--decode-threads", str(advanced["player_decode_threads"])]
        cmd.append("--save-raw-players")
        if advanced["player_include_features"]:
            cmd.append("--include-features")
        annotated = folder / "players-annotated.mp4"
        if advanced["player_output_video"]:
            cmd += ["--output-video", annotated]
        run(cmd, folder / "players.log")
        raw_path = player_path.with_name("players.raw.jsonl")
        raw_meta = player_path.with_name("players.raw.json")
        raw_v2 = player_path.with_name("players.raw.v2.jsonl")
        if not raw_path.is_file() or not raw_meta.is_file() or not raw_v2.is_file():
            raise RuntimeError("RAVEL не сохранил полный кэш кандидатов для постобработки")
        result = [player_path, raw_path, raw_v2, raw_meta]
        return result + [annotated] if advanced["player_output_video"] else result

    stages.begin(3)
    cached(folder, "players", player_key, players)
    from .inference_cache import publish
    publish(folder, source=source, source_id=source_id, model=model.name,
            ball_threshold=args.ball_threshold, backend=args.backend,
            analysis_offset_sec=args.analysis_start,
            court_present=court.is_file(),
            player_touch_smooth=advanced["player_touch_smooth"],
            player_touch_min_gap=advanced["player_touch_min_gap"])
    stages.end()
    if getattr(args, "inference_only", False):
        if court.is_file():
            annotation = json.loads(court.read_text(encoding="utf-8"))
            old_w = annotation.get("frame_width", annotation.get("image_width"))
            old_h = annotation.get("frame_height", annotation.get("image_height"))
            if not old_w or not old_h:
                raise ValueError("Разметка площадки должна содержать размеры изображения")
            for point in annotation["keypoints"]:
                if point.get("x") is not None:
                    point["x"] *= width / old_w
                if point.get("y") is not None:
                    point["y"] *= height / old_h
            annotation.update(frame_width=width, frame_height=height,
                              image_width=width, image_height=height)
            save(folder / "court.json", annotation)
        print(json.dumps({"status": "inference_ready", "cache": str(folder),
                          "manifest": str(folder / "inference.manifest.json")},
                         ensure_ascii=False), flush=True)
        return [], [], folder
    track_key = key([ball_key, player_key, identity(court) if court.exists() else None, track_cfg, args.beach])

    def tracks():
        track_dir = folder / "tracks/analysis/tracks"
        if track_dir.exists():
            for old in track_dir.glob("track_*.json"):
                old.unlink()
        command = [sys.executable, ROOT / "src/track_calculator_with_court.py", "--csv_path", csv_path, "--players_json_path", player_path, "--fps", "30", "--video_width", str(width), "--video_height", str(height), "--min_duration_sec", str(advanced["track_min_duration"]),
                   "--max_distance", str(advanced["track_max_distance"]),
                   "--player_score_threshold", str(advanced["track_player_threshold"]),
                   "--ball_score_threshold", str(advanced["track_ball_threshold"]),
                   "--output_dir", folder / "tracks"]
        if advanced["track_no_ball_fill"]:
            command += ["--no_ball_fill"]
        if court.exists():
            annotation = json.loads(court.read_text(encoding="utf-8"))
            old_w = annotation.get("frame_width", annotation.get("image_width"))
            old_h = annotation.get("frame_height", annotation.get("image_height"))
            if not old_w or not old_h:
                raise ValueError("Разметка площадки должна содержать размеры изображения")
            for point in annotation["keypoints"]:
                if point.get("x") is not None:
                    point["x"] *= width / old_w
                if point.get("y") is not None:
                    point["y"] *= height / old_h
            annotation.update(frame_width=width, frame_height=height, image_width=width, image_height=height)
            scaled_court = folder / "court.json"
            save(scaled_court, annotation)
            sys.path.insert(0, str(ROOT / "src"))
            from court_transformer import CourtTransformer
            calibrated = CourtTransformer(str(scaled_court)).load()
            known_corners = all(
                point.get("x") is not None and point.get("y") is not None and
                point.get("status", "visible" if point.get("visible") else "unknown") != "unknown"
                for point in annotation["keypoints"][:4]
            )
            if not known_corners:
                print("COURT: unknown corner(s); no perspective transform used", flush=True)
            elif calibrated.matrix is None:
                raise ValueError("Некорректная геометрия площадки; координаты не будут выдуманы")
            else:
                command += ["--court_json_path", scaled_court]
        if args.beach:
            command += ["--beach"]
        run(command, folder / "tracks.log")
        result = sorted(track_dir.glob("track_*.json"))
        index = folder / "track-index.json"
        save(index, [str(p) for p in result])
        return [index] + result

    stages.begin(4)
    paths = cached(folder, "tracks", track_key, tracks)
    stages.end()

    # These are explicit exports. No GUI windows are launched in the HTTP worker.
    track_dir = folder / "tracks/analysis/tracks"
    exports = folder / "exports"
    export_key = key([track_key, {k:v for k,v in advanced.items()
                                if k.startswith(("tracks_", "show_rally_"))}])
    def diagnostic_exports():
        exports.mkdir(parents=True, exist_ok=True)
        generated = []
        mode = advanced["tracks_export_mode"]
        base = [sys.executable, ROOT / "src/track_processor.py", "--video_path", proxy,
                "--json_dir", track_dir, "--fps", "30",
                "--padding", str(advanced["tracks_padding"])]
        if advanced["tracks_include_not_rally"]:
            base += ["--include-not-rally"]
        if advanced["tracks_debug"]:
            base += ["--debug"]
        if not advanced["tracks_mark_ball"]:
            base += ["--no-mark"]
        if mode in ("combined", "both"):
            combined = exports / "rallies-combined.mp4"
            run([*base, "--output_path", combined], exports / "combined.log")
            generated.append(combined)
        if mode in ("split", "both"):
            clips = exports / "rallies-split"
            run([*base, "--split_dir", clips], exports / "split.log")
            generated += sorted(clips.glob("*.mp4"))
        if advanced["show_rally_snapshot"]:
            if not paths[1:]:
                print("SHOW_RALLY: no track available for snapshot", flush=True)
            else:
                snapshot = exports / "track-snapshot.png"
                cmd = [sys.executable, ROOT / "src/show_rally.py",
                       track_dir, proxy, "--players_json_path", player_path,
                       "--fps", "30", "--track", str(advanced["show_rally_track"]),
                       "--snapshot", snapshot]
                court_file = folder / "court.json"
                if court_file.is_file():
                    cmd += ["--court_json_path", court_file]
                run(cmd, exports / "snapshot.log")
                generated.append(snapshot)
        return generated
    if advanced["tracks_export_mode"] != "none" or advanced["show_rally_snapshot"]:
        cached(folder, "diagnostic_exports", export_key, diagnostic_exports)

    ball_rows = {}
    with csv_path.open() as stream:
        for row in csv.DictReader(stream):
            if float(row["Visibility"]) > 0:
                ball_rows[int(row["Frame"])] = (float(row["X"]), float(row["Y"]))
    predictions = json.loads(player_path.read_text())["predictions"]
    balls, players_by_frame = {}, {}
    for item in predictions:
        frame = item["frame_index"]
        if item.get("class_name") == "ball":
            box = item["bbox_xyxy"]
            candidate = {"x": (box[0] + box[2]) / 2, "y": (box[1] + box[3]) / 2, "confidence": item["score"]}
            if candidate["confidence"] > balls.get(frame, {}).get("confidence", 0):
                balls[frame] = candidate
        elif item.get("class_name") == "player" and item["score"] >= .5:
            players_by_frame.setdefault(frame, []).append(item)
    warnings = [] if court.exists() else ["Нет разметки площадки: стороны, подачи и блоки не подтверждены; классификация эвристическая."]
    event_path = folder / "analysis/events.json"
    enriched_path = folder / "analysis/game-tracks.json"
    use_ravel_peaks = advanced["player_model"] == "RAVEL-VB-013-9f"
    event_key = key([track_key, digest(Path(__file__).with_name("events.py")),
                     digest(Path(__file__)),
                     digest(Path(__file__).with_name("touch_peaks.py")),
                     digest(Path(__file__).with_name("reprocess.py"))])

    def game_analysis():
        tracks = [json.loads(path.read_text()) for path in paths[1:]]
        detector = image_contact_detector(player_path, width, height) if not court.exists() else None
        touch_peaks = []
        if use_ravel_peaks:
            from .reprocess import _proposal_records, _ravel_contacts, _merge_contacts
            from .touch_peaks import select_touch_peaks, associate_touch_players
            # Use the complete cached probability sequence; raw proposals do
            # not have Track IDs until aligned with the tracked player boxes.
            raw_touches, _ = _proposal_records(
                folder, {"outputs": {"players.raw.v2.jsonl": True}}, width, height,
                advanced["player_action_threshold"], advanced["player_ball_threshold"],
                include_balls=False)
            touch_peaks = associate_touch_players(select_touch_peaks(
                raw_touches, advanced["player_action_threshold"],
                advanced["player_touch_smooth"], advanced["player_touch_min_gap"]),
                predictions)
        for track in tracks:
            if detector:
                track["player_interaction"] = uncalibrated_contacts(track, detector)
            if use_ravel_peaks:
                classification = track.get("rally_classification") or {}
                if (classification.get("is_rally") is True
                        or classification.get("label") == "rally"):
                    interaction = track.get("player_interaction") or {}
                    neural = _ravel_contacts(
                        track, [], advanced["player_action_threshold"],
                        selected_peaks=touch_peaks)
                    interaction["contacts"] = _merge_contacts(
                        interaction.get("contacts", []), neural)
                    track["player_interaction"] = interaction
        timeline = event_timeline(tracks, source, args.analysis_start, width, height, court.exists())
        if use_ravel_peaks:
            for event in timeline["events"]:
                if (event.get("contact") or {}).get("origin") == "ravel_touch":
                    event["status"] = "model_prediction"
        save(event_path, timeline)
        save(enriched_path, tracks)
        return [event_path, enriched_path]

    stages.begin(5)
    cached(folder, "game_analysis", event_key, game_analysis)
    stages.end()
    stages.begin(6)
    timeline = json.loads(event_path.read_text())
    candidates = []
    all_tracks = json.loads(enriched_path.read_text())
    for track in all_tracks:
        track["player_interaction"]["contacts"] = contacts_for(timeline, track)
        # Keep a broad set of scored candidates. User montage thresholds are
        # applied later, so changing the edit does not rerun detection.
        candidate = score_track(track, balls, width, 0.5, 0.05, bool(court.exists()))
        if candidate:
            candidate.update(id=key([str(source), candidate["start"], candidate["end"]]), source=str(source), source_content=source_id[-1], analysis_offset=args.analysis_start, analysis_width=width, analysis_height=height, track=track, ball=balls, players=players_by_frame, ball_rows=ball_rows, cache=str(folder), events=str(event_path), source_duration=float(source_probe["format"]["duration"]))
            candidate["components"].update(player_motion=player_motion(players_by_frame, candidate["start"], candidate["end"], width))
            earlier = [t["last_frame"]+1 for t in all_tracks if t["last_frame"] < candidate["start"]]
            later = [t["start_frame"]-1 for t in all_tracks if t["start_frame"] > candidate["end"]]
            candidate["safe_context_start"] = max(earlier, default=0)
            candidate["safe_context_end"] = min(later, default=int(video.get("nb_frames", round(float(video["duration"])*30)))-1)
            candidate["score"] = round(sum(candidate["components"].values()) * candidate["confidence"], 4)
            candidates.append(candidate)
    save(folder / "ranking.json", [{k: v for k, v in x.items() if k not in ("ball", "players", "ball_rows", "track")} for x in candidates])
    stages.end()
    return candidates, warnings, folder


def player_motion(players, start, end, width):
    previous, speeds = {}, []
    for frame in range(start, end + 1):
        for player in players.get(frame, []):
            identifier = player.get("track_id")
            box = player["bbox_xyxy"]
            point = ((box[0]+box[2])/2, box[3])
            if identifier in previous:
                old_frame, old_point = previous[identifier]
                if 0 < frame-old_frame <= 3:
                    speeds.append(math.dist(point, old_point)*30/(frame-old_frame)/width)
            previous[identifier] = (frame, point)
    return min(4., float(np.percentile(speeds, 80))*12) if speeds else 0.


def score_track(track, balls, width, min_duration, threshold, has_court):
    start, end = track["start_frame"], track["last_frame"]
    duration = (end - start + 1) / 30
    if duration < min_duration:
        return None
    classification = track.get("rally_classification", {})
    if classification.get("not_rally_reason") == "technical_return":
        return None
    interaction = track.get("player_interaction", {})
    contacts = [dict(c) for c in interaction.get("contacts", []) if c.get("confidence", 0) >= threshold]
    for contact in contacts:
        contact["status"] = "heuristic"
        if not has_court and contact["contact_type"] in ("block", "serve"):
            contact["contact_type"] = "unknown_touch"
    visible = [p for f, p in balls.items() if start <= f <= end and p["confidence"] >= threshold]
    coverage = len(visible) / max(1, end - start + 1)
    rally_conf = float(classification.get("rally_confidence", 0))
    confidence = min(rally_conf, min(1., coverage * 2))
    if confidence < threshold:
        return None
    points = sorted((int(p[1]), *p[0][:2]) for p in track.get("positions", []))
    speeds = [math.hypot(b[1] - a[1], b[2] - a[2]) * 30 / (b[0] - a[0]) / width for a, b in zip(points, points[1:]) if 0 < b[0] - a[0] <= 3]
    fast = float(np.percentile(speeds, 85)) if speeds else 0.
    direction_changes = sum(c.get("direction_change_deg", 0) >= 60 for c in contacts)
    sides = [c.get("side") for c in contacts if c.get("side") in ("near", "far", "left", "right")]
    exchanges = sum(a != b for a, b in zip(sides, sides[1:])) if has_court else 0
    weights = {"attack": 3., "block": 3., "dig": 1.5, "serve": .8, "overhead_pass": 1., "low_touch": 1.5}
    components = {"events": min(18., sum(weights.get(c["contact_type"], .4) * c["confidence"] for c in contacts)), "duration": min(2., duration / 10), "contacts": min(6., len(contacts) * .6), "motion": min(5., fast * 3), "exchanges": min(8., exchanges * 2), "confidence": confidence * 5}
    components["direction_changes"] = min(4., direction_changes * .6)
    if not contacts and fast < .12:
        return None
    score = sum(components.values()) * confidence
    return {"start": start, "end": end, "duration": duration, "score": round(score, 4), "components": components, "confidence": confidence, "coverage": coverage, "contacts": contacts, "speed_widths_sec_p85": fast, "court_available": has_court}


def select(candidates, duration, count):
    groups, used = [[] for _ in range(count)], []
    occupied = []
    totals = [0.] * count
    for candidate in sorted(candidates, key=lambda x: x["score"], reverse=True):
        if any((candidate.get("source_content", candidate["source"]) == x.get("source_content", x["source"])) and max(candidate["start"], x["start"]) <= min(candidate["end"], x["end"]) for x in used):
            continue
        choices = [i for i, total in enumerate(totals) if total < duration - .25]
        if not choices:
            break
        index = min(choices, key=lambda i: totals[i])
        remaining = duration - totals[index]
        item = dict(candidate)
        available = candidate["duration"]
        if available < remaining and "safe_context_start" in item:
            extra = max(0, int((remaining-available)*30))
            before = min(8, extra//2, item["start"]-item["safe_context_start"])
            after = min(8, extra-before, item["safe_context_end"]-item["end"])
            item["start"] -= max(0, before)
            item["end"] += max(0, after)
            item["context_frames"] = max(0, before)+max(0, after)
            if any(item.get("source_content", item["source"]) == old.get("source_content", old["source"]) and max(item["start"], old["start"]) <= min(item["end"], old["end"]) for old in occupied):
                item["start"], item["end"] = candidate["start"], candidate["end"]
                item["context_frames"] = 0
        if available > remaining:
            if remaining < 2:
                continue
            peak = max(candidate.get("selection_contacts", candidate["contacts"]), key=lambda x: ({"attack": 3, "block": 3, "dig": 2}.get(x["contact_type"], 1)*x["confidence"], x.get("speed_after_px_frame", 0)), default={"frame": (item["start"] + item["end"]) // 2})["frame"]
            length = int(remaining * 30)
            item["start"] = max(item["start"], min(peak - length // 2, item["end"] - length + 1))
            item["end"] = item["start"] + length - 1
            item["trimmed"] = True
        item["duration"] = (item["end"] - item["start"] + 1) / 30
        groups[index].append(item)
        totals[index] += item["duration"]
        used.append(candidate)
        occupied.append(item)
    return [g for g in groups if g]
