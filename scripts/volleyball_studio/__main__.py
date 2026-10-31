import argparse
from datetime import datetime
import importlib.metadata
import json
import math
import os
from pathlib import Path
import platform
import sys
import time

from .common import ROOT, VERSION, acquire_lock, cached, digest, discover, encoder, key, probe, progress_context, run, save, tools_present


def positive(value):
    number = float(value)
    if not math.isfinite(number) or number <= 0:
        raise argparse.ArgumentTypeError("Требуется положительное конечное число")
    return number


def parser():
    p = argparse.ArgumentParser(description="Волейбольная Reels Studio: локальный анализ и монтаж")
    p.add_argument("--input", type=Path, default=ROOT / "project")
    p.add_argument("--output", type=Path, default=ROOT / "studio_output")
    p.add_argument("--work", type=Path, default=ROOT / "studio_work")
    p.add_argument("--scratch-dir", type=Path)
    p.add_argument("--duration", type=positive, default=30.)
    p.add_argument("--request")
    p.add_argument("--plan-only", action="store_true")
    p.add_argument("--events", nargs="+", choices=["attack", "block", "serve"], default=[])
    p.add_argument("--long-rallies", action="store_true")
    p.add_argument("--event-confidence", type=positive, default=.75)
    p.add_argument("--min-rally", type=positive, default=4.)
    p.add_argument("--count", type=int, default=1)
    p.add_argument("--effects", choices=["off", "subtle", "cinematic"], default=None)
    p.add_argument("--style", choices=["classic", "dynamic", "cinematic"], default="dynamic")
    p.add_argument("--music", choices=["yes", "no"], default="no")
    p.add_argument("--music-file", type=Path)
    p.add_argument("--match-volume", type=float, default=1.)
    p.add_argument("--music-volume", type=float, default=.15)
    p.add_argument("--accent-volume", type=float, default=.5)
    p.add_argument("--accents", action="store_true")
    p.add_argument("--no-ducking", dest="ducking", action="store_false")
    p.add_argument("--preview-only", action="store_true")
    p.add_argument("--analyze-only", action="store_true")
    p.add_argument("--selection-file", type=Path)
    p.add_argument("--workers", type=int, default=0)
    p.add_argument("--benchmark", action="store_true")
    p.add_argument("--encoder", choices=["auto", "cpu", "nvenc"], default="auto")
    p.add_argument("--backend", choices=["onnx", "openvino"], default="onnx")
    p.add_argument("--threads", type=int, default=8)
    p.add_argument("--min-confidence", type=positive, default=.65)
    p.add_argument("--analysis-width", type=int, default=1280)
    p.add_argument("--analysis-seconds", type=positive)
    p.add_argument("--analysis-start", type=float, default=0.)
    p.add_argument("--width", type=int, default=1080)
    p.add_argument("--court-json", type=Path, default=None,
                   help="Explicit validated project-scoped court calibration")
    p.add_argument("--no-court-fallback", action="store_true",
                   help="Do not use global/adjacent court annotation files")
    p.add_argument("--beach", action="store_true")
    p.add_argument("--non-interactive", action="store_true")
    p.add_argument("--resume", type=Path)
    p.add_argument("--doctor", action="store_true")
    return p


def ask(args):
    for field, label, convert in (("duration", "Длительность ролика, сек", positive), ("min_rally", "Минимальный розыгрыш, сек", positive), ("count", "Количество роликов", int), ("style", "Стиль: classic/dynamic/cinematic", str), ("music", "Добавить музыку: yes/no", str)):
        while True:
            try:
                text = input(f"{label} [{getattr(args, field)}]: ").strip()
                value = convert(text.replace(",", ".")) if text else getattr(args, field)
                if field == "count" and value < 1:
                    raise ValueError()
                if field == "style" and value not in ("classic", "dynamic", "cinematic"):
                    raise ValueError()
                if field == "music" and value not in ("yes", "no"):
                    raise ValueError()
                setattr(args, field, value)
                break
            except (ValueError, argparse.ArgumentTypeError):
                print("Некорректное значение")


