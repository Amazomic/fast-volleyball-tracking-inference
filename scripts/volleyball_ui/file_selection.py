"""Resolve browser-selected local files to source paths without uploading bytes.

A normal browser intentionally hides full client paths. We match the file's
name/size/mtime against local Windows directories that the WSL process can
read; ambiguous matches are rejected rather than silently choosing a file.
"""
from __future__ import annotations

import os
import re
from pathlib import Path

from . import data
from .local_sources import VIDEO_TYPES
from .quick_reel_selection import save_selections
from .quick_reel_audio import inspect_audio
from .montage_audio import AUDIO_EXTENSIONS

MAX_FILES = 30
MAX_VISITED_DIRECTORIES = 3000
MAX_DEPTH = 7


def _roots() -> list[Path]:
    roots = []
    # Only the logged-in user's standard locations, not every user's
    # private directories. Additional drives are checked at their root
    # and common media folders, without crawling entire disks.
    user = Path.home().name
    windows_home = Path("/mnt/c/Users") / user
    # Under WSL, the Linux login name may differ from Windows' username.
    # Derive the Windows profile from the repository's actual mounted path.
    parts = data.ROOT.parts
    if len(parts) >= 5 and parts[:4] == ("/", "mnt", "c", "Users"):
        windows_home = Path(*parts[:5])
    for base in (windows_home,):
        roots.extend(base / name for name in
                     ("Desktop", "Downloads", "Videos", "Music", "Documents"))
    for letter in "defghijklmnopqrstuvwxyz":
        drive = Path("/mnt") / letter
        if drive.is_dir():
            roots.extend((drive / item for item in
                          ("Videos","Video","Music","Movies","Recordings","Desktop")))
            roots.append(drive)
    seen=set()
    result=[]
    for root in roots:
        try:
            resolved=root.resolve(strict=True)
            if resolved not in seen and resolved.is_dir():
                seen.add(resolved);result.append(root)
        except (OSError, RuntimeError):
            continue
    return result


def _candidates(filename: str, roots: list[Path]):
    visited=0
    # Directory enumeration is bounded, and symlinks are not followed.
    for root in roots:
        stack=[(root,0)]
        while stack and visited < MAX_VISITED_DIRECTORIES:
            folder,depth=stack.pop()
            visited+=1
            try:
                with os.scandir(folder) as entries:
                    children=list(entries)
            except (OSError,PermissionError):
                continue
            for child in children:
                try:
                    if child.name.casefold() == filename.casefold() and child.is_file():
                        yield Path(child.path)
                    elif depth<MAX_DEPTH and child.is_dir(follow_symlinks=False):
                        # Avoid scanning virtual environment and hidden system folders.
                        if (not child.name.startswith(".") and child.name.casefold()
                            not in {"node_modules","__pycache__",".git",
                                    "windows","program files","program files (x86)",
                                    "appdata","system volume information",
                                    "$recycle.bin","windowsapps"}):
                            stack.append((Path(child.path),depth+1))
                except (OSError,PermissionError):
                    continue


def _validate_item(item: dict, *, kind: str):
    if not isinstance(item,dict):
        raise ValueError("Неверные данные выбранного файла")
    filename=item.get("name")
    if (not isinstance(filename,str) or filename != Path(filename).name
            or len(filename)>240 or not filename):
        raise ValueError("Некорректное имя файла")
    extensions=VIDEO_TYPES if kind=="video" else AUDIO_EXTENSIONS
    if Path(filename).suffix.lower() not in extensions:
        raise ValueError("Неподдерживаемый тип файла: "+filename)
    size=item.get("size")
    mtime=item.get("last_modified")
    if (isinstance(size,bool) or not isinstance(size,int) or size<=0 or
            isinstance(mtime,bool) or not isinstance(mtime,(int,float)) or
            mtime<=0):
        raise ValueError("Браузер не передал размер или время файла")
    return filename,size,mtime


def resolve_files(items: list, *, kind: str, roots: list[Path] | None=None,
                  project_sources: bool=False) -> dict:
    if kind not in ("video","audio"):
        raise ValueError("Неизвестный тип исходного файла")
    if project_sources and kind != "video":
        raise ValueError("Для проекта можно выбирать только видео")
    if (not isinstance(items,list) or not items or
            len(items) > (MAX_FILES if kind=="video" else 1)):
        raise ValueError("Выберите от 1 до 30 видео или один аудиотрек")
    roots=_roots() if roots is None else roots
    matched=[]
    for item in items:
        name,size,mtime=_validate_item(item,kind=kind)
        matches=[]
        seen=set()
        # Desktop/Downloads/Videos are the common case. Check their immediate
        # children first; don't crawl an entire drive for a Desktop file.
        immediate = [root / name for root in roots
                     if (root / name).is_file()]
        candidates = immediate if immediate else _candidates(name,roots)
        for file in candidates:
            try:
                stat=file.stat()
                if stat.st_size != size or abs(stat.st_mtime*1000-mtime)>2000:
                    continue
                resolved=file.resolve(strict=True)
                if resolved in seen:
                    continue
                seen.add(resolved);matches.append(resolved)
            except (OSError,RuntimeError):
                continue
        if not matches:
            raise ValueError("Не найден локальный оригинал: "+name+
                             ". Выберите файл с Рабочего стола или из папок "
                             "Видео/Загрузки; копирование запрещено.")
        if len(matches)>1:
            raise ValueError("Есть несколько совпадающих файлов «"+name+
                             "». Невозможно безопасно определить оригинал.")
        matched.append(str(matches[0]))
    if kind=="video":
        if project_sources:
            from .local_sources import register_paths
            return {"videos": register_paths(matched)}
        return {"videos":save_selections(matched)}
    return {"audio":inspect_audio(matched[0])}
