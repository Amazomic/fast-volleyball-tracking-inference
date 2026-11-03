"""Project montage outputs: discover, stream, download, remove and preview safely."""
from __future__ import annotations
import hashlib
from datetime import datetime
import os
import subprocess
import threading
from functools import lru_cache
from pathlib import Path
from urllib.parse import quote
from .data import RESULTS, read_json
from .projects import project_dir

_PREVIEW_LOCK = threading.RLock()
_PREVIEW_WORK = set()
_ALLOWED = ("vertical_reels", "montages")


def _candidates(project_id: str):
    root = project_dir(project_id)
    # Never expose intermediate analysis, source videos, or segment caches as
    # finished exports. All user-facing outputs are kept in these locations.
    output = root / "highlights.mp4"
    if output.is_file():
        yield "project:highlights.mp4", output, "Подборка проекта"
    for folder in _ALLOWED:
        directory = root / folder
        if directory.exists():
            for file in sorted(directory.rglob("*.mp4")):
                if file.is_file() and "_render_temp" not in file.parts:
                    relative = file.relative_to(root).as_posix()
                    yield "project:" + relative, file, ("Вертикальные Reels" if folder == "vertical_reels" else "Монтаж")
    # New, versioned montage outputs are children of the exact analysis run.
    for directory in sorted((root / "runs" / "branches").glob("run_*/montages")):
        for file in sorted(directory.rglob("*.mp4")):
            if file.is_file() and "_render_temp" not in file.parts:
                relative = file.relative_to(root).as_posix()
                yield "project:" + relative, file, "Монтаж анализа"
    # Backward compatibility with pre-project exports.
    for file in sorted(RESULTS.glob("*.mp4")):
        if file.is_file():
            yield "legacy:" + file.name, file, "Общие ролики (старые)"


def output_path(project_id: str, file_id: str) -> Path:
    if not isinstance(file_id, str):
        raise ValueError("Некорректный идентификатор файла")
    for ident, file, _ in _candidates(project_id):
        if ident == file_id:
            resolved = file.resolve()
            project_root = project_dir(project_id).resolve()
            if (resolved.is_relative_to(project_root) or
                    resolved.parent == RESULTS.resolve()):
                return resolved
    raise FileNotFoundError("Ролик не найден")


@lru_cache(maxsize=512)
def _codec(path: str, mtime: int, size: int):
    try:
        result = subprocess.run(
            ["ffprobe", "-v", "error", "-select_streams", "v:0",
             "-show_entries", "stream=codec_name", "-of", "default=nw=1:nk=1", path],
            capture_output=True, text=True, timeout=12, check=True)
        return result.stdout.strip().splitlines()[0].strip() if result.stdout.strip() else ""
    except (OSError, subprocess.SubprocessError):
        return ""


def codec_for(path: Path):
    stat = path.stat()
    return _codec(str(path), stat.st_mtime_ns, stat.st_size)


def listing(project_id: str) -> list[dict]:
    items = []
    for file_id, file, group in _candidates(project_id):
        stat = file.stat()
        codec = codec_for(file)
        query = "?project_id=" + quote(project_id) + "&id=" + quote(file_id, safe="")
        # New montage directories contain a stable creation timestamp in their
        # job ID. For old exports, filesystem modification time is the fallback.
        date = datetime.fromtimestamp(stat.st_mtime)
        recorded_creation = False
        relative = file.relative_to(project_dir(project_id)).parts if file_id.startswith("project:") else ()
        analysis_run_id = None
        if len(relative) >= 6 and relative[:2] == ("runs", "branches") and relative[3] == "montages":
            analysis_run_id = relative[2]
            folder_id = relative[4]
        elif len(relative) >= 3 and relative[0] == "montages":
            folder_id = relative[1]
            # Historical exports may already declare their selected run.
            selection = read_json(project_dir(project_id) / "montages" / folder_id / "selection.json", {})
            if isinstance(selection, dict):
                analysis_run_id = selection.get("analysis_run_id")
        else:
            folder_id = ""
        if folder_id:
            try:
                date = datetime.strptime(folder_id[:15], "%Y%m%d_%H%M%S")
                recorded_creation = True
            except ValueError:
                pass
        date_text = date.strftime("%d.%m.%Y · %H:%M:%S")
        sort_time = date.timestamp() if recorded_creation else stat.st_mtime
        items.append({
            "id": file_id,
            "analysis_run_id": analysis_run_id,
            "title": date_text,
            "created_at": date.isoformat(timespec="seconds"),
            "date_source": "created" if recorded_creation else "file_time",
            "sort_time": sort_time,
            "name": file.name,
            "group": group,
            "size_mb": round(stat.st_size / (1024 * 1024), 1),
            "codec": codec,
            "playable": codec in ("h264", "vp8", "vp9", "av1"),
            "url": "/api/montage/media" + query,
            "download_url": "/api/montage/download" + query,
            "folder": str(file.parent),
            "modified": int(stat.st_mtime),
        })
    return sorted(items, key=lambda x: (x["sort_time"], x["id"]), reverse=True)


