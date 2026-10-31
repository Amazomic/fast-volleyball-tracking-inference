#!/usr/bin/env python3
"""Build a single complete-rally highlight compilation from ALL videos in a project."""
from __future__ import annotations

import argparse
import json
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
from scripts.volleyball_ui.projects import project_output, project_videos, project_dir

def locate_cache(source: Path) -> Path:
    base = ROOT / "results" / "analysis"
    for folder in sorted(base.iterdir(), key=lambda p:p.stat().st_mtime, reverse=True):
        ev = folder / "analysis" / "events.json"
        if not (ev.is_file() and (folder/"ranking.json").is_file() and (folder/"analysis.mp4").is_file()):
            continue
        payload=json.loads(ev.read_text(encoding="utf-8"))
        if Path(str(payload.get("source","")).replace("\\","/")).name == source.name:
            return folder
    raise FileNotFoundError("Сначала проанализируйте видео: "+source.name)

def main():
    parser=argparse.ArgumentParser()
    parser.add_argument("--project-id",required=True)
    parser.add_argument("--duration",type=float,default=120)
    args=parser.parse_args()
    if not 10<=args.duration<=600:
        parser.error("duration: 10..600")
    videos=project_videos(args.project_id)
    project_root=project_dir(args.project_id)
    project_root.mkdir(parents=True,exist_ok=True)
    groups=[]
    caches={}
    for src in videos:
        cache=locate_cache(src)
        caches[src.name]=cache
        ranking=json.loads((cache/"ranking.json").read_text(encoding="utf-8"))
        for group in load_groups(cache,ranking):
            if not group["ranked"]:
                continue
            group["video"]=src.name
            group["cache_id"]=cache.name
            groups.append(group)
    if not groups:
        raise RuntimeError("Нет подходящих розыгрышей ни в одном видео")
    selected=choose_whole_groups(groups,args.duration,max_over_sec=8)
    by_video=defaultdict(list)
    for item in selected:
        by_video[item["video"]].append(item)
    output=project_output(args.project_id)
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
        render(src,caches[src.name]/"analysis.mp4",entries,output_path=part,
               segments_folder=project_root/f"segments_{idx:02}")
        temp_outputs.append(part)
    if not temp_outputs:
        raise RuntimeError("Не выбрано ни одного розыгрыша")
    if len(temp_outputs)==1:
        temp_outputs[0].replace(output)
    else:
        # Robust ffconcat with safe=0 and proper quote escaping (project paths are local).
        playlist=project_root/"concat_sources.txt"
        playlist.write_text("".join("file '"+str(x.resolve()).replace("'","'\\''")+"'\n" for x in temp_outputs),encoding="utf-8")
        subprocess.run(["ffmpeg","-hide_banner","-v","error","-nostdin","-y",
                        "-f","concat","-safe","0","-i",str(playlist),
                        "-c","copy","-movflags","+faststart",str(output)],check=True)
    manifest={"project_id":args.project_id,"source_videos":[x.name for x in videos],
              "selected_rallies":selected,
              "total_seconds":round(sum(x["duration_sec"] for x in selected),3),
              "method":"globally ranked complete detected rallies across all project videos"}
    (project_root/"highlights_manifest.json").write_text(json.dumps(manifest,ensure_ascii=False,indent=2),encoding="utf-8")
    print(f"PROJECT VIDEO READY {output} ({manifest['total_seconds']} sec, {len(selected)} rallies, "
          f"{len(by_video)} source videos)",flush=True)

if __name__=="__main__":
    main()
