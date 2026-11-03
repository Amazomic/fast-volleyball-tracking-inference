import argparse
from concurrent.futures import ProcessPoolExecutor, as_completed
import json
import os
from pathlib import Path
import shutil
import subprocess
import time

from .common import cached, digest, key


def configure_cpu_threads(count):
    """Configure CPU math-library thread pools without pinning CPU cores.

    ONNX Runtime and OpenVINO receive their own explicit --num-threads limits
    from the inference pipeline. Do not conflate a thread count with the
    OS CPU-affinity mask: they control different resources.
    """
    if count < 1:
        raise ValueError("Количество потоков CPU должно быть положительным")
    for name in ("OMP_NUM_THREADS", "OPENBLAS_NUM_THREADS", "MKL_NUM_THREADS", "NUMEXPR_NUM_THREADS"):
        os.environ[name] = str(count)


def measure_resources():
    report = {"cpu_count": os.cpu_count(), "available_ram_bytes": None, "gpu": None}
    try:
        for line in Path("/proc/meminfo").read_text().splitlines():
            if line.startswith("MemAvailable:"):
                report["available_ram_bytes"] = int(line.split()[1])*1024
    except OSError:
        pass
    program = shutil.which("nvidia-smi")
    if program is None and Path("/usr/lib/wsl/lib/nvidia-smi").exists():
        program = "/usr/lib/wsl/lib/nvidia-smi"
    if program:
        try:
            result = subprocess.run([program, "--query-gpu=name,memory.used,memory.total,utilization.gpu", "--format=csv,noheader,nounits"], capture_output=True, text=True, timeout=3)
            if result.returncode == 0:
                report["gpu"] = result.stdout.strip()
        except (OSError, subprocess.TimeoutExpired):
            pass
    return report


def adaptive_workers(requested, threads, codec):
    ram = measure_resources()["available_ram_bytes"]
    maximum = min(2, max(1, (os.cpu_count() or 2)//max(4, threads)))
    if ram is not None:
        maximum = min(maximum, max(1, int(ram // (4*1024**3))))
    return min(maximum, requested) if requested > 0 else maximum


def render_job(job):
    from .render import render_segment
    item, filename, values, codec = job
    path = Path(filename)
    args = argparse.Namespace(**values)
    options = {name: getattr(args, name, None) for name in ("width", "effects", "style", "min_confidence", "grade", "titles", "speed_ramp")}
    code = {p.name: digest(p) for p in Path(__file__).parent.glob("*.py") if p.name in ("render.py", "vfx.py", "resources.py")}
    signature = key([item["source"], item["cache"], item["start"], item["end"], item["contacts"], options, codec, code])

    def action():
        render_segment(item, path, args, codec)
        return [path, path.with_suffix(".timeline.json")]

    cached(path.parent, path.stem, signature, action)
    return str(path)


def render_jobs(jobs, workers):
    started = time.monotonic()
    print(json.dumps({"stage": "E/F", "jobs": len(jobs), "workers": workers, "resources": measure_resources()}), flush=True)
    if workers == 1 or len(jobs) == 1:
        for index, job in enumerate(jobs, 1):
            render_job(job)
            elapsed = time.monotonic()-started
            print(json.dumps({"stage": "E/F", "ready_segments": index, "total_segments": len(jobs), "overall_percent": round(60+20*index/len(jobs), 1), "eta_sec": round(elapsed/index*(len(jobs)-index), 1)}), flush=True)
    else:
        with ProcessPoolExecutor(max_workers=workers) as pool:
            futures = [pool.submit(render_job, job) for job in jobs]
            for index, future in enumerate(as_completed(futures), 1):
                future.result()
                elapsed = time.monotonic()-started
                print(json.dumps({"stage": "E/F", "ready_segments": index, "total_segments": len(jobs), "overall_percent": round(60+20*index/len(jobs), 1), "eta_sec": round(elapsed/index*(len(jobs)-index), 1)}), flush=True)