def remove(project_id: str, file_id: str):
    target = output_path(project_id, file_id)
    # No deletion during active output writing (guarded by server as well).
    target.unlink()
    return {"deleted": file_id}


def show_folder(project_id: str, file_id: str):
    source = output_path(project_id, file_id)
    folder = source.parent
    if os.name == "nt":
        os.startfile(str(folder))  # local-only HTTP interface
    else:
        # WSL opens Windows Explorer, not a command shell.
        try:
            converted = subprocess.run(["wslpath", "-w", str(folder)],
                                       capture_output=True, text=True,
                                       check=True, timeout=5).stdout.strip()
            subprocess.Popen(["explorer.exe", converted],
                             stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                             stderr=subprocess.DEVNULL)
        except (OSError, subprocess.SubprocessError):
            subprocess.Popen(["xdg-open", str(folder)], stdin=subprocess.DEVNULL,
                             stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    return {"folder": str(folder)}


def preview_info(project_id: str, file_id: str):
    source = output_path(project_id, file_id)
    query = "?project_id=" + quote(project_id) + "&id=" + quote(file_id, safe="")
    if codec_for(source) in ("h264", "vp8", "vp9", "av1"):
        return {"status": "ready", "url": "/api/montage/media" + query}
    key = hashlib.sha256((str(source) + ":" + str(source.stat().st_mtime_ns) +
                          ":" + str(source.stat().st_size)).encode()).hexdigest()[:28]
    cache = RESULTS / "preview_cache" / (key + ".mp4")
    if cache.with_suffix(".error").exists():
        return {"status": "failed", "error": "Не удалось подготовить H.264-просмотр. Скачайте оригинал."}
    if cache.is_file() and cache.stat().st_size > 0:
        return {"status": "ready", "url": "/api/montage/preview-media" + query}
    with _PREVIEW_LOCK:
        if key not in _PREVIEW_WORK:
            _PREVIEW_WORK.add(key)
            threading.Thread(target=_transcode, args=(key, source, cache),
                             daemon=True, name="montage-h264-preview").start()
    return {"status": "preparing"}


def _transcode(key: str, source: Path, destination: Path):
    destination.parent.mkdir(parents=True, exist_ok=True)
    temporary = destination.with_suffix(".partial.mp4")
    try:
        subprocess.run(["ffmpeg", "-nostdin", "-hide_banner", "-loglevel", "error",
                        "-y", "-i", str(source), "-map", "0:v:0", "-map", "0:a:0?",
                        "-c:v", "libx264", "-preset", "veryfast", "-crf", "25",
                        "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "128k",
                        "-movflags", "+faststart", str(temporary)],
                       check=True, timeout=3600)
        temporary.replace(destination)
    except (OSError, subprocess.SubprocessError) as error:
        print("Preview conversion failed:", error, flush=True)
        temporary.unlink(missing_ok=True)
        destination.with_suffix(".error").write_text(str(error)[:400], encoding="utf-8")
    finally:
        with _PREVIEW_LOCK:
            _PREVIEW_WORK.discard(key)


def preview_file(project_id: str, file_id: str):
    source = output_path(project_id, file_id)
    key = hashlib.sha256((str(source) + ":" + str(source.stat().st_mtime_ns) +
                          ":" + str(source.stat().st_size)).encode()).hexdigest()[:28]
    file = RESULTS / "preview_cache" / (key + ".mp4")
    if not file.is_file():
        raise FileNotFoundError("Просмотр ещё подготавливается")
    return file
