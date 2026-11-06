"""Local-only HTTP server for Volleyball Studio dashboard (Python standard library)."""
from __future__ import annotations

import argparse
import json
import math
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

from .data import (RESULTS, ROOT, assign, create_player, rename_player, delete_player,
                   players_for, global_players, attach_player, find_cache, list_videos,
                   report, unassign, video_path, video_renders)
from .preview import preview_frame, target_width, video_info
from .playback import SEGMENT_SECONDS, get_segment
from .court import get_court, save_context, project_context, reset_context
from .timeline import timeline
from .projects import (all_projects, create_project, update_project, get_project,
                       project_videos, project_summary, project_output, project_report,
                       project_logs_root, archive_project, project_dir)
from .photos import photo_path, photos_metadata
from .player_library import (photo_counts, sync_photos, album_dir, collect_existing_player_photos,
                             album_photos, album_photo_path, remove_album_photo,
                             save_player_photos)
from .projects import assert_project_video
from .data import read_json

PUBLIC = Path(__file__).with_name("static")
# The launcher can distinguish a stale Python process from freshly served JS.
_API_VERSION = 28
_INSTANCE_ID = uuid.uuid4().hex
_RESTART_PENDING = threading.Event()
_SOURCE_FILES = [Path(__file__).with_name(name) for name in
                 ("server.py", "analysis_runs.py", "data.py", "projects.py", "player_identity_groups.py", "player_retracking.py", "player_library.py", "montage_runner.py", "montage_library.py", "analysis_profiles.py", "prepared_videos.py", "video_uploads.py", "local_sources.py", "montage_audio.py", "project_lifecycle.py", "montage_source.py", "montage_encoding.py", "trajectory.py", "player_overlay.py", "player_actions.py", "quick_reel_audio.py", "quick_reel_runner.py", "quick_reel_jobs.py", "quick_reel_prepare.py", "quick_reel_analyze.py", "quick_reel_render.py", "quick_reel_selection.py", "file_selection.py", "video_compat.py")]
_SOURCE_FILES.append(Path(__file__).parents[1] / "volleyball_studio" / "advanced.py")
_SOURCE_FILES.append(Path(__file__).with_name("report_statistics.py"))
_SOURCE_FILES.append(Path(__file__).with_name("timeline.py"))
_SOURCE_STAMPS = {str(p): p.stat().st_mtime_ns for p in _SOURCE_FILES if p.is_file()}
RUN_DIR = RESULTS / "ui_runs"
JOB_LOCK = threading.RLock()
JOB = {"process": None, "id": None, "kind": None, "log": None, "started_at": None,
       "command": [], "started_epoch": None, "finished_epoch": None,
       "progress_event": None, "progress_offset": 0, "progress_buffer": b"",
       "montage_mode": None, "stage_durations": {},
       "project_id": None, "profile_name": None, "quick_request_id": None,
       "run_id": None}


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


def _refresh_progress_events():
    """Read only new log bytes. Large jobs cannot lose earlier stage metadata."""
    path=JOB["log"]
    if path is None or not path.is_file():
        return
    size=path.stat().st_size
    offset=JOB.get("progress_offset",0)
    if size<offset:
        offset=0
        JOB["progress_buffer"]=b""
    with path.open("rb") as handle:
        handle.seek(offset)
        incoming=handle.read()
        JOB["progress_offset"]=handle.tell()
    if not incoming:
        return
    data=JOB.get("progress_buffer",b"")+incoming
    lines=data.split(b"\n")
    JOB["progress_buffer"]=lines.pop()[-8192:]
    for raw in lines:
        if not raw.startswith(b"{"):
            continue
        try:
            event=json.loads(raw.decode("utf-8"))
        except (ValueError,UnicodeError):
            continue
        if event.get("event")!="UI_PROGRESS" or event.get("kind")!=JOB["kind"]:
            continue
        if (JOB["kind"] == "highlights" and JOB.get("montage_mode") and
                event.get("origin") != "montage_runner"):
            # Nested builders may emit percent estimates; the parent runner
            # alone owns the discrete stage count.
            continue
        previous=JOB.get("progress_event")
        if previous is not None and (float(event.get("overall_percent",0)) <
                                      float(previous.get("overall_percent",0))):
            # A completed step must never be undone by late child output.
            continue
        JOB["progress_event"]=event
        if (JOB["kind"]=="analyze" and event.get("phase")=="complete"
                and isinstance(event.get("stage_duration_sec"),(int,float))):
            video_index,stage_index=event.get("video_index"),event.get("stage_index")
            if (isinstance(video_index,int) and video_index>0 and
                    isinstance(stage_index,int) and stage_index>0):
                JOB.setdefault("stage_durations",{})[(video_index,stage_index)]={
                    "video_index":video_index, "stage_index":stage_index,
                    "stage":event.get("stage"),
                    "duration_sec":round(max(0,float(event["stage_duration_sec"])),2)}


def job_state() -> dict:
    with JOB_LOCK:
        process = JOB["process"]
        status = "idle" if not process else "running" if process.poll() is None else (
            "complete" if process.returncode == 0 else "failed" if process.returncode != -signal.SIGTERM else "cancelled"
        )
        if process and status!="running" and JOB.get("finished_epoch") is None:
            JOB["finished_epoch"]=time.time()
        _refresh_progress_events()
        log = _tail(JOB["log"])
        progress, stage = None, ""
        meta=JOB.get("progress_event") if JOB["kind"] in ("analyze","highlights","experiment","quick_reel","reprocess") else None
        if meta is not None:
            progress=meta.get("overall_percent")
            stage=meta.get("stage","")
        else:
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
        if status=="complete":
            progress=100
        if status=="running" and JOB["kind"] in ("analyze","highlights","experiment","quick_reel","reprocess"):
            progress=min(99.9,float(progress or 0))
        started=JOB.get("started_epoch")
        now=JOB.get("finished_epoch") or time.time()
        elapsed=round(max(0,now-started),1) if started else None
        return {
            "id": JOB["id"], "status": status, "kind": JOB["kind"],
            "project_id": JOB.get("project_id"),
            "reel_id": JOB.get("quick_request_id") if JOB.get("kind")=="quick_reel" else None,
            "quick_request_id": JOB.get("quick_request_id"),
            "progress": progress, "stage": stage, "started_at": JOB["started_at"],
            "elapsed_sec":elapsed,
            "finished_at":(time.strftime("%Y-%m-%d %H:%M:%S",
                           time.localtime(JOB["finished_epoch"]))
                           if JOB.get("finished_epoch") is not None else None),
            "stage_percent":meta.get("stage_percent") if meta else None,
            "stage_index":meta.get("stage_index") if meta else None,
            "stage_total":meta.get("stage_total") if meta else None,
            "completed_stages":meta.get("completed_stages") if meta else None,
            "total_stages":meta.get("total_stages") if meta else None,
            "video_index":meta.get("video_index") if meta else None,
            "video_name":meta.get("video_name") if meta else None,
            "videos_total":meta.get("videos_total") if meta else None,
            "frames":meta.get("frames") if meta else None,
            "total_frames":meta.get("total_frames") if meta else None,
            "eta_sec":meta.get("eta_sec") if meta else None,
            "profile_name":(meta.get("profile_name") if meta else None)
                or JOB.get("profile_name"),
            "stage_durations":sorted(JOB.get("stage_durations",{}).values(),
                    key=lambda row:(row["video_index"],row["stage_index"])),
            "log": log[-7000:],
            "return_code": process.returncode if process and process.poll() is not None else None,
        }


