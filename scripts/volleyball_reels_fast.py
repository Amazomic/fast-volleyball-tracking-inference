#!/usr/bin/env python3
"""Fast per-track vertical exports for fast-volleyball-tracking-inference.

Run from repository root with .venv/bin/python scripts/volleyball_reels_fast.py
(or python /path/to/volleyball_reels_fast.py). Keeps original files unchanged.
"""
import argparse
import json
import logging
import math
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

import cv2
import numpy as np

# Reuse original repository's trajectory interpolation, coordinate scaling, smoothing.
ROOT = Path(__file__).resolve().parent
if not (ROOT / 'src' / 'make_reels.py').is_file():
    ROOT = Path.cwd()
sys.path.insert(0, str(ROOT / 'src'))
from make_reels import (  # noqa: E402
    load_track_from_payload,
    scale_track_to_video,
    interpolate_positions,
    smooth_values,
    crop_frame,
)


def ask_float(prompt, default, minimum=0.01):
    while True:
        raw = input(f'{prompt} [{default:g}]: ').strip().replace(',', '.')
        try:
            result = float(raw) if raw else float(default)
            if not math.isfinite(result) or result < minimum:
                raise ValueError()
            return result
        except ValueError:
            print(f'Введите число не меньше {minimum:g}.')


def choose_encoder(width, height, fps):
    if not shutil.which('ffmpeg'):
        raise RuntimeError('Не найден ffmpeg. Установи: sudo apt install ffmpeg')
    # Do real test, not only ffmpeg -encoders: NVIDIA / WSL driver may be incompatible.
    cmd = [
        'ffmpeg', '-hide_banner', '-loglevel', 'error', '-y',
        '-f', 'lavfi', '-i', f'color=c=black:s={width}x{height}:r={fps}:d=0.1',
        '-frames:v', '1', '-an', '-c:v', 'h264_nvenc', '-f', 'null', '-'
    ]
    p = subprocess.run(cmd, capture_output=True, text=True)
    if p.returncode == 0:
        return 'h264_nvenc'
    print('⚠️ NVIDIA NVENC недоступен в FFmpeg — переключаю кодирование на CPU (libx264).')
    if p.stderr:
        print('   Причина:', p.stderr.strip().splitlines()[-1][:240])
    return 'libx264'


def ffmpeg_writer(output, width, height, fps, encoder):
    cmd = [
        'ffmpeg', '-hide_banner', '-loglevel', 'error', '-y',
        '-f', 'rawvideo', '-pix_fmt', 'bgr24', '-s:v', f'{width}x{height}',
        '-r', str(fps), '-i', 'pipe:0', '-an',
        '-c:v', encoder,
    ]
    if encoder == 'h264_nvenc':
        cmd += ['-preset', 'p4', '-cq', '23', '-b:v', '0']
    else:
        cmd += ['-preset', 'veryfast', '-crf', '23', '-threads', '4']
    cmd += ['-pix_fmt', 'yuv420p', '-movflags', '+faststart', str(output)]
    return subprocess.Popen(cmd, stdin=subprocess.PIPE, stderr=subprocess.PIPE)


