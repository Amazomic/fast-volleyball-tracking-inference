"""Shared, content-addressed 30 FPS / H.264 video preparation.

Projects keep references to source recordings, not prepared copies. Files in
results/prepared/ are reused across projects when the resolved source identity
and normalization parameters match. Project deletion never deletes this cache.
"""
from __future__ import annotations

import fcntl
import hashlib
import json
import os
import signal
import subprocess
import threading
import time
from concurrent.futures import CancelledError, ThreadPoolExecutor, TimeoutError as FutureTimeout
from pathlib import Path

from . import data
from .preview import video_info, target_width
from .projects import deletion_requested, get_project, project_dir
from .video_compat import is_analysis_ready

_LOCK = threading.RLock()
_POOL = ThreadPoolExecutor(max_workers=1, thread_name_prefix="volley-video-preparation")
_QUEUED = set()
# Held for the entire transcode, including when two UI servers share results/.
_HELD_LOCKS = {}
_FUTURES = {}
_ACTIVE = {}


def _acquire_preparation_lock(output: Path):
    lockfile = output.with_suffix(".prepare.lock")
    lockfile.parent.mkdir(parents=True, exist_ok=True)
    handle = lockfile.open("a+b")
    try:
        fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        handle.close()
        return None
    except Exception:
        handle.close()
        raise
    return handle