def start_job(kind: str, options: dict) -> dict:
    if not isinstance(options, dict):
        raise ValueError("Неправильный формат параметров")
    video_name = options.get("video")
    project_id = None if kind=="quick_reel" else options.get("project_id")
    project_sources = project_videos(project_id) if project_id else None
    source = project_sources[0] if project_sources else (video_path(video_name) if video_name else None)
    if project_sources and kind in ("analyze", "experiment", "highlights"):
        from .prepared_videos import status as preparation_status
        state = preparation_status(project_id, auto_queue=True)
        pending = [row["video"] for row in state["videos"] if row["state"] != "ready"]
        if pending:
            raise ValueError("Сначала дождитесь подготовки видео проекта: " +
                             ", ".join(pending[:4]) +
                             ". Прогресс показан во вкладке «Анализ».")
    with JOB_LOCK:
        if _RESTART_PENDING.is_set():
            raise ValueError("Интерфейс перезапускается — дождитесь его загрузки")
        if JOB["process"] and JOB["process"].poll() is None:
            raise ValueError("Уже запущена обработка — дождитесь завершения или остановите её")
        quick_request_id = None
        if kind == "quick_reel":
            # Quick reels are independent jobs, not ordinary projects or
            # analysis-profile runs. Only results/prepared is shared.
            from .quick_reel_jobs import create_quick_reel
            from .quick_reel_audio import inspect_audio
            videos = options.get("videos", [])
            audio = inspect_audio(options.get("audio_path"))
            quick_request_id, directory = create_quick_reel(videos, audio)
            project_id = None
            cmd = [sys.executable, "-m",
                   "scripts.volleyball_ui.quick_reel_runner",
                   "--id", quick_request_id]
        elif kind == "analyze":
            from scripts.volleyball_studio.ball_models import BALL_MODELS, DEFAULT_BALL_MODEL
            ball_model = options.get("ball_model", DEFAULT_BALL_MODEL)
            if ball_model not in BALL_MODELS:
                raise ValueError("Выберите поддерживаемую модель мяча")
            backend = options.get("backend", "onnx")
            if backend not in ("onnx", "onnx-cpu", "openvino"):
                raise ValueError("Неизвестный backend")
            analysis_width = (0 if project_sources and options.get("analysis_width", "auto") in ("auto", None, "")
                              else target_width(video_info(source.name), options.get("analysis_width", "auto")))
            threads = int(options.get(
                "threads", min(128, max(1, round((getattr(os, "process_cpu_count", os.cpu_count)() or 1) * 0.8)))
            ))
            ball_threshold = float(options.get("ball_threshold", 0.5))
            start = float(options.get("analysis_start", 0))
            length = float(options.get("analysis_seconds", 0))
            if not (analysis_width == 0 or 320 <= analysis_width <= 1920 and analysis_width % 2 == 0):
                raise ValueError("Ширина анализа должна быть чётной: 320–1920")
            if not 1 <= threads <= 128:
                raise ValueError("CPU-потоки: от 1 до 128")
            if not math.isfinite(ball_threshold) or not 0.05 <= ball_threshold <= 0.95:
                raise ValueError("Порог уверенности мяча: от 0,05 до 0,95")
            if not (0 <= start < 86400 and 0 <= length <= 86400):
                raise ValueError("Недопустимый диапазон анализа")
            gpu_device_id = int(options.get("gpu_device_id", 0))
            gpu_memory_limit_mb = int(options.get("gpu_memory_limit_mb", 0))
            if not 0 <= gpu_device_id <= 15:
                raise ValueError("GPU ID: от 0 до 15")
            if not 0 <= gpu_memory_limit_mb <= 131072:
                raise ValueError("Арена VRAM: от 0 до 131072 МБ")
            from scripts.volleyball_studio.advanced import validated, as_cli
            advanced = validated(options)
            cmd = [
                sys.executable, "-m", "scripts.volleyball_studio", "--input", str(source),
                "--output", str(RUN_DIR), "--work", str(RESULTS / "analysis"),
                "--analyze-only", "--non-interactive", "--backend", backend,
                "--ball-model", ball_model,
                "--ball-threshold", str(ball_threshold),
                "--analysis-width", str(analysis_width), "--threads", str(threads),
                "--gpu-device-id", str(gpu_device_id),
                "--gpu-memory-limit-mb", str(gpu_memory_limit_mb),
                "--analysis-start", str(start), *as_cli(advanced),
            ]
            if length:
                cmd += ["--analysis-seconds", str(length)]
            if options.get("beach", True):
                cmd += ["--beach"]
            if project_sources:
                from .analysis_profiles import resolve_profile_reference
                profile_id, name = resolve_profile_reference(
                    options.get("profile_id"), options.get("profile_name"))
                cmd = [sys.executable, str(ROOT / "scripts" / "run_project_analysis.py"),
                       "--project-id", project_id, "--profile-id", profile_id,
                       "--profile-name", name] + cmd[cmd.index("--backend"):]
        elif kind == "reprocess":
            from scripts.volleyball_studio.reprocess import MODES
            from .cpu_defaults import golden_cpu_settings
            golden = golden_cpu_settings()
            # No explicit mode: worker chooses combined Touch peaks for RAVEL-013,
            # or baseline geometry when the source model lacks Touch.
            mode = options.get("mode") or None
            run_id = options.get("run_id")
            touch_threshold = float(options.get("touch_threshold", golden["touch_threshold"]))
            if (not project_id or not isinstance(run_id, str) or not run_id or
                    mode is not None and mode not in MODES or
                    not math.isfinite(touch_threshold) or
                    not 0 <= touch_threshold <= 1):
                raise ValueError("Выберите анализ и корректный режим постобработки")
            touch_gap = float(options.get("touch_min_interval_sec", golden["touch_min_interval_sec"]))
            touch_reach = float(options.get("touch_max_reach_ratio", golden["touch_max_reach_ratio"]))
            if (not math.isfinite(touch_gap) or not .10 <= touch_gap <= 1.5 or
                    not math.isfinite(touch_reach) or not .10 <= touch_reach <= 1.0):
                raise ValueError("Touch: интервал 0.1–1.5 с, допуск 0.1–1 высоты игрока")
            tracking = {}
            for key,lo,hi in (
                ("track_min_duration", .05, 30.),
                ("track_max_distance", 1., 2000.),
                ("track_player_threshold", 0., 1.),
                ("track_ball_threshold", 0., 1.),
            ):
                value = float(options.get(key, golden[key]))
                if not math.isfinite(value) or not lo <= value <= hi:
                    raise ValueError(f"{key}: недопустимое значение {lo}..{hi}")
                tracking[key] = value
            hold_frames = options.get("player_hold_frames", golden["player_hold_frames"])
            if (isinstance(hold_frames, bool) or
                    not isinstance(hold_frames, int) or
                    not 0 <= hold_frames <= 180):
                raise ValueError("Удержание ID на CPU: целое число от 0 до 180 кадров")
            match_distance = float(options.get("player_match_distance", golden["player_match_distance"]))
            if not math.isfinite(match_distance) or not .02 <= match_distance <= .30:
                raise ValueError("Расстояние сопоставления игрока: 0.02–0.30")
            cmd = [sys.executable, "-m", "scripts.run_project_reprocess",
                   "--project-id", project_id, "--run-id", run_id,
                   "--touch-threshold", str(touch_threshold),
                   "--touch-min-interval-sec", str(touch_gap),
                   "--touch-max-reach-ratio", str(touch_reach)]
            if mode is not None:
                cmd.extend(("--mode", mode))
            for name,value in tracking.items():
                cmd.extend(("--"+name.replace("_","-"),str(value)))
            cmd.extend(("--player-hold-frames", str(hold_frames),
                        "--player-match-distance", str(match_distance)))
            if options.get("track_no_ball_fill") is True:
                cmd.append("--track-no-ball-fill")
            if options.get("beach", golden["beach"]) is True:
                cmd.append("--beach")
            for key, flag in (("ball_threshold", "--ball-threshold"),
                              ("player_threshold", "--player-threshold")):
                value = options.get(key)
                if value is not None:
                    number = float(value)
                    lower = .02 if key == "player_threshold" else .05
                    if not math.isfinite(number) or not lower <= number <= .95:
                        raise ValueError(f"{key}: threshold must be {lower:.2f}..0.95")
                    cmd += [flag, str(number)]
        elif kind == "experiment":
            from scripts.run_analysis_experiment import validate_config, experiment_root
            config=validate_config(options)
            identifier=time.strftime("%Y%m%d_%H%M%S_")+uuid.uuid4().hex[:8]
            base=experiment_root(config["project_id"])
            base.mkdir(parents=True,exist_ok=True)
            config_file=base/"requests"/(identifier+".json")
            config_file.parent.mkdir(parents=True,exist_ok=True)
            config_file.write_text(json.dumps(config,ensure_ascii=False),encoding="utf-8")
            cmd=[sys.executable,"-m","scripts.run_analysis_experiment",
                 "--config",str(config_file)]
        elif kind == "diagnostics":
            from .diagnostics import validated as diagnostic_settings, paths as diagnostic_paths
            video_name = options.get("video")
            if not project_id or not isinstance(video_name,str):
                raise ValueError("Выберите проект и видео для диагностики")
            diagnostic_paths(project_id,video_name)
            settings = diagnostic_settings(options)
            cmd = [sys.executable,"-m","scripts.volleyball_ui.diagnostics",
                   "--project-id",project_id,"--video",video_name,
                   "--tracks-export-mode",settings["tracks_export_mode"],
                   "--tracks-padding",str(settings["tracks_padding"]),
                   "--show-rally-track",str(settings["show_rally_track"])]
            if settings["tracks_include_not_rally"]:cmd += ["--tracks-include-not-rally"]
            if settings["tracks_debug"]:cmd += ["--tracks-debug"]
            if not settings["tracks_mark_ball"]:cmd += ["--no-tracks-mark-ball"]
            if settings["show_rally_snapshot"]:cmd += ["--show-rally-snapshot"]
        elif kind == "benchmark":
            if not project_id:
                raise ValueError("Для бенчмарка сначала выберите проект")
            from scripts.benchmark_project import available_models, MODELS
            if len(available_models()) != len(MODELS):
                raise ValueError("Для бенчмарка нужны 5 ONNX-моделей: два VballNet и три RAVEL")
            video = str(options.get("video") or "")
            if video and video not in {path.name for path in (project_sources or [])}:
                raise ValueError("Видео отсутствует в выбранном проекте")
            start = float(options.get("start",0))
            threads = int(options.get("threads",min(128,max(1,round((getattr(os, "process_cpu_count", os.cpu_count)() or 1)*.8)))))
            warmup = int(options.get("warmup",3))
            runs = int(options.get("runs",12))
            repeats = int(options.get("repeats",2))
            if not (1<=threads<=128 and 0<=warmup<=30 and 3<=runs<=300 and
                    1<=repeats<=5 and math.isfinite(start) and 0<=start<=86400):
                raise ValueError("Потоки 1–128, прогрев 0–30, замеры 3–300, повторы 1–5, начало 0–86400")
            cmd = [
                sys.executable, str(ROOT / "scripts" / "benchmark_project.py"),
                "--project-id",project_id,"--threads",str(threads),
                "--warmup",str(warmup),"--runs",str(runs),
                "--repeats",str(repeats),"--start",str(start)
            ]
            if video:
                cmd += ["--video", video]
        elif kind == "highlights" and options.get("mode") in ("montage", "tracks", "vertical", "custom"):
            from .montage_runner import validate_request
            if not options.get("run_id"):
                raise ValueError("Выберите профиль результатов анализа для монтажа")
            validated_request = validate_request(options)
            identifier = time.strftime("%Y%m%d_%H%M%S_") + uuid.uuid4().hex[:8]
            requests = project_dir(project_id) / "montages" / "_requests"
            requests.mkdir(parents=True, exist_ok=True)
            request_file = requests / (identifier + ".json")
            validated_request["request_id"] = identifier
            request_file.write_text(json.dumps(validated_request, ensure_ascii=False), encoding="utf-8")
            cmd = [sys.executable, "-m", "scripts.volleyball_ui.montage_runner",
                   "--config", str(request_file)]
        elif kind == "highlights":
            duration = float(options.get("duration", 120))
            min_rally = float(options.get("min_rally", 1.5))
            confidence = float(options.get("min_confidence", .5))
            if not (math.isfinite(duration) and 10 <= duration <= 600):
                raise ValueError("Длина ролика: от 10 до 600 секунд")
            if not (math.isfinite(min_rally) and .5 <= min_rally <= 30 and
                    math.isfinite(confidence) and .05 <= confidence <= 1):
                raise ValueError("Проверьте минимум розыгрыша и порог уверенности для монтажа")
            from scripts.volleyball_studio.advanced import validated, as_cli
            advanced = validated(options)
            if project_sources:
                missing = [p.name for p in project_sources if not find_cache(p.name, project_id)]
                if missing:
                    raise ValueError("Сначала проанализируйте все видео: " + ", ".join(missing))
                cmd = [sys.executable, str(ROOT / "scripts" / "build_project_highlights.py"),
                       "--project-id", project_id, "--duration", str(duration),
                       "--min-rally", str(min_rally), "--min-confidence", str(confidence),
                       *as_cli({k:v for k,v in advanced.items()
                                if k.startswith(("reels_", "montage_"))})]
            else:
                if not find_cache(source.name):
                    raise ValueError("Сначала запустите анализ этого видео")
                cmd = [sys.executable, str(ROOT / "scripts" / "build_match_highlights.py"),
                       "--video", str(source), "--duration", str(duration)]
        else:
            raise ValueError("Неизвестный тип задачи")
        if kind == "quick_reel":
            from .quick_reel_jobs import quick_dir
            logs = quick_dir(quick_request_id) / "logs"
        else:
            logs = project_logs_root(project_id) if project_id else RUN_DIR
        logs.mkdir(parents=True, exist_ok=True)
        job_id = uuid.uuid4().hex[:10]
        log_file = logs / (job_id + ".log")
        # Only one UI process: a user, not arbitrary paths or shell text,
        # selects a known video and validated flags. No shell=True.
        from .project_lifecycle import job_launch_guard
        from contextlib import nullcontext
        launch_guard = nullcontext() if kind=="quick_reel" else job_launch_guard(project_id)
        with launch_guard:
            log_handle = log_file.open("w", encoding="utf-8")
            try:
                process = subprocess.Popen(
                    cmd, cwd=ROOT, stdout=log_handle, stderr=subprocess.STDOUT,
                    stdin=subprocess.DEVNULL, start_new_session=True, bufsize=1,
                    env={**os.environ, "VOLLEY_JOB_LOG_NAME": log_file.name,
                         "VOLLEY_PROJECT_ID": str(project_id or "")},
                )
            finally:
                log_handle.close()
        if kind == "quick_reel":
            from .quick_reel_jobs import publish_latest
            publish_latest(quick_request_id, audio["path"])
        JOB.update(process=process, id=job_id, kind=kind, log=log_file,
                   montage_mode=(options.get("mode") if kind == "highlights" else None),
                   started_at=time.strftime("%Y-%m-%d %H:%M:%S"), command=cmd,
                   started_epoch=time.time(), finished_epoch=None,
                   progress_event=None, progress_offset=0, progress_buffer=b"",
                   stage_durations={}, project_id=project_id,
                   quick_request_id=quick_request_id,
                   run_id=(options.get("run_id") if kind in
                           ("reprocess", "highlights") else None),
                   profile_name=(options.get("profile_name") if kind in
                                 ("analyze", "experiment") else None))
    return job_state()


