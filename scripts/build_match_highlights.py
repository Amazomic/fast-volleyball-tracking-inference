#!/usr/bin/env python3
"""Build a 120-second highlight and per-track-ID statistics from cached studio analysis.

Files: results/highlights_120s.mp4, results/player_statistics.{csv,json},
results/highlights_manifest.json, results/match_report.md.
Does NOT rerun ball/player inference, and does NOT overwrite source assets.
"""
from __future__ import annotations

import argparse
import csv
import hashlib
import json
import shutil
import subprocess
import sys
from collections import Counter, defaultdict
from full_rallies import choose_whole_groups, load_groups
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
RESULTS = ROOT / "results"
SOURCE_DIR = ROOT / "project"
TARGET_FRAMES = 120 * 30
TYPES = ("attack", "block", "serve", "dig", "overhead_pass", "low_touch")
LABELS = {
    "attack": "Атаки", "block": "Блоки", "serve": "Подачи",
    "dig": "Приёмы/защита", "overhead_pass": "Верхние передачи",
    "low_touch": "Низкие касания",
}


def save_json(path: Path, data: object) -> None:
    path.write_text(json.dumps(data, ensure_ascii=False, indent=2), encoding="utf-8")


def source_matches(events: dict, video: Path) -> bool:
    return Path(str(events.get("source", "")).replace("\\", "/")).name.lower() == video.name.lower()


def find_cache(video: Path) -> tuple[Path, dict]:
    for d in sorted((RESULTS / "analysis").iterdir(), key=lambda p: p.stat().st_mtime, reverse=True):
        events_path = d / "analysis" / "events.json"
        if not events_path.is_file() or not (d / "ranking.json").is_file() or not (d / "analysis.mp4").is_file():
            continue
        events = json.loads(events_path.read_text(encoding="utf-8"))
        if source_matches(events, video):
            return d, events
    raise RuntimeError(f"No cached full match analysis for {video.name} in {RESULTS / 'analysis'}")


def build_statistics(cache: Path, events: dict) -> tuple[list[dict], Counter]:
    stats = defaultdict(lambda: {
        "detections": 0, "first_seen_frame": None, "last_seen_frame": None,
        "confidence_sum": 0.0, "events": 0, **{t: 0 for t in TYPES},
    })
    print("Reading saved player detections (without running the neural network)...", flush=True)
    payload = json.loads((cache / "players.json").read_text(encoding="utf-8"))
    for item in payload.get("predictions", []):
        if item.get("class_name") != "player" or item.get("track_id") is None:
            continue
        player = stats[str(item["track_id"])]
        frame = int(item.get("frame_index", 0))
        player["detections"] += 1
        player["first_seen_frame"] = frame if player["first_seen_frame"] is None else min(player["first_seen_frame"], frame)
        player["last_seen_frame"] = frame if player["last_seen_frame"] is None else max(player["last_seen_frame"], frame)
    del payload
    totals = Counter()
    seen_event_ids = set()
    for event in events.get("events", []):
        kind = str(event.get("type", ""))
        if kind not in TYPES or event.get("id") in seen_event_ids:
            continue
        seen_event_ids.add(event.get("id"))
        totals[kind] += 1
        player_id = event.get("player_track_id")
        if player_id is None:
            continue
        row = stats[str(player_id)]
        row[kind] += 1
        row["events"] += 1
        row["confidence_sum"] += float(event.get("confidence") or 0.0)
    records = []
    for id_, item in stats.items():
        records.append({
            "player_track_id": id_,
            "detected_frames": item["detections"],
            "first_seen_sec": round((item["first_seen_frame"] or 0) / 30, 2) if item["first_seen_frame"] is not None else None,
            "last_seen_sec": round((item["last_seen_frame"] or 0) / 30, 2) if item["last_seen_frame"] is not None else None,
            **{t: item[t] for t in TYPES},
            "total_actions": item["events"],
            "average_action_confidence": round(item["confidence_sum"] / item["events"], 3) if item["events"] else None,
        })
    records.sort(key=lambda r: (-r["total_actions"], -r["detected_frames"], int(r["player_track_id"])))
    fields = ["player_track_id", "detected_frames", "first_seen_sec", "last_seen_sec", *TYPES, "total_actions", "average_action_confidence"]
    with (RESULTS / "player_statistics.csv").open("w", encoding="utf-8-sig", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=fields)
        writer.writeheader()
        writer.writerows(records)
    save_json(RESULTS / "player_statistics.json", {
        "source": str(cache / "players.json"),
        "method": "model-generated track IDs and heuristic contact classifications, NOT verified unique athlete identities",
        "caution": "Track IDs can split/reassign; zero means not recognized, not necessarily no occurrence",
        "players": records, "event_totals": dict(totals),
    })
    return records, totals


