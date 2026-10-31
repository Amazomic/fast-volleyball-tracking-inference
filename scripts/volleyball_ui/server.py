"""Local-only HTTP server for Volleyball Studio dashboard (Python standard library)."""
from __future__ import annotations

import argparse
import json
import os
import re
import signal
import subprocess
import sys
import threading
import time
import uuid
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, quote, unquote, urlsplit

from .data import RESULTS, ROOT, assign, find_cache, list_videos, report, unassign, video_path, video_renders
from .preview import preview_frame, target_width, video_info
from .court import get_court, save_context, project_context
from .timeline import timeline
from .projects import (all_projects, create_project, update_project, get_project,
                       project_videos, project_summary, project_output, project_report,
                       project_logs_root)
from .photos import photo_path, photos_metadata
from .projects import assert_project_video
from .data import read_json

PUBLIC = Path(__file__).with_name("static")
RUN_DIR = RESULTS / "ui_runs"
JOB_LOCK = threading.RLock()
JOB = {"process": None, "id": None, "kind": None, "log": None, "started_at": None, "command": []}


from functools import lru_cache


@lru_cache(maxsize=32)
def _probe_duration(path: str, mtime_ns: int, size: int) -> float:
    result = subprocess.run(
        ["ffprobe", "-v", "error", "-show_entries", "format=duration",
         "-of", "default=nokey=1:noprint_wrappers=1", path],
        capture_output=True, text=True, timeout=25, check=True,
    )
    return float(result.stdout.strip())


def _normalized_duration(source: Path) -> float:
    stat = source.stat()
    return _probe_duration(str(source), stat.st_mtime_ns, stat.st_size)


def _tail(path: Path | None, limit: int = 14000) -> str:
    if path is None or not path.is_file():
        return ""
    with path.open("rb") as handle:
        handle.seek(max(0, path.stat().st_size - limit))
        return handle.read().decode("utf-8", errors="replace")[-limit:]


def job_state() -> dict:
    with JOB_LOCK:
        process = JOB["process"]
        status = "idle" if not process else "running" if process.poll() is None else (
            "complete" if process.returncode == 0 else "failed" if process.returncode != -signal.SIGTERM else "cancelled"
        )
        log = _tail(JOB["log"])
        progress, stage = None, ""
        for line in log.splitlines():
            if line.startswith("{"):
                try:
                    event = json.loads(line)
                except ValueError:
                    continue
                if "overall_percent" in event:
                    progress = event["overall_percent"]
                if event.get("stage"):
                    stage = event["stage"]
        return {
            "id": JOB["id"], "status": status, "kind": JOB["kind"],
            "progress": progress, "stage": stage, "started_at": JOB["started_at"],
            "log": log[-7000:], "return_code": process.returncode if process and process.poll() is not None else None,
        }


