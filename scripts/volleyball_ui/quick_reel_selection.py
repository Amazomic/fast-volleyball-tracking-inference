"""Private file selections for Quick Reel, independent of project/ registry."""
from __future__ import annotations

import re
import time
import uuid
from pathlib import Path
from . import data

_ID = re.compile(r"[a-f0-9]{32}")
_ALLOWED = {".mp4", ".mov", ".mkv", ".avi", ".webm", ".m4v", ".mts"}


def selection_file(token: str) -> Path:
    if not isinstance(token, str) or not _ID.fullmatch(token):
        raise ValueError("Некорректный идентификатор исходного видео")
    # Read QUICK_ROOT from the module to support isolated test roots.
    from . import quick_reel_jobs
    return quick_reel_jobs.QUICK_ROOT / "selections" / (token + ".json")


def save_selections(paths: list[str]) -> list[dict]:
    result=[]
    for value in paths:
        path=Path(value).resolve(strict=True)
        if not path.is_file() or path.suffix.lower() not in _ALLOWED:
            raise ValueError("Выбран неподдерживаемый видеофайл")
        stat=path.stat()
        token=uuid.uuid4().hex
        entry={"id":token,"name":path.name,"source":str(path),
               "size":stat.st_size,"mtime_ns":stat.st_mtime_ns,
               "created_at":time.time()}
        data.save_json(selection_file(token), entry)
        result.append({"id":token,"name":path.name, "size_gb":
                       round(stat.st_size/1e9,2),"linked":True})
    return result


def selection_for(token: str) -> dict:
    entry=data.read_json(selection_file(token), {})
    if not isinstance(entry,dict) or entry.get("id")!=token:
        raise ValueError("Выбранный файл быстрого рилса недоступен")
    path=Path(entry["source"]).resolve(strict=True)
    stat=path.stat()
    if (path.suffix.lower() not in _ALLOWED
            or str(path)!=entry.get("source")
            or stat.st_size!=entry.get("size")
            or stat.st_mtime_ns!=entry.get("mtime_ns")):
        raise ValueError("Исходное видео изменилось после выбора")
    return entry
