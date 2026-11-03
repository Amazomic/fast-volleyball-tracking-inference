"""Archive a project only after cancelling all of its in-flight work.

The tombstone and lock are shared between independent UI servers. The
Linux process-group scan handles analysis/montage subprocess trees started
by a different server; all matching rules are project-scoped.
"""
from __future__ import annotations

import fcntl
from contextlib import contextmanager
import os
import signal
import time
from pathlib import Path

from . import projects

def _project_processes(project_id: str) -> dict[int, int]:
    """Return pid -> process group for child workloads, never the UI server."""
    directory = str(projects.PROJECTS_DIR / project_id) + "/"
    current = os.getpid()
    found = {}
    for entry in Path("/proc").iterdir():
        if not entry.name.isdecimal():
            continue
        pid = int(entry.name)
        if pid == current:
            continue
        try:
            raw = (entry / "cmdline").read_bytes()
            if not raw:
                continue
            args = [x.decode("utf-8", "surrogateescape")
                    for x in raw.split(b"\x00") if x]
            executable = Path(args[0]).name.lower()
            if not any(x in executable for x in ("python", "ffmpeg", "ffprobe")):
                continue
            same_project = any(
                arg == "--project-id" and i + 1 < len(args) and
                args[i+1] == project_id
                for i, arg in enumerate(args)
            )
            has_project_path = any(
                directory in arg
                for arg in args[1:]
            )
            # Child subprocesses inherit this value. It is explicitly set
            # for every new UI job, including workers with no project CLI.
            try:
                environment = (entry / "environ").read_bytes()
                inherited = (b"VOLLEY_PROJECT_ID=" +
                             project_id.encode("ascii") + b"\x00") in (
                                 b"\x00" + environment)
            except (OSError, UnicodeEncodeError):
                inherited = False
            if executable.startswith("ffmpeg") or executable.startswith("ffprobe"):
                match = has_project_path or inherited
            else:
                # All descendants of a UI job inherit the project tag,
                # including model backends with no project CLI argument.
                match = same_project or has_project_path or inherited
            if not match:
                continue
            pgid = os.getpgid(pid)
            found[pid] = pgid
        except (OSError, ProcessLookupError, PermissionError, ValueError):
            continue
    return found


def _alive(pid: int) -> bool:
    try:
        state = (Path("/proc") / str(pid) / "stat").read_text().rsplit(")", 1)[1].strip()
        return bool(state and state[0] not in "ZX")
    except (FileNotFoundError, ProcessLookupError):
        return False


def _signal_workloads(processes: dict[int, int], sig: signal.Signals) -> None:
    own_group = os.getpgrp()
    groups = set()
    for pid, pgid in processes.items():
        if not _alive(pid):
            continue
        try:
            if pgid != own_group and pgid == pid:
                groups.add(pgid)
            elif pgid != own_group:
                # An orphaned descendant may remain after its group leader
                # exits. Kill its group only if it is not the UI server's.
                groups.add(pgid)
            else:
                os.kill(pid, sig)
        except ProcessLookupError:
            pass
    for group in groups:
        try:
            os.killpg(group, sig)
        except ProcessLookupError:
            pass


def stop_project_workloads(project_id: str, timeout: float = 12) -> None:
    """Stop matching process groups on both servers, never unrelated jobs."""
    deadline = time.monotonic() + timeout
    forced = False
    while True:
        processes = _project_processes(project_id)
        if not processes:
            return
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise RuntimeError(
                "Не удалось остановить все процессы проекта. Удаление отменено.")
        if remaining < 4 and not forced:
            _signal_workloads(processes, signal.SIGKILL)
            forced = True
        elif not forced:
            _signal_workloads(processes, signal.SIGTERM)
        time.sleep(min(0.25, max(0.05, remaining)))


@contextmanager
def job_launch_guard(project_id: str | None):
    """Atomic project job start versus archive, across both server ports."""
    if not project_id:
        yield
        return
    if not isinstance(project_id, str) or not project_id.isalnum():
        raise ValueError("Некорректный проект")
    locks = projects.PROJECTS_DIR / ".archive-locks"
    locks.mkdir(parents=True, exist_ok=True)
    with (locks / (project_id + ".lock")).open("a+b") as handle:
        fcntl.flock(handle.fileno(), fcntl.LOCK_EX)
        if projects.deletion_requested(project_id):
            raise ValueError("Проект удаляется: запуск обработки отменён")
        projects.get_project(project_id)
        yield


def archive_with_cancellation(project_id: str) -> dict:
    """Mark deletion, cancel workers, then archive; retain original sources."""
    if not isinstance(project_id, str) or not project_id.isalnum():
        raise ValueError("Некорректный идентификатор проекта")
    projects.get_project(project_id)  # validate before writing anything
    root = projects.PROJECTS_DIR
    locks = root / ".archive-locks"
    locks.mkdir(parents=True, exist_ok=True)
    with (locks / (project_id + ".lock")).open("a+b") as handle:
        fcntl.flock(handle.fileno(), fcntl.LOCK_EX)
        projects.get_project(project_id)
        marker = root / project_id / ".delete-requested"
        marker.write_text(str(os.getpid()), encoding="ascii")
        success = False
        try:
            from .prepared_videos import cancel_project
            cancel_project(project_id)
            stop_project_workloads(project_id)
            # Interrupted FFmpeg partials must not bloat the archived project.
            prepared = root / project_id / "prepared"
            if prepared.is_dir():
                for partial in prepared.glob("*.partial.mp4"):
                    partial.unlink(missing_ok=True)
            result = projects.archive_project(project_id)
            # The archived directory remains recoverable; a restored project
            # must not inherit its old deletion tombstone.
            (root / ".trash" / project_id / ".delete-requested").unlink(missing_ok=True)
            success = True
            return result
        finally:
            # A failed archive remains accessible and may be retried.
            if not success and marker.exists():
                marker.unlink()
