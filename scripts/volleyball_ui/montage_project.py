"""Shared complete-rally selection and per-rally rendering metadata.

Every montage mode consumes the same ranking, boundaries and source frames.
Never select fragments of individual ball tracks or shorten a chosen rally.
"""
from __future__ import annotations

import json
import math
import subprocess
from pathlib import Path

from scripts.full_rallies import choose_whole_groups, load_groups
from scripts.rally_boundaries import rally_selection_id

FPS = 30

SELECTION_STRATEGIES = (
    "highlights", "longest", "most_actions", "attacks", "blocks",
    "digs", "passes", "serves",
)
COUNT_FILTERS = ("attack", "block", "dig", "overhead_pass", "serve")


def selection_score(group: dict, strategy: str) -> float:
    """Prioritize the selected feature over the existing spectacle score."""
    counts=group.get("action_counts") or {}
    base=float(group["ranking_score"])
    if strategy=="highlights":
        return base+.45*min(
            (group["detected_end_frame"]-group["detected_start_frame"]+1)/FPS,16)
    key={"attacks":"attack","blocks":"block","digs":"dig",
         "passes":"overhead_pass","serves":"serve"}.get(strategy)
    if strategy=="longest":
        primary=(group["detected_end_frame"]-group["detected_start_frame"]+1)/FPS
    elif strategy=="most_actions":
        primary=int(group.get("actions_total",0))
    else:
        primary=int(counts.get(key,0))
    return primary*10000+min(9999,max(0,base))/10000



def montage_eligible(group: dict, min_rally: float, min_confidence: float) -> bool:
    """Apply thresholds to the detected play, not the context-padded clip."""
    try:
        frames = int(group["detected_end_frame"]) - int(group["detected_start_frame"]) + 1
        confidence = float(group.get("confidence") or 0)
        return (bool(group.get("ranked")) and frames >= min_rally * FPS
                and math.isfinite(confidence) and confidence >= min_confidence)
    except (ValueError, TypeError, KeyError):
        return False


