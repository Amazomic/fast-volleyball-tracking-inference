"""Streaming browser uploads into the shared project/ source-video library.

Upload chunks are bounded in memory. A source becomes visible only after full
transfer and ffprobe validation; an existing video is never overwritten.
"""
from __future__ import annotations

import os
import re
import subprocess
import threading
import time
import uuid
from pathlib import Path

from . import data

CHUNK_LIMIT = 2 * 1024 * 1024
MAX_FILE_SIZE = 120 * 1024 ** 3
ALLOWED_SUFFIXES = {".mp4", ".mov", ".mkv", ".avi", ".webm", ".m4v", ".mts"}
SESSION_LIFETIME = 4 * 3600
_LOCK = threading.RLock()
_SESSIONS: dict[str, dict] = {}


def _prune() -> None:
    now = time.monotonic()
    for token, entry in list(_SESSIONS.items()):
        if now - entry["touched"] > SESSION_LIFETIME:
            entry["temporary"].unlink(missing_ok=True)
            _SESSIONS.pop(token, None)


def _name(value: str) -> str:
    if not isinstance(value, str) or not 1 <= len(value) <= 200:
        raise ValueError("Имя видео должно содержать от 1 до 200 символов")
    if (value != Path(value).name or re.search(r'[<>:"/\\|?*\x00-\x1f]', value)
            or value.startswith(".") or value.endswith((" ", "."))):
        raise ValueError("Недопустимое имя файла видео")
    if Path(value).suffix.lower() not in ALLOWED_SUFFIXES:
        raise ValueError("Поддерживаются MP4, MOV, MKV, AVI, WEBM, M4V и MTS")
    if (Path(value).stem.upper() in {"CON", "PRN", "AUX", "NUL"} or
            re.fullmatch(r"(COM|LPT)[1-9]", Path(value).stem, re.IGNORECASE)):
        raise ValueError("Недопустимое системное имя файла")
    return value


def _unique_name(name: str) -> str:
    existing = {p.name.casefold() for p in data.PROJECT.iterdir() if p.is_file()}
    existing.update(entry["name"].casefold() for entry in _SESSIONS.values())
    stem, suffix = Path(name).stem, Path(name).suffix
    candidate = name
    index = 2
    while candidate.casefold() in existing:
        candidate = f"{stem} ({index}){suffix}"
        index += 1
    return candidate


def start(filename: str, size: int) -> dict:
    filename = _name(filename)
    if isinstance(size, bool) or not isinstance(size, int) or not 0 < size <= MAX_FILE_SIZE:
        raise ValueError("Размер видео должен быть от 1 байта до 120 ГБ")
    with _LOCK:
        _prune()
        if len(_SESSIONS) >= 30:
            raise ValueError("Слишком много одновременных загрузок")
        data.PROJECT.mkdir(parents=True, exist_ok=True)
        directory = data.PROJECT / ".incoming"
        directory.mkdir(parents=True, exist_ok=True)
        name = _unique_name(filename)
        token = uuid.uuid4().hex
        temporary = directory / (token + ".part")
        with temporary.open("xb"):
            pass
        _SESSIONS[token] = {
            "name": name, "size": size, "written": 0,
            "temporary": temporary, "touched": time.monotonic(),
        }
        return {"upload_id": token, "filename": name, "size": size,
                "chunk_size": CHUNK_LIMIT}


def append(token: str, offset: int, stream, length: int) -> dict:
    if not isinstance(token, str) or not re.fullmatch(r"[a-f0-9]{32}", token):
        raise ValueError("Некорректный идентификатор загрузки")
    if not isinstance(offset, int) or not isinstance(length, int):
        raise ValueError("Некорректная позиция загрузки")
    if not 0 < length <= CHUNK_LIMIT:
        raise ValueError("Размер части видео превышает лимит 2 МБ")
    with _LOCK:
        entry = _SESSIONS.get(token)
        if not entry:
            raise ValueError("Загрузка не найдена: выберите видео заново")
        if offset + length <= entry["written"]:
            file = entry["temporary"]
            with file.open("rb") as existing:
                existing.seek(offset)
                remaining = length
                while remaining:
                    block = stream.read(min(1024 * 1024, remaining))
                    if not block or existing.read(len(block)) != block:
                        raise ValueError("Повторно переданная часть видео отличается")
                    remaining -= len(block)
            entry["touched"] = time.monotonic()
            return {"received": entry["written"], "size": entry["size"],
                    "filename": entry["name"]}
        if offset != entry["written"] or offset + length > entry["size"]:
            raise ValueError("Нарушен порядок частей видео")
        file = entry["temporary"]
        try:
            with file.open("r+b") as out:
                out.seek(offset)
                remaining = length
                while remaining:
                    block = stream.read(min(1024 * 1024, remaining))
                    if not block:
                        raise ValueError("Передача видео прервана")
                    out.write(block)
                    remaining -= len(block)
        except Exception:
            with file.open("r+b") as out:
                out.truncate(offset)
            raise
        entry["written"] += length
        entry["touched"] = time.monotonic()
        return {"received": entry["written"], "size": entry["size"],
                "filename": entry["name"]}


def finish(token: str) -> dict:
    with _LOCK:
        entry = _SESSIONS.get(token)
        if not entry:
            raise ValueError("Загрузка не найдена")
        temporary = entry["temporary"]
        if entry["written"] != entry["size"] or temporary.stat().st_size != entry["size"]:
            raise ValueError("Видео загружено не полностью")
        # Validate media before publishing it in project/.
        try:
            probe = subprocess.run(
                ["ffprobe", "-v", "error", "-select_streams", "v:0",
                 "-show_entries", "stream=codec_type",
                 "-of", "default=noprint_wrappers=1:nokey=1", str(temporary)],
                capture_output=True, text=True, timeout=40, check=True)
        except (OSError, subprocess.CalledProcessError, subprocess.TimeoutExpired) as exc:
            raise ValueError("Файл не распознан как видео. Проверьте исходный файл.") from exc
        if "video" not in probe.stdout.split():
            raise ValueError("Файл не содержит видеодорожку")
        name = entry["name"]
        stem, suffix = Path(name).stem, Path(name).suffix
        index = 2
        while True:
            destination = data.PROJECT / name
            try:
                # Atomic, exclusive publication: no partial files and no overwrite.
                os.link(temporary, destination)
                break
            except FileExistsError:
                name = f"{stem} ({index}){suffix}"
                index += 1
        temporary.unlink(missing_ok=True)
        _SESSIONS.pop(token, None)
        return {"filename": name, "size": entry["size"]}


def abort(token: str) -> dict:
    with _LOCK:
        entry = _SESSIONS.pop(token, None)
        if entry:
            entry["temporary"].unlink(missing_ok=True)
        return {"aborted": True}
