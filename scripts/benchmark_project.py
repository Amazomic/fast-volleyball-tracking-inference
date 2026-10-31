#!/usr/bin/env python3
"""Measure real throughput for a selected project video; no global cache writes."""
from __future__ import annotations

import argparse
import json
import platform
import re
import subprocess
import sys
import time
import uuid
from datetime import datetime, timezone
from pathlib import Path

ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT))
from scripts.volleyball_ui.projects import project_dir, get_project, assert_project_video
from scripts.volleyball_ui.preview import video_info, target_width
from scripts.volleyball_ui.data import video_path, read_json, save_json


def parse_args(argv=None):
    p=argparse.ArgumentParser()
    p.add_argument("--project-id",required=True)
    p.add_argument("--video",required=True)
    p.add_argument("--mode",choices=("decode","pipeline"),default="decode")
    p.add_argument("--duration",type=float,default=15)
    p.add_argument("--start",type=float,default=0)
    p.add_argument("--analysis-width",default="auto")
    p.add_argument("--backend",choices=("onnx","openvino"),default="onnx")
    p.add_argument("--threads",type=int,default=8)
    return p.parse_args(argv)


def benchmark(args) -> dict:
    assert_project_video(args.project_id,args.video)
    source=video_path(args.video)
    info=video_info(args.video)
    if not 2<=args.duration<=120 or not 0<=args.start<info["duration_sec"]:
        raise ValueError("Время бенчмарка должно быть от 2 до 120 секунд внутри видео")
    duration=min(args.duration,info["duration_sec"]-args.start)
    if duration<2:
        raise ValueError("Осталось менее двух секунд видео")
    if not 1<=args.threads<=32:
        raise ValueError("Число потоков: 1–32")
    width=target_width(info,args.analysis_width)
    root=project_dir(args.project_id)/"benchmarks"
    run_id=uuid.uuid4().hex[:12]
    work=root/"runs"/run_id
    work.mkdir(parents=True,exist_ok=False)
    print(f"BENCHMARK START: {args.mode}, {duration:g} sec, {source.name}, {width}px",flush=True)
    start=time.perf_counter()
    expected=round(duration*30)
    if args.mode=="decode":
        command=[
            "ffmpeg","-nostdin","-hide_banner","-loglevel","error","-threads",str(args.threads),
            "-ss",str(args.start),"-t",str(duration),"-i",str(source),
            "-an","-vf",f"fps=30,scale={width}:-2,setsar=1",
            "-f","null","-","-progress","pipe:1",
        ]
    else:
        command=[
            sys.executable,"-m","scripts.volleyball_studio",
            "--input",str(source),"--output",str(work/"output"),
            "--work",str(work/"analysis"),
            "--analyze-only","--non-interactive",
            "--backend",args.backend,"--analysis-width",str(width),
            "--analysis-start",str(args.start),"--analysis-seconds",str(duration),
            "--threads",str(args.threads),"--beach",
        ]
    proc=subprocess.Popen(command,cwd=ROOT,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,
                          universal_newlines=True,bufsize=1)
    max_frames=0
    try:
        with (work/"execution.log").open("w",encoding="utf-8") as out:
            assert proc.stdout is not None
            for line in proc.stdout:
                out.write(line)
                if args.mode=="decode" and line.startswith("frame="):
                    try:max_frames=max(max_frames,int(line.split("=",1)[1]))
                    except ValueError:pass
                if "frame=" in line or "BENCHMARK" in line:
                    print(line.rstrip(),flush=True)
        rc=proc.wait()
    except BaseException:
        proc.terminate()
        try:proc.wait(timeout=5)
        except subprocess.TimeoutExpired:proc.kill()
        raise
    elapsed=time.perf_counter()-start
    if rc!=0:
        tail=(work/"execution.log").read_text(encoding="utf-8",errors="replace")[-1200:]
        raise RuntimeError(f"Бенчмарк завершился с кодом {rc}: {tail}")
    frames=max_frames if args.mode=="decode" else expected
    result={
        "id":run_id,"project_id":args.project_id,"video":args.video,
        "mode":args.mode,"duration_sec":round(duration,3),"start_sec":args.start,
        "decoded_frames":frames if args.mode=="decode" else None,
        "estimated_analysis_frames":expected if args.mode=="pipeline" else None,
        "wall_sec":round(elapsed,3),"fps":round(frames/max(elapsed,.001),2),
        "realtime_multiplier":round(duration/max(elapsed,.001),3),
        "width":width,"backend":args.backend if args.mode=="pipeline" else None,
        "threads":args.threads,"created_at":datetime.now(timezone.utc).isoformat(),
        "host":platform.node(),
        "description":("FFmpeg decode+scale throughput, NOT neural network speed"
                       if args.mode=="decode" else
                       "Full fresh model analysis including startup, normalization, players, events"),
    }
    save_json(root/"runs"/run_id/"result.json",result)
    history=read_json(root/"history.json",{"runs":[]})
    if not isinstance(history,dict) or not isinstance(history.get("runs"),list):
        history={"runs":[]}
    history["runs"].append(result)
    save_json(root/"history.json",history)
    print("BENCHMARK_RESULT="+json.dumps(result,ensure_ascii=False),flush=True)
    return result


if __name__=="__main__":
    try:benchmark(parse_args())
    except Exception as exc:
        print("BENCHMARK_ERROR: "+str(exc),file=sys.stderr,flush=True)
        raise
