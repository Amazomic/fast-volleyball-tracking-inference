import json
from pathlib import Path
import re
import subprocess

from .common import digest, probe, save


def check(path, timeline, requested, threshold):
    info = probe(path)
    video = next(s for s in info["streams"] if s["codec_type"] == "video")
    audio = next((s for s in info["streams"] if s["codec_type"] == "audio"), None)
    duration = float(video["duration"])
    process = subprocess.run(["ffmpeg", "-hide_banner", "-nostdin", "-loglevel", "info", "-xerror", "-i", str(path), "-vf", "blackdetect=d=0.75:pix_th=0.08:pic_th=0.98", "-f", "null", "-"], capture_output=True, text=True, encoding="utf-8", errors="replace")
    black = [dict(zip(("start", "end", "duration"), map(float, match))) for match in re.findall(r"black_start:([\d.]+) black_end:([\d.]+) black_duration:([\d.]+)", process.stderr)]
    checks = {"decodes": process.returncode == 0, "aspect_9_16": video["width"]*16 == video["height"]*9, "h264": video["codec_name"] == "h264", "has_audio": audio is not None, "no_long_black": not black, "frame_mapping": abs(int(video.get("nb_frames", len(timeline["mapping"]))) - len(timeline["mapping"])) <= 1, "av_duration": audio is not None and abs(float(audio.get("duration", duration))-duration) < .15, "duration_not_exceeded": duration <= requested+.1}
    bad_effects = [e for e in timeline["effects"] if any(e.get(k) for k in ("fire", "trail", "flash", "slow", "shake")) and (not e.get("ball") or e["ball"]["confidence"] < threshold)]
    checks["effects_have_ball"] = not bad_effects
    cropped = 0
    for event in timeline["effects"]:
        if not event.get("flash") or not event.get("ball") or event["output_frame"] >= len(timeline["mapping"]):
            continue
        row = timeline["mapping"][event["output_frame"]]
        ball = event["ball"]
        inside = row["crop_x"] <= ball["x"] <= row["crop_x"]+row["crop_width"] and row["crop_y"] <= ball["y"] <= row["crop_y"]+row["crop_height"]
        cropped += not inside
    checks["key_events_visible"] = cropped == 0
    warnings = []
    if duration < requested-.5:
        warnings.append("Недостаточно материала для целевой длительности без случайного контекста")
    if timeline["effects"]:
        near_end = any(e.get("flash") and 0 <= duration-e["output_frame"]/30 < .15 for e in timeline["effects"])
        if near_end:
            warnings.append("Конец ролика близок к касанию: проверьте завершение эпизода")
    return {"schema": "volleyball-quality/2", "passed": all(checks.values()), "checks": checks, "warnings": warnings, "black_segments": black, "duration": duration, "requested": requested, "sha256": digest(path), "av_sync_scope": "stream timing and shared retiming map; perceptual lip/impact sync not automatically proven", "semantic_accuracy": "requires human review", "decode_error": process.stderr[-2000:] if process.returncode else None}


def final_timeline(segments, duration):
    mapping, effects, offset = [], [], 0
    limit = round(duration*30)
    for segment in segments:
        timeline = json.loads(Path(segment).with_suffix(".timeline.json").read_text())
        for row in timeline["mapping"]:
            if len(mapping) < limit:
                mapping.append({"output_frame": len(mapping), "source": timeline["source"], **row})
        effects.extend({**event, "output_frame": event["output_frame"]+offset, "source": timeline["source"]} for event in timeline["effects"] if event["output_frame"]+offset < limit)
        offset += len(timeline["mapping"])
    return {"fps": 30, "mapping": mapping, "effects": effects}
