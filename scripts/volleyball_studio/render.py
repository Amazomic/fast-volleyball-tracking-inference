from collections import deque
import json
import math
from pathlib import Path
import subprocess
import time
import wave

import cv2
import numpy as np

from .common import encoder, probe, run, save
from .vfx import apply_effects, effect_state, finish_frame, playback_speed, valid_ball


def camera_targets(item):
    start, end = item["start"], item["end"]
    width = item["analysis_width"]
    crop_width = min(width, item["analysis_height"] * 9 / 16)
    positions = {int(p[1]): p[0] for p in item["track"].get("positions", [])}
    center = width / 2
    velocity = 0.
    targets = []
    last_known = start - 100
    last_x = None
    ball_velocity = 0.
    for frame in range(start, end + 1):
        if frame in positions:
            ball_x, ball_y = positions[frame]
            if last_x is not None and frame-last_known <= 3:
                ball_velocity = float(np.clip((ball_x-last_x)/max(1, frame-last_known), -width/60, width/60))
            last_x = ball_x
            last_known = frame
            players = item["players"].get(frame, [])
            nearest = min(players, key=lambda p: abs((p["bbox_xyxy"][0] + p["bbox_xyxy"][2]) / 2 - ball_x), default=None)
            desired = ball_x
            if nearest:
                box = nearest["bbox_xyxy"]
                player_x = (box[0] + box[2]) / 2
                if abs(player_x - ball_x) < crop_width * .8:
                    desired = .65 * ball_x + .35 * player_x
        elif frame - last_known <= 8 and last_x is not None:
            desired = last_x + ball_velocity*(frame-last_known)
        elif frame - last_known > 15:
            desired = width / 2
        else:
            desired = center
        if not targets:
            center = float(np.clip(desired, crop_width / 2, width - crop_width / 2))
        acceleration = .035 * (desired - center) - .35 * velocity
        velocity = float(np.clip(velocity + acceleration, -width / 120, width / 120))
        center = float(np.clip(center + velocity, crop_width / 2, width - crop_width / 2))
        targets.append(center)
    return targets