def plan_rallies(sources: list[dict], seconds: float, *, min_rally: float = 1.5,
                 min_confidence: float = .5, padding: float = 1.5,
                 loader=None, chooser=None,
                 gap_sec: float = 3.0, rally_min_sec: float = 0,
                 selection_strategy: str = "highlights",
                 min_action_counts: dict | None = None,
                 selected_rally_ids: list[str] | None = None,
                 list_candidates: bool = False) -> list[dict]:
    """Select top complete scenes across the entire project.

    Shared by custom, track and portrait exports. Selection never depends on
    the export mode. A quality bonus favors long exchanges over noise.
    """
    loader = loader or load_groups
    chooser = chooser or choose_whole_groups
    if not math.isfinite(seconds) or seconds <= 0:
        raise ValueError("Некорректная длительность монтажа")
    if selection_strategy not in SELECTION_STRATEGIES:
        raise ValueError("Неизвестный критерий отбора розыгрышей")
    filters=min_action_counts or {}
    manual=list_candidates or selected_rally_ids is not None
    candidates = []
    for source in sources:
        cache = Path(source["cache"])
        ranking_file = cache / "ranking.json"
        if not ranking_file.is_file() and not manual:
            raise FileNotFoundError(f"Нет рейтинга анализа: {ranking_file}")
        # The Results tab can display reconstructed rallies without a score.
        # Manual selection and the full catalogue must do so as well.
        ranking = json.loads(ranking_file.read_text(encoding="utf-8")) if ranking_file.is_file() else []
        if loader is load_groups:
            groups = loader(cache, ranking, merge_gap=round(gap_sec*FPS),
                            min_sec=rally_min_sec, context_padding=padding)
        else:
            groups = loader(cache, ranking, context_padding=padding)
        identity_by_track={}
        if source.get("project_id"):
            from .data import groups_for
            neural_cache=(cache.parent.parent if cache.parent.name=="postprocessing" else cache)
            for player in groups_for(source["video"],source["project_id"],neural_cache.name):
                for tid in player.get("track_ids",[]):
                    identity_by_track[str(tid)]={
                        "id":player["id"],"name":player["name"]}
        for group in groups:
            verified={}
            for tid in group.get("player_track_ids",[]):
                person=identity_by_track.get(str(tid))
                if person:
                    verified.setdefault(person["id"],{
                        "id":person["id"],"name":person["name"],"track_ids":[]})
                    verified[person["id"]]["track_ids"].append(str(tid))
            group["linked_player_ids"]=sorted(verified)
            group["linked_players"]=list(verified.values())
            if manual:
                # Manual list mirrors reconstructed rallies from Results,
                # including those without an automatic ranking score.
                if not group.get("track_ids"):
                    continue
            elif not montage_eligible(group, min_rally, min_confidence):
                continue
            counts=group.get("action_counts") or {}
            focus={"attacks":"attack","blocks":"block","digs":"dig",
                   "passes":"overhead_pass","serves":"serve"}.get(selection_strategy)
            if not manual and focus and not counts.get(focus,0):
                continue
            if not manual and any(int(counts.get(kind,0))<minimum
                   for kind,minimum in filters.items() if kind in COUNT_FILTERS):
                continue
            if not manual and int(group.get("actions_total",0))<filters.get("actions",0):
                continue
            item = dict(group)
            item["video"] = source["video"]
            item["cache_id"] = cache.name
            # The underlying ranking already scores attacks, blocks, contacts,
            # ball motion and team exchanges; reward sustained play modestly.
            detected = (int(item["detected_end_frame"]) -
                        int(item["detected_start_frame"]) + 1) / FPS
            item["base_ranking_score"] = float(item["ranking_score"])
            item["ranking_score"] = round(
                selection_score(item, selection_strategy), 4)
            item["selection_strategy"] = selection_strategy
            item["rally_id"]=rally_selection_id(source["video"], cache.name, item)
            candidates.append(item)
    if not candidates:
        if list_candidates:
            return []
        raise ValueError("Нет подходящих полных розыгрышей. Проверьте результаты анализа и фильтры.")
    if list_candidates or selected_rally_ids is not None:
        # Manual choice never applies automatic ranking or a duration budget.
        # Keep source-video order, then the original timestamps.
        candidates.sort(key=lambda r:(next(i for i,s in enumerate(sources)
            if s["video"]==r["video"]),r["source_start_frame"],r["source_end_frame"]))
        if list_candidates:
            return candidates
        lookup={r["rally_id"]:r for r in candidates}
        invalid=[rid for rid in selected_rally_ids if rid not in lookup]
        if invalid:
            raise ValueError("Выбранные розыгрыши изменились. Обновите список для монтажа.")
        return [r for r in candidates if r["rally_id"] in set(selected_rally_ids)]
    # No over-budget allowance: a final -t crop would cut a rally in half.
    selected = chooser(candidates, seconds, max_over_sec=0)
    # Protect source identity and stable ordering in the exported timeline.
    return selected


def build_rally_payload(track_dir: Path, rally: dict) -> dict:
    """Reconstruct ONE continuous ball path from all tracks of a selected rally."""
    a, b = int(rally["source_start_frame"]), int(rally["source_end_frame"])
    ids = [int(value) for value in rally.get("track_ids", []) if value is not None]
    if not ids or b < a:
        raise ValueError("У розыгрыша нет корректных границ или треков мяча")
    tracks = {}
    for file in track_dir.glob("track_*.json"):
        try:
            track_id = int(file.stem.rsplit("_", 1)[-1])
        except ValueError:
            continue
        if track_id in ids:
            tracks[track_id] = json.loads(file.read_text(encoding="utf-8"))
    missing = set(ids) - set(tracks)
    if missing:
        raise ValueError("Не найдены треки розыгрыша: " +
                         ", ".join(map(str, sorted(missing))))
    positions = {}
    sample = tracks[ids[0]]
    for track_id in ids:
        for row in tracks[track_id].get("positions", []):
            try:
                xy, frame = row[:2]
                frame = int(frame)
                point = [float(xy[0]), float(xy[1])]
            except (ValueError, TypeError, IndexError):
                continue
            if a <= frame <= b and all(math.isfinite(x) for x in point):
                positions.setdefault(frame, [point, frame])
    if not positions:
        raise ValueError("У розыгрыша нет координат мяча для визуализации")
    return {
        "track_id": ids[0],
        "track_ids": ids,
        "start_frame": a,
        "last_frame": b,
        "positions": [positions[frame] for frame in sorted(positions)],
        # track_processor.py calls Track.from_dict, which requires prediction.
        # These optional tracking fields do not alter the original ball path.
        "prediction": sample.get("prediction") or [],
        "ball_sizes": sample.get("ball_sizes") or [],
        "fps": sample.get("fps", FPS),
        "tracking_scale": sample.get("tracking_scale") or {},
        "court_info": sample.get("court_info") or {},
        "rally_classification": {
            "label": "rally", "is_rally": True,
            "rally_confidence": float(rally.get("confidence", 0))
        },
        "rally_group_id": f"{rally['detected_start_frame']}-{rally['detected_end_frame']}",
        "boundary_method": rally.get("boundary_method", "reconstructed_ball_tracks"),
    }


