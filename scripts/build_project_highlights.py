#!/usr/bin/env python3
"""Build a single complete-rally highlight compilation from ALL videos in a project."""
from __future__ import annotations

import argparse
import json
import time
from datetime import datetime, timezone
import subprocess
import sys
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
from full_rallies import choose_whole_groups, load_groups
from build_match_highlights import render

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from scripts.volleyball_ui.montage_project import plan_rallies, montage_eligible as _montage_eligible
from scripts.volleyball_ui.projects import project_output, project_videos, project_dir
from scripts.volleyball_ui.data import find_cache

def locate_cache(source: Path, project_id: str) -> Path:
    cache = find_cache(source.name, project_id)
    if not cache or not (cache / "ranking.json").is_file() or not (cache / "analysis.mp4").is_file():
        raise FileNotFoundError("Нет полного результата анализа проекта для: "+source.name)
    return cache


def montage_eligible(group: dict, min_rally: float, min_confidence: float) -> bool:
    """Filter scored, detected rally duration before context padding."""
    return _montage_eligible(group, min_rally, min_confidence)


def export_vertical_reels(videos, caches, by_video, project_root, cfg, *, progress=None):
    """One 9:16 clip per selected WHOLE rally, not one clip per ball track."""
    remaining=cfg["reels_limit"]
    exports=project_root/"vertical_reels"
    exports.mkdir(parents=True,exist_ok=True)
    completed=[]
    for source in videos:
        selected=by_video.get(source.name,[])
        if not selected:
            continue
        if progress:
            progress("Вертикальные розыгрыши: "+source.name,"start")
        cache=caches[source.name]
        base=cache/"tracks/analysis/tracks"
        outdir=exports/source.stem
        outdir.mkdir(parents=True,exist_ok=True)
        json_dir=outdir/"rally_metadata"
        json_dir.mkdir(parents=True,exist_ok=True)
        chosen=[]
        for index,group in enumerate(selected[:remaining],1):
            # Reel padding applies independently but cannot cross the safe
            # neighboring-rally boundaries established by montage.
            reel_pad=round(cfg["reels_padding"]*30)
            clip_start=max(int(group["source_start_frame"]),
                           int(group["detected_start_frame"])-reel_pad)
            clip_end=min(int(group["source_end_frame"]),
                         int(group["detected_end_frame"])+reel_pad)
            positions={}
            sample=None
            for track_id in group.get("track_ids",[]):
                try:
                    track_path=base/("track_"+str(int(track_id)).zfill(4)+".json")
                except (TypeError,ValueError,OverflowError):
                    continue
                if not track_path.is_file():
                    candidates=list(base.glob("track_*.json"))
                    track_path=next((path for path in candidates if
                        path.stem.rsplit("_",1)[-1].isdigit() and
                        int(path.stem.rsplit("_",1)[-1])==int(track_id)),track_path)
                if not track_path.is_file():
                    continue
                track=json.loads(track_path.read_text(encoding="utf-8"))
                if sample is None:sample=track
                for entry in track.get("positions",[]):
                    try:
                        xy,frame=entry[:2]
                        frame=int(frame)
                        x,y=float(xy[0]),float(xy[1])
                    except (ValueError,TypeError,IndexError):
                        continue
                    if clip_start<=frame<=clip_end:
                        positions[frame]=[[x,y],frame]
            if not positions:
                print("REEL SKIP: no ball coordinates in full rally "+str(index),flush=True)
                continue
            payload={
                "start_frame":clip_start,
                "last_frame":clip_end,
                "positions":[positions[frame] for frame in sorted(positions)],
                "tracking_scale":sample.get("tracking_scale") or {},
                "court_info":sample.get("court_info") or {},
                "rally_group_id":str(group["detected_start_frame"])+"-"+str(group["detected_end_frame"]),
                "track_ids":group["track_ids"],
                "boundary_method":group["boundary_method"],
            }
            path=json_dir/("rally_"+str(index).zfill(4)+".json")
            path.write_text(json.dumps(payload,ensure_ascii=False),encoding="utf-8")
            chosen.append(path)
        if chosen:
            cmd=[sys.executable,str(ROOT/"src/make_reels.py"),
                 "--video_path",str(cache/"analysis.mp4"),
                 "--track_jsons",*(str(path) for path in chosen),
                 "--output_dir",str(outdir),
                 "--smoothing",cfg["reels_smoothing"],
                 "--interpolation",cfg["reels_interpolation"],
                 "--margin",str(cfg["reels_margin"]),
                 # The group already contains video context. Do not double-pad.
                 "--padding","0",
                 "--smooth_window",str(cfg["reels_smooth_window"]),
                 "--smooth_polyorder",str(cfg["reels_smooth_polyorder"])]
            if cfg["reels_fade_transition"] and len(chosen)>1:
                cmd.append("--fade-transition")
            print(f"VERTICAL REELS: {source.name}: {len(chosen)} whole rallies",flush=True)
            subprocess.run(cmd,cwd=ROOT,check=True)
            remaining-=len(chosen)
            completed.append({"video":source.name,"rallies":len(chosen),
                              "output_dir":str(outdir),
                              "rally_metadata":[str(path) for path in chosen]})
        else:
            print("VERTICAL REELS: no usable full-rally trajectories for "+source.name,flush=True)
        if progress:
            progress("Вертикальные розыгрыши: "+source.name,"complete")
    (exports/"manifest.json").write_text(json.dumps({
        "method":"make_reels.py; reconstructed complete-rally track payloads",
        "settings":{k:v for k,v in cfg.items() if k.startswith("reels_")},
        "exports":completed,
    },ensure_ascii=False,indent=2),encoding="utf-8")
    print("VERTICAL REELS READY "+str(exports),flush=True)


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument("--project-id",required=True)
    parser.add_argument("--duration",type=float,default=120)
    parser.add_argument("--min-rally",type=float,default=1.5)
    parser.add_argument("--min-confidence",type=float,default=.5)
    parser.add_argument("--run-id",default=None)
    parser.add_argument("--rally-gap-sec",type=float,default=3)
    parser.add_argument("--rally-min-sec",type=float,default=3)
    parser.add_argument("--context-sec",type=float,default=1.5)
    from scripts.volleyball_studio.advanced import add_arguments, from_namespace
    add_arguments(parser)
    args=parser.parse_args()
    advanced=from_namespace(args)
    import math
    if not (math.isfinite(args.duration) and 10<=args.duration<=600):
        parser.error("duration: 10..600")
    if not (math.isfinite(args.min_rally) and 1.5<=args.min_rally<=30):
        parser.error("min-rally: 1.5..30")
    if not (math.isfinite(args.min_confidence) and .05<=args.min_confidence<=1):
        parser.error("min-confidence: .05..1")
    from scripts.volleyball_ui.rally_view import settings as rally_settings
    rally = rally_settings(args.rally_gap_sec, args.rally_min_sec, args.context_sec)
    from scripts.volleyball_ui.analysis_runs import list_runs, cache_for
    cache_ids = {}
    if args.run_id:
        found = [run for run in list_runs(args.project_id) if run["id"] == args.run_id]
        if len(found) != 1:
            raise ValueError("Выбранный анализ не найден")
        cache_ids = found[0]["videos"]
    videos=project_videos(args.project_id)
    started=time.monotonic()
    started_at=datetime.now(timezone.utc).isoformat()
    project_root=project_dir(args.project_id)
    project_root.mkdir(parents=True,exist_ok=True)
    caches={}
    sources=[]
    for src in videos:
        cache=(cache_for(src.name, args.project_id, cache_ids[src.name])
               if args.run_id else locate_cache(src,args.project_id))
        if not cache or not (cache / "ranking.json").is_file():
            raise ValueError("Отсутствует рейтинг выбранного анализа: " + src.name)
        caches[src.name]=cache
        sources.append({"video":src.name,"cache":cache})
    selected=plan_rallies(
        sources,args.duration,min_rally=args.min_rally,
        min_confidence=args.min_confidence,
        padding=rally["context_sec"],
        gap_sec=rally["gap_sec"], rally_min_sec=rally["min_sec"],
        loader=load_groups,chooser=choose_whole_groups)
    by_video=defaultdict(list)
    for item in selected:
        by_video[item["video"]].append(item)
    output=project_output(args.project_id)
    source_count=sum(bool(rows) for rows in by_video.values())
    total_steps=1+len(selected)+source_count+1+(
        source_count if advanced["reels_export"] else 0)
    completed=1  # Step 1: selection from saved analysis.
    duration_steps=[]
    current_started=time.monotonic()
    def progress(stage,phase,*,frame=None,total_frames=None):
        nonlocal completed,current_started
        if phase=="start":
            current_started=time.monotonic()
        if phase=="complete":
            completed+=1
            duration_steps.append({"stage":stage,"elapsed_sec":round(time.monotonic()-current_started,2)})
        stage_percent=100 if phase=="complete" else (
            round(100*frame/total_frames,2)
            if frame is not None and total_frames and total_frames>0 else 0)
        # Complete is accounted for already; running includes partial frames.
        ratio=completed+(stage_percent/100 if phase=="running" else 0)
        overall=min(99.9,round(100*ratio/total_steps,2))
        print(json.dumps({
            "event":"UI_PROGRESS","kind":"highlights","stage":stage,
            "phase":phase,"overall_percent":overall,
            "stage_percent":stage_percent,
            "completed_stages":completed,"total_stages":total_steps,
            "stage_index":min(completed if phase=="complete" else completed+1,total_steps),"stage_total":total_steps,
            "frames":frame,"total_frames":total_frames,
            "elapsed_sec":round(time.monotonic()-started,1),
        },ensure_ascii=False),flush=True)
    def clip_progress(which,phase,index,total,frame,frame_total):
        stage=("Кодирование клипа "+str(index)+"/"+str(total)
               if which=="clip" else "Сборка клипов источника")
        progress(stage,phase,frame=frame,total_frames=frame_total)
    print(json.dumps({
        "event":"UI_PROGRESS","kind":"highlights","stage":"Отбор эпизодов из кэша",
        "phase":"complete","overall_percent":round(100/total_steps,2),
        "stage_percent":100,"completed_stages":1,"total_stages":total_steps,
        "stage_index":2,"stage_total":total_steps,
        "elapsed_sec":round(time.monotonic()-started,2),
        "clips_total":len(selected),
    },ensure_ascii=False),flush=True)
    temp_outputs=[]
    for idx,src in enumerate(videos):
        entries=by_video.get(src.name)
        if not entries:
            print("No selected rallies in "+src.name,flush=True)
            continue
        for sequence,entry in enumerate(entries,1):
            entry["sequence"]=sequence
        part=project_root/f"source_{idx:02}.mp4"
        print(f"Rendering video {idx+1}/{len(videos)}: {src.name}; {len(entries)} full rallies",flush=True)
        from scripts.volleyball_ui.montage_source import analysis_offset
        render(src,caches[src.name]/"analysis.mp4",entries,output_path=part,
               segments_folder=project_root/f"segments_{idx:02}",
               progress_callback=clip_progress, original_quality=True,
               analysis_offset=analysis_offset(caches[src.name]))
        temp_outputs.append(part)
    if not temp_outputs:
        raise RuntimeError("Не выбрано ни одного розыгрыша")
    progress("Финальная сборка MP4","start")
    if len(temp_outputs)==1:
        temp_outputs[0].replace(output)
    else:
        # Robust ffconcat with safe=0 and proper quote escaping (project paths are local).
        playlist=project_root/"concat_sources.txt"
        playlist.write_text("".join("file '"+str(x.resolve()).replace("'","'\\''")+"'\n" for x in temp_outputs),encoding="utf-8")
        subprocess.run(["ffmpeg","-hide_banner","-v","error","-nostdin","-y",
                        "-f","concat","-safe","0","-i",str(playlist),
                        "-c","copy","-movflags","+faststart",str(output)],check=True)
    progress("Финальная сборка MP4","complete")
    manifest={"project_id":args.project_id,"source_videos":[x.name for x in videos],
              "selected_rallies":selected,
              "total_seconds":round(sum(x["duration_sec"] for x in selected),3),
              "min_rally_sec": args.min_rally,
              "min_confidence": args.min_confidence,
              "method":"globally ranked complete detected rallies across all project videos"}
    (project_root/"highlights_manifest.json").write_text(json.dumps(manifest,ensure_ascii=False,indent=2),encoding="utf-8")
    print(f"PROJECT VIDEO READY {output} ({manifest['total_seconds']} sec, {len(selected)} rallies, "
          f"{len(by_video)} source videos)",flush=True)
    if advanced["reels_export"]:
        export_vertical_reels(videos, caches, by_video, project_root, advanced,
                              progress=progress)
    elapsed=round(time.monotonic()-started,2)
    manifest["elapsed_sec"]=elapsed
    manifest["started_at"]=started_at
    manifest["finished_at"]=datetime.now(timezone.utc).isoformat()
    manifest["step_durations"]=duration_steps
    (project_root/"highlights_manifest.json").write_text(
        json.dumps(manifest,ensure_ascii=False,indent=2),encoding="utf-8")
    print(json.dumps({"event":"UI_PROGRESS","kind":"highlights",
        "stage":"Монтаж завершён","phase":"complete","overall_percent":100,
        "stage_percent":100,"completed_stages":total_steps,
        "total_stages":total_steps,"stage_index":total_steps,
        "stage_total":total_steps,"elapsed_sec":elapsed},ensure_ascii=False),flush=True)

if __name__=="__main__":
    main()