def render_segment(item, path, args, codec):
    start, end = item["start"], item["end"]
    centers = camera_targets(item)
    width, height = item["analysis_width"], item["analysis_height"]
    crop_h = min(height, int(width * 16 / 9))
    crop_w = min(width, int(crop_h * 9 / 16))
    base_crop_h, base_crop_w = crop_h, crop_w
    zoom = 1.
    source_info = next(s for s in probe(item["source"])["streams"] if s["codec_type"] == "video")
    native_w, native_h = source_info["width"], source_info["height"]
    rotation = next((int(x.get("rotation", 0)) for x in source_info.get("side_data_list", []) if "rotation" in x), 0)
    if abs(rotation) % 180 == 90:
        native_w, native_h = native_h, native_w
    decoder_log = path.with_suffix(".decode.log").open("wb")
    decoder = subprocess.Popen(["ffmpeg", "-v", "error", "-nostdin", "-ss", str(start / 30 + item["analysis_offset"]), "-i", item["source"], "-frames:v", str(end - start + 1), "-vf", f"fps=30,scale={native_w}:{native_h},setsar=1", "-f", "rawvideo", "-pix_fmt", "bgr24", "pipe:1"], stdout=subprocess.PIPE, stderr=decoder_log)
    silent = path.with_name(path.stem + ".silent.mp4")
    log_path = path.with_suffix(".encode.log")
    effects, mapping, history = [], [], deque(maxlen=18)
    audio_runs = []
    phase = 0.
    started = time.monotonic()
    with log_path.open("wb") as log:
        writer = subprocess.Popen(["ffmpeg", "-v", "error", "-nostdin", "-y", "-f", "rawvideo", "-pix_fmt", "bgr24", "-s", f"{args.width}x{args.width * 16 // 9}", "-r", "30", "-i", "pipe:0", "-an", *codec, "-pix_fmt", "yuv420p", "-movflags", "+faststart", silent], stdin=subprocess.PIPE, stderr=log)
        try:
            for index, frame in enumerate(range(start, end + 1)):
                raw = decoder.stdout.read(native_w * native_h * 3)
                if len(raw) != native_w * native_h * 3:
                    raise RuntimeError(f"Не удалось декодировать кадр {frame}")
                image = np.frombuffer(raw, np.uint8).reshape(native_h, native_w, 3)
                state = effect_state(item, frame, args.effects, args.min_confidence)
                target_zoom = 1.05 if state["fire"] or state["flash"] and args.effects != "off" else 1.
                zoom += (target_zoom-zoom)*.12
                crop_h, crop_w = int(base_crop_h/zoom), int(base_crop_w/zoom)
                left = int(np.clip(centers[index] - crop_w / 2, 0, width - crop_w))
                top = max(0, (height - crop_h) // 2)
                sx, sy = native_w / width, native_h / height
                cropped = image[int(top * sy):int((top + crop_h) * sy), int(left * sx):int((left + crop_w) * sx)]
                output = cv2.resize(cropped, (args.width, args.width * 16 // 9))
                ball = valid_ball(item, frame, args.min_confidence)
                point = (int((ball["x"] - left) * args.width / crop_w), int((ball["y"] - top) * args.width * 16 / 9 / crop_h)) if ball else None
                if args.effects != "off":
                    output = apply_effects(output, point, history, state, args.effects, frame)
                transition_scale = 1.
                edge_distance = min(index, end - frame)
                if args.effects != "off" and edge_distance < 12:
                    transition_scale = 1 + (.10 if args.effects == "cinematic" else .05) * ((12 - edge_distance) / 12) ** 2
                    oh, ow = output.shape[:2]
                    zw, zh = int(ow / transition_scale), int(oh / transition_scale)
                    zx, zy = (ow - zw) // 2, (oh - zh) // 2
                    output = cv2.resize(output[zy:zy+zh, zx:zx+zw], (ow, oh))
                speed = playback_speed(item, frame, args, state)
                if getattr(args, "speed_ramp", False):
                    state["slow"] = speed < 1
                if not audio_runs or audio_runs[-1]["speed"] != speed:
                    phase = 0.
                    audio_runs.append({"start": frame, "end": frame, "speed": speed, "output_frames": 0})
                else:
                    audio_runs[-1]["end"] = frame
                phase += 1 / speed
                repeats = int(phase + 1e-9)
                phase -= repeats
                audio_runs[-1]["output_frames"] += repeats
                output = finish_frame(output, args, state, index, end-start+1, "VOLLEYBALL | " + getattr(args, "style", "classic").upper())
                for repeat in range(repeats):
                    writer.stdin.write(output.tobytes())
                    mapping.append({"source_frame": frame, "source_sec": frame / 30 + item["analysis_offset"], "repeat": repeat, "crop_x": left, "crop_y": top, "crop_width": crop_w, "crop_height": crop_h, "transition_scale": transition_scale})
                if repeats and any(state.get(k) for k in ("trail", "fire", "flash", "slow", "shake")):
                    effects.append({"source_frame": frame, "output_frame": len(mapping) - repeats, "ball": ball, "event_ids": [c.get("event_id") for c in item["contacts"] if abs(frame-c["frame"]) <= 18], **state})
                if index and index % 150 == 0:
                    speed = (index + 1) / max(.001, time.monotonic() - started)
                    print(json.dumps({"stage": 5, "frames": index + 1, "total": end - start + 1, "fps": round(speed, 1), "eta_sec": round((end - frame) / speed, 1)}), flush=True)
            writer.stdin.close()
            if writer.wait() != 0:
                raise RuntimeError(log_path.read_text(errors="replace")[-3000:])
            if decoder.wait() != 0:
                raise RuntimeError("Ошибка декодера источника")
        finally:
            if writer.poll() is None:
                writer.terminate()
                writer.wait()
            if decoder.poll() is None:
                decoder.terminate()
                decoder.wait()
            decoder.stdout.close()
            if not writer.stdin.closed:
                writer.stdin.close()
            decoder_log.close()
    total = len(mapping) / 30
    audio_available = any(s["codec_type"] == "audio" for s in probe(item["source"])["streams"])
    audio = path.with_suffix(".wav")
    if audio_available:
        chains = []
        usable = [r for r in audio_runs if r["output_frames"]]
        for i, part in enumerate(usable):
            a, b = part["start"], part["end"]
            rate = (b-a+1) / part["output_frames"]
            chains.append(f"[0:a]atrim=start={(a-start)/30}:end={(b-start+1)/30},asetpts=PTS-STARTPTS,atempo={rate},apad,atrim=duration={part['output_frames']/30}[a{i}]")
        chains.append("".join(f"[a{i}]" for i in range(len(usable))) + f"concat=n={len(usable)}:v=0:a=1,apad,atrim=duration={total}[out]")
        graph = path.with_suffix(".audio.txt")
        graph.write_text(";".join(chains))
        run(["ffmpeg", "-v", "error", "-nostdin", "-y", "-ss", str(start / 30 + item["analysis_offset"]), "-t", str((end - start + 1) / 30), "-i", item["source"], "-filter_complex_script", graph, "-map", "[out]", "-ar", "48000", "-ac", "2", audio])
    else:
        run(["ffmpeg", "-v", "error", "-nostdin", "-y", "-f", "lavfi", "-i", "anullsrc=r=48000:cl=stereo", "-t", str(total), audio])
    fade = min(.15, total / 4)
    af = f"afade=t=in:d={fade},afade=t=out:st={total-fade}:d={fade}"
    run(["ffmpeg", "-v", "error", "-nostdin", "-y", "-i", silent, "-i", audio, "-map", "0:v", "-map", "1:a", "-af", af, *codec, "-c:a", "aac", "-ar", "48000", "-ac", "2", "-pix_fmt", "yuv420p", "-t", str(total), path])
    save(path.with_suffix(".timeline.json"), {"source": item["source"], "fps": 30, "mapping": mapping, "effects": effects, "audio_runs": audio_runs, "duration": total, "seconds": time.monotonic() - started, "original_audio": audio_available})
    return total, effects


def accents(path, duration, hits):
    sample_rate = 48000
    data = np.zeros(int(math.ceil(duration * sample_rate)), np.float32)
    for hit in hits:
        offset = int(hit * sample_rate)
        t = np.arange(int(.12 * sample_rate)) / sample_rate
        pulse = .12 * np.sin(2 * np.pi * (160 * t - 250 * t * t)) * np.exp(-t * 35)
        length = max(0, min(len(pulse), len(data) - offset))
        if length:
            data[offset:offset + length] += pulse[:length]
    with wave.open(str(path), "wb") as stream:
        stream.setnchannels(1)
        stream.setsampwidth(2)
        stream.setframerate(sample_rate)
        stream.writeframes((np.clip(data, -1, 1) * 32767).astype("<i2").tobytes())


def assemble(segments, dest, music, args, codec, hits):
    inputs = []
    for segment in segments:
        inputs += ["-i", str(segment)]
    graph = "".join(f"[{i}:v][{i}:a]" for i in range(len(segments))) + f"concat=n={len(segments)}:v=1:a=1[v][match]"
    graph += f";[match]volume={args.match_volume}[original]"
    mixed = "original"
    extras = []
    if music:
        idx = len(segments)
        inputs += ["-stream_loop", "-1", "-i", str(music)]
        graph += f";[{idx}:a]atrim=duration={args.duration},asetpts=PTS-STARTPTS,volume={args.music_volume},afade=t=in:d=0.5,afade=t=out:st={max(0,args.duration-.8)}:d=0.8[music]"
        if getattr(args, "ducking", True):
            graph += ";[original]asplit=2[original_mix][sidechain];[music][sidechain]sidechaincompress=threshold=0.035:ratio=6:attack=10:release=250:makeup=1[ducked]"
            graph += ";[original_mix]anull[match_mix]"
            extras.append("ducked")
            mixed = "match_mix"
        else:
            extras.append("music")
    if args.accents and hits and args.effects != "off":
        sound = Path(segments[0]).parent / (dest.stem + ".accents.wav")
        accents(sound, args.duration, hits)
        idx = len(segments) + bool(music)
        inputs += ["-i", str(sound)]
        graph += f";[{idx}:a]volume={args.accent_volume}[accents]"
        extras.append("accents")
    if extras:
        graph += f";[{mixed}]" + "".join(f"[{x}]" for x in extras) + f"amix=inputs={len(extras)+1}:duration=first:normalize=0[mixture]"
        mixed = "mixture"
    graph += f";[{mixed}]alimiter=limit=.95:level=0,aresample=48000[a]"
    run(["ffmpeg", "-v", "error", "-nostdin", "-y", *inputs, "-filter_complex", graph, "-map", "[v]", "-map", "[a]", "-t", str(args.duration), *codec, "-c:a", "aac", "-b:a", "192k", "-ac", "2", "-pix_fmt", "yuv420p", "-movflags", "+faststart", dest])
    info = probe(dest)
    video = next(s for s in info["streams"] if s["codec_type"] == "video")
    if video["width"] * 16 != video["height"] * 9 or video["codec_name"] != "h264" or not any(s["codec_type"] == "audio" for s in info["streams"]):
        raise RuntimeError("Экспорт не прошёл проверку формата")
    run(["ffmpeg", "-v", "error", "-xerror", "-i", dest, "-f", "null", "-"])
    return info
