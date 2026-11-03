#!/usr/bin/env python3
"""Parallel CPU exporter for fast-volleyball-tracking-inference (WSL/Linux).

Does not change the project's original scripts or regenerate ball.csv/tracks.
Each worker owns one persistent video decoder and renders a series of independent
vertical clips using libx264 (or OpenCV mp4v when ffmpeg is not available).
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
            if (end - start + 1) / fps < min_rally:
                rejected += 1
                continue
            points = []
            for value in data.get("positions", []):
                try:
                    xy, frame = value[:2]
                    points.append((float(xy[0]), float(xy[1]), int(frame)))
                except (ValueError, TypeError, IndexError):
                    continue
            if not points:
                raise ValueError("empty ball coordinates")
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


def cli():
    parser = argparse.ArgumentParser(description="Параллельное создание вертикальных волейбольных роликов на CPU")
    parser.add_argument("--video", type=Path)
    parser.add_argument("--tracks", type=Path)
    parser.add_argument("--output", type=Path, help="Папка для новых роликов")
    parser.add_argument("--duration", type=float, help="Желаемая длительность ролика, секунд")
    parser.add_argument("--min-rally", type=float, help="Минимальная длина найденного розыгрыша, секунд")
    parser.add_argument("--workers", type=int, default=0, help="Число параллельных процессов (0 = автоматически)")
    parser.add_argument("--smoothing", choices=["none", "moving_avg", "kalman", "savitzky_golay"], default="moving_avg")
    parser.add_argument("--interpolation", choices=["hold", "linear"], default="linear")
    parser.add_argument("--margin", type=float, default=100)
    args = parser.parse_args()

    root = Path(__file__).resolve().parent.parent
    if not (root / "src" / "make_reels.py").exists():
        root = Path.cwd()
    video = args.video or next(iter(sorted((root / "project").glob("*.mp4"))), None)
    if video is None or not video.is_file():
        parser.error("Видео не найдено. Запускай из корня проекта или укажи --video")
    tracks = args.tracks or root / "output" / video.stem / "tracks"
    if not tracks.is_dir():
        parser.error(f"Траектории не найдены: {tracks}")
    if not sys.stdin.isatty() and (args.duration is None or args.min_rally is None):
        parser.error("Для запуска без терминала передай --duration и --min-rally")
    duration = args.duration if args.duration is not None else prompt_seconds("Продолжительность готового ролика (сек)", 12)
    min_rally = args.min_rally if args.min_rally is not None else prompt_seconds("Минимальная длительность розыгрыша (сек)", 5)
    if not (math.isfinite(duration) and duration > 0 and math.isfinite(min_rally) and min_rally > 0):
        parser.error("Длительности должны быть положительными")
    if not math.isfinite(args.margin):
        parser.error("--margin должен быть конечным числом")
    fps, width, height, total = video_properties(video)
    ncpu = len(os.sched_getaffinity(0)) if hasattr(os, "sched_getaffinity") else (os.cpu_count() or 2)
    workers = args.workers or max(1, min(6, (ncpu + 1) // 2))
    if workers < 1 or workers > max(1, ncpu):
        parser.error(f"--workers должен быть от 1 до {ncpu}")
    tag = f"{duration:g}s_min{min_rally:g}s_m{args.margin:g}_{args.smoothing}_{args.interpolation}"
    output = args.output or root / "output" / video.stem / f"reels_cpu_{tag}"
    output.mkdir(parents=True, exist_ok=True)
    total_tracks, rejected, invalid, jobs = read_jobs(tracks, video, fps, min_rally, output)
    existing = [j for j in jobs if Path(j.destination).exists() and Path(j.destination).stat().st_size > 128]
    pending = [j for j in jobs if j not in existing]
    has_ffmpeg = shutil.which("ffmpeg") is not None

    print("\n" + "=" * 68)
    print("ЭТАП 1/3 — Проверка файлов: ГОТОВО")
    print(f"  Видео: {video.name} | {width}x{height}, {fps:.2f} FPS")
    print(f"  Найдено треков: {total_tracks} | Подходящих (>= {min_rally:g} с): {len(jobs)}")
    print(f"  Отброшено коротких: {rejected} | Повреждено/пустых: {invalid}")
    print("ЭТАП 2/3 — Настройки: ГОТОВО")
    print(f"  Длительность ролика: {duration:g} с | Сглаживание: {args.smoothing}")
    print(f"  Камера: {args.interpolation}, смещение: {args.margin:g} px")
    print(f"  Процессов CPU: {workers} из {ncpu} логических | Кодирование: {'libx264' if has_ffmpeg else 'OpenCV mp4v'}")
    print(f"  Уже готово: {len(existing)} | Осталось создать: {len(pending)}")
    print(f"  Папка результата: {output}")
    print("ЭТАП 3/3 — Нарезка и кодирование\n" + "=" * 68, flush=True)
    if not pending:
        print(f"✅ Все {len(existing)} файлов уже готовы. Новых задач нет.")
        return 0

    started = time.monotonic()
    success = 0
    failures = 0
    complete = len(existing)
    duration_frames = max(1, round(duration * fps))
    with ProcessPoolExecutor(max_workers=workers, mp_context=get_context("spawn"),
                             initializer=worker_init, initargs=(str(video),)) as pool:
        futures = {
            pool.submit(export_clip, j, fps, width, height, total, duration_frames,
                        args.smoothing, args.interpolation, args.margin, has_ffmpeg): j
            for j in pending
        }
        for future in as_completed(futures):
            complete += 1
            elapsed = time.monotonic() - started
            try:
                name, frames, spent = future.result()
                success += 1
                description = f"✓ {name} | {frames} кадр. за {spent:.1f}с"
            except Exception as e:
                failures += 1
                description = f"✗ {Path(futures[future].source).name}: {str(e)[:180]}"
            remaining = len(pending) - (success + failures)
            eta = elapsed / max(1, success + failures) * remaining
            print(f"[{complete}/{len(jobs)} | {complete/len(jobs):.1%}] {description} "
                  f"| осталось ~{int(eta//60)}м {int(eta%60)}с", flush=True)
    elapsed = time.monotonic() - started
    print("\n" + "=" * 68)
    print(f"ГОТОВО: {success} новых, {len(existing)} ранее созданных, ошибок: {failures}")
    print(f"Время: {elapsed/60:.1f} мин | Результат: {output}")
    print("Звук в этом режиме не копируется (как в исходном make_reels.py).")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(cli())