def run(cmd: list[str], logfile: Path) -> bool:
    with logfile.open("a", encoding="utf-8") as log:
        log.write("\n$ " + " ".join(map(str, cmd)) + "\n")
        log.flush()
        p = subprocess.run(cmd, stdout=log, stderr=subprocess.STDOUT, check=False)
        return p.returncode == 0


def render(video: Path, normalized: Path, selected: list[dict], output_path: Path | None = None, segments_folder: Path | None = None) -> None:
    ffmpeg = shutil.which("ffmpeg")
    if not ffmpeg:
        raise RuntimeError("ffmpeg not found in PATH")
    fingerprint = hashlib.sha256((str(video) + str(normalized) + json.dumps(selected, sort_keys=True)).encode()).hexdigest()[:12]
    work = (segments_folder or (RESULTS / "highlight_segments")) / fingerprint
    work.mkdir(parents=True, exist_ok=True)
    log = RESULTS / "highlights_render.log"
    if log.exists():
        log.unlink()
    segments = []
    encoder = "h264_nvenc"
    for item in selected:
        index = item["sequence"]
        start = item["source_start_frame"] / 30
        count = item["source_end_frame"] - item["source_start_frame"] + 1
        duration = count / 30
        output = work / f"clip_{index:03d}.mp4"
        segments.append(output)
        if output.exists() and output.stat().st_size > 100000:
            print(f"Using cached segment {index}/{len(selected)}", flush=True)
            continue
        print(f"Encoding segment {index}/{len(selected)}, source time {start:.1f}s for {duration:.1f}s...", flush=True)
        def command(enc: str) -> list[str]:
            return [
                ffmpeg, "-hide_banner", "-nostdin", "-y", "-loglevel", "error",
                "-ss", f"{start:.6f}", "-i", str(normalized),
                "-ss", f"{start:.6f}", "-i", str(video),
                "-map", "0:v:0", "-map", "1:a:0?", "-t", f"{duration:.6f}",
                "-vf", "scale=720:1280:flags=bicubic,setsar=1,fps=30",
                "-frames:v", str(count), "-c:v", enc,
                *(["-preset", "p4", "-cq", "24", "-b:v", "0"] if enc == "h264_nvenc" else ["-preset", "veryfast", "-crf", "22"]),
                "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "128k",
                "-ar", "48000", "-ac", "2", "-movflags", "+faststart", str(output),
            ]
        if not run(command(encoder), log):
            if encoder == "h264_nvenc" and run(command("libx264"), log):
                encoder = "libx264"
            else:
                raise RuntimeError(f"Segment {index} failed. See {log}")
        if output.stat().st_size < 100000:
            raise RuntimeError(f"Segment {index} unexpectedly small")
    concat = work / "concat.txt"
    concat.write_text("\n".join("file '" + f.name + "'" for f in segments) + "\n", encoding="utf-8")
    out = output_path or (RESULTS / "highlights_120s.mp4")
    print("Joining clips into final highlights...", flush=True)
    cmd = [ffmpeg, "-hide_banner", "-nostdin", "-y", "-loglevel", "error",
           "-f", "concat", "-safe", "0", "-i", str(concat),
           "-c", "copy", "-movflags", "+faststart", str(out)]
    if not run(cmd, log):
        raise RuntimeError(f"Concatenation failed. See {log}")
    print(f"VIDEO READY: {out} ({out.stat().st_size / (1024**2):.1f} MiB)", flush=True)