def start_job(kind: str, options: dict) -> dict:
    if not isinstance(options, dict):
        raise ValueError("Неправильный формат параметров")
    video_name = options.get("video")
    project_id = options.get("project_id")
    project_sources = project_videos(project_id) if project_id else None
    source = project_sources[0] if project_sources else video_path(video_name)
    with JOB_LOCK:
        if JOB["process"] and JOB["process"].poll() is None:
            raise ValueError("Уже запущена обработка — дождитесь завершения или остановите её")
        if kind == "analyze":
            backend = options.get("backend", "onnx")
            if backend not in ("onnx", "openvino"):
                raise ValueError("Неизвестный backend")
            analysis_width = (0 if project_sources and options.get("analysis_width", "auto") in ("auto", None, "")
                              else target_width(video_info(source.name), options.get("analysis_width", "auto")))
            threads = int(options.get("threads", 8))
            min_rally = float(options.get("min_rally", 1.5))
            confidence = float(options.get("min_confidence", 0.5))
            start = float(options.get("analysis_start", 0))
            length = float(options.get("analysis_seconds", 0))
            if not (analysis_width == 0 or 320 <= analysis_width <= 1920 and analysis_width % 2 == 0):
                raise ValueError("Ширина анализа должна быть чётной: 320–1920")
            if not (1 <= threads <= 32 and 0.5 <= min_rally <= 30 and 0 < confidence <= 1):
                raise ValueError("Проверьте число потоков, длину розыгрыша и порог уверенности")
            if not (0 <= start < 86400 and 0 <= length <= 86400):
                raise ValueError("Недопустимый диапазон анализа")
            cmd = [
                sys.executable, "-m", "scripts.volleyball_studio", "--input", str(source),
                "--output", str(RUN_DIR), "--work", str(RESULTS / "analysis"),
                "--analyze-only", "--non-interactive", "--backend", backend,
                "--analysis-width", str(analysis_width), "--threads", str(threads),
                "--min-rally", str(min_rally), "--min-confidence", str(confidence),
                "--analysis-start", str(start),
            ]
            if length:
                cmd += ["--analysis-seconds", str(length)]
            if options.get("beach", True):
                cmd += ["--beach"]
            if project_sources:
                cmd = [sys.executable, str(ROOT / "scripts" / "run_project_analysis.py"),
                       "--project-id", project_id] + cmd[cmd.index("--backend"):]
        elif kind == "benchmark":
            if not project_id:
                raise ValueError("Для бенчмарка сначала выберите проект")
            if not video_name or video_name not in [p.name for p in project_sources]:
                raise ValueError("Выберите исходное видео из проекта")
            mode = options.get("mode", "decode")
            if mode not in ("decode", "pipeline"):
                raise ValueError("Неизвестный режим бенчмарка")
            duration = float(options.get("duration", 15))
            start = float(options.get("start", 0))
            width = options.get("analysis_width", "auto")
            backend = options.get("backend", "onnx")
            threads = int(options.get("threads", 8))
            info = video_info(video_name)
            if not (2 <= duration <= 120 and 0 <= start < info["duration_sec"] and
                    info["duration_sec"] - start >= 2):
                raise ValueError("Выберите участок длительностью 2–120 секунд")
            target_width(info, width)
            if backend not in ("onnx", "openvino") or not 1 <= threads <= 32:
                raise ValueError("Некорректный движок или число потоков")
            cmd = [
                sys.executable, str(ROOT / "scripts" / "benchmark_project.py"),
                "--project-id", project_id, "--video", video_name,
                "--mode", mode, "--duration", str(duration),
                "--start", str(start), "--analysis-width", str(width),
                "--backend", backend, "--threads", str(threads),
            ]
        elif kind == "highlights":
            duration = float(options.get("duration", 120))
            if not 10 <= duration <= 600:
                raise ValueError("Длина ролика: от 10 до 600 секунд")
            if project_sources:
                missing = [p.name for p in project_sources if not find_cache(p.name, project_id)]
                if missing:
                    raise ValueError("Сначала проанализируйте все видео: " + ", ".join(missing))
                cmd = [sys.executable, str(ROOT / "scripts" / "build_project_highlights.py"),
                       "--project-id", project_id, "--duration", str(duration)]
            else:
                if not find_cache(source.name):
                    raise ValueError("Сначала запустите анализ этого видео")
                cmd = [sys.executable, str(ROOT / "scripts" / "build_match_highlights.py"),
                       "--video", str(source), "--duration", str(duration)]
        else:
            raise ValueError("Неизвестный тип задачи")
        logs = project_logs_root(project_id) if project_id else RUN_DIR
        logs.mkdir(parents=True, exist_ok=True)
        job_id = uuid.uuid4().hex[:10]
        log_file = logs / (job_id + ".log")
        # Only one UI process: a user, not arbitrary paths or shell text,
        # selects a known video and validated flags. No shell=True.
        log_handle = log_file.open("w", encoding="utf-8")
        try:
            process = subprocess.Popen(
                cmd, cwd=ROOT, stdout=log_handle, stderr=subprocess.STDOUT,
                stdin=subprocess.DEVNULL, start_new_session=True, bufsize=1,
            )
        finally:
            log_handle.close()
        JOB.update(process=process, id=job_id, kind=kind, log=log_file,
                   started_at=time.strftime("%Y-%m-%d %H:%M:%S"), command=cmd)
    return job_state()


def cancel_job():
    with JOB_LOCK:
        process = JOB["process"]
        if process and process.poll() is None:
            os.killpg(process.pid, signal.SIGTERM)
    return job_state()


