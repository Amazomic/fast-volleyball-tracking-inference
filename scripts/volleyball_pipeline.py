#!/usr/bin/env python3
"""End-to-end volleyball video -> ball tracking -> rallies -> vertical reels.

Launch under WSL from the fast-volleyball-tracking-inference repository root:
    .venv/bin/python scripts/volleyball_pipeline.py

Uses the repository's own ONNX inference and track calculation code.
Caches complete ball.csv and track JSON files; creates parallel CPU reels.
"""

import os
# Avoid thread oversubscription when processing several clips in parallel.
os.environ.setdefault("OMP_NUM_THREADS", "1")
os.environ.setdefault("OPENBLAS_NUM_THREADS", "1")
os.environ.setdefault("MKL_NUM_THREADS", "1")

import argparse
from concurrent.futures import ProcessPoolExecutor, as_completed
from dataclasses import dataclass
import json
import math
import csv
import re
from multiprocessing import get_context
from pathlib import Path
import shutil
import subprocess
import sys
import time

import cv2
import numpy as np

# Initialized once per worker, rather than re-opening the large video per clip.
CAPTURE = None


@dataclass(frozen=True)
class Job:
    source: str
    destination: str
    start_frame: int
    end_frame: int
    positions: tuple
    source_width: int
    source_height: int


def prompt_seconds(title: str, default: float) -> float:
    while True:
        value = input(f"{title} [{default:g}]: ").strip().replace(",", ".")
        try:
            result = float(value) if value else default
            if math.isfinite(result) and result > 0:
                return result
        except ValueError:
            pass
        print("Введите положительное число, например 5 или 12.")


def video_properties(path: Path):
    cap = cv2.VideoCapture(str(path))
    if not cap.isOpened():
        raise RuntimeError(f"Невозможно открыть видео: {path}")
    try:
        fps = float(cap.get(cv2.CAP_PROP_FPS))
        width = int(cap.get(cv2.CAP_PROP_FRAME_WIDTH))
        height = int(cap.get(cv2.CAP_PROP_FRAME_HEIGHT))
        total = int(cap.get(cv2.CAP_PROP_FRAME_COUNT))
    finally:
        cap.release()
    if not math.isfinite(fps) or fps <= 0 or width < 4 or height < 4 or total <= 0:
        raise ValueError("Ошибка чтения FPS, размеров или количества кадров видео")
    return fps, width, height, total