def cancel_job():
    with JOB_LOCK:
        process = JOB["process"]
        if process and process.poll() is None:
            if os.name == "nt":
                # Windows has no os.killpg; stop descendant CPU/GPU workers too.
                subprocess.run(["taskkill", "/PID", str(process.pid), "/T", "/F"],
                               capture_output=True, check=False)
            else:
                os.killpg(process.pid, signal.SIGTERM)
    return job_state()


def cancel_quick_reel(reel_id: str) -> dict:
    with JOB_LOCK:
        if (JOB.get("kind") != "quick_reel"
                or JOB.get("quick_request_id") != reel_id
                or JOB.get("process") is None
                or JOB["process"].poll() is not None):
            raise ValueError("Нет активного быстрого рилса для отмены")
        return cancel_job()


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

    def _file(self, path: Path, content_type: str, allow_ranges=False, cache_control=None,
              download_name: str | None = None):
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
        if download_name:
            from urllib.parse import quote as encode_file
            self.send_header("Content-Disposition",
                             "attachment; filename*=UTF-8''" + encode_file(download_name))
        if cache_control:
            self.send_header("Cache-Control", cache_control)
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
            if route == "/api/health":
                return self._json({"ok": True, "api_version": _API_VERSION,
                                   "instance_id": _INSTANCE_ID, "pid": os.getpid(),
                                   "restarting": _RESTART_PENDING.is_set(),
                                   "reload_required": any(
                                       not Path(path).is_file() or
                                       Path(path).stat().st_mtime_ns != stamp
                                       for path, stamp in _SOURCE_STAMPS.items())})
            if route == "/api/analysis-resources":
                cores = getattr(os, "process_cpu_count", os.cpu_count)() or 1
                return self._json({"logical_cpu_count": cores,
                                   "default_threads": min(128, max(1, round(cores * 0.8)))})
            if route == "/api/videos":
                return self._json({"videos": list_videos()})
            if route == "/api/quick-reel/latest":
                from .quick_reel_jobs import latest_reel_id
                return self._json({"reel_id": latest_reel_id()})
            if route == "/api/quick-reel/history":
                from .quick_reel_jobs import completed_history
                return self._json({"results": completed_history()})
            if route == "/api/quick-reel/status":
                from .quick_reel_jobs import status_for, request_for, result_for
                reel_id = query.get("id", [""])[0]
                request = request_for(reel_id)
                info = dict(status_for(reel_id))
                state = job_state()
                if (state.get("kind")=="quick_reel" and
                        state.get("reel_id")==reel_id):
                    info["status"] = state["status"]
                    info["stage"] = state.get("stage") or info.get("stage")
                    if state.get("progress") is not None:
                        info["progress"] = state["progress"]
                    if state.get("status")=="failed" and not info.get("error"):
                        info["error"] = "Ошибка обработки. Подробности в журнале быстрого рилса."
                info["available"] = result_for(reel_id) is not None
                info["id"] = reel_id
                info["source_names"] = [row["video"] for row in request["videos"]]
                return self._json(info)
            if route == "/api/projects":
                return self._json({"projects": [project_summary(p["id"]) for p in all_projects()]})
            if route == "/api/local-videos/picker/status":
                from .local_sources import picker_status
                return self._json(picker_status(query.get("picker_id", [""])[0]))
            if route == "/api/project-preparation":
                from .prepared_videos import status
                return self._json(status(query.get("project_id", [""])[0], auto_queue=True))
            if route == "/api/project-report":
                return self._json(project_report(query.get("id", [""])[0], query.get("run_id", [None])[0]))
            if route == "/api/analysis-profiles":
                from .analysis_profiles import all_profiles
                return self._json({"profiles": all_profiles()})
            if route == "/api/analysis-profile-schema":
                from .analysis_profiles import profile_schema
                return self._json({"fields": profile_schema()})
            if route == "/api/analysis-runs":
                from .analysis_runs import list_runs
                return self._json({"runs": list_runs(query.get("project_id", [""])[0])})
            if route == "/api/analysis-history":
                from .analysis_history import list_history
                return self._json({"history": list_history(query.get("project_id", [""])[0])})
            if route == "/api/analysis-history-legacy":
                from .projects import project_run_root
                project_id = query.get("project_id", [""])[0]
                history_dir = project_run_root(project_id) / "analysis_timings"
                history = []
                if history_dir.is_dir():
                    for file in sorted(history_dir.glob("*.json"), reverse=True)[:10]:
                        item = read_json(file, {})
                        if not isinstance(item,dict):
                            continue
                        log_name = item.get("log_name")
                        log_text = ""
                        if isinstance(log_name, str) and re.fullmatch(r"[a-f0-9]{10}\.log", log_name):
                            log_text = _tail(project_logs_root(project_id) / log_name, 7000)
                        history.append({
                            "log": log_text,
                            "started_at": item.get("started_at"),
                            "finished_at": item.get("finished_at"),
                            "status": item.get("status"),
                            "duration_sec": item.get("duration_sec"),
                            "videos_total": item.get("videos_total"),
                            "videos_completed": sum(1 for v in item.get("videos",[])
                                                     if v.get("status")=="complete"),
                            "stage_names": item.get("stage_names", []),
                            "videos": [
                                {"video": v.get("video"), "status": v.get("status"),
                                 "duration_sec": v.get("duration_sec"),
                                 "stages": v.get("stages", [])}
                                for v in item.get("videos", []) if isinstance(v, dict)
                            ],
                        })
                return self._json({"history":history})
            if route == "/api/project-context":
                return self._json(project_context(query.get("project_id", [""])[0]))
            if route == "/api/court":
                return self._json({"court": get_court(
                    query.get("project_id", [""])[0], query.get("video", [""])[0])})
            if route == "/api/benchmark-models":
                from scripts.benchmark_project import available_models, MODELS
                return self._json({"models": available_models(), "expected": list(MODELS),
                                   "devices": ["cpu","cuda"],
                                   "defaults": {"warmup":3,"runs":12,"repeats":2}})
            if route == "/api/benchmark-results":
                project_id = query.get("project_id", [""])[0]
                from .projects import project_dir
                result = read_json(project_dir(project_id) / "benchmarks" / "history.json", {"runs": []})
                try:
                    from scripts.benchmark_project import available_models
                    allowed = set(available_models())
                except ImportError:
                    # Player/analysis views must work even if optional benchmark
                    # dependencies are not available in this Python environment.
                    allowed = set()
                runs = result.get("runs", []) if isinstance(result, dict) else []
                rows = [r for r in runs if isinstance(r, dict) and r.get("model") in allowed] if isinstance(runs, list) else []
                return self._json({"runs": rows[-80:]})
            if route == "/api/players":
                return self._json({"players": players_for(query.get("project_id", [""])[0])})
            if route == "/api/players/library":
                return self._json({"players": global_players()})
            if route == "/api/players/photos":
                return self._json({"counts": photo_counts(), "root": str(RESULTS / "players")})
            if route == "/api/players/album":
                return self._json(album_photos(query.get("player_id", [""])[0]))
            if route == "/api/players/album-photo":
                return self._file(album_photo_path(
                    query.get("player_id", [""])[0],
                    query.get("filename", [""])[0]), "image/jpeg")
            if route == "/api/report":
                return self._json(report(query.get("video", [""])[0], query.get("project_id", [None])[0], query.get("cache_id", [None])[0]))
            if route == "/api/players/actions":
                from .player_actions import get_actions
                return self._json(get_actions(
                    query.get("project_id", [""])[0], query.get("run_id", [""])[0],
                    player_id=query.get("player_id", [None])[0],
                    action=query.get("action", [None])[0]))
            if route == "/api/video-info":
                return self._json(video_info(query.get("video", [""])[0]))
            if route == "/api/video-view":
                project_id, name = query.get("project_id", [""])[0], query.get("video", [""])[0]
                assert_project_video(project_id, name)
                original = video_path(name)
                info = video_info(name)
                # The analysis workspace must preview the SAME normalized
                # source consumed by models. Do not silently play the raw file
                # while the cache is still being transcoded.
                from .prepared_videos import ready_path, status as preparation_status
                prepared = ready_path(project_id, name)
                state = ("ready" if prepared else next(
                    (item["state"] for item in preparation_status(project_id)["videos"]
                     if item["video"] == name), "missing"))
                media_url = ("/api/video-media?project_id=" + quote(project_id) +
                             "&video=" + quote(name)) if prepared else None
                return self._json({
                    "video": name, "duration_sec": info["duration_sec"],
                    "media_url": media_url,
                    "browser_preview": "prepared" if prepared else "preparing",
                    "preparation_state": state,
                    "segment_seconds": SEGMENT_SECONDS,
                    "preview_url": media_url,
                    "fallback_warning": "" if prepared else
                        "Подготовка видео ещё не завершена.",
                    "poster_url": ("/api/video-preview?video=" + quote(name) +
                                   "&time=2&mode=source") if prepared else None,
                })
            if route == "/api/video-segment":
                project_id, name = query.get("project_id", [""])[0], query.get("video", [""])[0]
                assert_project_video(project_id, name)
                try:
                    start_sec = int(query.get("start", ["0"])[0])
                except ValueError as exc:
                    raise ValueError("Некорректная позиция воспроизведения") from exc
                length = video_info(name)["duration_sec"]
                segment = get_segment(name, start_sec, length)
                return self._file(segment, "video/mp4", allow_ranges=True)
            if route == "/api/analysis-rallies":
                project_id, name = query.get("project_id", [""])[0], query.get("video", [""])[0]
                assert_project_video(project_id, name)
                from .analysis_runs import cache_for
                cache = cache_for(name, project_id, query.get("cache_id", [None])[0])
                if not cache or not (cache / "analysis/events.json").is_file():
                    return self._error(
                        "Эта версия ещё не содержит CPU-событий и розыгрышей", 409)
                from .rally_view import settings as rally_settings
                cfg = rally_settings(query.get("gap_sec", ["3"])[0],
                                     query.get("min_sec", ["3"])[0],
                                     query.get("context_sec", ["1.5"])[0])
                duration = _normalized_duration(cache / "analysis.mp4")
                response = timeline(name, project_id, duration_sec=duration,
                                    cache_id=query.get("cache_id", [None])[0],
                                    **cfg)
                response["reconstruction_available"] = (
                    cache / "analysis/game-tracks.json").is_file()
                response["preview_only"] = True
                return self._json(response)
            if route == "/api/analysis-view":
                project_id, name = query.get("project_id", [""])[0], query.get("video", [""])[0]
                assert_project_video(project_id, name)
                from .analysis_runs import cache_for
                cache = cache_for(name, project_id, query.get("cache_id", [None])[0])
                if not cache or not (cache / "analysis.mp4").is_file():
                    return self._json({"analyzed": False, "video": name, "rallies": [], "actions": []})
                if not (cache / "analysis/events.json").is_file():
                    if (cache / "inference.manifest.json").is_file():
                        manifest=read_json(cache/"inference.manifest.json",{})
                        duration=_normalized_duration(cache/"analysis.mp4")
                        from .trajectory import ball_detection_capability
                        from .player_overlay import player_detection_capability
                        return self._json({
                            "analyzed": False, "inference_ready": True,
                            "postprocessed": False,
                            "video": name, "rallies": [], "actions": [],
                            "analysis_offset_sec":float(manifest.get("analysis_offset_sec") or 0),
                            "coordinate_space":{"width":manifest.get("width"),
                                                "height":manifest.get("height")},
                            "duration_sec":duration,
                            "ball_detection":ball_detection_capability(cache),
                            "raw_ball_detection":ball_detection_capability(cache),
                            "player_detection":player_detection_capability(cache),
                            "media_url":("/api/analysis-media?project_id="+quote(project_id)
                                         +"&video="+quote(name)+"&cache_id="+quote(
                                            query.get("cache_id",[None])[0] or cache.name)),
                            "message": "Нейросети завершены. Запустите CPU-обработку для расчёта розыгрышей.",
                        })
                    return self._json({"analyzed": False, "video": name,
                                       "rallies": [], "actions": []})
                from .rally_view import settings as rally_settings
                cfg = rally_settings(query.get("gap_sec", ["3"])[0],
                                     query.get("min_sec", ["3"])[0],
                                     query.get("context_sec", ["1.5"])[0])
                duration = _normalized_duration(cache / "analysis.mp4")
                response = timeline(name, project_id, duration_sec=duration, cache_id=query.get("cache_id", [None])[0], **cfg)
                from .trajectory import ball_detection_capability
                response["ball_detection"] = ball_detection_capability(cache)
                # Raw detector mode always reads the immutable neural parent,
                # regardless of the selected CPU postprocessing algorithm.
                original = cache.parent.parent if cache.parent.name == "postprocessing" else cache
                response["raw_ball_detection"] = ball_detection_capability(original)
                from .player_overlay import player_detection_capability
                response["player_detection"] = player_detection_capability(cache)
                response["video"] = name
                selected_cache_id = query.get("cache_id", [None])[0] or cache.name
                response["media_url"] = "/api/analysis-media?project_id=" + quote(project_id) + "&video=" + quote(name) + "&cache_id=" + quote(selected_cache_id)
                response["duration_sec"] = duration
                touch_mp4 = cache / "players-annotated.mp4"
                if touch_mp4.is_file() and touch_mp4.stat().st_size:
                    response["upstream_touch_video_url"] = (
                        "/api/analysis-upstream-touch-media?project_id=" + quote(project_id)
                        + "&video=" + quote(name) + "&cache_id=" + quote(selected_cache_id))
                return self._json(response)
            if route == "/api/analysis-upstream-touch-media":
                project_id, name = query.get("project_id", [""])[0], query.get("video", [""])[0]
                assert_project_video(project_id, name)
                from .analysis_runs import cache_for
                cache = cache_for(name, project_id, query.get("cache_id", [None])[0])
                if not cache or not (cache / "players-annotated.mp4").is_file():
                    return self._error("Для выбранной ветки нет диагностического Touch MP4", 404)
                return self._file(cache / "players-annotated.mp4", "video/mp4",
                                  allow_ranges=True, download_name="ravel_touch_upstream.mp4")
            if route == "/api/video-media":
                project_id, name = query.get("project_id", [""])[0], query.get("video", [""])[0]
                assert_project_video(project_id, name)
                from .prepared_videos import ready_path
                source = ready_path(project_id, name)
                if source is None:
                    return self._error("Подготовленное видео ещё недоступно", 409)
                return self._file(source, "video/mp4", allow_ranges=True)
            if route == "/api/diagnostic-outputs":
                from .diagnostics import outputs
                return self._json({"files":outputs(
                    query.get("project_id", [""])[0],
                    query.get("video", [""])[0])})
            if route == "/api/diagnostic-file":
                from .diagnostics import output_path
                path = output_path(
                    query.get("project_id", [""])[0],
                    query.get("video", [""])[0],
                    query.get("name", [""])[0])
                return self._file(path, "image/png" if path.suffix.lower()==".png" else "video/mp4",
                                  allow_ranges=True)
            if route == "/api/analysis-model-touches":
                from .model_touches import touch_predictions
                project_id, name = query.get("project_id", [""])[0], query.get("video", [""])[0]
                return self._json(touch_predictions(
                    project_id, name, query.get("cache_id", [None])[0],
                    threshold=query.get("threshold", ["0.5"])[0],
                    start=query.get("start", [None])[0],
                    length=query.get("length", ["8"])[0]))
            if route == "/api/analysis-player-boxes":
                from .player_overlay import player_boxes_window
                project_id, name = query.get("project_id", [""])[0], query.get("video", [""])[0]
                return self._json(player_boxes_window(project_id, name,
                    query.get("start", ["0"])[0], query.get("length", ["6"])[0],
                    query.get("cache_id", [None])[0],
                    threshold=query.get("threshold", [None])[0]))
            if route == "/api/analysis-experiments":
                from scripts.run_analysis_experiment import history
                return self._json({"experiments":history(
                    query.get("project_id",[""])[0],limit=50)})
            if route == "/api/analysis-experiment-media":
                from scripts.run_analysis_experiment import result_media
                path=result_media(query.get("project_id",[""])[0],
                                  query.get("id",[""])[0])
                return self._file(path,"video/mp4",allow_ranges=True)
            if route == "/api/analysis-ravel-ball":
                from .ravel_ball import ravel_ball_window
                project_id, name = query.get("project_id", [""])[0], query.get("video", [""])[0]
                return self._json(ravel_ball_window(
                    project_id, name, query.get("start", ["0"])[0],
                    query.get("length", ["8"])[0],
                    query.get("cache_id", [None])[0],
                    threshold=query.get("threshold", [None])[0]))
            if route == "/api/analysis-trajectory":
                project_id, name = query.get("project_id", [""])[0], query.get("video", [""])[0]
                from .trajectory import trajectory_window
                return self._json(trajectory_window(project_id, name,
                    query.get("start", ["0"])[0], query.get("length", ["10"])[0],
                    query.get("cache_id", [None])[0],
                    threshold=query.get("threshold", [None])[0],
                    include_scores=query.get("scores", ["0"])[0] == "1"))
            if route == "/api/analysis-media":
                project_id, name = query.get("project_id", [""])[0], query.get("video", [""])[0]
                assert_project_video(project_id, name)
                from .analysis_runs import cache_for
                cache = cache_for(name, project_id, query.get("cache_id", [None])[0])
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
            if route == "/api/player-groups":
                from .player_identity_groups import overview
                return self._json(overview(
                    query.get("project_id", [""])[0], query.get("video", [""])[0],
                    query.get("cache_id", [None])[0]))
            if route == "/api/player-suggestions":
                from .player_tracks import suggest_continuations
                project_id = query.get("project_id", [""])[0]
                video = query.get("video", [""])[0]
                track_id = query.get("id", [""])[0]
                return self._json(suggest_continuations(video, project_id, track_id,
                    cache_id=query.get("cache_id", [None])[0]))
            if route == "/api/player-photos":
                return self._json(photos_metadata(query.get("video", [""])[0], query.get("id", [""])[0], query.get("project_id", [None])[0], query.get("cache_id", [None])[0]))
            if route == "/api/player-photo":
                source = photo_path(query.get("video", [""])[0], query.get("id", [""])[0],
                                    int(query.get("sample", ["0"])[0]), query.get("project_id", [None])[0],
                                    query.get("cache_id", [None])[0])
                return self._file(source, "image/jpeg")
            if route == "/api/renders":
                return self._json({"videos": video_renders()})
            if route == "/api/job":
                return self._json(job_state())
            if route in ("/api/quick-reel/result","/api/quick-reel/media",
                         "/api/quick-reel/download"):
                from .quick_reel_jobs import result_for
                reel_id = query.get("id", [""])[0]
                result = result_for(reel_id)
                if result is None:
                    return self._json({"available":False}) if route.endswith("/result") else self._error("Рилс пока не готов",404)
                manifest, reel_video_path = result
                query_string = "?id=" + quote(reel_id)
                if route.endswith("/result"):
                    return self._json({"available":True,
                       "duration":manifest.get("duration"),
                       "music_duration":manifest.get("music_duration"),
                       "videos":manifest.get("videos",[]),
                       "url":"/api/quick-reel/media"+query_string,
                       "download_url":"/api/quick-reel/download"+query_string})
                return self._file(reel_video_path,"video/mp4",allow_ranges=True,
                    download_name=("quick_reel.mp4" if route.endswith("/download") else None),
                    cache_control="no-store")
            if route == "/api/montage/outputs":
                from .montage_library import listing
                return self._json({"videos": listing(query.get("project_id", [""])[0])})
            if route in ("/api/montage/media", "/api/montage/download",
                         "/api/montage/preview-media", "/api/montage/preview"):
                from .montage_library import output_path, preview_file, preview_info
                pid, fid = query.get("project_id", [""])[0], query.get("id", [""])[0]
                if route == "/api/montage/preview":
                    return self._json(preview_info(pid, fid))
                path = preview_file(pid, fid) if route == "/api/montage/preview-media" else output_path(pid, fid)
                return self._file(path, "video/mp4", allow_ranges=True,
                                  download_name=path.name if route == "/api/montage/download" else None,
                                  cache_control="no-store")
            if route == "/api/vertical-reels":
                from .projects import project_dir
                identifier = query.get("project_id", [""])[0]
                root = project_dir(identifier) / "vertical_reels"
                files = []
                if root.is_dir():
                    for file in sorted(root.rglob("reel_*.mp4")):
                        if file.is_file() and len(files)<100:
                            relative = file.relative_to(root).as_posix()
                            files.append({
                                "name": relative,
                                "size_mb": round(file.stat().st_size/(1024*1024),1),
                                "url": "/api/vertical-reel-media?project_id="+quote(identifier)+
                                       "&name="+quote(relative),
                            })
                return self._json({"videos": files})
            if route == "/api/vertical-reel-media":
                from .projects import project_dir
                identifier = query.get("project_id", [""])[0]
                root = (project_dir(identifier) / "vertical_reels").resolve()
                requested = query.get("name", [""])[0]
                allowed = sorted(p.relative_to(root).as_posix()
                                 for p in root.rglob("reel_*.mp4")
                                 if p.is_file()) if root.is_dir() else []
                if requested not in allowed:
                    return self._error("Вертикальный ролик не найден",404)
                path = (root / requested).resolve()
                if not path.is_relative_to(root):
                    return self._error("Недопустимый путь",400)
                return self._file(path, "video/mp4", allow_ranges=True)
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
                "/data-cache.js": ("data-cache.js", "text/javascript; charset=utf-8"),
                "/cpu-defaults.json": ("cpu-defaults.json", "application/json; charset=utf-8"),
                "/player-photo-hover.js": ("player-photo-hover.js", "text/javascript; charset=utf-8"),
                "/header-chain.js": ("header-chain.js", "text/javascript; charset=utf-8"),
                "/header-chain-menu.js": ("header-chain-menu.js", "text/javascript; charset=utf-8"),
                "/quick-reel-ui.js": ("quick-reel-ui.js", "text/javascript; charset=utf-8"),
                "/quick-reel.css": ("quick-reel.css", "text/css; charset=utf-8"),
                "/i18n.js": ("i18n.js", "text/javascript; charset=utf-8"),
                "/i18n-en.js": ("i18n-en.js", "text/javascript; charset=utf-8"),
                "/analysis-ui.js": ("analysis-ui.js", "text/javascript; charset=utf-8"),
                "/court-ui.js": ("court-ui.js", "text/javascript; charset=utf-8"),
                "/model-touches-ui.js": ("model-touches-ui.js", "text/javascript; charset=utf-8"),
                "/results-ui.js": ("results-ui.js", "text/javascript; charset=utf-8"),
                "/results-diagnostics.js": ("results-diagnostics.js", "text/javascript; charset=utf-8"),
                "/style.css": ("style.css", "text/css; charset=utf-8"),
                "/pipeline-settings.css": ("pipeline-settings.css", "text/css; charset=utf-8"),
                "/montage-player.css": ("montage-player.css", "text/css; charset=utf-8"),
                "/montage-player.js": ("montage-player.js", "text/javascript; charset=utf-8"),
                "/result-media.css": ("result-media.css", "text/css; charset=utf-8"),
                "/settings-tooltips.css": ("settings-tooltips.css", "text/css; charset=utf-8"),
                "/settings-tooltips.js": ("settings-tooltips.js", "text/javascript; charset=utf-8"),
                "/analysis-presets.js": ("analysis-presets.js", "text/javascript; charset=utf-8"),
                "/analysis-experiments.js": ("analysis-experiments.js", "text/javascript; charset=utf-8"),
                "/analysis-experiments.css": ("analysis-experiments.css", "text/css; charset=utf-8"),
                "/analysis-presets.css": ("analysis-presets.css", "text/css; charset=utf-8"),
                "/players-workspace.css": ("players-workspace.css", "text/css; charset=utf-8"),
                "/favicon.svg": ("favicon.svg", "image/svg+xml"),
                "/favicon.ico": ("favicon.ico", "image/x-icon"),
                "/favicon-32.png": ("favicon-32.png", "image/png"),
            }
            if route in static_files:
                name, mime = static_files[route]
                return self._file(PUBLIC / name, mime, cache_control="no-cache")
            self._error("Страница не найдена", 404)
        except FileNotFoundError as exc:
            self._error(exc, 404)
        except (ValueError, OSError) as exc:
            self._error(exc)
        except Exception as exc:
            print(f"UI GET error {route}: {exc}", file=sys.stderr, flush=True)
            self._error("Внутренняя ошибка сервера", 500)

    def do_PUT(self):
        route = urlsplit(self.path).path
        if route not in ("/api/video-upload/chunk", "/api/quick-reel/audio/chunk"):
            return self._error("Страница не найдена", 404)
        origin = self.headers.get("Origin")
        expected = "http://" + self.headers.get("Host", "")
        if origin and origin != expected:
            return self._error("Запрос с другого сайта запрещён", 403)
        try:
            if self.headers.get("Content-Type", "").split(";")[0] != "application/octet-stream":
                raise ValueError("Ожидалась двоичная часть видео")
            from .video_uploads import append as video_append
            from .quick_reel_audio import append as audio_append
            append = audio_append if route == "/api/quick-reel/audio/chunk" else video_append
            query = parse_qs(urlsplit(self.path).query)
            token = query.get("upload_id", [""])[0]
            offset = int(query.get("offset", ["-1"])[0])
            size = int(self.headers.get("Content-Length", "0"))
            return self._json(append(token, offset, self.rfile, size))
        except (ValueError, TypeError) as exc:
            return self._error(exc)
        except Exception as exc:
            print("Video upload error:", exc, file=sys.stderr, flush=True)
            return self._error("Не удалось загрузить часть видео", 500)

    def do_POST(self):
        route = urlsplit(self.path).path
        # Requests from another website must not be able to start a job.
        origin = self.headers.get("Origin")
        expected = "http://" + self.headers.get("Host", "")
        if origin and origin != expected:
            return self._error("Запрос с другого сайта запрещён", 403)
        try:
            body = self._read_body()
            if route == "/api/system/restart":
                with JOB_LOCK:
                    if JOB["process"] is not None and JOB["process"].poll() is None:
                        return self._error("Идёт анализ, монтаж или пересчёт. Дождитесь завершения.", 409)
                    if _RESTART_PENDING.is_set():
                        return self._json({"accepted": True, "instance_id": _INSTANCE_ID})
                    _RESTART_PENDING.set()
                    self.server.reload_requested.set()
                self._json({"accepted": True, "instance_id": _INSTANCE_ID})
                # Shutdown runs outside the request thread, after the response.
                threading.Thread(target=self.server.shutdown, daemon=True,
                                 name="volleyball-manual-restart").start()
                return
            if route == "/api/quick-reel/audio/start":
                from .quick_reel_audio import start
                return self._json(start(body.get("filename"),body.get("size")))
            if route == "/api/quick-reel/audio/finish":
                from .quick_reel_audio import finish
                return self._json(finish(body.get("upload_id")))
            if route == "/api/quick-reel/audio/abort":
                from .quick_reel_audio import abort
                return self._json(abort(body.get("upload_id")))
            if route == "/api/quick-reel/audio-info":
                from .quick_reel_audio import inspect_audio
                return self._json(inspect_audio(body.get("path")))
            if route == "/api/quick-reel/start":
                return self._json(start_job("quick_reel", body))
            if route == "/api/quick-reel/cancel":
                return self._json(cancel_quick_reel(body.get("id")))
            if route == "/api/quick-reel/delete":
                from .quick_reel_jobs import delete_completed
                return self._json(delete_completed(body.get("id")))
            if route == "/api/video-upload/start":
                from .video_uploads import start
                return self._json(start(body.get("filename"), body.get("size")))
            if route == "/api/video-upload/finish":
                from .video_uploads import finish
                return self._json(finish(body.get("upload_id")))
            if route == "/api/video-upload/abort":
                from .video_uploads import abort
                return self._json(abort(body.get("upload_id")))
            if route == "/api/local-videos/add":
                from .local_sources import register_paths
                return self._json({"videos": register_paths(body.get("paths"))})
            if route == "/api/quick-reel/resolve-files":
                from .file_selection import resolve_files
                return self._json(resolve_files(body.get("files"),
                                                kind=body.get("kind")))
            if route == "/api/project/resolve-files":
                from .file_selection import resolve_files
                return self._json(resolve_files(body.get("files"),
                                                kind="video", project_sources=True))
            if route == "/api/local-videos/picker/start":
                from .local_sources import begin_picker
                return self._json(begin_picker(body.get("kind", "video")))
            if route == "/api/local-videos/picker/cancel":
                from .local_sources import cancel_picker
                return self._json(cancel_picker(body.get("picker_id")))
            if route == "/api/project-create":
                from .prepared_videos import queue_project
                project = create_project(body.get("name"), body.get("videos", []), body.get("ranges"), body.get("player_ids"))
                try:
                    queue_project(project["id"])
                except Exception as exc:
                    # Creation has already been committed. Return its ID rather
                    # than an HTTP error that causes a duplicate create attempt.
                    print("PROJECT PREPARATION QUEUE FAILED: " + project["id"] +
                          ": " + str(exc), file=sys.stderr, flush=True)
                return self._json({"project": project})
            if route == "/api/project-update":
                from .prepared_videos import queue_project
                project = update_project(body.get("id"), body.get("name"), body.get("videos"), body.get("ranges"), body.get("player_ids"))
                try:
                    queue_project(project["id"])
                except Exception as exc:
                    print("PROJECT PREPARATION QUEUE FAILED: " + project["id"] +
                          ": " + str(exc), file=sys.stderr, flush=True)
                return self._json({"project": project})
            if route in ("/api/analysis-run/rename", "/api/analysis-run/delete"):
                from .analysis_runs import rename_run, delete_run
                project_id = body.get("project_id")
                run_id = body.get("run_id")
                kind = body.get("kind")
                if kind not in ("inference", "postprocess"):
                    raise ValueError("Укажите тип записи: первичный анализ или CPU-версия")
                # Editing an entry is metadata-only. A GPU run or a CPU replay
                # of ANOTHER entry must not block deletion from this list.
                # Protect only the run currently consumed by an active task;
                # deleting a parent also hides every child of that parent.
                with JOB_LOCK:
                    active = (JOB["process"] is not None and
                              JOB["process"].poll() is None and
                              JOB.get("project_id") == project_id)
                    used_run = JOB.get("run_id") if active else None
                if (used_run and (run_id == used_run or
                        kind == "inference" and
                        used_run.startswith(str(run_id) + "_pp_"))):
                    raise ValueError("Этот анализ используется текущей задачей. Выберите другую запись")
                if route.endswith("/rename"):
                    return self._json(rename_run(project_id, run_id, body.get("name"),
                                                 kind=kind))
                return self._json(delete_run(project_id, run_id, kind=kind))
            if route == "/api/project-preparation/retry":
                from .prepared_videos import queue_project, status
                project_id = body.get("project_id")
                queue_project(project_id, retry_failed=True)
                return self._json(status(project_id))
            if route == "/api/project-archive":
                from .project_lifecycle import archive_with_cancellation
                return self._json({"project": archive_with_cancellation(body.get("id"))})
            if route == "/api/project-delete":
                from .project_lifecycle import delete_permanently_with_cancellation
                return self._json({"project": delete_permanently_with_cancellation(body.get("id"))})
            if route == "/api/analysis-context":
                return self._json(save_context(
                    body.get("project_id"), body.get("video"), body.get("range"),
                    body.get("court"), body.get("clear_court", False)))
            if route == "/api/analysis-context-reset":
                return self._json(reset_context(body.get("project_id"), body.get("video")))
            if route == "/api/montage/plan":
                from .montage_runner import preview_selection
                if not body.get("run_id"):
                    raise ValueError("Выберите профиль результатов анализа для монтажа")
                return self._json(preview_selection(body))
            if route == "/api/analysis-profiles/save":
                from .analysis_profiles import save_profile
                return self._json({"profile": save_profile(body.get("name"), body.get("values"))})
            if route == "/api/analysis-profiles/delete":
                from .analysis_profiles import remove_profile
                return self._json(remove_profile(body.get("id")))
            if route == "/api/analyze":
                return self._json(start_job("analyze", body))
            if route == "/api/reprocess":
                return self._json(start_job("reprocess", body))
            if route == "/api/analysis-experiment":
                return self._json(start_job("experiment", body))
            if route == "/api/highlights":
                return self._json(start_job("highlights", body))
            if route == "/api/montage/delete":
                from .montage_library import remove
                with JOB_LOCK:
                    if JOB["process"] and JOB["process"].poll() is None:
                        raise ValueError("Нельзя удалять видео во время обработки")
                    return self._json(remove(body.get("project_id"), body.get("id")))
            if route == "/api/montage/folder":
                from .montage_library import show_folder
                return self._json(show_folder(body.get("project_id"), body.get("id")))
            if route == "/api/diagnostics":
                return self._json(start_job("diagnostics", body))
            if route == "/api/benchmark":
                return self._json(start_job("benchmark", body))
            if route == "/api/cancel":
                return self._json(cancel_job())
            if route == "/api/players/recalculate":
                from .player_actions import recalculate
                return self._json(recalculate(body.get("project_id"), body.get("run_id")))
            if route == "/api/players/create":
                return self._json({"players": create_player(body.get("name"), body.get("project_id"))})
            if route == "/api/players/attach":
                return self._json({"players": attach_player(body.get("player_id"), body.get("project_id"))})
            if route == "/api/players/photos/remove":
                return self._json(remove_album_photo(
                    body.get("player_id"), body.get("filename")))
            if route == "/api/players/photos/save":
                project_id, video = body.get("project_id"), body.get("video")
                assert_project_video(project_id, video)
                return self._json(save_player_photos(
                    project_id, video, body.get("player_id"), body.get("cache_id")))
            if route == "/api/players/photos/collect":
                player_id = body.get("player_id")
                album_dir(player_id)  # Reject unknown athlete IDs before starting a worker.
                threading.Thread(target=_collect_photos_safe, args=(player_id,),
                                 daemon=True, name="player-photo-backfill").start()
                return self._json({"status": "started", "player_id": player_id})
            if route == "/api/players/rename":
                return self._json({"players": rename_player(body.get("player_id"), body.get("name"), body.get("project_id"))})
            if route == "/api/players/delete":
                return self._json({"players": delete_player(body.get("player_id"), body.get("project_id"))})
            if route == "/api/player-groups/start":
                from .player_identity_groups import start
                return self._json(start(body.get("project_id"), body.get("video"), body.get("cache_id")))
            if route == "/api/player-groups/threshold":
                from .player_identity_groups import set_threshold
                return self._json(set_threshold(
                    body.get("project_id"), body.get("video"), body.get("cache_id"),
                    body.get("threshold_percent")))
            if route == "/api/player-groups/auto-assign":
                from .player_identity_groups import set_auto_assign
                return self._json(set_auto_assign(body.get("project_id"),
                    body.get("video"), body.get("cache_id"),
                    body.get("threshold_percent"), body.get("enabled")))
            if route == "/api/player-groups/modify":
                from .player_identity_groups import modify
                return self._json(modify(body.get("project_id"), body.get("video"), body.get("cache_id"),
                                         body.get("action"), body.get("ids")))
            if route == "/api/player-groups/link":
                from .player_identity_groups import link_group
                project_id, video = body.get("project_id"), body.get("video")
                result = link_group(project_id, video, body.get("cache_id"),
                                    body.get("group_id"), body.get("player_id"))
                threading.Thread(target=_sync_photos_safe,
                                 args=(project_id, video, result.get("ids", []), body.get("cache_id")),
                                 daemon=True, name="player-group-photo-enrollment").start() if result.get("ids") else None
                return self._json(result)
            if route == "/api/assign":
                groups = assign(body.get("video"), body.get("ids"), body.get("name"),
                                body.get("project_id"), body.get("player_id"), body.get("cache_id"))
                if body.get("project_id"):
                    threading.Thread(target=_sync_photos_safe,
                                     args=(body["project_id"], body["video"], body["ids"], body.get("cache_id")),
                                     daemon=True, name="player-photo-enrollment").start()
                return self._json({"groups": groups, "photo_collection": "started"})
            if route == "/api/players/exclude-tracks":
                from .data import exclude_tracks
                ids = exclude_tracks(body.get("video"), body.get("ids"),
                                     body.get("project_id"), body.get("cache_id"))
                return self._json({"excluded_track_ids": ids})
            if route == "/api/unassign":
                groups = unassign(body.get("video"), body.get("ids"), body.get("project_id"), body.get("cache_id"))
                if body.get("project_id"):
                    threading.Thread(target=_sync_photos_safe,
                                     args=(body["project_id"], body["video"], body["ids"], body.get("cache_id")),
                                     daemon=True, name="player-photo-reconcile").start()
                return self._json({"groups": groups})
            self._error("Неизвестная команда", 404)
        except (ValueError, TypeError, FileNotFoundError, json.JSONDecodeError) as exc:
            self._error(exc)
        except Exception as exc:
            print(f"UI POST error {route}: {exc}", file=sys.stderr, flush=True)
            self._error("Внутренняя ошибка сервера", 500)


