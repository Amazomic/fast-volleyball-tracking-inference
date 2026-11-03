"""Optional background music for a finished montage MP4; source never copied."""
from __future__ import annotations

import math
import os
import re
import subprocess
from urllib.parse import urlsplit, unquote
from pathlib import Path

AUDIO_EXTENSIONS = {".mp3", ".m4a", ".wav", ".flac", ".aac", ".ogg", ".opus"}


def resolve_audio(value: str | None) -> Path | None:
    if value is None or value == "":
        return None
    if not isinstance(value, str) or len(value) > 4096 or "\x00" in value:
        raise ValueError("Некорректный путь к аудиотреку")
    name = value.strip().strip('"').strip("'")
    if name.lower().startswith("file:"):
        parsed = urlsplit(name)
        if parsed.scheme.lower() != "file" or parsed.netloc not in ("", "localhost"):
            raise ValueError("Для музыки поддерживаются только локальные ссылки file:///")
        name = unquote(parsed.path)
        if re.match(r"^/[A-Za-z]:/", name):
            name = name[1:]
    if os.name != "nt" and re.match(r"^[A-Za-z]:[\\/]", name):
        try:
            name = subprocess.run(
                ["wslpath", "-u", name], capture_output=True, text=True,
                timeout=10, check=True).stdout.strip()
        except (OSError, subprocess.SubprocessError) as exc:
            raise ValueError("Аудиотрек недоступен в WSL") from exc
    source = Path(name).expanduser()
    if not source.is_absolute() or source.suffix.lower() not in AUDIO_EXTENSIONS:
        raise ValueError("Выберите MP3, WAV, M4A, FLAC, AAC, OGG или OPUS")
    if not source.is_file():
        raise ValueError("Аудиотрек не найден: " + str(source))
    return source.resolve(strict=True)


def validate_volume(value: object) -> float:
    try:
        volume=float(value)
    except (ValueError,TypeError,OverflowError) as exc:
        raise ValueError("Громкость музыки: число от 0 до 100") from exc
    if not math.isfinite(volume) or not 0 <= volume <= 100:
        raise ValueError("Громкость музыки: число от 0 до 100")
    return volume


def _streams(path: Path) -> dict:
    process=subprocess.run(
        ["ffprobe","-v","error","-show_entries","stream=codec_type:"
         "format=duration","-of","json",str(path)],
        capture_output=True,text=True,timeout=40,check=True)
    import json
    return json.loads(process.stdout)


def overlay_music(video: Path, soundtrack: Path, volume: float) -> dict:
    """Mux a looped/faded soundtrack; preserve the video stream byte-for-byte.

    If the rendered video already has native audio, mix it with the music;
    otherwise add the music as the only audio stream.
    """
    if not video.is_file() or not video.stat().st_size:
        raise ValueError("Готовое видео для наложения звука не найдено")
    soundtrack = resolve_audio(str(soundtrack))
    video_data=_streams(video)
    music_data=_streams(soundtrack)
    if not any(entry.get("codec_type")=="audio"
               for entry in music_data.get("streams",[])):
        raise ValueError("В выбранном файле отсутствует аудиодорожка")
    seconds=float(video_data.get("format",{}).get("duration",0))
    if not math.isfinite(seconds) or seconds<=0:
        raise ValueError("Неизвестная длительность готового видеомонтажа")
    has_original_audio=any(entry.get("codec_type")=="audio"
                           for entry in video_data.get("streams",[]))
    fade_start=max(0.0,seconds-min(1.5,seconds/4))
    fade_duration=max(0.01,seconds-fade_start)
    fade=f"afade=t=out:st={fade_start:.4f}:d={fade_duration:.4f}"
    music=f"[1:a:0]volume={volume/100:.4f},atrim=0:{seconds:.4f},asetpts=PTS-STARTPTS,{fade}[music]"
    filters=[music]
    if has_original_audio:
        filters.append("[0:a:0][music]amix=inputs=2:duration=longest:normalize=0[aout]")
        output_audio="[aout]"
    else:
        output_audio="[music]"
    temp=video.with_name(video.stem+".music-tmp.mp4")
    cmd=["ffmpeg","-nostdin","-hide_banner","-loglevel","error","-y",
         "-i",str(video),"-stream_loop","-1","-i",str(soundtrack),
         "-filter_complex",";".join(filters),
         "-map","0:v:0","-map",output_audio,"-c:v","copy","-c:a","aac",
         "-b:a","192k","-t",f"{seconds:.4f}","-movflags","+faststart",
         str(temp)]
    try:
        subprocess.run(cmd,check=True)
        output=_streams(temp)
        if not any(entry.get("codec_type")=="audio"
                   for entry in output.get("streams",[])):
            raise RuntimeError("Не удалось добавить аудиодорожку")
        os.replace(temp,video)
    finally:
        temp.unlink(missing_ok=True)
    return {"path": str(soundtrack), "volume_percent":volume,
            "mixed_with_original":has_original_audio,
            "duration_sec":round(seconds,3)}
