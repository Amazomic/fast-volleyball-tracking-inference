"""Reference locally accessible source videos without copying bytes.

The UI remains browser-based; it cannot retrieve absolute paths from
<input type=file>. A Windows native file dialog (via WSL interop) returns
paths instead. An absolute source is linked under project/, and the shared
normalizer writes one reusable prepared copy under results/prepared/.
"""
from __future__ import annotations

import base64
import json
import os
import re
import subprocess
import sys
import threading
import time
import uuid
from urllib.parse import urlsplit, unquote
from pathlib import Path

from . import data

VIDEO_TYPES = {".mp4", ".mov", ".mkv", ".avi", ".webm", ".m4v", ".mts"}
_LOCK = threading.RLock()
_PICKERS: dict[str, dict] = {}
# Abandoned browser dialogs must not block new file selection indefinitely.
_PICKER_IDLE_SEC = 15
_PICKER_MAX_SEC = 180

_PICKER_SCRIPT = r"""
Add-Type -AssemblyName System.Windows.Forms
[System.Windows.Forms.Application]::EnableVisualStyles()
$picker=New-Object System.Windows.Forms.OpenFileDialog
$picker.Title='Select volleyball source videos'
$picker.Multiselect=$true
$picker.Filter='Video files|*.mp4;*.mov;*.mkv;*.avi;*.webm;*.m4v;*.mts|All files|*.*'
$picker.CheckFileExists=$true
$picker.RestoreDirectory=$true
if($picker.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK){
  $payload=ConvertTo-Json -InputObject @($picker.FileNames) -Compress
  [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($payload))
}else{
  [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes('[]'))
}
"""


def _source_path(value: str) -> Path:
    if not isinstance(value, str) or not value.strip():
        raise ValueError("Укажите полный путь к исходному видео")
    value = value.strip().strip('"').strip("'")
    if value.lower().startswith("file:"):
        parsed = urlsplit(value)
        if parsed.scheme.lower() != "file" or parsed.netloc not in ("", "localhost"):
            raise ValueError("Поддерживаются только локальные ссылки file:///")
        value = unquote(parsed.path)
        if re.match(r"^/[A-Za-z]:/", value):
            value = value[1:]
    if len(value) > 4096 or "\x00" in value:
        raise ValueError("Недопустимый путь к видео")
    # WSL's file system uses /mnt/<drive>/... for Windows drive paths.
    if os.name != "nt" and re.match(r"^[A-Za-z]:[\\/]", value):
        try:
            converted = subprocess.run(
                ["wslpath", "-u", value], capture_output=True,
                text=True, timeout=10, check=True)
            value = converted.stdout.strip()
        except (OSError, subprocess.CalledProcessError, subprocess.TimeoutExpired) as exc:
            raise ValueError("Диск с выбранным видео недоступен в WSL") from exc
    source = Path(value).expanduser()
    if not source.is_absolute() or source.suffix.lower() not in VIDEO_TYPES:
        raise ValueError("Укажите абсолютный путь к видео MP4, MOV, MKV, AVI, WEBM, M4V или MTS")
    if not source.is_file():
        raise ValueError("Исходное видео не найдено: " + str(source))
    return source.resolve(strict=True)


def _link_source(source: Path) -> str:
    """A tiny symlink makes the existing project pipeline work unmodified."""
    data.PROJECT.mkdir(parents=True, exist_ok=True)
    if source.parent == data.PROJECT.resolve():
        return source.name
    with _LOCK:
        for entry in data.PROJECT.iterdir():
            if entry.is_symlink() and entry.resolve() == source:
                return entry.name
        name, stem, suffix = source.name, source.stem, source.suffix
        i = 2
        while True:
            destination = data.PROJECT / name
            try:
                os.symlink(source, destination)
                return name
            except FileExistsError:
                name = f"{stem} ({i}){suffix}"
                i += 1


def register_paths(paths: list[str]) -> list[dict]:
    if not isinstance(paths, list) or not 1 <= len(paths) <= 30:
        raise ValueError("Выберите от 1 до 30 видео")
    sources = [_source_path(path) for path in paths]
    unique = list(dict.fromkeys(sources))
    output = []
    for source in unique:
        name = _link_source(source)
        output.append({
            "name": name,
            "source_path": str(source),
            "size_gb": round(source.stat().st_size / 1e9, 2),
            "linked": True,
        })
    return output