def main() -> None:
    parser = argparse.ArgumentParser(description="Build whole-rally highlights from cached analysis")
    parser.add_argument("--video", type=Path, help="Existing source video in project/")
    parser.add_argument("--duration", type=float, default=120)
    args = parser.parse_args()
    if not 10 <= args.duration <= 600:
        parser.error("--duration must be in range 10..600")
    RESULTS.mkdir(exist_ok=True)
    videos = sorted(SOURCE_DIR.glob("*.mp4"), key=lambda p: p.stat().st_mtime, reverse=True)
    if not videos:
        raise RuntimeError(f"No video in {SOURCE_DIR}")
    source = args.video.resolve() if args.video else videos[0]
    if source not in [p.resolve() for p in videos]:
        parser.error("--video must point to an MP4 file inside project/")
    cache, events = find_cache(source)
    print(f"Source: {source}\nCached analysis: {cache}", flush=True)
    ranking = json.loads((cache / "ranking.json").read_text(encoding="utf-8"))
    records, totals = build_statistics(cache, events)
    selected = choose_whole_groups(load_groups(cache, ranking), target_sec=args.duration)
    selected_seconds = sum(x["duration_sec"] for x in selected)
    manifest = {
        "source_video": str(source), "analysis_root": str(cache),
        "analysis_video": str(cache / "analysis.mp4"),
        "events_file": str(cache / "analysis" / "events.json"),
        "fps": 30, "target_duration_sec": args.duration, "actual_selected_duration_sec": round(selected_seconds, 4),
        "selection_method": "Complete rally groups (short trajectory gaps merged), ranked by excitement; no clips trimmed",
        "clips": selected,
    }
    save_json(RESULTS / "highlights_manifest.json", manifest)
    types_summary = "\n".join(f"- {LABELS[t]}: {totals[t]}" for t in TYPES)
    top_players = "\n".join(f"| {p['player_track_id']} | {p['attack']} | {p['block']} | {p['serve']} | {p['dig']} | {p['overhead_pass']} | {p['total_actions']} |" for p in records[:25])
    report = f"""# Volleyball match analysis and highlights

Source: `{source.name}`

Saved analysis: `{cache.relative_to(ROOT)}`

Selected highlight length: **{selected_seconds:.1f} seconds** ({len(selected)} complete detected rallies; source-video time in `results/highlights_manifest.json`).

## Detected game actions
{types_summary}

## Per-track player statistics
Total tracked IDs: **{len(records)}**. Top 25 by actions:

| Track ID | Attacks | Blocks | Serves | Digs | Overhead passes | Actions |
|---|---:|---:|---:|---:|---:|---:|
{top_players}

Full table: `results/player_statistics.csv` and `results/player_statistics.json`.

**Reliability:** Action labels are heuristic, not ground truth. Track IDs may change for the same person as tracking loses/reacquires them. No court annotation was available for this video. A zero for a particular action means the model did not recognize it, not that it did not occur. Scores cannot be verified from this dataset.

**No model reprocessing:** The highlight uses the saved normalized video and ranking; the statistics use previously saved player predictions and events. The full event timeline is in `{(cache / 'analysis/events.json').relative_to(ROOT)}`.
"""
    (RESULTS / "match_report.md").write_text(report, encoding="utf-8")
    print(f"Statistics ready: {len(records)} tracked IDs. Event counts: {dict(totals)}", flush=True)
    render(source, cache / "analysis.mp4", selected,
           output_path=RESULTS / f"highlights_{int(args.duration)}s.mp4")


if __name__ == "__main__":
    try:
        main()
    except Exception as ex:
        print(f"ERROR: {ex}", file=sys.stderr, flush=True)
        sys.exit(1)