def _sync_photos_safe(project_id: str, video: str, ids: list, cache_id=None) -> None:
    try:
        result = sync_photos(project_id, video, ids, cache_id)
        if result["errors"]:
            print("Player-photo enrollment:", "; ".join(result["errors"][:5]),
                  file=sys.stderr, flush=True)
    except Exception as error:
        print(f"Player-photo enrollment error: {error}", file=sys.stderr, flush=True)


def _collect_photos_safe(player_id: str) -> None:
    try:
        result = collect_existing_player_photos(player_id)
        if result["errors"]:
            print("Player-photo backfill:", "; ".join(result["errors"][:5]),
                  file=sys.stderr, flush=True)
    except Exception as error:
        print(f"Player-photo backfill error: {error}", file=sys.stderr, flush=True)


def main():
    parser = argparse.ArgumentParser(description="Local volleyball analysis dashboard")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8765)
    args = parser.parse_args()
    if args.host not in ("127.0.0.1", "localhost"):
        parser.error("Для безопасности UI доступен только локально")
    RESULTS.mkdir(parents=True, exist_ok=True)
    try:
        server = ThreadingHTTPServer((args.host, args.port), Handler)
    except OSError as error:
        # Binding the listening port is the single-instance guarantee.
        if error.errno in (98, 10048):
            print(f"Volleyball UI already listens on {args.host}:{args.port}; "
                  "not starting another instance.", flush=True)
            return
        raise
    reload_requested = threading.Event()
    server.reload_requested = reload_requested

    def reload_when_idle():
        """Adopt backend updates without leaving new controls on an old API."""
        while not reload_requested.wait(2.5):
            changed = any(
                not Path(path).is_file() or Path(path).stat().st_mtime_ns != stamp
                for path, stamp in _SOURCE_STAMPS.items()
            )
            if not changed:
                continue
            with JOB_LOCK:
                active = JOB["process"] is not None and JOB["process"].poll() is None
            if active:
                continue
            with JOB_LOCK:
                if _RESTART_PENDING.is_set():
                    return
                if JOB["process"] is not None and JOB["process"].poll() is None:
                    continue
                _RESTART_PENDING.set()
                reload_requested.set()
            server.shutdown()

    threading.Thread(target=reload_when_idle, daemon=True,
                     name="volleyball-backend-reload").start()
    print(f"Volleyball UI: http://127.0.0.1:{args.port}", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
    if reload_requested.is_set():
        print("Volleyball UI: backend changed, restarting idle server", flush=True)
        os.execv(sys.executable, [
            sys.executable, "-m", "scripts.volleyball_ui",
            "--host", args.host, "--port", str(args.port),
        ])


if __name__ == "__main__":
    main()