def _open_windows_picker(kind: str = "video", *,
                         cancel_event: threading.Event | None = None) -> list[str]:
    if not sys.platform.startswith("linux"):
        raise ValueError("Автоматический проводник доступен при запуске Volley Studio в WSL")
    windows_powershell = Path("/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe")
    if not windows_powershell.is_file():
        raise ValueError("Не найден проводник Windows. Вставьте полные пути к видео вручную.")
    if kind not in ("video", "audio"):
        raise ValueError("Неподдерживаемый тип файлов для проводника")
    data.PROJECT.mkdir(parents=True, exist_ok=True)
    try:
        directory = subprocess.run(
            ["wslpath", "-w", str(data.PROJECT.resolve())],
            capture_output=True, text=True, timeout=10, check=True,
        ).stdout.strip()
        if not directory:
            raise ValueError("Путь к папке исходных видео пуст")
        # InitialDirectory is the real repo project/ folder, not the last folder
        # the Windows picker happened to remember from a previous invocation.
        escaped_directory = directory.replace("'", "''")
        script = _PICKER_SCRIPT.replace(
            "$picker.Multiselect=$true",
            f"$picker.InitialDirectory='{escaped_directory}'\n$picker.Multiselect=$true",
            1,
        )
        if kind == "audio":
            script = script.replace(
                "Video files|*.mp4;*.mov;*.mkv;*.avi;*.webm;*.m4v;*.mts|All files|*.*",
                "Audio files|*.mp3;*.m4a;*.wav;*.flac;*.aac;*.ogg;*.opus|All files|*.*",
                1,
            )
            script = script.replace("Select volleyball source videos",
                                    "Select music for volleyball montage", 1)
        encoded = base64.b64encode(script.encode("utf-16le")).decode("ascii")
        command = [str(windows_powershell), "-NoProfile", "-STA",
                   "-NonInteractive", "-EncodedCommand", encoded]
        if cancel_event is None:
            # Keep direct calls mockable, e.g. isolated picker integration tests.
            output_bytes = subprocess.run(
                command, capture_output=True, timeout=_PICKER_MAX_SEC,
                check=True).stdout
        else:
            # A separate process can be cancelled when the page is closed,
            # reloaded or stops polling. Never kill unrelated Windows windows.
            process = subprocess.Popen(command, stdout=subprocess.PIPE,
                                       stderr=subprocess.PIPE)
            started = time.monotonic()
            try:
                while process.poll() is None:
                    if cancel_event.is_set():
                        raise ValueError("Выбор файлов отменён")
                    if time.monotonic() - started > _PICKER_MAX_SEC:
                        raise ValueError("Проводник не ответил. Выберите файлы повторно.")
                    cancel_event.wait(.25)
                output_bytes, stderr = process.communicate(timeout=5)
                if process.returncode:
                    raise RuntimeError("Проводник завершился с ошибкой: " +
                                       stderr.decode("utf-8", errors="replace")[-200:])
            finally:
                if process.poll() is None:
                    process.kill()
                    process.communicate(timeout=5)
        lines = output_bytes.decode("ascii", errors="replace").splitlines()
        result = json.loads(base64.b64decode(lines[-1]).decode("utf-8")) if lines else []
    except ValueError as exc:
        if str(exc) in ("Выбор файлов отменён",
                        "Проводник не ответил. Выберите файлы повторно."):
            raise
        raise ValueError("Проводник Windows недоступен. Попробуйте открыть его повторно.") from exc
    except (OSError, IndexError, subprocess.SubprocessError) as exc:
        raise ValueError("Проводник Windows недоступен. Попробуйте открыть его повторно.") from exc
    if isinstance(result, str):
        result = [result]
    if not isinstance(result, list):
        raise ValueError("Некорректный ответ проводника Windows")
    return result


def begin_picker(kind: str = "video") -> dict:
    if kind not in ("video", "audio"):
        raise ValueError("Недопустимый тип файлов для проводника")
    token = uuid.uuid4().hex
    now = time.monotonic()
    with _LOCK:
        for old_token, old in list(_PICKERS.items()):
            if old["status"] == "pending":
                expired = (now - old["created"] >= _PICKER_MAX_SEC or
                           now - old["last_poll"] >= _PICKER_IDLE_SEC)
                if expired:
                    old["cancel_event"].set()
                    old.update(status="cancelled", paths=[],
                               error="Предыдущий выбор файлов больше не активен")
                else:
                    raise ValueError("Проводник уже открыт. Завершите текущий выбор файлов.")
            if old["status"] != "pending" and now - old["created"] > 600:
                _PICKERS.pop(old_token, None)
        event = threading.Event()
        _PICKERS[token] = {"status": "pending", "paths": [], "error": "",
                           "created": now, "last_poll": now,
                           "cancel_event": event}
    def worker():
        try:
            paths = _open_windows_picker(kind, cancel_event=event)
            outcome = {"status": "done", "paths": paths, "error": ""}
        except Exception as error:
            outcome = {"status": "error", "paths": [], "error": str(error)}
        with _LOCK:
            # A retired worker must not overwrite a cancellation/new picker.
            if token in _PICKERS and not event.is_set():
                _PICKERS[token].update(outcome)
    threading.Thread(target=worker, daemon=True, name="volley-source-picker").start()
    return {"picker_id": token}


def picker_status(token: str) -> dict:
    if not isinstance(token, str) or not re.fullmatch(r"[a-f0-9]{32}", token):
        raise ValueError("Недопустимый идентификатор проводника")
    with _LOCK:
        result = _PICKERS.get(token)
        if result is None:
            raise ValueError("Выбор файлов не найден")
        result["last_poll"] = time.monotonic()
        return {key: result[key] for key in ("status", "paths", "error")}


def cancel_picker(token: str) -> dict:
    if not isinstance(token, str) or not re.fullmatch(r"[a-f0-9]{32}", token):
        raise ValueError("Недопустимый идентификатор проводника")
    with _LOCK:
        entry = _PICKERS.get(token)
        if entry is not None and entry["status"] == "pending":
            entry["cancel_event"].set()
            entry.update(status="cancelled", paths=[], error="Выбор файлов отменён")
    return {"cancelled": True}
