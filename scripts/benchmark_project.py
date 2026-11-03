#!/usr/bin/env python3
"""Sequential real-video ONNX Runtime CPU vs CUDA benchmark for five detectors.

All detectors see the SAME nine source frames; frames are resized/channel-converted
to their required ONNX inputs before the timer. One model/device is loaded at a
time. Results are inference-only, never mislabeled as end-to-end video FPS.
"""
from __future__ import annotations

import argparse
import gc
import hashlib
import json
import math
import os
import platform
import statistics
import sys
import time
import uuid
from datetime import datetime, timezone
from pathlib import Path

import cv2
import numpy as np
import onnxruntime as ort

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from scripts.volleyball_ui.projects import project_dir, get_project, project_videos
from scripts.volleyball_ui.data import read_json, save_json
from scripts.volleyball_studio.ball_models import BALL_MODELS

RAVEL_MODELS = ("RAVEL-VB-011-9f.onnx", "RAVEL-VB-012-9f.onnx", "RAVEL-VB-013-9f.onnx")
MODELS = (*BALL_MODELS, *RAVEL_MODELS)
DEVICES = ("cpu", "cuda")
DEFAULT_WARMUP = 3
DEFAULT_RUNS = 12
DEFAULT_REPEATS = 2


def available_models() -> list[str]:
    return [name for name in MODELS if (ROOT / "models" / name).is_file()]


def model_paths() -> list[Path]:
    missing = [name for name in MODELS if name not in available_models()]
    if missing:
        raise FileNotFoundError("Отсутствуют модели для сравнения: " + ", ".join(missing))
    return [ROOT / "models" / name for name in MODELS]


