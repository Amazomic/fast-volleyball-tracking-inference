import hashlib
import json
import os
import re
from pathlib import Path
import shutil
import subprocess
import time

ROOT = Path(__file__).resolve().parents[2]
VERSION = "2.0.0"
ANALYSIS_INPUT_VERSION = "1.0.0"
VIDEO = {".mp4", ".mov", ".mkv", ".avi", ".webm", ".m4v", ".mts"}
AUDIO = {".mp3", ".wav", ".flac", ".m4a", ".ogg", ".aac"}
PROGRESS_CONTEXT = {}


def progress_context(**values):
    PROGRESS_CONTEXT.update(values)


def digest(path):
    h = hashlib.sha256()
    started = last_report = time.monotonic()
    size = Path(path).stat().st_size
    completed = 0
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(4 * 1024 * 1024), b""):
            h.update(block)
            completed += len(block)
            now = time.monotonic()
            if now - last_report >= 20:
                speed = completed / max(.001, now - started)
                print(json.dumps({"operation": "sha256", "file": str(path), "percent": round(100*completed/max(1,size), 1), "MB_sec": round(speed/1e6, 1), "eta_sec": round((size-completed)/speed, 1)}), flush=True)
                last_report = now
    return h.hexdigest()


def key(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, default=str).encode()).hexdigest()[:24]


def identity(path):
    path = Path(path).resolve()
    stat = path.stat()
    return [str(path), stat.st_size, stat.st_mtime_ns, digest(path)]


def save(path, data):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    temp = path.with_suffix(path.suffix + ".tmp")
    temp.write_text(json.dumps(data, ensure_ascii=False, indent=2), encoding="utf-8")
    os.replace(temp, path)


def run(args, log=None):
    if log:
        with Path(log).open("w", encoding="utf-8") as stream:
            process = subprocess.Popen([str(x) for x in args], stdout=stream, stderr=stream)
            started = time.monotonic()
            try:
                while process.poll() is None:
                    try:
                        process.wait(timeout=20)
                    except subprocess.TimeoutExpired:
                        elapsed = time.monotonic() - started
                        progress = {**PROGRESS_CONTEXT, "status": "running", "elapsed_sec": round(elapsed), "log": str(log), "eta_sec": None}
                        with Path(log).open("rb") as reader:
                            reader.seek(max(0, Path(log).stat().st_size - 4000))
                            tail = reader.read().decode(errors="replace")
                        matches = re.findall(r"(\d+)/(\d+)\s*\[", tail)
                        if matches:
                            done, total = map(int, matches[-1])
                            speed = done / max(.001, elapsed)
                            progress.update(frames=done, total_frames=total, fps=round(speed, 1), eta_sec=round((total-done)/max(.001,speed), 1))
                        print(json.dumps(progress), flush=True)
            except BaseException:
                process.terminate()
                process.wait()
                raise
            if process.returncode:
                tail = Path(log).read_text(encoding="utf-8", errors="replace")[-3000:]
                raise RuntimeError(f"Процесс завершился с кодом {process.returncode}: {tail}")
    else:
        result = subprocess.run([str(x) for x in args], capture_output=True, text=True, encoding="utf-8", errors="replace")
        if result.returncode:
            raise RuntimeError(result.stderr[-3000:])
        return result.stdout


def probe(path):
    return json.loads(run(["ffprobe", "-v", "error", "-show_format", "-show_streams", "-of", "json", path]))


def cached(folder, name, signature, action):
    state = folder / f"{name}.status.json"
    marker = folder / f"{name}.complete.json"
    if marker.exists():
        try:
            old = json.loads(marker.read_text())
            if old["signature"] == signature and all(Path(p).is_file() and digest(p) == h for p, h in old["outputs"].items()):
                print(f"Кэш: {name}", flush=True)
                save(state, {"stage": name, "status": "cached", "signature": signature})
                return [Path(p) for p in old["outputs"]]
        except (ValueError, KeyError, OSError):
            pass
    started = time.monotonic()
    save(state, {"stage": name, "status": "running", "signature": signature})
    try:
        paths = action()
        if not paths or any(not p.is_file() for p in paths):
            raise RuntimeError(f"Этап {name} не создал все результаты")
    except BaseException as error:
        save(state, {"stage": name, "status": "interrupted" if isinstance(error, KeyboardInterrupt) else "failed", "signature": signature, "error": str(error)})
        raise
    save(marker, {"signature": signature, "seconds": time.monotonic() - started, "outputs": {str(p): digest(p) for p in paths}})
    save(state, {"stage": name, "status": "complete", "signature": signature, "seconds": time.monotonic() - started})
    return paths


def encoder(preference="auto"):
    if preference != "cpu":
        test = subprocess.run(["ffmpeg", "-v", "error", "-f", "lavfi", "-i", "color=s=1080x1920:d=0.1", "-frames:v", "1", "-c:v", "h264_nvenc", "-f", "null", "-"], capture_output=True)
        if test.returncode == 0:
            return ["-c:v", "h264_nvenc", "-preset", "p4", "-cq", "21", "-b:v", "0"]
        if preference == "nvenc":
            raise RuntimeError(test.stderr.decode(errors="replace"))
    return ["-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-threads", "4"]


def discover(folder):
    root = Path(folder)
    if root.is_file():
        source = root.resolve()
        return ([source] if source.suffix.lower() in VIDEO else []), []
    files = sorted(p.resolve() for p in root.iterdir() if p.is_file())
    return [p for p in files if p.suffix.lower() in VIDEO], [p for p in files if p.suffix.lower() in AUDIO]


def tools_present():
    missing = [name for name in ("ffmpeg", "ffprobe") if not shutil.which(name)]
    if missing:
        raise RuntimeError("Не найдены: " + ", ".join(missing))


def acquire_lock(folder):
    stream = (folder / ".studio.lock").open("a+b")
    try:
        if os.name == "nt":
            import msvcrt
            stream.seek(0)
            stream.write(b"0")
            stream.flush()
            stream.seek(0)
            msvcrt.locking(stream.fileno(), msvcrt.LK_NBLCK, 1)
        else:
            import fcntl
            fcntl.flock(stream.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        stream.close()
        raise RuntimeError("Другой процесс использует этот work; дождитесь завершения или укажите отдельный --work")
    return stream
