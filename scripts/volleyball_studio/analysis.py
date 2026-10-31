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
    model = ROOT / "models/VballNetV4c_seq9_grayscale_20260908_213829.onnx"
    players_model = ROOT / "ov/RAVEL-VB-012-9f.xml"
    court_in_results = ROOT / "results" / (source.stem + "_court.json")
    court = court_in_results if court_in_results.is_file() else source.with_name(source.stem + "_court.json")
    source_id = identity(source)
    code = {p.name: digest(p) for p in (ROOT / "src").glob("*.py")}
    signature = key([ANALYSIS_INPUT_VERSION, source_id, args.analysis_seconds, args.analysis_start, args.analysis_width, code])
    folder = work / signature
    folder.mkdir(parents=True, exist_ok=True)
    source_probe = probe(source)
    save(folder / "source.probe.json", source_probe)
    proxy = folder / "analysis.mp4"
    command = ["ffmpeg", "-v", "error", "-nostdin", "-y", "-ss", str(args.analysis_start), "-i", source]
    if args.analysis_seconds:
        command += ["-t", str(args.analysis_seconds)]
    command += ["-vf", f"fps=30,scale={args.analysis_width}:-2,setsar=1", "-an", "-c:v", "libx264", "-preset", "ultrafast", "-crf", "18", "-threads", "4", proxy]
    cached(folder, "normalize", signature, lambda: (run(command, folder / "normalize.log"), [proxy])[1])
    inference_input = proxy
    if getattr(args, "scratch_dir", None):
        scratch = args.scratch_dir.resolve() / signature
        scratch.mkdir(parents=True, exist_ok=True)
        inference_input = scratch / "analysis.mp4"
        cached(scratch, "proxy", key([digest(proxy)]), lambda: (shutil.copyfile(proxy, inference_input), [inference_input])[1])
    video = next(s for s in probe(proxy)["streams"] if s["codec_type"] == "video")
    width, height = video["width"], video["height"]
    csv_path = folder / "ball/analysis/ball.csv"
    ball_key = key([signature, identity(model), identity(ROOT / "ov/VballNetV4c_seq9_grayscale_20260908_213829.xml"), identity(ROOT / "ov/VballNetV4c_seq9_grayscale_20260908_213829.bin"), args.backend])

    def ball():
        csv_path.parent.mkdir(parents=True, exist_ok=True)
        if csv_path.exists():
            csv_path.unlink()
        script = "inference_openvino_seq_gray_v2.py" if args.backend == "openvino" else "inference_onnx_seq_gray_v2.py"
        command = [sys.executable, ROOT / "src" / script, "--video_path", inference_input, "--output_dir", folder / "ball", "--only_csv"]
        command += ["--model_xml", ROOT / "ov/VballNetV4c_seq9_grayscale_20260908_213829.xml", "--device", "CPU"] if args.backend == "openvino" else ["--model_path", model]
        run(command, folder / "ball.log")
        if args.backend == "openvino":
            shutil.copyfile(folder / "ball/analysis_predict_ball.csv", csv_path)
        return [csv_path]

    cached(folder, "ball", ball_key, ball)
    player_path = folder / "players.json"
    player_key = key([signature, identity(players_model), identity(players_model.with_suffix(".bin")), identity(players_model.with_suffix(".json"))])

    def players():
        run([sys.executable, ROOT / "src/inference_player_ball_openvino.py", inference_input, "--model", players_model, "--output", player_path, "--device", "CPU", "--num-threads", str(args.threads)], folder / "players.log")
        return [player_path]

    cached(folder, "players", player_key, players)
    track_key = key([ball_key, player_key, identity(court) if court.exists() else None, 1.5, args.beach])

    def tracks():
        track_dir = folder / "tracks/analysis/tracks"
        if track_dir.exists():
            for old in track_dir.glob("track_*.json"):
                old.unlink()
        command = [sys.executable, ROOT / "src/track_calculator_with_court.py", "--csv_path", csv_path, "--players_json_path", player_path, "--fps", "30", "--video_width", str(width), "--video_height", str(height), "--min_duration_sec", "1.5", "--output_dir", folder / "tracks"]
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
            if CourtTransformer(str(scaled_court)).load().matrix is None:
                raise ValueError("Некорректная геометрия площадки; координаты не будут выдуманы")
            command += ["--court_json_path", scaled_court]
        if args.beach:
            command += ["--beach"]
        run(command, folder / "tracks.log")
        result = sorted(track_dir.glob("track_*.json"))
        index = folder / "track-index.json"
        save(index, [str(p) for p in result])
        return [index] + result

    paths = cached(folder, "tracks", track_key, tracks)
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
    event_key = key([track_key, digest(Path(__file__).with_name("events.py")), digest(Path(__file__))])

    def game_analysis():
        tracks = [json.loads(path.read_text()) for path in paths[1:]]
        detector = image_contact_detector(player_path, width, height) if not court.exists() else None
        for track in tracks:
            if detector:
                track["player_interaction"] = uncalibrated_contacts(track, detector)
        timeline = event_timeline(tracks, source, args.analysis_start, width, height, court.exists())
        save(event_path, timeline)
        save(enriched_path, tracks)
        return [event_path, enriched_path]

    cached(folder, "game_analysis", event_key, game_analysis)
    timeline = json.loads(event_path.read_text())
    candidates = []
    all_tracks = json.loads(enriched_path.read_text())
    for track in all_tracks:
        track["player_interaction"]["contacts"] = contacts_for(timeline, track)
        candidate = score_track(track, balls, width, args.min_rally, args.min_confidence, bool(court.exists()))
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