def parse_args(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project-id", required=True)
    parser.add_argument("--video", default="", help="project video name; default first")
    parser.add_argument("--start", type=float, default=0.0,
                        help="common start time in seconds for all models")
    parser.add_argument("--threads", type=int, default=max(1, round((os.cpu_count() or 1)*0.8)))
    parser.add_argument("--warmup", type=int, default=DEFAULT_WARMUP)
    parser.add_argument("--runs", type=int, default=DEFAULT_RUNS)
    parser.add_argument("--repeats", type=int, default=DEFAULT_REPEATS)
    return parser.parse_args(argv)


def validate(args):
    paths = model_paths()
    if not (1 <= args.threads <= 128 and 0 <= args.warmup <= 30 and
            3 <= args.runs <= 300 and 1 <= args.repeats <= 5):
        raise ValueError("Потоки 1–128; прогревы 0–30; замеры 3–300; повторы 1–5")
    if not math.isfinite(args.start) or not 0 <= args.start <= 86400:
        raise ValueError("Начало диапазона должно быть от 0 до 86400 секунд")
    return paths


def emit(stage: str, percent: float, **extra):
    print(json.dumps({"stage": stage, "overall_percent": round(percent, 1), **extra},
                     ensure_ascii=False), flush=True)


def hardware():
    cpu = platform.processor() or platform.machine()
    try:
        for line in Path("/proc/cpuinfo").read_text().splitlines():
            if line.startswith("model name"):
                cpu = line.split(":", 1)[1].strip()
                break
    except OSError:
        pass
    gpu = ""
    try:
        import subprocess
        result = subprocess.run(
            ["nvidia-smi", "--query-gpu=name", "--format=csv,noheader"],
            text=True, capture_output=True, timeout=6, check=True
        )
        gpu = result.stdout.splitlines()[0].strip() if result.stdout else ""
    except (OSError, IndexError, subprocess.SubprocessError):
        pass
    return {"cpu": cpu, "gpu": gpu, "logical_cpus": os.cpu_count(),
            "host": platform.node(), "ort_version": ort.__version__}


def select_video(project_id: str, video: str) -> Path:
    candidates = project_videos(project_id)
    if not candidates:
        raise ValueError("В проекте нет видео для бенчмарка")
    if video:
        for path in candidates:
            if path.name == video:
                return path
        raise ValueError("Выбранное видео отсутствует в проекте")
    return candidates[0]


def source_frames(path: Path, start: float, count: int = 9):
    """Decode one shared set of consecutive real frames, only once."""
    capture = cv2.VideoCapture(str(path))
    if not capture.isOpened():
        raise ValueError(f"Невозможно открыть видео: {path}")
    try:
        if start > 0:
            capture.set(cv2.CAP_PROP_POS_MSEC, start * 1000)
        frames = []
        for _ in range(count):
            ok, frame = capture.read()
            if not ok:
                break
            frames.append(frame)
    finally:
        capture.release()
    if len(frames) != count:
        raise ValueError(f"Для теста нужно {count} кадров после {start:.2f} с, найдено {len(frames)}")
    digest = hashlib.sha256()
    for frame in frames:
        digest.update(frame.tobytes())
    return frames, digest.hexdigest()


def input_tensor(frames, input_shape):
    """Convert same source frames to each architecture's fixed tensor shape."""
    # ONNX exports often use symbolic batch/sequence dimensions (B, N).
    shape = tuple(1 if index == 0 and not isinstance(value, int)
                  else len(frames) if index == 1 and not isinstance(value, int)
                  else int(value)
                  for index, value in enumerate(input_shape))
    if len(shape) == 4:
        batch, steps, height, width = shape
        if batch != 1 or steps != len(frames):
            raise ValueError(f"Unsupported VballNet input {shape}")
        converted = []
        for frame in frames:
            resized = cv2.resize(frame, (width, height), interpolation=cv2.INTER_AREA)
            grey = cv2.cvtColor(resized, cv2.COLOR_BGR2GRAY)
            converted.append(grey.astype(np.float32)/255.0)
        return np.ascontiguousarray(np.stack(converted)[None], dtype=np.float32)
    if len(shape) == 5:
        batch, steps, channels, height, width = shape
        if batch != 1 or steps != len(frames) or channels not in (1,3):
            raise ValueError(f"Unsupported RAVEL input {shape}")
        converted = []
        for frame in frames:
            resized = cv2.resize(frame, (width, height), interpolation=cv2.INTER_AREA)
            if channels == 1:
                value = cv2.cvtColor(resized, cv2.COLOR_BGR2GRAY)[None]
            else:
                value = cv2.cvtColor(resized, cv2.COLOR_BGR2RGB).transpose(2,0,1)
            converted.append(value.astype(np.float32)/255.0)
        return np.ascontiguousarray(np.stack(converted)[None], dtype=np.float32)
    raise ValueError(f"Unsupported input rank {shape}")


def load_executor(path: Path, device: str, threads: int, frames):
    """Create provider and real-frame tensor. GPU fallback is forbidden."""
    if device == "cuda":
        if "CUDAExecutionProvider" not in ort.get_available_providers():
            raise RuntimeError("CUDAExecutionProvider is unavailable")
        ort.preload_dlls(directory="")
    options = ort.SessionOptions()
    options.intra_op_num_threads = threads
    providers = (["CUDAExecutionProvider", "CPUExecutionProvider"] if device == "cuda"
                 else ["CPUExecutionProvider"])
    session = ort.InferenceSession(str(path), sess_options=options, providers=providers)
    active = session.get_providers()
    if device == "cuda" and (not active or active[0] != "CUDAExecutionProvider"):
        raise RuntimeError(f"CUDA failed to initialize for {path.name}; no CPU fallback")
    inputs = session.get_inputs()
    tensor = input_tensor(frames, inputs[0].shape)
    feed = {inputs[0].name: tensor}
    for input_info in inputs[1:]:
        dims = tuple(1 if not isinstance(v, int) else v for v in input_info.shape)
        feed[input_info.name] = np.zeros(dims, dtype=np.float32)
    def infer_once():
        return session.run(None, feed)
    return infer_once, {"provider": active[0], "seq": len(frames),
                        "shape": list(tensor.shape), "session": session}


def bench_one(path: Path, device: str, args, frames, position: int, total: int) -> dict:
    fps_repeats, latency_repeats = [], []
    begun = time.perf_counter()
    provider = ""
    shape = None
    for repeat in range(args.repeats):
        stage = f"{path.stem} · {'GPU' if device=='cuda' else 'CPU'} · {repeat+1}/{args.repeats}"
        emit(stage, 100*(position+repeat/args.repeats)/total,
             model=path.name, device=device, position=position+1, total=total)
        execute, meta = load_executor(path, device, args.threads, frames)
        provider = meta["provider"]
        shape = meta["shape"]
        for _ in range(args.warmup):
            execute()
        laps = []
        for run_index in range(args.runs):
            started = time.perf_counter()
            execute()
            laps.append(time.perf_counter()-started)
            if (run_index+1) % max(1,args.runs//5)==0 or run_index+1==args.runs:
                emit(stage,
                     100*(position+(repeat+(run_index+1)/args.runs)/args.repeats)/total,
                     model=path.name, device=device, position=position+1, total=total,
                     current_fps=round(9/statistics.median(laps),2))
        median = statistics.median(laps)
        fps_repeats.append(9/median)
        latency_repeats.append(median)
        del execute, meta
        gc.collect()
    return {
        "model": path.name, "family": "RAVEL" if path.name in RAVEL_MODELS else "VballNet",
        "device": device, "backend": "onnx" if device=="cuda" else "onnx-cpu",
        "provider": provider, "threads": args.threads,
        "seq": 9, "shape": shape, "warmup": args.warmup,
        "runs": args.runs, "repeats": args.repeats,
        "fps": round(statistics.median(fps_repeats),2),
        "latency_ms": round(statistics.median(latency_repeats)*1000,3),
        "repeat_fps": [round(value,2) for value in fps_repeats],
        "wall_sec": round(time.perf_counter()-begun,3),
        "status": "complete", "mode": "video-inference",
        "created_at": datetime.now(timezone.utc).isoformat(),
        "method": "same nine real decoded source frames; median of synchronous inference calls; GPU/CPU measured sequentially; excludes preprocessing and FFmpeg"
    }


def benchmark(args):
    get_project(args.project_id)
    paths = validate(args)
    video = select_video(args.project_id, args.video)
    emit("Однократное чтение исходного видео",0)
    frames, digest = source_frames(video, args.start)
    hardware_info = hardware()
    root = project_dir(args.project_id) / "benchmarks"
    run_id = uuid.uuid4().hex[:12]
    root.mkdir(parents=True,exist_ok=True)
    total = len(paths)*len(DEVICES)
    rows = []

    def persist(status):
        # Publish after EACH device. An interrupted run keeps genuine partial
        # measurements instead of losing all progress.
        payload = {"id":run_id, "project_id":args.project_id, "video":video.name,
                   "start_sec":args.start, "input_hash":digest,
                   "hardware":hardware_info, "method":"real video frames, sequential CPU/GPU",
                   "status":status, "results":list(rows)}
        save_json(root/"runs"/run_id/"result.json",payload)
        history = read_json(root/"history.json",{"runs":[]})
        if not isinstance(history,dict) or not isinstance(history.get("runs"),list):
            history={"runs":[]}
        history["runs"]=[r for r in history["runs"] if r.get("batch_id")!=run_id]
        history["runs"].extend(rows)
        save_json(root/"history.json",history)
        return payload

    # Match CPU/GPU on identical inputs for each model and don't overlap tests.
    for index, path in enumerate(paths):
        for device in DEVICES:
            step = index*len(DEVICES)+DEVICES.index(device)
            print(f"BENCHMARK_SEQUENCE {step+1}/{total}: {path.name} / {device}",flush=True)
            try:
                result = bench_one(path,device,args,frames,step,total)
            except Exception as error:
                result = {
                    "model":path.name, "family":"RAVEL" if path.name in RAVEL_MODELS else "VballNet",
                    "device":device, "status":"failed", "error":str(error),
                    "fps":None, "latency_ms":None,
                    "mode":"video-inference", "backend":"onnx" if device=="cuda" else "onnx-cpu",
                    "threads":args.threads, "warmup":args.warmup,
                    "runs":args.runs, "repeats":args.repeats,
                    "created_at":datetime.now(timezone.utc).isoformat()
                }
            result.update({"id":run_id+f"-{step+1}", "batch_id":run_id,
                           "expected_tests":total,
                           "project_id":args.project_id, "video":video.name,
                           "start_sec":args.start, "input_hash":digest,
                           "hardware":hardware_info})
            rows.append(result)
            persist("running")
            print("BENCHMARK_MODEL="+json.dumps(result,ensure_ascii=False),flush=True)
    payload = persist("complete")
    complete = sum(row["status"]=="complete" for row in rows)
    emit(f"Проверено {complete}/{total} конфигураций",100,results=complete)
    print("BENCHMARK_RESULT="+json.dumps(payload,ensure_ascii=False),flush=True)
    return payload


if __name__=="__main__":
    try:
        benchmark(parse_args())
    except Exception as error:
        print("BENCHMARK_ERROR: "+str(error),file=sys.stderr,flush=True)
        raise