def _entry(project_id: str, video: str, width: int | None = None):
    project = get_project(project_id)
    if video not in project["videos"]:
        raise ValueError("Видео не принадлежит проекту")
    source = data.video_path(video)
    info = video_info(video)
    fallback = max(2, min(1280, int(info.get("width", 1280))) // 2 * 2)
    target = int(width if width is not None else info.get("auto_width", fallback))
    if target % 2 or not 2 <= target <= 1920:
        raise ValueError("Неправильная ширина нормализации")
    # The real path, not the project alias, identifies the source. Different
    # projects may import the same original as match.mp4 and match (2).mp4.
    resolved = source.resolve(strict=True)
    stat = resolved.stat()
    fingerprint = {
        "source": str(resolved), "size": stat.st_size, "mtime_ns": stat.st_mtime_ns,
        "width": target, "fps": 30, "schema": 2,
    }
    ident = hashlib.sha256(json.dumps(fingerprint, sort_keys=True).encode("utf-8")).hexdigest()[:28]
    directory = data.RESULTS / "prepared"
    return source, info, directory / (ident + ".mp4"), directory / (ident + ".json"), fingerprint


def _manifest(path: Path) -> dict:
    value = data.read_json(path, {})
    return value if isinstance(value, dict) else {}


def _reuse_project_cache(project_id: str, video: str, output: Path,
                         metadata: Path, expected: dict) -> bool:
    """Promote an old project-local proxy via hardlink without re-encoding.

    Only a completed cache for the *same actual source* and format is eligible.
    The original project can be archived after promotion without losing data.
    """
    if deletion_requested(project_id):
        return False
    # Other projects may already have prepared the same source under another
    # import alias. Look at their completed manifests, never their partial MP4s.
    root = data.RESULTS / "projects"
    handle = _acquire_preparation_lock(output)
    if handle is None:
        return False
    try:
        saved = _manifest(metadata)
        if (saved.get("identity") == expected and saved.get("status") == "ready"
                and output.is_file() and output.stat().st_size > 0):
            return True
        for manifest in root.glob("*/prepared/*.json"):
            legacy = _manifest(manifest)
            identity = legacy.get("identity")
            if not isinstance(identity, dict) or legacy.get("status") != "ready":
                continue
            if (identity.get("schema") != 1
                    or identity.get("size") != expected["size"]
                    or identity.get("mtime_ns") != expected["mtime_ns"]
                    or identity.get("width") != expected["width"]
                    or identity.get("fps") != 30):
                continue
            name = identity.get("name")
            try:
                if data.video_path(name).resolve(strict=True) != Path(expected["source"]):
                    continue
            except (OSError, ValueError, RuntimeError, TypeError):
                continue
            old = manifest.with_suffix(".mp4")
            if not old.is_file() or old.stat().st_size <= 0:
                continue
            try:
                # Results are on the same filesystem; hardlinks avoid duplicate
                # multi-gigabyte videos and remain valid after project deletion.
                output.parent.mkdir(parents=True, exist_ok=True)
                if output.exists():
                    output.unlink()
                os.link(old, output)
            except OSError:
                return False
            _save(metadata, {"identity": expected, "status": "ready",
                             "progress": 100, "migrated_from": str(old)})
            return True
        return False
    finally:
        handle.close()


def ready_path(project_id: str, video: str, width: int | None = None) -> Path | None:
    source, _, output, metadata, expected = _entry(project_id, video, width)
    if is_analysis_ready(source, expected["width"]):
        return source.resolve(strict=True)
    saved = _manifest(metadata)
    if (saved.get("identity") == expected and saved.get("status") == "ready"
            and output.is_file() and output.stat().st_size > 0):
        return output
    if _reuse_project_cache(project_id, video, output, metadata, expected):
        return output
    return None


def _save(path: Path, entry: dict):
    path.parent.mkdir(parents=True, exist_ok=True)
    temp = path.with_suffix(".json.tmp")
    data.save_json(temp, entry)
    temp.replace(path)


def status(project_id: str, *, auto_queue: bool = False) -> dict:
    project = get_project(project_id)
    rows = []
    for video in project["videos"]:
        source, info, output, metadata, identity = _entry(project_id, video)
        stored = _manifest(metadata)
        valid = stored.get("identity") == identity
        prepared = ready_path(project_id, video)
        if prepared:
            state = "ready"
        elif valid and stored.get("status") in ("preparing", "queued"):
            # Another HTTP server may be running the same preparation.
            state = stored["status"]
        elif valid and stored.get("status") == "failed":
            state = "failed"
        else:
            state = "missing"
        rows.append({
            "video": video, "state": state,
            "progress": round(min(100, max(0, float(stored.get("progress", 0)) if valid else 0)), 1),
            "width": identity["width"], "fps": 30,
            "error": stored.get("error", "") if valid and state == "failed" else "",
            "size_mb": round(prepared.stat().st_size/1024**2, 1) if prepared else 0,
            "preparation": ("not_required" if prepared and prepared.resolve() == source.resolve()
                            else "cached" if prepared else "pending"),
        })
    if auto_queue:
        queue_project(project_id)
        # New tasks are registered in-memory before the background worker starts.
        with _LOCK:
            for item in rows:
                if item["state"] == "missing" and (
                        project_id, item["video"], item["width"]) in _QUEUED:
                    item["state"] = "queued"
    return {"project_id": project_id, "videos": rows,
            "ready": sum(v["state"] == "ready" for v in rows),
            "total": len(rows)}


def queue_project(project_id: str, *, retry_failed: bool = False, videos=None) -> None:
    project = get_project(project_id)
    targets = [video for video in project["videos"]
               if videos is None or video in videos]
    with _LOCK:
        for video in targets:
            source, _, output, metadata, identity = _entry(project_id, video)
            if ready_path(project_id, video):
                continue
            row = _manifest(metadata)
            if row.get("identity") == identity and row.get("status") == "failed" and not retry_failed:
                continue
            if deletion_requested(project_id):
                raise ValueError("Проект удаляется: подготовка отменена")
            key = (project_id, video, identity["width"])
            if key in _QUEUED:
                continue
            handle = _acquire_preparation_lock(output)
            if handle is None:
                # Another HTTP server owns this video: leave progress untouched.
                continue
            try:
                if ready_path(project_id, video):
                    handle.close()
                    continue
                _QUEUED.add(key)
                _HELD_LOCKS[key] = handle
                _save(metadata, {"identity": identity, "status": "queued", "progress": 0})
                _FUTURES[key] = _POOL.submit(_prepare, project_id, video, key)
            except Exception:
                _QUEUED.discard(key)
                _FUTURES.pop(key, None)
                _HELD_LOCKS.pop(key, None)
                handle.close()
                raise


def _terminate_preparation(process: subprocess.Popen) -> None:
    if process.poll() is not None:
        return
    try:
        if os.getpgid(process.pid) == process.pid:
            os.killpg(process.pid, signal.SIGTERM)
        else:
            process.terminate()
    except ProcessLookupError:
        pass


def cancel_project(project_id: str, *, timeout: float = 15) -> None:
    """Cancel local queued/running work without touching another project.

    Called only after project .delete-requested has been persisted. Other
    server processes see the same tombstone and can skip queued work.
    """
    with _LOCK:
        futures = []
        running = []
        for key in list(_QUEUED):
            if key[0] != project_id:
                continue
            future = _FUTURES.get(key)
            if future is not None and future.cancel():
                _QUEUED.discard(key)
                _FUTURES.pop(key, None)
                handle = _HELD_LOCKS.pop(key, None)
                if handle is not None:
                    handle.close()
                continue
            if future is not None:
                futures.append(future)
            if key in _ACTIVE:
                running.append(_ACTIVE[key])
    for process in running:
        _terminate_preparation(process)
    for future in futures:
        try:
            future.result(timeout=timeout)
        except (CancelledError, FutureTimeout):
            if not future.done():
                raise RuntimeError("Не удалось остановить подготовку проекта")
        except Exception:
            # A cancelled transcoder may end with a non-zero status.
            pass


def _prepare(project_id: str, video: str, key: tuple) -> None:
    process = None
    try:
        # Do not fight a GPU/CPU analysis job for disk and codec resources.
        # The preparation request remains visibly queued in the interface.
        from . import server
        while (server.JOB.get("process") is not None and
               server.JOB["process"].poll() is None):
            if deletion_requested(project_id):
                return
            time.sleep(2)
        if deletion_requested(project_id):
            return
        source, info, output, metadata, identity = _entry(project_id, video)
        temp = output.with_suffix(".partial.mp4")
        temp.unlink(missing_ok=True)
        total = max(0.1, float(info["duration_sec"]))
        _save(metadata, {"identity": identity, "status": "preparing", "progress": 0})
        cmd = [
            "ffmpeg", "-hide_banner", "-loglevel", "error", "-nostdin", "-y",
            "-i", str(source), "-vf", f"fps=30,scale={identity['width']}:-2,setsar=1",
            "-map", "0:v:0", "-map", "0:a:0?",
            "-c:a", "aac", "-b:a", "128k",
            "-c:v", "libx264", "-preset", "veryfast", "-crf", "20",
            "-threads", "4", "-movflags", "+faststart",
            "-progress", "pipe:1", "-nostats", str(temp),
        ]
        error_log = output.with_suffix(".ffmpeg.log")
        with error_log.open("wb") as log:
            if deletion_requested(project_id):
                return
            process = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=log,
                                       text=True, encoding="utf-8", errors="replace",
                                       stdin=subprocess.DEVNULL, start_new_session=True)
            with _LOCK:
                _ACTIVE[key] = process
            if deletion_requested(project_id):
                _terminate_preparation(process)
            last = 0.0
            for raw in process.stdout:
                if deletion_requested(project_id):
                    _terminate_preparation(process)
                if raw.startswith("out_time="):
                    try:
                        h, m, s = raw.split("=", 1)[1].strip().split(":")
                        elapsed = int(h)*3600 + int(m)*60 + float(s)
                        progress = min(99.5, elapsed / total * 100)
                        if time.monotonic() - last >= .6:
                            _save(metadata, {"identity": identity, "status": "preparing",
                                             "progress": round(progress, 1)})
                            last = time.monotonic()
                    except (ValueError, IndexError):
                        pass
            code = process.wait()
            if process.stdout is not None:
                process.stdout.close()
            if code:
                raise RuntimeError(f"FFmpeg завершился с кодом {code}: "
                                   + error_log.read_text(encoding="utf-8", errors="replace")[-400:])
        if deletion_requested(project_id):
            temp.unlink(missing_ok=True)
            return
        if not temp.is_file() or temp.stat().st_size == 0:
            raise RuntimeError("FFmpeg не создал подготовленное видео")
        # Never publish a stale file if the source changed during decoding.
        if _entry(project_id, video)[4] != identity:
            temp.unlink(missing_ok=True)
            raise RuntimeError("Исходное видео изменилось во время подготовки")
        temp.replace(output)
        _save(metadata, {"identity": identity, "status": "ready", "progress": 100})
        print("VIDEO READY:", project_id, video, output, flush=True)
    except Exception as exc:
        if deletion_requested(project_id):
            print("VIDEO PREPARATION CANCELLED:", project_id, video, flush=True)
            return
        try:
            _, _, output, metadata, identity = _entry(project_id, video)
            _save(metadata, {"identity": identity, "status": "failed",
                             "progress": 0, "error": str(exc)[:500]})
        except Exception:
            pass
        print("VIDEO PREPARATION ERROR:", project_id, video, exc, flush=True)
    finally:
        with _LOCK:
            _QUEUED.discard(key)
            _FUTURES.pop(key, None)
            _ACTIVE.pop(key, None)
            handle = _HELD_LOCKS.pop(key, None)
            if handle is not None:
                handle.close()


def is_preparing(project_id: str) -> bool:
    with _LOCK:
        return any(item[0] == project_id for item in _QUEUED)