def doctor():
    tools_present()
    import cv2
    import numpy as np
    import onnxruntime as ort
    import openvino as ov
    sys.path.insert(0, str(ROOT / "src"))
    from inference_onnx_seq_gray_v2 import load_onnx_model
    report = {"platform": platform.platform(), "python": sys.version, "opencv": cv2.__version__, "ort": ort.__version__, "providers_advertised": ort.get_available_providers(), "openvino": ov.__version__, "ov_devices": ov.Core().available_devices, "ffmpeg": run(["ffmpeg", "-version"]).splitlines()[0], "encoder": encoder()[1]}
    started = time.monotonic()
    session, _, _, _, _, names, _, params = load_onnx_model(str(ROOT / "models/VballNetV4c_seq9_grayscale_20260908_213829.onnx"))
    data = np.zeros((1, params["input_seq"], params["input_height"], params["input_width"]), np.float32)
    for _ in range(3):
        session.run(None, {names[0]: data})
    report.update(providers_session=session.get_providers(), smoke_inference_seconds=time.monotonic() - started)
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return report


def main():
    args = parser().parse_args()
    from .request import parse_request, themed_candidates
    if args.request:
        for name, value in parse_request(args.request).items():
            option = "--" + name.replace("_", "-")
            if not any(token == option or token.startswith(option+"=") for token in sys.argv[1:]):
                setattr(args, name, value)
        args.non_interactive = True
    if args.plan_only:
        print(json.dumps(vars(args), ensure_ascii=False, default=str, indent=2))
        return
    from .resources import limit_cpus
    limit_cpus(args.threads)
    tools_present()
    if args.doctor:
        doctor()
        return
    if args.benchmark:
        from .benchmark import benchmark
        report, path = benchmark(args.input, args.work)
        print(json.dumps({"report": str(path), **report}, ensure_ascii=False, indent=2))
        return
    if not args.non_interactive:
        ask(args)
    from .styles import apply_style
    apply_style(args)
    if args.count < 1 or args.threads < 1 or args.duration <= 0 or args.min_rally <= 0 or args.min_confidence > 1 or args.event_confidence > 1 or args.width % 18 or args.width < 180 or args.analysis_width < 320 or args.analysis_width % 2 or not math.isfinite(args.analysis_start) or args.analysis_start < 0:
        raise ValueError("Некорректные параметры: width кратна 18, analysis-width чётная, count/threads >=1, confidence <=1")
    if any(not math.isfinite(v) or v < 0 or v > 4 for v in (args.match_volume, args.music_volume, args.accent_volume)):
        raise ValueError("Громкость должна быть в диапазоне 0..4")
    source_root = args.input.resolve()
    for destination in (args.work.resolve(), args.output.resolve()):
        if destination == source_root or source_root in destination.parents:
            raise ValueError("Выходные файлы и кэш должны находиться вне папки исходников")
    videos, audios = discover(source_root)
    if not videos:
        raise ValueError("Видео в папке исходников не найдены")
    music = args.music_file.resolve() if args.music_file else (audios[0] if audios and args.music == "yes" else None)
    missing_music = args.music == "yes" and not music
    if music and not music.is_file():
        raise FileNotFoundError(music)
    from .analysis import analyze, select
    from .render import assemble, render_segment
    from .preview import previews, review_filter
    from .quality import check, final_timeline
    from .resources import adaptive_workers, measure_resources, render_jobs
    args.work = args.work.resolve()
    args.work.mkdir(parents=True, exist_ok=True)
    lock = acquire_lock(args.work)
    signature = key(["studio-edit-v2", {k: str(v) for k, v in vars(args).items() if k not in ("resume", "non_interactive")}, [[str(p), p.stat().st_size, p.stat().st_mtime_ns] for p in videos], digest(music) if music else None, digest(args.selection_file) if args.selection_file else None])
    if args.resume:
        result = args.resume.resolve()
        previous = json.loads((result / "manifest.json").read_text())
        if previous["signature"] != signature:
            raise ValueError("Параметры или исходники изменились; запустите новый запуск без --resume")
    else:
        result = args.output.resolve() / (datetime.now().strftime("%Y%m%d_%H%M%S_%f") + "_" + signature[:6])
        result.mkdir(parents=True, exist_ok=False)
    manifest = {"version": VERSION, "signature": signature, "status": "running", "options": {k: str(v) for k, v in vars(args).items()}, "outputs": [], "warnings": [], "errors": [], "analysis": []}
    manifest["resources_start"] = measure_resources()
    if missing_music:
        manifest["warnings"].append("Музыка не найдена: используется исходный звук")
    save(result / "manifest.json", manifest)
    started = time.monotonic()
    try:
        print(json.dumps({"stage": "A Discovery", "overall_percent": 3, "videos_total": len(videos), "audio": len(audios), "result": str(result)}), flush=True)
        candidates = []
        for number, video in enumerate(videos, 1):
            progress_context(stage="B/C", video=video.name, video_number=number, videos_total=len(videos), overall_percent=round(3+50*(number-1)/len(videos), 1))
            print(json.dumps({"stage": "B Detection / C Game Analysis", "video": video.name, "video_number": number, "videos_total": len(videos), "overall_percent": round(3+50*(number-1)/len(videos), 1), "backend": args.backend, "players_backend": "OpenVINO CPU"}), flush=True)
            try:
                items, warnings, cache = analyze(video, args.work, args)
                candidates.extend(items)
                manifest["warnings"].extend(warnings)
                manifest["analysis"].append({"source": str(video), "cache": str(cache), "candidates": len(items)})
                per_video = result / "videos" / (video.stem + "_" + key(str(video))[:8])
                save(per_video / "analysis/index.json", {"source": str(video), "ball_csv": str(cache / "ball/analysis/ball.csv"), "players": str(cache / "players.json"), "events": str(cache / "analysis/events.json"), "tracks": str(cache / "tracks/analysis/tracks")})
                save(per_video / "reports/processing.json", {"status": "analyzed", "cache": str(cache), "source": str(video), "candidates": len(items)})
                print(json.dumps({"stage": "C Game Analysis", "videos_done": number, "videos_total": len(videos), "highlights": len(candidates), "overall_percent": round(3+50*number/len(videos), 1)}), flush=True)
            except Exception as error:
                manifest["errors"].append({"source": str(video), "error": str(error)})
                save(result / "videos" / (video.stem + "_" + key(str(video))[:8]) / "reports/processing.json", {"status": "failed", "source": str(video), "error": str(error)})
                print(f"Ошибка анализа {video.name}: {error}", flush=True)
            save(result / "manifest.json", manifest)
        candidates = themed_candidates(candidates, args.events, args.event_confidence, args.long_rallies)
        if args.events and not candidates:
            message = "Недостаточно уверенных событий " + str(args.events) + "; другие игровые события не подставляются."
            manifest["warnings"].append(message)
            print(message, flush=True)
        manifest["selection_request"] = {"events": args.events, "event_confidence": args.event_confidence, "long_rallies": args.long_rallies, "duration": args.duration, "count": args.count}
        previews(candidates, result / "previews")
        if args.analyze_only or args.preview_only:
            manifest.update(status="partial" if manifest["errors"] else "analyzed" if args.analyze_only else "preview_ready", seconds=time.monotonic()-started, previews=str(result / "previews/index.html"))
            save(result / "manifest.json", manifest)
            print(json.dumps({"status": manifest["status"], "result": str(result), "candidates": len(candidates)}, ensure_ascii=False))
            if manifest["errors"]:
                raise SystemExit(2)
            return
        candidates = review_filter(candidates, args.selection_file)
        groups = select(candidates, args.duration, args.count)
        from .composition import protect_timing
        groups = protect_timing(groups, args.duration, args)
        save(result / "selection.json", [[{"id": c["id"], "source": c["source"], "start_frame": c["start"], "end_frame": c["end"], "score": c["score"]} for c in group] for group in groups])
        save(result / "reports/D.status.json", {"status": "complete", "selected": sum(map(len, groups))})
        print(json.dumps({"stage": "D Highlight Selection", "selected": sum(map(len, groups)), "reels": len(groups), "overall_percent": 60}), flush=True)
        if not groups:
            raise RuntimeError("Недостаточно уверенных событий для запроса " + str(args.events or "розыгрыши") + ". Подмена другими событиями не выполняется.")
        if len(groups) < args.count:
            manifest["warnings"].append(f"Достаточно материала для {len(groups)} из {args.count} роликов")
        codec = encoder(args.encoder)
        manifest["encoder"] = codec[1]
        manifest["workers"] = adaptive_workers(args.workers, args.threads, codec[1])
        segment_dir = args.work / "renders" / signature
        segment_dir.mkdir(parents=True, exist_ok=True)
        jobs = []
        for index, group in enumerate(groups, 1):
            for j, item in enumerate(group):
                jobs.append((item, str(segment_dir / f"reel{index}_segment{j}.mp4"), vars(args), codec))
        render_jobs(jobs, manifest["workers"])
        for stage in ("E", "F"):
            save(result / f"reports/{stage}.status.json", {"status": "complete", "segments": [job[1] for job in jobs]})
        for index, group in enumerate(groups, 1):
            segments, hits, offset = [], [], 0.
            for j, item in enumerate(group):
                segment_dir = args.work / "renders" / signature
                segment_dir.mkdir(parents=True, exist_ok=True)
                segment = segment_dir / f"reel{index}_segment{j}.mp4"
                timeline = json.loads(segment.with_suffix(".timeline.json").read_text())
                hits.extend(offset + e["output_frame"] / 30 for e in timeline["effects"] if e["flash"])
                offset += timeline["duration"]
                segments.append(segment)
            print(json.dumps({"stage": "G Audio / H Rendering / I Quality Control", "reel": index, "reels_total": len(groups), "overall_percent": round(80+20*(index-1)/len(groups), 1), "codec": codec[1]}), flush=True)
            render_root = result / "renders" / args.style
            render_root.mkdir(parents=True, exist_ok=True)
            final = render_root / f"reel_{index:02d}.mp4"
            final_key = key([signature, [digest(p) for p in segments]])

            def export():
                partial = render_root / f"reel_{index:02d}.partial.mp4"
                for stage in ("G", "H"):
                    save(result / f"reports/reel_{index:02d}.{stage}.status.json", {"status": "running", "operation": "fused FFmpeg audio and encode graph"})
                info = assemble(segments, partial, music, args, codec, sorted(set(round(h, 1) for h in hits)))
                for stage in ("G", "H"):
                    save(result / f"reports/reel_{index:02d}.{stage}.status.json", {"status": "complete", "operation": "fused FFmpeg audio and encode graph"})
                save(result / f"reports/reel_{index:02d}.I.status.json", {"status": "running"})
                timeline = final_timeline(segments, float(info["format"]["duration"]))
                quality = check(partial, timeline, args.duration, args.min_confidence)
                save(result / f"reports/reel_{index:02d}.quality.json", quality)
                if not quality["passed"]:
                    info = assemble(segments, partial, music, args, encoder("cpu"), sorted(set(round(h, 1) for h in hits)))
                    timeline = final_timeline(segments, float(info["format"]["duration"]))
                    quality = check(partial, timeline, args.duration, args.min_confidence)
                    quality["repair_attempt"] = "re-encode final with libx264"
                    save(result / f"reports/reel_{index:02d}.quality.json", quality)
                    if not quality["passed"]:
                        save(result / f"reports/reel_{index:02d}.I.status.json", {"status": "failed"})
                        raise RuntimeError("QC не пройден: " + str(quality["checks"]))
                save(result / f"reports/reel_{index:02d}.I.status.json", {"status": "complete"})
                if final.exists():
                    raise FileExistsError(f"Результат уже существует и не подтверждён кэшем: {final}")
                partial.rename(final)
                save(final.with_suffix(".probe.json"), info)
                save(final.with_suffix(".timeline.json"), timeline)
                return [final, final.with_suffix(".probe.json"), final.with_suffix(".timeline.json"), result / f"reports/reel_{index:02d}.quality.json"]

            cached(result, f"export{index}", final_key, export)
            actual = float(probe(final)["format"]["duration"])
            if actual < args.duration - .5:
                manifest["warnings"].append(f"Ролик {index}: {actual:.2f} с при запросе {args.duration} с; недостаточно уверенных эпизодов")
            manifest["outputs"].append({"path": str(final), "duration": actual, "segments": [str(p) for p in segments], "selected": [{k: v for k, v in x.items() if k not in ("track", "ball", "players", "ball_rows")} for x in group]})
            save(result / "manifest.json", manifest)
            print(json.dumps({"stage": "I Quality Control", "ready_reels": index, "reels_total": len(groups), "overall_percent": round(80+20*index/len(groups), 1), "output": str(final)}), flush=True)
        manifest.update(status="partial" if manifest["errors"] else "complete", seconds=time.monotonic() - started)
        hashes = [digest(Path(item["path"])) for item in manifest["outputs"]]
        if len(set(hashes)) != len(hashes):
            raise RuntimeError("QC: обнаружены одинаковые итоговые ролики")
        manifest["resources_end"] = measure_resources()
        save(result / "manifest.json", manifest)
        print(json.dumps({"status": manifest["status"], "ready": len(groups), "errors": len(manifest["errors"]), "seconds": manifest["seconds"], "result": str(result)}, ensure_ascii=False))
        if manifest["errors"]:
            raise SystemExit(2)
    except BaseException as error:
        if isinstance(error, SystemExit) and error.code == 2 and manifest["status"] == "partial":
            raise
        manifest.update(status="interrupted" if isinstance(error, KeyboardInterrupt) else "failed", last_error=str(error), seconds=time.monotonic() - started)
        save(result / "manifest.json", manifest)
        raise
    finally:
        lock.close()


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        print("Прервано. Готовые этапы сохранены; используйте --resume.", file=sys.stderr)
        raise SystemExit(130)
    except Exception as error:
        print(str(error), file=sys.stderr)
        raise SystemExit(1)