class Handler(BaseHTTPRequestHandler):
    server_version = "VolleyballStudio/0.1"

    def _json(self, value, status=200):
        payload = json.dumps(value, ensure_ascii=False, default=str).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(payload)))
        self.send_header("Cache-Control", "no-store")
        self.send_header("X-Content-Type-Options", "nosniff")
        self.end_headers()
        self.wfile.write(payload)

    def _error(self, message, status=400):
        self._json({"error": str(message)}, status)

    def _read_body(self):
        length = int(self.headers.get("Content-Length", "0"))
        if not 0 < length <= 65536 or self.headers.get("Content-Type", "").split(";")[0] != "application/json":
            raise ValueError("Ожидался JSON размером не больше 64 КБ")
        return json.loads(self.rfile.read(length))

    def _file(self, path: Path, content_type: str, allow_ranges=False):
        size = path.stat().st_size
        start, end, status = 0, size - 1, 200
        if allow_ranges and self.headers.get("Range"):
            match = re.fullmatch(r"bytes=(\d+)-(\d*)", self.headers["Range"])
            if not match:
                self.send_error(416)
                return
            start = int(match.group(1))
            end = min(size - 1, int(match.group(2))) if match.group(2) else size - 1
            if start > end or start >= size:
                self.send_error(416)
                return
            status = 206
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(end - start + 1))
        self.send_header("X-Content-Type-Options", "nosniff")
        if allow_ranges:
            self.send_header("Accept-Ranges", "bytes")
            if status == 206:
                self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
        self.end_headers()
        with path.open("rb") as inp:
            inp.seek(start)
            remaining = end - start + 1
            try:
                while remaining > 0:
                    data = inp.read(min(256 * 1024, remaining))
                    if not data:
                        break
                    self.wfile.write(data)
                    remaining -= len(data)
            except (BrokenPipeError, ConnectionResetError):
                return

    def do_GET(self):
        parts = urlsplit(self.path)
        route = parts.path
        query = parse_qs(parts.query)
        try:
            if route == "/api/videos":
                return self._json({"videos": list_videos()})
            if route == "/api/projects":
                return self._json({"projects": [project_summary(p["id"]) for p in all_projects()]})
            if route == "/api/project-report":
                return self._json(project_report(query.get("id", [""])[0]))
            if route == "/api/project-context":
                return self._json(project_context(query.get("project_id", [""])[0]))
            if route == "/api/court":
                return self._json({"court": get_court(
                    query.get("project_id", [""])[0], query.get("video", [""])[0])})
            if route == "/api/benchmark-results":
                project_id = query.get("project_id", [""])[0]
                from .projects import project_dir
                result = read_json(project_dir(project_id) / "benchmarks" / "history.json", {"runs": []})
                return self._json({"runs": result.get("runs", [])[-30:]})
            if route == "/api/report":
                return self._json(report(query.get("video", [""])[0], query.get("project_id", [None])[0]))
            if route == "/api/video-info":
                return self._json(video_info(query.get("video", [""])[0]))
            if route == "/api/video-view":
                project_id, name = query.get("project_id", [""])[0], query.get("video", [""])[0]
                assert_project_video(project_id, name)
                original = video_path(name)
                info = video_info(name)
                cache = find_cache(name, project_id)
                normalized = cache / "analysis.mp4" if cache else None
                payload = read_json(cache / "analysis/events.json", {}) if cache else {}
                offset = float(payload.get("analysis_offset_sec") or 0)
                # An existing complete H264 analysis is a browser-compatible preview.
                # Partial analyses must not masquerade as the complete source.
                use_normalized = bool(normalized and normalized.is_file() and
                    offset == 0 and normalized.stat().st_size > 0 and
                    abs(video_info(name)["duration_sec"] - _normalized_duration(normalized)) < 3.0)
                return self._json({
                    "video": name, "duration_sec": info["duration_sec"],
                    "media_url": "/api/video-media?project_id=" + project_id + "&video=" + quote(name),
                    "browser_preview": "normalized" if use_normalized else "original",
                    "preview_url": ("/api/analysis-media?project_id=" + project_id + "&video=" + quote(name)
                                    if use_normalized else
                                    "/api/video-media?project_id=" + project_id + "&video=" + quote(name)),
                    "fallback_warning": ("" if use_normalized else
                         "Формат оригинала может не поддерживать воспроизведение в браузере."),
                    "poster_url": "/api/video-preview?video=" + quote(name) + "&time=2&mode=source"
                })
            if route == "/api/analysis-view":
                project_id, name = query.get("project_id", [""])[0], query.get("video", [""])[0]
                assert_project_video(project_id, name)
                cache = find_cache(name, project_id)
                if not cache or not (cache / "analysis.mp4").is_file():
                    return self._json({"analyzed": False, "video": name, "rallies": [], "actions": []})
                response = timeline(name, project_id)
                response["video"] = name
                response["media_url"] = "/api/analysis-media?project_id=" + project_id + "&video=" + quote(name)
                response["duration_sec"] = _normalized_duration(cache / "analysis.mp4")
                return self._json(response)
            if route == "/api/video-media":
                project_id, name = query.get("project_id", [""])[0], query.get("video", [""])[0]
                assert_project_video(project_id, name)
                source = video_path(name)
                mime = "video/mp4" if source.suffix.lower() in (".mp4", ".m4v") else "video/" + source.suffix.lower().lstrip(".")
                return self._file(source, mime, allow_ranges=True)
            if route == "/api/analysis-media":
                project_id, name = query.get("project_id", [""])[0], query.get("video", [""])[0]
                assert_project_video(project_id, name)
                cache = find_cache(name, project_id)
                if not cache or not (cache / "analysis.mp4").is_file():
                    return self._error("Анализ этого видео ещё не выполнен", 404)
                return self._file(cache / "analysis.mp4", "video/mp4", allow_ranges=True)
            if route == "/api/timeline":
                return self._json(timeline(query.get("video", [""])[0], query.get("project_id", [None])[0]))
            if route == "/api/video-preview":
                img = preview_frame(query.get("video", [""])[0],
                                    query.get("time", ["0"])[0],
                                    query.get("mode", ["source"])[0],
                                    query.get("width", ["auto"])[0])
                return self._file(img, "image/jpeg")
            if route == "/api/player-photos":
                return self._json(photos_metadata(query.get("video", [""])[0], query.get("id", [""])[0], query.get("project_id", [None])[0]))
            if route == "/api/player-photo":
                source = photo_path(query.get("video", [""])[0], query.get("id", [""])[0],
                                    int(query.get("sample", ["0"])[0]), query.get("project_id", [None])[0])
                return self._file(source, "image/jpeg")
            if route == "/api/renders":
                return self._json({"videos": video_renders()})
            if route == "/api/job":
                return self._json(job_state())
            if route == "/api/project-media":
                identifier = query.get("id", [""])[0]
                path = project_output(identifier)
                if not path.is_file():
                    return self._error("Видео проекта не найдено", 404)
                return self._file(path, "video/mp4", allow_ranges=True)
            if route.startswith("/media/"):
                filename = unquote(route[len("/media/"):])
                names = {x["name"] for x in video_renders()}
                if filename not in names:
                    return self._error("Видео не найдено", 404)
                return self._file(RESULTS / filename, "video/mp4", allow_ranges=True)
            static_files = {
                "/": ("index.html", "text/html; charset=utf-8"),
                "/app.js": ("app.js", "text/javascript; charset=utf-8"),
                "/analysis-ui.js": ("analysis-ui.js", "text/javascript; charset=utf-8"),
                "/court-ui.js": ("court-ui.js", "text/javascript; charset=utf-8"),
                "/results-ui.js": ("results-ui.js", "text/javascript; charset=utf-8"),
                "/style.css": ("style.css", "text/css; charset=utf-8"),
            }
            if route in static_files:
                name, mime = static_files[route]
                return self._file(PUBLIC / name, mime)
            self._error("Страница не найдена", 404)
        except FileNotFoundError as exc:
            self._error(exc, 404)
        except (ValueError, OSError) as exc:
            self._error(exc)
        except Exception as exc:
            print(f"UI GET error {route}: {exc}", file=sys.stderr, flush=True)
            self._error("Внутренняя ошибка сервера", 500)

    def do_POST(self):
        route = urlsplit(self.path).path
        # Requests from another website must not be able to start a job.
        origin = self.headers.get("Origin")
        expected = "http://" + self.headers.get("Host", "")
        if origin and origin != expected:
            return self._error("Запрос с другого сайта запрещён", 403)
        try:
            body = self._read_body()
            if route == "/api/project-create":
                return self._json({"project": create_project(body.get("name"), body.get("videos", []), body.get("ranges"))})
            if route == "/api/project-update":
                return self._json({"project": update_project(body.get("id"), body.get("name"), body.get("videos"), body.get("ranges"))})
            if route == "/api/analysis-context":
                return self._json(save_context(
                    body.get("project_id"), body.get("video"), body.get("range"),
                    body.get("court"), body.get("clear_court", False)))
            if route == "/api/analyze":
                return self._json(start_job("analyze", body))
            if route == "/api/highlights":
                return self._json(start_job("highlights", body))
            if route == "/api/benchmark":
                return self._json(start_job("benchmark", body))
            if route == "/api/cancel":
                return self._json(cancel_job())
            if route == "/api/assign":
                return self._json({"groups": assign(body.get("video"), body.get("ids"), body.get("name"), body.get("project_id"))})
            if route == "/api/unassign":
                return self._json({"groups": unassign(body.get("video"), body.get("ids"), body.get("project_id"))})
            self._error("Неизвестная команда", 404)
        except (ValueError, TypeError, json.JSONDecodeError) as exc:
            self._error(exc)
        except Exception as exc:
            print(f"UI POST error {route}: {exc}", file=sys.stderr, flush=True)
            self._error("Внутренняя ошибка сервера", 500)


def main():
    parser = argparse.ArgumentParser(description="Local volleyball analysis dashboard")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8765)
    args = parser.parse_args()
    if args.host not in ("127.0.0.1", "localhost"):
        parser.error("Для безопасности UI доступен только локально")
    RESULTS.mkdir(parents=True, exist_ok=True)
    server = ThreadingHTTPServer((args.host, args.port), Handler)
    print(f"Volleyball UI: http://127.0.0.1:{args.port}", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