def plan_clips(sources: list[dict], selected_dir: Path, seconds: float, *,
               mode: str, padding: float, include_non_rally: bool = False,
               min_rally: float = 1.5, min_confidence: float = .5,
               gap_sec: float = 3.0, rally_min_sec: float = 0,
               selection_strategy: str = "highlights",
               min_action_counts: dict | None = None,
               selected_rally_ids: list[str] | None = None) -> list[dict]:
    """Create one metadata payload per whole selected rally, regardless of mode."""
    # Legacy include_non_rally option never overrides the full-rally contract.
    _ = (mode, include_non_rally)
    selected = plan_rallies(sources, seconds, min_rally=min_rally,
                            min_confidence=min_confidence, padding=padding,
                            gap_sec=gap_sec, rally_min_sec=rally_min_sec,
                            selection_strategy=selection_strategy,
                            min_action_counts=min_action_counts,
                            selected_rally_ids=selected_rally_ids)
    selected_dir.mkdir(parents=True, exist_ok=True)
    result = []
    timeline = 0
    for group in selected:
        source = next(s for s in sources if s["video"] == group["video"])
        cache = Path(source["cache"])
        payload = build_rally_payload(cache / "tracks/analysis/tracks", group)
        position = len(result)
        folder = selected_dir / f"{position:03d}"
        folder.mkdir(parents=True, exist_ok=True)
        track_file = folder / f"track_{position:04d}.json"
        track_file.write_text(json.dumps(payload, ensure_ascii=False), encoding="utf-8")
        span = (payload["last_frame"] - payload["start_frame"] + 1) / FPS
        result.append({
            "index": position, "video": group["video"],
            "cache_id": group["cache_id"],
            "video_path": source["video_path"],
            "analysis_offset_sec": float(source.get("analysis_offset_sec", 0)),
            "track_dir": folder,
            "track_json": track_file, "track_ids": payload["track_ids"],
            "start_frame": payload["start_frame"], "end_frame": payload["last_frame"],
            "detected_start_frame": group["detected_start_frame"],
            "detected_end_frame": group["detected_end_frame"],
            "track_id": payload["track_id"], "fps": FPS,
            "score": round(group["ranking_score"], 2),
            "confidence": round(group["confidence"], 4),
            "action_counts": group.get("action_counts", {}),
            "actions_total": group.get("actions_total", 0),
            "source_duration": round(span, 3), "montage_start_sec": round(timeline / FPS, 3),
        })
        timeline += payload["last_frame"] - payload["start_frame"] + 1
    if not result:
        raise ValueError("Нет полных розыгрышей с координатами мяча")
    # Saved selection makes it possible to audit exactly what was included.
    (selected_dir.parent / "selection.json").write_text(json.dumps({
        "selection_method": "shared complete rallies: ranking + whole boundaries",
        "requested_duration": seconds, "selected_duration": round(timeline / FPS, 3),
        "source_count": len(sources),
        "render_source": "original_recording",
        "selected": [{k: v for k, v in clip.items()
                      if k not in ("video_path", "track_dir", "track_json")}
                     for clip in result],
    }, ensure_ascii=False, indent=2), encoding="utf-8")
    return result


def _clip_dimensions(path: Path) -> tuple[int, int, str, str]:
    info = subprocess.run(
        ["ffprobe", "-v", "error", "-select_streams", "v:0",
         "-show_entries", "stream=width,height,codec_name,r_frame_rate",
         "-of", "json", str(path)], capture_output=True, text=True,
        check=True, timeout=20)
    rows = json.loads(info.stdout).get("streams") or []
    if not rows:
        raise ValueError("Не удалось определить размер видео: " + path.name)
    stream = rows[0]
    return (int(stream["width"]), int(stream["height"]),
            stream.get("codec_name", ""), stream.get("r_frame_rate", ""))