def clip_bounds(start, end, target_frames, total):
    # Target length, centred on the middle of the tracked rally; extend short
    # tracks with context, shorten longer rallies symmetrically.
    count = min(max(1, target_frames), total)
    middle = (start + end) // 2
    left = max(0, min(middle - count // 2, total - count))
    return left, left + count - 1


def render_one(cap, payload, path, width, height, fps, total, target_frames,
               encoder, smoothing, interpolation, margin):
    track = scale_track_to_video(load_track_from_payload(payload), width, height)
    positions = {int(f): (float(x), float(y)) for x, y, f in track['positions']}
    if not positions:
        raise ValueError('В треке нет координат мяча')
    first, last = clip_bounds(track['start_frame'], track['last_frame'], target_frames, total)
    x = interpolate_positions(positions, first, last, interpolation)
    x = smooth_values(x, smoothing, 15, 2)
    crop_width = min(width, int(height * 9 / 16))
    crop_width -= crop_width % 2
    out_height = height - height % 2
    if crop_width < 2 or out_height < 2:
        raise ValueError('Видео слишком маленькое')
    cap.set(cv2.CAP_PROP_POS_FRAMES, first)
    p = ffmpeg_writer(path, crop_width, out_height, fps, encoder)
    wrote = 0
    try:
        for i in range(last - first + 1):
            ok, frame = cap.read()
            if not ok:
                raise RuntimeError(f'Не прочитан кадр {first+i}')
            cx = int(x[i])
            if i and margin:
                cx += int(margin * np.sign(x[i] - x[i-1]))
            cropped = crop_frame(frame, cx, crop_width)
            cropped = cropped[:out_height, :crop_width]
            if cropped.shape != (out_height, crop_width, 3):
                raise RuntimeError(f'Некорректный размер кадра {cropped.shape}')
            try:
                p.stdin.write(cropped.tobytes())
            except BrokenPipeError:
                raise RuntimeError('FFmpeg остановился во время записи')
            wrote += 1
    finally:
        if p.stdin:
            try:
                p.stdin.close()
            except OSError:
                pass
    err = p.stderr.read().decode('utf-8', errors='replace')
    code = p.wait()
    if code:
        raise RuntimeError('FFmpeg: ' + err[-1000:])
    if wrote == 0 or not path.is_file() or path.stat().st_size == 0:
        raise RuntimeError('Пустой итоговый ролик')
    return wrote


def main():
    parser = argparse.ArgumentParser(description='Быстрая нарезка волейбольных Reels с общим прогрессом')
    parser.add_argument('--video', type=Path)
    parser.add_argument('--tracks', type=Path)
    parser.add_argument('--output', type=Path)
    parser.add_argument('--duration', type=float, help='Желаемая длина итогового ролика в секундах')
    parser.add_argument('--min-rally', type=float, help='Минимальная длительность трека в секундах')
    parser.add_argument('--margin', type=float, default=100)
    parser.add_argument('--smoothing', choices=['none', 'moving_avg', 'kalman', 'savitzky_golay'], default='moving_avg')
    parser.add_argument('--interpolation', choices=['hold', 'linear'], default='linear')
    parser.add_argument('--retry-errors', action='store_true', help='Продолжать после ошибки отдельного трека')
    args = parser.parse_args()
    video = args.video or next(iter(sorted((ROOT / 'project').glob('*.mp4'))), None)
    if not video or not video.is_file():
        parser.error('Видео не найдено. Укажи --video /путь/к/видео.mp4')
    tracks = args.tracks or ROOT / 'output' / video.stem / 'tracks'
    if not tracks.is_dir():
        parser.error(f'Папка с траекториями не найдена: {tracks}')
    out_dir = args.output or ROOT / 'output' / video.stem / 'reels_fast'
    if args.duration is None and not sys.stdin.isatty():
        parser.error('В неинтерактивном режиме нужен --duration')
    if args.min_rally is None and not sys.stdin.isatty():
        parser.error('В неинтерактивном режиме нужен --min-rally')
    duration = args.duration if args.duration is not None else ask_float('Желаемая продолжительность одного ролика, секунд', 12)
    min_rally = args.min_rally if args.min_rally is not None else ask_float('Минимальная длительность найденного розыгрыша, секунд', 5)
    if not (math.isfinite(duration) and math.isfinite(min_rally) and duration > 0 and min_rally > 0):
        parser.error('Длительности должны быть положительными числами')
    cap = cv2.VideoCapture(str(video))
    if not cap.isOpened():
        parser.error(f'Невозможно открыть видео: {video}')
    fps = cap.get(cv2.CAP_PROP_FPS) or 30.0
    total_frames = int(cap.get(cv2.CAP_PROP_FRAME_COUNT))
    width = int(cap.get(cv2.CAP_PROP_FRAME_WIDTH))
    height = int(cap.get(cv2.CAP_PROP_FRAME_HEIGHT))
    if total_frames <= 0 or width < 2 or height < 2:
        parser.error('Некорректные параметры видео')
    target_frames = max(1, int(round(duration * fps)))
    files = sorted(tracks.glob('track_*.json'))
    chosen = []
    invalid = 0
    for file in files:
        try:
            data = json.loads(file.read_text(encoding='utf-8'))
            start = int(data['start_frame'])
            end = int(data['last_frame'])
            if start < 0 or end < start:
                raise ValueError('bad frame range')
            if (end-start+1) / fps >= min_rally:
                chosen.append((start, file, data))
        except (KeyError, TypeError, ValueError, json.JSONDecodeError) as ex:
            invalid += 1
            print(f'⚠️ Некорректный трек {file.name}: {ex}')
    chosen.sort(key=lambda row: row[0])
    out_dir.mkdir(parents=True, exist_ok=True)
    output_width = min(width, int(height * 9 / 16)) // 2 * 2
    output_height = height // 2 * 2
    encoder = choose_encoder(output_width, output_height, fps)
    print('\n' + '='*62)
    print('  ВОЛЕЙБОЛ: ЭТАП 3/3 — ЭКСПОРТ ВЕРТИКАЛЬНЫХ РОЛИКОВ')
    print('='*62)
    print(f'Видео: {video.name} | {width}x{height} | {fps:.2f} FPS')
    print(f'Найдено треков: {len(files)} | Подходят (>= {min_rally:g} с): {len(chosen)}')
    print(f'Невалидных треков: {invalid} | Отфильтровано: {len(files)-len(chosen)-invalid}')
    print(f'Целевая длина ролика: {duration:g} с | Экспорт: {encoder}')
    print(f'Результат: {out_dir}\n')
    if not chosen:
        print('Подходящих эпизодов нет. Уменьши минимальную длительность.')
        return
    done = 0
    reused = 0
    errors = 0
    t0 = time.monotonic()
    for index, (_, file, payload) in enumerate(chosen, 1):
        output = out_dir / f'reel_{video.stem}_{file.stem.split("_")[-1]}.mp4'
        if output.is_file() and output.stat().st_size > 0:
            reused += 1
            done += 1
            print(f'[{index:>4}/{len(chosen)}] ПРОПУЩЕН (уже есть) {output.name} | {index/len(chosen):.0%}', flush=True)
            continue
        partial = output.with_name(output.stem + '.partial.mp4')
        partial.unlink(missing_ok=True)
        started = time.monotonic()
        try:
            frames = render_one(cap, payload, partial, width, height, fps, total_frames,
                                target_frames, encoder, args.smoothing, args.interpolation, args.margin)
            partial.replace(output)
            done += 1
            spent = time.monotonic()-started
            elapsed = time.monotonic()-t0
            active = max(1, index-reused)
            eta = int((len(chosen)-index) * (elapsed / active))
            print(f'[{index:>4}/{len(chosen)}] ✓ {output.name} | {frames} кадров | {spent:.1f} с | готово {index/len(chosen):.1%} | ETA ~{eta//60}м {eta%60}с',flush=True)
        except (Exception, KeyboardInterrupt) as ex:
            partial.unlink(missing_ok=True)
            if isinstance(ex, KeyboardInterrupt):
                print('\nОстановлено пользователем. Готовые файлы сохранены.')
                raise
            errors += 1
            print(f'[{index}/{len(chosen)}] ❌ {file.name}: {ex}', file=sys.stderr, flush=True)
            if not args.retry_errors:
                raise
    cap.release()
    elapsed = time.monotonic() - t0
    print('\n' + '='*62)
    print(f'✅ Готово файлов: {done}/{len(chosen)} | Пропущено готовых: {reused} | Ошибок: {errors}')
    print(f'Время этапа: {elapsed/60:.1f} мин | Папка: {out_dir}')
    print('Примечание: исходные звуковые дорожки не копируются, как и в оригинальном make_reels.py.')


if __name__ == '__main__':
    main()
