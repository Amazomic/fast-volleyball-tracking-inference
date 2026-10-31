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
from urllib.parse import parse_qs, unquote, urlsplit

from .data import RESULTS, ROOT, assign, find_cache, list_videos, report, unassign, video_path, video_renders

PUBLIC = Path(__file__).with_name("static")
RUN_DIR = RESULTS / "ui_runs"
JOB_LOCK = threading.RLock()
JOB = {"process": None, "id": None, "kind": None, "log": None, "started_at": None, "command": []}


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
    source = video_path(video_name)
    with JOB_LOCK:
        if JOB["process"] and JOB["process"].poll() is None:
            raise ValueError("Уже запущена обработка — дождитесь завершения или остановите её")
        if kind == "analyze":
            backend = options.get("backend", "onnx")
            if backend not in ("onnx", "openvino"):
                raise ValueError("Неизвестный backend")
            analysis_width = int(options.get("analysis_width", 1280))
            threads = int(options.get("threads", 8))
            min_rally = float(options.get("min_rally", 1.5))
            confidence = float(options.get("min_confidence", 0.5))
            start = float(options.get("analysis_start", 0))
            length = float(options.get("analysis_seconds", 0))
            if not (320 <= analysis_width <= 1920 and analysis_width % 2 == 0):
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
        elif kind == "highlights":
            duration = float(options.get("duration", 120))
            if not 10 <= duration <= 600:
                raise ValueError("Длина ролика: от 10 до 600 секунд")
            if not find_cache(source.name):
                raise ValueError("Сначала запустите анализ этого видео")
            cmd = [
                sys.executable, str(ROOT / "scripts" / "build_match_highlights.py"),
                "--video", str(source), "--duration", str(duration),
            ]
        else:
            raise ValueError("Неизвестный тип задачи")
        RUN_DIR.mkdir(parents=True, exist_ok=True)
        job_id = uuid.uuid4().hex[:10]
        log_file = RUN_DIR / (job_id + ".log")
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
            if route == "/api/report":
                return self._json(report(query.get("video", [""])[0]))
            if route == "/api/renders":
                return self._json({"videos": video_renders()})
            if route == "/api/job":
                return self._json(job_state())
            if route.startswith("/media/"):
                filename = unquote(route[len("/media/"):])
                names = {x["name"] for x in video_renders()}
                if filename not in names:
                    return self._error("Видео не найдено", 404)
                return self._file(RESULTS / filename, "video/mp4", allow_ranges=True)
            static_files = {
                "/": ("index.html", "text/html; charset=utf-8"),
                "/app.js": ("app.js", "text/javascript; charset=utf-8"),
                "/style.css": ("style.css", "text/css; charset=utf-8"),
            }
            if route in static_files:
                name, mime = static_files[route]
                return self._file(PUBLIC / name, mime)
            self._error("Страница не найдена", 404)
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
            if route == "/api/analyze":
                return self._json(start_job("analyze", body))
            if route == "/api/highlights":
                return self._json(start_job("highlights", body))
            if route == "/api/cancel":
                return self._json(cancel_job())
            if route == "/api/assign":
                return self._json({"groups": assign(body.get("video"), body.get("ids"), body.get("name"))})
            if route == "/api/unassign":
                return self._json({"groups": unassign(body.get("video"), body.get("ids"))})
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
