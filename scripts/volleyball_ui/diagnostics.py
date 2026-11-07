"""On-demand local diagnostic exports from cached tracks; no inference."""
from __future__ import annotations

import argparse
import math
import subprocess
import sys
from pathlib import Path
from urllib.parse import quote

from .data import ROOT, find_cache
from .projects import assert_project_video

MODES = ("none", "combined", "split", "both")


def validated(value: dict) -> dict:
    mode=value.get("tracks_export_mode","combined")
    if mode not in MODES:
        raise ValueError("Неизвестный режим диагностического экспорта")
    try:
        padding=float(value.get("tracks_padding",0.33))
        track=int(value.get("show_rally_track",0))
    except (ValueError,TypeError,OverflowError) as exc:
        raise ValueError("Неверные настройки диагностики") from exc
    if not math.isfinite(padding) or not 0<=padding<=10 or not 0<=track<=99999:
        raise ValueError("Проверьте длительность запаса и номер трека")
    snap=value.get("show_rally_snapshot",False)
    if not isinstance(snap,bool):
        snap=snap in ("true","1","on",1)
    if mode=="none" and not snap:
        raise ValueError("Выберите MP4 или снимок трека для диагностики")
    return {
        "tracks_export_mode":mode,"tracks_padding":padding,
        "show_rally_snapshot":snap,"show_rally_track":track,
        **{name: value.get(name,False) in (True,"true","1","on",1)
           for name in ("tracks_include_not_rally","tracks_debug")},
        "tracks_mark_ball":value.get("tracks_mark_ball",True) in (True,"true","1","on",1),
    }


def paths(project_id: str, video: str, cache_id: str | None = None):
    assert_project_video(project_id,video)
    if cache_id:
        from .analysis_runs import cache_for
        cache=cache_for(video,project_id,cache_id)
    else:
        cache=find_cache(video,project_id)
    if not cache:
        raise FileNotFoundError("Сначала проанализируйте это видео")
    proxy=cache/"analysis.mp4"
    tracks=cache/"tracks/analysis/tracks"
    if not proxy.is_file() or not tracks.is_dir():
        raise FileNotFoundError("Нет сохранённых кадров или треков для диагностики")
    return cache,proxy,tracks,cache/"exports/diagnostics"


def outputs(project_id: str, video: str, cache_id: str | None = None) -> list[dict]:
    *_,directory=paths(project_id,video,cache_id)
    results=[]
    if not directory.is_dir():
        return results
    for file in sorted(directory.rglob("*")):
        if not file.is_file() or file.suffix.lower() not in (".mp4",".png"):
            continue
        relative=file.relative_to(directory).as_posix()
        results.append({
            "name":relative,
            "size_mb":round(file.stat().st_size/1e6,2),
            "url":"/api/diagnostic-file?project_id="+quote(project_id)+
                  "&video="+quote(video)+"&name="+quote(relative)+
                  ("&cache_id="+quote(cache_id) if cache_id else ""),
        })
    return results[:250]


def output_path(project_id: str, video: str, name: str, cache_id: str | None = None) -> Path:
    *_,directory=paths(project_id,video,cache_id)
    allowed={p["name"] for p in outputs(project_id,video,cache_id)}
    if name not in allowed:
        raise FileNotFoundError("Диагностический файл не найден")
    target=(directory/name).resolve()
    if not target.is_relative_to(directory.resolve()):
        raise ValueError("Недопустимый путь к файлу")
    return target


def generate(project_id: str, video: str, options: dict, cache_id: str | None = None):
    options=validated(options)
    cache,proxy,tracks,directory=paths(project_id,video,cache_id)
    directory.mkdir(parents=True,exist_ok=True)
    mode=options["tracks_export_mode"]
    base=[sys.executable,str(ROOT/"src/track_processor.py"),
        "--video_path",str(proxy),"--json_dir",str(tracks),
        "--fps","30","--padding",str(options["tracks_padding"])]
    if options["tracks_include_not_rally"]:base.append("--include-not-rally")
    if options["tracks_debug"]:base.append("--debug")
    if not options["tracks_mark_ball"]:base.append("--no-mark")
    if mode in ("combined","both"):
        command=[*base,"--output_path",str(directory/"rallies-combined.mp4")]
        print("DIAGNOSTICS: combined video from saved tracks",flush=True)
        subprocess.run(command,cwd=ROOT,check=True)
    if mode in ("split","both"):
        command=[*base,"--split_dir",str(directory/"rallies-split")]
        print("DIAGNOSTICS: split clips from saved tracks",flush=True)
        subprocess.run(command,cwd=ROOT,check=True)
    if options["show_rally_snapshot"]:
        if not any(tracks.glob("track_*.json")):
            raise ValueError("Нет треков для диагностического снимка")
        command=[sys.executable,str(ROOT/"src/show_rally.py"),str(tracks),str(proxy),
          "--players_json_path",str(cache/"players.json"),"--fps","30",
          "--track",str(options["show_rally_track"]),
          "--snapshot",str(directory/"track-snapshot.png")]
        if (cache/"court.json").is_file():
            command += ["--court_json_path",str(cache/"court.json")]
        print("DIAGNOSTICS: snapshot from saved tracks",flush=True)
        subprocess.run(command,cwd=ROOT,check=True)
    print("DIAGNOSTICS READY: "+str(directory),flush=True)


def main():
    parser=argparse.ArgumentParser(description="Offline debug exports; no inference")
    parser.add_argument("--project-id",required=True)
    parser.add_argument("--video",required=True)
    parser.add_argument("--cache-id")
    parser.add_argument("--tracks-export-mode",choices=MODES,default="combined")
    parser.add_argument("--tracks-padding",type=float,default=.33)
    parser.add_argument("--tracks-include-not-rally",action="store_true")
    parser.add_argument("--tracks-debug",action="store_true")
    parser.add_argument("--no-tracks-mark-ball",action="store_true")
    parser.add_argument("--show-rally-snapshot",action="store_true")
    parser.add_argument("--show-rally-track",type=int,default=0)
    a=parser.parse_args()
    generate(a.project_id,a.video,{
        "tracks_export_mode":a.tracks_export_mode,
        "tracks_padding":a.tracks_padding,
        "tracks_include_not_rally":a.tracks_include_not_rally,
        "tracks_debug":a.tracks_debug,
        "tracks_mark_ball":not a.no_tracks_mark_ball,
        "show_rally_snapshot":a.show_rally_snapshot,
        "show_rally_track":a.show_rally_track,
    },cache_id=a.cache_id)


if __name__=="__main__":
    main()
