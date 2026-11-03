"""Bounded audio uploads and soundtrack-duration validation for Quick Reel."""
from __future__ import annotations

import json
import math
import os
import re
import subprocess
import threading
import time
import uuid
from pathlib import Path

from .data import RESULTS
from .montage_audio import AUDIO_EXTENSIONS, resolve_audio

CHUNK_SIZE=2*1024*1024
MAX_SIZE=256*1024*1024
ROOT=RESULTS/"quick_reel_audio"
_LOCK=threading.RLock()
_UPLOADS={}


def inspect_audio(value):
    source=resolve_audio(value)
    if source is None:
        raise ValueError("Добавьте музыкальный трек для быстрого рилса")
    try:
        result=subprocess.run([
            "ffprobe","-v","error","-show_entries","format=duration:stream=codec_type",
            "-of","json",str(source)
        ],check=True,capture_output=True,text=True,timeout=35)
        meta=json.loads(result.stdout)
    except (OSError, ValueError, subprocess.SubprocessError) as exc:
        raise ValueError("Не удалось прочитать аудиофайл") from exc
    if not any(track.get("codec_type")=="audio" for track in meta.get("streams",[])):
        raise ValueError("Музыкальный файл не содержит аудиодорожку")
    duration=float(meta.get("format",{}).get("duration") or 0)
    if not math.isfinite(duration) or not 10<=duration<=600:
        raise ValueError("Для быстрого рилса выберите трек длительностью от 10 секунд до 10 минут")
    return {"path":str(source),"duration":round(duration,3),
            "name":source.name}


def start(filename, size):
    if not isinstance(filename,str) or filename!=Path(filename).name or len(filename)>200:
        raise ValueError("Некорректное название музыкального файла")
    suffix=Path(filename).suffix.lower()
    if suffix not in AUDIO_EXTENSIONS:
        raise ValueError("Поддерживаются MP3, WAV, M4A, FLAC, AAC, OGG, OPUS")
    if isinstance(size,bool) or not isinstance(size,int) or not 0<size<=MAX_SIZE:
        raise ValueError("Размер аудиофайла должен быть меньше 256 МБ")
    with _LOCK:
        ROOT.mkdir(parents=True,exist_ok=True)
        for token,entry in list(_UPLOADS.items()):
            if time.monotonic()-entry["created"]>4*3600:
                entry["path"].unlink(missing_ok=True)
                _UPLOADS.pop(token,None)
        if len(_UPLOADS)>=10:
            raise ValueError("Слишком много незавершённых загрузок музыки")
        token=uuid.uuid4().hex
        temp=ROOT/(token+".part")
        with temp.open("xb"):
            pass
        _UPLOADS[token]={"path":temp,"size":size,"written":0,
                         "suffix":suffix,"created":time.monotonic()}
        return {"upload_id":token,"chunk_size":CHUNK_SIZE}


def append(token, offset, stream, length):
    if not isinstance(token,str) or re.fullmatch(r"[a-f0-9]{32}",token) is None:
        raise ValueError("Неверный идентификатор загрузки")
    with _LOCK:
        entry=_UPLOADS.get(token)
        if not entry or not isinstance(offset,int) or not isinstance(length,int):
            raise ValueError("Передача музыки не найдена")
        if not 0<length<=CHUNK_SIZE or offset!=entry["written"] or offset+length>entry["size"]:
            raise ValueError("Неверная последовательность частей музыки")
        with entry["path"].open("ab") as output:
            remaining=length
            while remaining:
                part=stream.read(min(remaining,1024*1024))
                if not part:
                    raise ValueError("Загрузка музыки прервана")
                output.write(part)
                remaining-=len(part)
        entry["written"]+=length
        return {"received":entry["written"],"size":entry["size"]}


def finish(token):
    if not isinstance(token,str) or re.fullmatch(r"[a-f0-9]{32}",token) is None:
        raise ValueError("Неверный идентификатор загрузки")
    with _LOCK:
        entry=_UPLOADS.get(token)
        if not entry or entry["written"]!=entry["size"]:
            raise ValueError("Музыка загружена не полностью")
        # ffprobe uses container content, not a trusted filename.
        temp=entry["path"]
        try:
            result=subprocess.run([
                "ffprobe","-v","error","-show_entries","format=duration:stream=codec_type",
                "-of","json",str(temp)],check=True,capture_output=True,text=True,timeout=35)
            meta=json.loads(result.stdout)
        except (OSError, ValueError, subprocess.SubprocessError) as exc:
            raise ValueError("Загруженный файл не распознан как музыкальный трек") from exc
        duration=float(meta.get("format",{}).get("duration") or 0)
        if (not math.isfinite(duration) or not 10<=duration<=600 or
            not any(s.get("codec_type")=="audio" for s in meta.get("streams",[]))):
            raise ValueError("Файл должен содержать музыку длиной 10–600 секунд")
        destination=ROOT/(token+entry["suffix"])
        os.replace(temp,destination)
        _UPLOADS.pop(token,None)
    return {"path":str(destination),"duration":round(duration,3)}


def abort(token):
    with _LOCK:
        entry=_UPLOADS.pop(token,None)
        if entry:
            entry["path"].unlink(missing_ok=True)
        return {"aborted":True}
