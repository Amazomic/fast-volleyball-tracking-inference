#!/usr/bin/env python3
"""Sequential analysis of all videos in one registered Volley Studio project."""
from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path

ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT))
from scripts.volleyball_ui.projects import (project_videos, project_ranges,
    project_cache_root, project_run_root)

def main():
    parser=argparse.ArgumentParser()
    parser.add_argument("--project-id",required=True)
    parser.add_argument("--backend",choices=("onnx","openvino"),default="onnx")
    parser.add_argument("--analysis-width",type=int,default=1280)
    parser.add_argument("--threads",type=int,default=8)
    parser.add_argument("--analysis-start",type=float,default=0)
    parser.add_argument("--analysis-seconds",type=float,default=0)
    parser.add_argument("--beach",action="store_true")
    args=parser.parse_args()
    videos=project_videos(args.project_id)
    ranges=project_ranges(args.project_id)
    work=project_cache_root(args.project_id)
    output=project_run_root(args.project_id)
    work.mkdir(parents=True, exist_ok=True)
    output.mkdir(parents=True, exist_ok=True)
    total=len(videos)
    for idx,source in enumerate(videos,1):
        print(f"PROJECT ANALYSIS {idx}/{total}: {source.name}",flush=True)
        # A separate automatic width is selected for each source video.
        from scripts.volleyball_ui.preview import target_width,video_info
        width=target_width(video_info(source.name),"auto") if args.analysis_width==0 else args.analysis_width
        segment=ranges.get(source.name)
        seg_start=float(segment["start"]) if segment else 0
        seg_end=float(segment["end"]) if segment else 0
        seg_length=(round(seg_end-seg_start,3) if segment and seg_end<video_info(source.name)["duration_sec"]-.01 else 0)
        cmd=[sys.executable,"-m","scripts.volleyball_studio",
              "--input",str(source),"--output",str(output),
             "--work",str(work),"--analyze-only",
             "--non-interactive","--backend",args.backend,
             "--analysis-width",str(width),"--threads",str(args.threads),
             "--analysis-start",str(seg_start)]
        if seg_length>0:
            cmd+=["--analysis-seconds",str(seg_length)]
        from scripts.volleyball_ui.court import court_path
        court = court_path(args.project_id, source.name)
        if court.is_file():
            cmd += ["--court-json", str(court)]
            print("PROJECT COURT: "+str(court),flush=True)
        else:
            cmd.append("--no-court-fallback")
            print("PROJECT COURT: no saved calibration for "+source.name,flush=True)
        if args.beach:
            cmd.append("--beach")
        subprocess.run(cmd,cwd=ROOT,check=True)
        print(f"PROJECT ANALYSIS COMPLETE {idx}/{total}: {source.name}",flush=True)
    print(f"PROJECT ANALYSIS FINISHED ({total} videos)",flush=True)

if __name__=="__main__":
    main()