def compose_clips(segments: list[Path], output: Path, seconds: float, mode: str) -> None:
    """Join native-resolution original clips; use stream copy whenever safe."""
    if not segments:
        raise ValueError("Нет фрагментов для монтажа")
    for file in segments:
        if not file.is_file() or not file.stat().st_size:
            raise RuntimeError("Пустой фрагмент монтажа: " + file.name)
    formats = [_clip_dimensions(file) for file in segments]
    output.parent.mkdir(parents=True, exist_ok=True)
    # A stream-copy join avoids *all* generation loss in the final step.
    # Only uniform H.264, same size and frame rate can be concatenated.
    compatible = (all(fmt == formats[0] for fmt in formats) and
                  all(fmt[2] == "h264" for fmt in formats))
    if compatible:
        if len(segments) == 1:
            cmd = ["ffmpeg", "-nostdin", "-hide_banner", "-loglevel", "error",
                   "-y", "-i", str(segments[0])]
        else:
            playlist = output.with_suffix(".concat.txt")
            playlist.write_text("".join(
                "file '" + str(file.resolve()).replace("'", "'\\''") + "'\n"
                for file in segments), encoding="utf-8")
            cmd = ["ffmpeg", "-nostdin", "-hide_banner", "-loglevel", "error",
                   "-y", "-f", "concat", "-safe", "0", "-i", str(playlist)]
        cmd += ["-map", "0:v:0", "-t", f"{seconds:.3f}",
                "-c:v", "copy", "-movflags", "+faststart", str(output)]
        try:
            subprocess.run(cmd, check=True)
            print("MONTAGE HIGH QUALITY: source resolution "
                  f"{formats[0][0]}x{formats[0][1]}, stream copy",flush=True)
        except subprocess.CalledProcessError:
            print("MONTAGE: stream copy incompatible; falling back to re-encode",
                  flush=True)
            compatible = False
        finally:
            if len(segments) != 1:
                playlist.unlink(missing_ok=True)
    if not compatible:
        # Keep the largest native source dimensions instead of forcing 720p.
        width, height = max(((fmt[0], fmt[1]) for fmt in formats),
                            key=lambda dimension: dimension[0] * dimension[1])
        if width % 2: width += 1
        if height % 2: height += 1
        cmd = ["ffmpeg", "-nostdin", "-hide_banner", "-loglevel", "error", "-y"]
        for file in segments:
            cmd.extend(["-i", str(file)])
        filters = []
        refs = []
        from fractions import Fraction
        native_fps=max(
            (float(Fraction(fmt[3])) for fmt in formats if fmt[3]
             and fmt[3] not in ("0/0", "N/A")), default=30.0)
        target_fps=min(240.0,max(24.0,native_fps))
        for i in range(len(segments)):
            filters.append(
                f"[{i}:v:0]setpts=PTS-STARTPTS,fps={target_fps:.8f},"
                f"scale={width}:{height}:force_original_aspect_ratio=decrease:"
                f"flags=lanczos,pad={width}:{height}:(ow-iw)/2:(oh-ih)/2,"
                f"setsar=1,format=yuv420p[v{i}]")
            refs.append(f"[v{i}]")
        if len(segments) == 1:
            output_ref = "[v0]"
        else:
            filters.append("".join(refs) +
                           f"concat=n={len(segments)}:v=1:a=0[combined]")
            output_ref = "[combined]"
        from .montage_encoding import encode_ffmpeg
        def command(flags):
            return cmd + ["-filter_complex", ";".join(filters),
                          "-map", output_ref, "-t", f"{seconds:.3f}",
                          *flags, "-movflags", "+faststart", str(output)]
        encode_ffmpeg(command, seconds)
        print("MONTAGE HIGH QUALITY: composited at "
              f"{width}x{height} using high-fidelity H.264",flush=True)
    result = subprocess.run(
        ["ffprobe", "-v", "error", "-show_entries", "format=duration",
         "-of", "default=nw=1:nk=1", str(output)],
        capture_output=True, text=True, timeout=15, check=True)
    try:
        measured = float(result.stdout.strip())
    except ValueError as exc:
        raise RuntimeError("Не удалось измерить итоговый MP4") from exc
    if not 0 < measured <= seconds+.15:
        raise RuntimeError(f"Неверная длительность итогового видео: {measured:.2f} с")
    print(f"MIXED OUTPUT: {len(segments)} полных розыгрышей; "
          f"{measured:.2f}/{seconds:.2f} сек", flush=True)
