from pathlib import Path
import shutil
import sys
import tempfile
import time

from .common import ROOT, discover, encoder, run, save
from .resources import measure_resources


def benchmark(input_dir, work):
    videos, _ = discover(input_dir)
    if not videos:
        raise ValueError("Нет видео для benchmark")
    folder = work.resolve() / ("benchmark_" + str(time.time_ns()))
    folder.mkdir(parents=True)
    sample = folder / "sample.mp4"
    started = time.monotonic()
    run(["ffmpeg", "-v", "error", "-y", "-i", videos[0], "-t", "10", "-vf", "fps=30,scale=1280:-2", "-an", "-c:v", "libx264", "-preset", "ultrafast", "-threads", "4", sample])
    report = {"source": str(videos[0]), "sample_seconds": 10, "prepare_seconds": time.monotonic()-started, "resources": measure_resources(), "measurements": {}}
    with tempfile.TemporaryDirectory(prefix="volleyball-benchmark-") as temp:
        local = Path(temp) / "sample.mp4"
        started = time.monotonic()
        shutil.copyfile(sample, local)
        report["copy_to_linux_seconds"] = time.monotonic()-started
        for label, path in (("workspace_decode", sample), ("linux_temp_decode", local)):
            started = time.monotonic()
            run(["ffmpeg", "-v", "error", "-i", path, "-an", "-f", "null", "-"])
            report["measurements"][label] = time.monotonic()-started
        for backend in ("onnx", "openvino"):
            script = "inference_onnx_seq_gray_v2.py" if backend == "onnx" else "inference_openvino_seq_gray_v2.py"
            command = [sys.executable, ROOT / "src" / script, "--video_path", local, "--output_dir", folder / backend, "--only_csv"]
            command += ["--model_path", ROOT / "models/VballNetV4c_seq9_grayscale_20260908_213829.onnx"] if backend == "onnx" else ["--model_xml", ROOT / "ov/VballNetV4c_seq9_grayscale_20260908_213829.xml", "--device", "CPU"]
            started = time.monotonic()
            run(command, folder / f"{backend}.log")
            report["measurements"][backend] = time.monotonic()-started
        for name in ("cpu", "auto"):
            codec = encoder(name)
            started = time.monotonic()
            run(["ffmpeg", "-v", "error", "-i", local, "-vf", "scale=360:640", "-an", *codec, "-f", "null", "-"])
            report["measurements"][codec[1]] = time.monotonic()-started
    measurements = report["measurements"]
    report["recommended_backend"] = min(("onnx", "openvino"), key=measurements.get)
    report["linux_scratch_worthwhile"] = measurements["workspace_decode"] > measurements["linux_temp_decode"]*1.1 + report["copy_to_linux_seconds"]
    report["scope"] = "one cold actual 10-second sample; includes process/model startup; not a sustained whole-match throughput guarantee"
    save(folder / "benchmark.json", report)
    return report, folder / "benchmark.json"