def clip_span(start: int, end: int, wanted: int, total: int):
    """Fit requested duration around the middle of the original track."""
    count = min(total, max(1, wanted))
    center = (start + end) // 2
    first = max(0, min(center - count // 2, total - count))
    return first, first + count - 1


def read_jobs(tracks: Path, video: Path, fps: float, min_rally: float, dest: Path):
    all_files = sorted(tracks.glob("track_*.json"))
    jobs = []
    rejected = 0
    invalid = 0
    for item in all_files:
        try:
            data = json.loads(item.read_text(encoding="utf-8"))
            start, end = int(data["start_frame"]), int(data["last_frame"])
            if start < 0 or end < start:
                raise ValueError("invalid track bounds")
            points = []
            for value in data.get("positions", []):
                try:
                    xy, frame = value[:2]
                    points.append((float(xy[0]), float(xy[1]), int(frame)))
                except (ValueError, TypeError, IndexError):
                    continue
            if not points:
                raise ValueError("empty ball coordinates")
            # The upstream calculator extends start/end with context frames.
            # Filter by actual position times instead of the padded segment.
            position_frames = [f for _, _, f in points]
            observed_span_seconds = (max(position_frames) - min(position_frames) + 1) / fps
            if observed_span_seconds < min_rally:
                rejected += 1
                continue
            scaling = data.get("tracking_scale") or {}
            court = data.get("court_info") or {}
            sw = int(scaling.get("frame_width_px") or court.get("image_width") or 0)
            sh = int(scaling.get("frame_height_px") or court.get("image_height") or 0)
            name = f"reel_{video.stem}_{item.stem.rsplit('_', 1)[-1]}.mp4"
            jobs.append(Job(str(item), str(dest / name), start, end, tuple(points), sw, sh))
        except (OSError, TypeError, ValueError, KeyError, json.JSONDecodeError) as exc:
            invalid += 1
            print(f"  ⚠️ Невалидный трек {item.name}: {exc}", flush=True)
    jobs.sort(key=lambda j: (j.start_frame, j.source))
    return len(all_files), rejected, invalid, jobs


def smooth_x(values: np.ndarray, method: str):
    if method == "none" or len(values) < 2:
        return values
    if method == "moving_avg":
        window = min(15, len(values))
        if window % 2 == 0:
            window = max(1, window - 1)
        pad = window // 2
        return np.convolve(np.pad(values, (pad, pad), mode="edge"),
                           np.ones(window) / window, mode="valid")
    if method == "kalman":
        result = np.empty(len(values))
        x, p = values[0], 1.0
        for i, measurement in enumerate(values):
            p += 1e-3
            k = p / (p + 1e-1)
            x += k * (measurement - x)
            p *= 1 - k
            result[i] = x
        return result
    if method == "savitzky_golay":
        from scipy.signal import savgol_filter
        window = min(15, len(values) if len(values) % 2 else len(values) - 1)
        if window < 3:
            return values
        return savgol_filter(values, window, 2, mode="interp")
    raise ValueError(f"Unknown smoothing method: {method}")


def worker_init(video: str):
    global CAPTURE
    cv2.setNumThreads(1)
    CAPTURE = cv2.VideoCapture(video)
    if not CAPTURE.isOpened():
        raise RuntimeError(f"Не удалось открыть исходное видео: {video}")


def export_clip(job: Job, fps: float, width: int, height: int, total: int,
                duration_frames: int, smoothing: str, interpolation: str,
                margin: float, has_ffmpeg: bool):
    global CAPTURE
    start = time.monotonic()
    if CAPTURE is None or not CAPTURE.isOpened():
        raise RuntimeError("Видеодекодер рабочего процесса не открыт")
    first, last = clip_span(job.start_frame, job.end_frame, duration_frames, total)
    known = sorted((frame, x) for x, _, frame in job.positions)
    frames = np.arange(first, last + 1)
    known_idx = np.array([i for i, _ in known])
    known_x = np.array([x for _, x in known])
    if job.source_width > 0:
        known_x = known_x * (width / job.source_width)
    if interpolation == "linear":
        x = np.interp(frames, known_idx, known_x)
    else:
        x = np.empty(len(frames))
        j = 0
        last_x = known_x[0]
        for i, frame in enumerate(frames):
            while j < len(known_idx) and known_idx[j] <= frame:
                last_x = known_x[j]
                j += 1
            x[i] = last_x
    x = smooth_x(x, smoothing)

    out_w = min(width, int(height * 9 / 16)) // 2 * 2
    out_h = height // 2 * 2
    if out_w < 2 or out_h < 2:
        raise RuntimeError("Слишком маленькое видео")
    dest = Path(job.destination)
    partial = dest.with_name(dest.stem + ".partial.mp4")
    partial.unlink(missing_ok=True)
    process = None
    writer = None
    CAPTURE.set(cv2.CAP_PROP_POS_FRAMES, first)
    count = 0
    try:
        if has_ffmpeg:
            command = ["ffmpeg", "-hide_banner", "-loglevel", "error", "-nostdin", "-y",
                       "-f", "rawvideo", "-pix_fmt", "bgr24", "-s:v", f"{out_w}x{out_h}",
                       "-r", f"{fps:.8f}", "-i", "pipe:0", "-an", "-c:v", "libx264",
                       "-preset", "veryfast", "-crf", "23", "-threads", "1",
                       "-pix_fmt", "yuv420p", "-movflags", "+faststart", str(partial)]
            process = subprocess.Popen(command, stdin=subprocess.PIPE, stderr=subprocess.PIPE)
        else:
            writer = cv2.VideoWriter(str(partial), cv2.VideoWriter_fourcc(*"mp4v"), fps,
                                     (out_w, out_h))
            if not writer.isOpened():
                raise RuntimeError("Не удалось открыть VideoWriter (mp4v)")
        for i in range(last - first + 1):
            ok, frame = CAPTURE.read()
            if not ok:
                raise RuntimeError(f"Не удалось прочитать кадр {first + i}")
            cx = int(x[i])
            if margin and i > 0:
                cx += int(margin * np.sign(x[i] - x[i - 1]))
            left = max(0, min(cx - out_w // 2, width - out_w))
            crop = np.ascontiguousarray(frame[:out_h, left:left + out_w])
            if crop.shape != (out_h, out_w, 3):
                raise RuntimeError(f"Неправильный размер кадра {crop.shape}")
            if process:
                assert process.stdin is not None
                process.stdin.write(memoryview(crop))
            else:
                writer.write(crop)
            count += 1
        if process:
            assert process.stdin is not None
            process.stdin.close()
            assert process.stderr is not None
            errors = process.stderr.read().decode("utf-8", errors="replace")
            code = process.wait()
            if code:
                raise RuntimeError(f"Ошибка ffmpeg (код {code}): {errors[-500:]}")
        if writer:
            writer.release()
        if not partial.exists() or partial.stat().st_size < 128:
            raise RuntimeError("Результат пустой")
        partial.replace(dest)
        return dest.name, count, time.monotonic() - start
    except Exception:
        if process and process.poll() is None:
            process.kill()
            process.wait()
        if process:
            if process.stdin and not process.stdin.closed:
                process.stdin.close()
            if process.stderr:
                process.stderr.close()
        if writer:
            writer.release()
        partial.unlink(missing_ok=True)
        raise



VIDEO_EXTENSIONS = {".mp4", ".mov", ".mkv", ".avi", ".m4v"}


def elapsed_text(seconds: float) -> str:
    if not math.isfinite(seconds):
        return "?"
    secs = max(0, int(seconds))
    return f"{secs // 3600:02d}:{secs // 60 % 60:02d}:{secs % 60:02d}"


def ensure_valid_args(parser: argparse.ArgumentParser, args):
    for n in ("duration", "min_rally"):
        x = getattr(args, n)
        if x is not None and (not math.isfinite(x) or x <= 0):
            parser.error(f"--{n.replace('_', '-')} должен быть положительным числом")
    if not math.isfinite(args.margin):
        parser.error("--margin должен быть конечным числом")
    if args.workers < 0:
        parser.error("--workers должен быть >= 0")


def csv_is_complete(csv_path: Path, expected_frames: int) -> bool:
    """Require exactly one record for every video frame, in order.

    Verifies reused results made by the original detector, including incomplete
    files that can appear after a terminated inference run.
    """
    if not csv_path.is_file() or csv_path.stat().st_size < 100 or expected_frames <= 0:
        return False
    try:
        with csv_path.open(encoding="utf-8", newline="") as f:
            rdr = csv.DictReader(f)
            if not {"Frame", "Visibility", "X", "Y"}.issubset(rdr.fieldnames or []):
                return False
            seen = 0
            for row in rdr:
                if int(row["Frame"]) != seen:
                    return False
                seen += 1
            return seen == expected_frames
    except (OSError, UnicodeError, ValueError, KeyError, csv.Error):
        return False


def launch_stage(argv: list[str], title: str, repo: Path):
    print("\n" + title, flush=True)
    print("Команда:", " ".join(argv), flush=True)
    env = os.environ.copy()
    env.setdefault("PYTHONUNBUFFERED", "1")
    proc = subprocess.Popen(argv, cwd=str(repo), env=env)
    started = time.monotonic()
    while True:
        try:
            code = proc.wait(timeout=30)
            break
        except subprocess.TimeoutExpired:
            print(f"  ⏳ Этап выполняется {elapsed_text(time.monotonic() - started)}...", flush=True)
        except KeyboardInterrupt:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait()
            raise
    if code:
        raise RuntimeError(f"Этап завершился с ошибкой, код {code}")


def detector_backend(model_path: Path):
    """Report the providers selected for the exact model (not merely installed)."""
    try:
        import onnxruntime as ort
        if hasattr(ort, "preload_dlls"):
            ort.preload_dlls()
        session = ort.InferenceSession(str(model_path), providers=[
            "CUDAExecutionProvider", "CPUExecutionProvider"
        ])
        providers = session.get_providers()
        return "CUDA + CPU fallback" if "CUDAExecutionProvider" in providers else "CPU (CUDA недоступна)"
    except Exception as e:
        # Give the project's own detector the final say and a full error message.
        return f"Не удалось проверить провайдер ({str(e)[:120]}); проверит исходный скрипт"


def track_files_usable(tracks_dir: Path, csv_file: Path) -> bool:
    """For legacy runs no manifest exists: reuse if recent, non-empty JSON exists."""
    try:
        files = sorted(tracks_dir.glob("track_*.json"))
        if not files or not csv_file.is_file():
            return False
        if min(f.stat().st_mtime_ns for f in files) < csv_file.stat().st_mtime_ns:
            return False
        sample = json.loads(files[0].read_text(encoding="utf-8"))
        return all(k in sample for k in ("positions", "start_frame", "last_frame"))
    except (OSError, ValueError, KeyError, TypeError):
        return False


def video_list(video_dir: Path) -> list[Path]:
    if not video_dir.is_dir():
        raise RuntimeError(f"Папка с видео не найдена: {video_dir}")
    return sorted((p for p in video_dir.iterdir()
                   if p.is_file() and p.suffix.lower() in VIDEO_EXTENSIONS),
                  key=lambda p: p.name.casefold())


def render_many(video: Path, tracks: Path, output: Path, fps: float, width: int,
                height: int, total_frames: int, duration: float, min_rally: float,
                workers: int, smoothing: str, interpolation: str, margin: float):
    output.mkdir(parents=True, exist_ok=True)
    total_tracks, rejected, invalid, jobs = read_jobs(tracks, video, fps, min_rally, output)
    existing = [j for j in jobs if Path(j.destination).is_file()
                and Path(j.destination).stat().st_size > 1024
                and Path(j.destination).stat().st_mtime_ns >= Path(j.source).stat().st_mtime_ns
                and Path(j.destination).stat().st_mtime_ns >= video.stat().st_mtime_ns]
    done = set(existing)
    pending = [j for j in jobs if j not in done]
    encode = "FFmpeg libx264 (CPU)" if shutil.which("ffmpeg") else "OpenCV mp4v (CPU)"

    print(f"  JSON-траекторий: {total_tracks}")
    print(f"  Розыгрышей от {min_rally:g} с: {len(jobs)}")
    print(f"  Отсеяно коротких: {rejected} | Повреждённых JSON: {invalid}")
    print(f"  Видео для создания: {len(pending)} | Уже готово: {len(existing)}")
    print(f"  Режим: {workers} CPU-процессов | Кодирование: {encode}")
    print(f"  Результат: {output}", flush=True)
    if not jobs:
        print("  ⚠️ Подходящих розыгрышей не найдено.", flush=True)
        return {"tracks": total_tracks, "eligible": 0, "existing": 0,
                "created": 0, "failed": 0, "short": rejected, "invalid": invalid}
    if not pending:
        print(f"  ✅ Все {len(existing)} роликов уже созданы.", flush=True)
        return {"tracks": total_tracks, "eligible": len(jobs), "existing": len(existing),
                "created": 0, "failed": 0, "short": rejected, "invalid": invalid}

    start = time.monotonic()
    successes = 0
    failures = 0
    duration_frames = max(1, round(duration * fps))
    with ProcessPoolExecutor(max_workers=workers, mp_context=get_context("spawn"),
                             initializer=worker_init, initargs=(str(video),)) as pool:
        futures = {
            pool.submit(export_clip, j, fps, width, height, total_frames,
                        duration_frames, smoothing, interpolation, margin,
                        shutil.which("ffmpeg") is not None): j
            for j in pending
        }
        for future in as_completed(futures):
            elapsed = time.monotonic() - start
            completed = successes + failures + 1
            try:
                name, frames, seconds = future.result()
                successes += 1
                result = f"✓ {name} ({frames} кадров, {seconds:.1f}с)"
            except Exception as exc:
                failures += 1
                result = f"✗ {Path(futures[future].source).name}: {str(exc)[:160]}"
            remaining = len(pending) - completed
            eta = elapsed / max(1, completed) * remaining
            print(f"  [{len(existing)+completed:>4}/{len(jobs)} | "
                  f"{(len(existing)+completed)/len(jobs):5.1%}] "
                  f"{result} | осталось ~{elapsed_text(eta)}", flush=True)
    print(f"  ✅ Новых роликов: {successes}; ошибок: {failures}; "
          f"прошло {elapsed_text(time.monotonic()-start)}", flush=True)
    return {"tracks": total_tracks, "eligible": len(jobs), "existing": len(existing),
            "created": successes, "failed": failures, "short": rejected, "invalid": invalid}


def main() -> int:
    parser = argparse.ArgumentParser(description="Полный цикл: много видео → нейросеть → траектории → вертикальные Reels")
    parser.add_argument("--input-dir", type=Path, help="Папка с исходными видео; по умолчанию ./project")
    parser.add_argument("--output-dir", type=Path, help="Корень результатов; по умолчанию ./output")
    parser.add_argument("--model", type=Path, help="ONNX-модель, по умолчанию VballNetV4c")
    parser.add_argument("--duration", type=float, help="Целевая длительность каждого Reels в секундах")
    parser.add_argument("--min-rally", type=float, help="Минимальная длительность найденного розыгрыша")
    parser.add_argument("--workers", type=int, default=0, help="Число CPU-процессов нарезки, 0 = автоматически")
    parser.add_argument("--smoothing", choices=["moving_avg", "kalman", "savitzky_golay", "none"], default="moving_avg")
    parser.add_argument("--interpolation", choices=["linear", "hold"], default="linear")
    parser.add_argument("--margin", type=float, default=100)
    parser.add_argument("--force-tracks", action="store_true", help="Пересчитать траектории даже при наличии JSON")
    parser.add_argument("--force-detection", action="store_true", help="Запустить поиск мяча заново (долго)")
    args = parser.parse_args()
    ensure_valid_args(parser, args)

    repo = Path.cwd()
    if not (repo / "src" / "inference_onnx_seq_gray_v2.py").is_file():
        repo = Path(__file__).resolve().parent.parent
    if not (repo / "src" / "inference_onnx_seq_gray_v2.py").is_file():
        parser.error("Запусти из корня fast-volleyball-tracking-inference или положи файл в scripts/")
    repo = repo.resolve()
    input_dir = (args.input_dir or repo / "project").resolve()
    output_root = (args.output_dir or repo / "output").resolve()
    model = (args.model or repo / "models/VballNetV4c_seq9_grayscale_20260908_213829.onnx").resolve()
    if not model.is_file():
        parser.error(f"Модель не найдена: {model}")
    videos = video_list(input_dir)
    if not videos:
        parser.error(f"Нет MP4/MOV/MKV/AVI/M4V в папке: {input_dir}")

    stems = [v.stem.casefold() for v in videos]
    if len(stems) != len(set(stems)):
        parser.error("Обнаружены два видео с одинаковыми именами, но разными расширениями. Переименуй один из них")

    duration = args.duration if args.duration is not None else (
        prompt_seconds("Желаемая длительность вертикального ролика (сек)", 12)
        if sys.stdin.isatty() else 12.0)
    min_rally = args.min_rally if args.min_rally is not None else (
        prompt_seconds("Минимальная длительность розыгрыша (сек)", 5)
        if sys.stdin.isatty() else 5.0)
    ncpu = len(os.sched_getaffinity(0)) if hasattr(os, "sched_getaffinity") else (os.cpu_count() or 2)
    workers = args.workers or max(1, min(6, ncpu // 2))
    if workers > ncpu:
        parser.error(f"Слишком много процессов: --workers={workers}, доступно CPU: {ncpu}")

    print("\n" + "=" * 74)
    print("VOLLEYBALL PIPELINE — ОБРАБОТКА ВСЕХ ВИДЕО")
    print("=" * 74)
    print(f"Найдено исходных видео: {len(videos)}")
    for i, video in enumerate(videos, 1):
        print(f"  {i}. {video.name}")
    print(f"Модель: {model.name}")
    print(f"Ролик: {duration:g} с | Минимальный розыгрыш: {min_rally:g} с")
    print(f"Плавность: {args.smoothing} | Интерполяция: {args.interpolation} | "
          f"Смещение: {args.margin:g} px")
    print(f"CPU-процессов для монтажа: {workers}/{ncpu}")
    print("Звук: не переносится (как в исходном make_reels.py)")
    print("=" * 74, flush=True)

    tag = f"d{duration:g}_min{min_rally:g}_{args.smoothing}_{args.interpolation}_m{args.margin:g}"
    totals = {"videos_ok": 0, "videos_failed": 0, "tracks": 0,
              "eligible": 0, "new_reels": 0, "old_reels": 0, "errors": 0}
    started = time.monotonic()
    for index, video in enumerate(videos, 1):
        print(f"\n{'#'*74}\nВИДЕО {index}/{len(videos)}: {video.name}\n{'#'*74}", flush=True)
        try:
            fps, width, height, frames = video_properties(video)
            print(f"Источник: {width}x{height}, {fps:.3f} FPS, {frames} кадров")
            video_out = output_root / video.stem
            video_out.mkdir(parents=True, exist_ok=True)
            csv_path = video_out / "ball.csv"
            tracks = video_out / "tracks"

            print("\n[ЭТАП 1/3] Поиск мяча (нейросеть)", flush=True)
            if not args.force_detection and csv_is_complete(csv_path, frames):
                print(f"  ✅ Полный ball.csv уже есть ({frames} кадров). Повторно не считаем.", flush=True)
            else:
                backend = detector_backend(model)
                print(f"  Устройство инференса: {backend}", flush=True)
                launch_stage([sys.executable, "src/inference_onnx_seq_gray_v2.py",
                              "--video_path", str(video), "--model_path", str(model),
                              "--output_dir", str(output_root), "--only_csv"],
                             "  Обработка кадров:", repo)
                if not csv_is_complete(csv_path, frames):
                    raise RuntimeError(f"Неполный ball.csv после инференса: {csv_path}")
                print(f"  ✅ Найдены координаты для всех {frames} кадров", flush=True)

            print("\n[ЭТАП 2/3] Поиск и сохранение розыгрышей", flush=True)
            if not args.force_tracks and not args.force_detection and track_files_usable(tracks, csv_path):
                print(f"  ✅ Используем готовые {len(list(tracks.glob('track_*.json')))} JSON-траектории.", flush=True)
                print("  Примечание: при неизвестных настройках старого запуска возможна потеря ранее отсечённых эпизодов.")
            else:
                # Low internal filter (1s) so users can change the export filter
                # without re-running heavy track detection each time.
                launch_stage([sys.executable, "src/track_calculator.py", "--csv_path", str(csv_path),
                              "--output_dir", str(output_root), "--fps", str(fps),
                              "--video_width", str(width), "--video_height", str(height),
                              "--min_duration_sec", "1"],
                             "  Анализ траекторий:", repo)
                print(f"  ✅ Траекторий: {len(list(tracks.glob('track_*.json')))}", flush=True)

            print("\n[ЭТАП 3/3] Вертикальные Reels — параллельно на CPU", flush=True)
            dest = video_out / f"reels_cpu_{tag}"
            results = render_many(video, tracks, dest, fps, width, height, frames,
                                  duration, min_rally, workers, args.smoothing,
                                  args.interpolation, args.margin)
            totals["videos_ok"] += 1
            totals["tracks"] += results["tracks"]
            totals["eligible"] += results["eligible"]
            totals["new_reels"] += results["created"]
            totals["old_reels"] += results["existing"]
            totals["errors"] += results["failed"] + results["invalid"]
            if results["failed"]:
                totals["videos_failed"] += 1
            print(f"\n✅ Видео {index}/{len(videos)} завершено. "
                  f"Новых: {results['created']}, повторно использовано: {results['existing']}, "
                  f"ошибок: {results['failed']}", flush=True)
        except KeyboardInterrupt:
            print("\nОстановлено пользователем. Готовые MP4 сохраняются.", flush=True)
            return 130
        except Exception as e:
            totals["videos_failed"] += 1
            print(f"\n❌ Ошибка видео {video.name}: {e}", flush=True)

    print("\n" + "=" * 74)
    print("ИТОГОВЫЙ ОТЧЁТ")
    print(f"Всего видео:           {len(videos)}")
    print(f"С ошибками:            {totals['videos_failed']}")
    print(f"Траекторий найдено:    {totals['tracks']}")
    print(f"Подходящих розыгрышей: {totals['eligible']}")
    print(f"Новых MP4:             {totals['new_reels']}")
    print(f"Ранее созданных MP4:   {totals['old_reels']}")
    print(f"Ошибок роликов/JSON:   {totals['errors']}")
    print(f"Времени:               {elapsed_text(time.monotonic()-started)}")
    print(f"Все результаты:        {output_root}")
    print("=" * 74, flush=True)
    return 1 if totals["videos_failed"] or totals["errors"] else 0


if __name__ == "__main__":
    sys.exit(main())
