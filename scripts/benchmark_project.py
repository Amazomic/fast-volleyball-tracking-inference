#!/usr/bin/env python3
"""Run the upstream inference-only benchmark (no FFmpeg, no video, no detection pipeline).

Same methodology as scripts/bench_inference_fps.py:
synthetic [1, seq, H, W] float32, warm-up calls, median call latency,
seq/median_latency FPS, and median FPS over independently loaded repeats.
"""
from __future__ import annotations

import argparse
import gc
import json
import math
import platform
import statistics
import sys
import time
import uuid
from datetime import datetime, timezone
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT / "src"))
from scripts.volleyball_ui.projects import project_dir, get_project
from scripts.volleyball_ui.data import read_json, save_json

DEFAULT_WARMUP = 10
DEFAULT_RUNS = 60
DEFAULT_REPEATS = 3
BACKENDS = ("onnx-cpu", "onnx", "openvino")


def available_models() -> list[str]:
    """Only the VballNet models compatible with the upstream inference runners."""
    return sorted(p.name for p in (ROOT / "models").glob("VballNet*.onnx")
                  if p.is_file())


def model_paths(name: str) -> list[Path]:
    available = available_models()
    if name == "all":
        if not available:
            raise ValueError("Нет подходящих моделей в models/")
        return [ROOT / "models" / entry for entry in available]
    if name not in available:
        raise ValueError("Неизвестная модель, выберите файл из списка models/")
    return [ROOT / "models" / name]


def parse_args(argv=None):
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--project-id", required=True)
    p.add_argument("--model", default="VballNetV4c_seq9_grayscale_20260908_213829.onnx")
    p.add_argument("--backend", choices=BACKENDS, default="onnx-cpu")
    p.add_argument("--threads", type=int, default=8)
    p.add_argument("--warmup", type=int, default=DEFAULT_WARMUP)
    p.add_argument("--runs", type=int, default=DEFAULT_RUNS)
    p.add_argument("--repeats", type=int, default=DEFAULT_REPEATS)
    return p.parse_args(argv)


def validate(args):
    if args.backend not in BACKENDS:
        raise ValueError("Неизвестный движок")
    if not (1 <= args.threads <= 32 and 0 <= args.warmup <= 30 and
            5 <= args.runs <= 300 and 1 <= args.repeats <= 5):
        raise ValueError("Потоки 1–32, прогрев 0–30, измерения 5–300, повторы 1–5")
    return model_paths(args.model)


def emit(stage: str, progress: float, **data):
    print(json.dumps({"stage": stage, "overall_percent": round(progress, 1), **data},
                     ensure_ascii=False), flush=True)


def load_executor(model: Path, backend: str, threads: int):
    """Return (infer_once, model_metadata); reloaded for each repeated measurement."""
    if backend == "openvino":
        from inference_openvino_seq_gray_v2 import load_model
        compiled, _, output_layer, params = load_model(
            str(model), device="CPU", num_threads=threads)
        tensor_shape = (1, params["seq"], params["input_height"], params["input_width"])
        def execute(tensor):
            return compiled(tensor)[output_layer]
        return execute, {"seq": int(params["seq"]), "shape": tensor_shape,
                         "provider": "OpenVINO CPU", "_owner": compiled}
    from inference_onnx_seq_gray_v2 import load_onnx_model
    session, _, _, h0shape, _, input_names, _, params = load_onnx_model(
        str(model), device="cuda" if backend=="onnx" else "cpu", num_threads=threads)
    tensor_shape = (1, params["input_seq"], params["input_height"], params["input_width"])
    inputs = {name: np.zeros(h0shape, np.float32) for name in input_names[1:]} if h0shape else {}
    def execute(tensor):
        session.run(None, {input_names[0]: tensor, **inputs})
    return execute, {"seq": int(params["seq"]), "shape": tensor_shape,
                     "provider": session.get_providers()[0], "_owner": session}


def bench_one(model: Path, args, rng, position: int, total: int) -> dict:
    fps_repeats = []
    latency_repeats = []
    shape = None
    seq = None
    provider = None
    begun = time.perf_counter()
    for repeat in range(args.repeats):
        stage = f"{model.stem}: повтор {repeat+1}/{args.repeats}"
        emit(stage, 100*(position+repeat/args.repeats)/total, backend=args.backend,
             model=model.name)
        execute, meta = load_executor(model, args.backend, args.threads)
        seq = meta["seq"]; shape = meta["shape"]; provider = meta["provider"]
        rng_tensor = rng.random(shape).astype(np.float32)
        for _ in range(args.warmup):
            execute(rng_tensor)
        laps = []
        for i in range(args.runs):
            started = time.perf_counter()
            execute(rng_tensor)
            laps.append(time.perf_counter()-started)
            if (i+1)%max(5,args.runs//6)==0 or i+1==args.runs:
                emit(stage, 100*(position+(repeat+(i+1)/args.runs)/args.repeats)/total,
                     model=model.name, run=i+1, runs=args.runs,
                     current_fps=round(seq/statistics.median(laps),2))
        latency = statistics.median(laps)
        latency_repeats.append(latency)
        fps_repeats.append(seq/latency)
        # Close all runtime references before next repeat to match upstream.
        del execute, meta, rng_tensor
        gc.collect()
    result = {
        "model": model.name,
        "backend": args.backend,
        "provider": provider,
        "threads": args.threads if args.backend!="onnx" else None,
        "seq": seq,
        "shape": list(shape),
        "warmup": args.warmup,
        "runs": args.runs,
        "repeats": args.repeats,
        "fps": round(statistics.median(fps_repeats), 2),
        "repeat_fps": [round(x,2) for x in fps_repeats],
        "latency_ms": round(statistics.median(latency_repeats)*1000,3),
        "wall_sec": round(time.perf_counter()-begun,3),
        "mode": "inference",
        "created_at": datetime.now(timezone.utc).isoformat(),
        "host": platform.node(),
        "method": "Upstream synthetic-input inference, seq/median latency, median of repeats; excludes FFmpeg",
    }
    return result


def benchmark(args):
    get_project(args.project_id)
    paths = validate(args)
    root = project_dir(args.project_id) / "benchmarks"
    run_id = uuid.uuid4().hex[:12]
    root.mkdir(parents=True, exist_ok=True)
    emit("Подготовка нейросетевого бенчмарка", 0, model_count=len(paths))
    rng = np.random.default_rng(0)
    results = []
    for i, path in enumerate(paths):
        result = bench_one(path,args,rng,i,len(paths))
        result["id"] = run_id + f"-{i+1}"
        result["project_id"] = args.project_id
        results.append(result)
        print("BENCHMARK_MODEL="+json.dumps(result,ensure_ascii=False),flush=True)
    payload = {"id":run_id,"project_id":args.project_id,
               "method":"bench_inference_fps.py methodology",
               "results":results}
    save_json(root/"runs"/run_id/"result.json",payload)
    history=read_json(root/"history.json",{"runs":[]})
    if not isinstance(history,dict) or not isinstance(history.get("runs"),list):
        history={"runs":[]}
    history["runs"].extend(results)
    save_json(root/"history.json",history)
    emit("Измерение завершено",100,results=len(results))
    print("BENCHMARK_RESULT="+json.dumps(payload,ensure_ascii=False),flush=True)
    return payload


if __name__=="__main__":
    try:
        benchmark(parse_args())
    except Exception as exc:
        print("BENCHMARK_ERROR: "+str(exc), file=sys.stderr, flush=True)
        raise
