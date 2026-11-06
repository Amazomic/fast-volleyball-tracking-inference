#!/usr/bin/env python3
"""Sequential analysis of all videos in one registered Volley Studio project."""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT))
from scripts.volleyball_ui.projects import (project_videos, project_ranges,
    project_cache_root, project_run_root)
from scripts.volleyball_studio.ball_models import BALL_MODELS, DEFAULT_BALL_MODEL


def rebuild_after_interrupted(history_dir: Path, profile_name: str) -> bool:
    """Never reuse an interrupted profile run's intermediate cache on retry."""
    for file in sorted(history_dir.glob("*.json"), reverse=True):
        try:
            entry=json.loads(file.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            continue
        if not isinstance(entry,dict) or entry.get("profile_name")!=profile_name:
            continue
        return entry.get("status")!="complete"
    return False


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument("--project-id",required=True)
    parser.add_argument("--profile-name",default="Рекомендуемый")
    parser.add_argument("--profile-id",default="legacy")
    parser.add_argument("--backend",choices=("onnx","onnx-cpu","openvino"),default="onnx")
    parser.add_argument("--ball-model",choices=BALL_MODELS,default=DEFAULT_BALL_MODEL)
    parser.add_argument("--ball-threshold",type=float,default=0.5)
    parser.add_argument("--analysis-width",type=int,default=1280)
    parser.add_argument("--threads",type=int,default=8)
    parser.add_argument("--gpu-device-id",type=int,default=0)
    parser.add_argument("--gpu-memory-limit-mb",type=int,default=0)
    parser.add_argument("--analysis-start",type=float,default=0)
    parser.add_argument("--analysis-seconds",type=float,default=0)
    parser.add_argument("--beach",action="store_true")
    from scripts.volleyball_studio.advanced import add_arguments, from_namespace, as_cli
    add_arguments(parser)
    args=parser.parse_args()
    advanced=from_namespace(args)
    print(json.dumps({"event": "ANALYSIS_CONFIG", "backend": args.backend,
                      "cpu_threads": args.threads,
                      "ball_model": args.ball_model,
                      "ball_threshold": args.ball_threshold,
                      "player_model": args.player_model,
                      "player_backend": args.player_backend,
                      "player_device": args.player_device,
                      "gpu_device_id": args.gpu_device_id,
                      "gpu_memory_limit_mb": args.gpu_memory_limit_mb}, ensure_ascii=False), flush=True)
    videos=project_videos(args.project_id)
    ranges=project_ranges(args.project_id)
    work=project_cache_root(args.project_id)
    output=project_run_root(args.project_id)
    work.mkdir(parents=True, exist_ok=True)
    output.mkdir(parents=True, exist_ok=True)
    total=len(videos)
    from scripts.volleyball_studio.analysis_stages import STAGES
    # Primary analysis saves only neural outputs. CPU versions are independent.
    stage_names=STAGES[:3]
    total_steps=total*len(stage_names)
    completed_steps=0
    started=time.monotonic()
    started_at=datetime.now(timezone.utc).isoformat()
    summary={"project_id":args.project_id,"profile_id":args.profile_id,
             "profile_name":args.profile_name,
             # Snapshot actual options, independent of future global profile edits.
             "settings":{**{key:value for key,value in vars(args).items()
                         if key not in ("profile_id","profile_name")},
                         "inference_only":True},
             "started_at":started_at,
             "log_name":os.environ.get("VOLLEY_JOB_LOG_NAME"),
             "videos_total":total,"stages_per_video":len(stage_names),
             "stage_names":list(stage_names),"videos":[],"status":"running",
             "result_kind":"neural_inference","inference_only":True}
    history_dir=project_run_root(args.project_id)/"analysis_timings"
    history_dir.mkdir(parents=True,exist_ok=True)
    run_id=datetime.now(timezone.utc).strftime("%Y%m%d_%H%M%S_%f")
    fresh_after_interrupt=rebuild_after_interrupted(history_dir,args.profile_name)
    # A cancelled run may have already produced complete stage caches. To
    # recreate its analysis cleanly, isolate the retry's content address.
    # Previously completed profiles and the shared prepared video stay intact.
    cache_salt=run_id if fresh_after_interrupt else ""
    summary["rebuild_after_interruption"]=fresh_after_interrupt
    summary_path=history_dir/(run_id+".json")
    def write_summary():
        tmp=summary_path.with_suffix(".json.tmp")
        tmp.write_text(json.dumps(summary,ensure_ascii=False,indent=2),encoding="utf-8")
        tmp.replace(summary_path)
    def emit(video_index,stage_index,stage_percent,phase,*,stage_name=None,
             frames=None,total_frames=None,eta_sec=None,stage_duration_sec=None):
        ratio=max(0,min(100,float(stage_percent)))
        percentage=min(99.9,100*(completed_steps+(0 if phase=="complete" else ratio/100))/total_steps) if total_steps else 0
        event={"event":"UI_PROGRESS","kind":"analyze","stage":stage_name or (
                   stage_names[stage_index-1] if stage_index else "Подготовка задачи"),
               "phase":phase,"overall_percent":round(percentage,2),
               "stage_percent":round(ratio,2),"stage_index":stage_index,
               "stage_total":len(stage_names),"completed_stages":completed_steps,
               "total_stages":total_steps,"video_index":video_index,"videos_total":total,
               "elapsed_sec":round(time.monotonic()-started,1),
               "profile_name":args.profile_name}
        if stage_duration_sec is not None:
            event["stage_duration_sec"]=stage_duration_sec
        if frames is not None and total_frames is not None:
            event.update(frames=frames,total_frames=total_frames,eta_sec=eta_sec)
        print(json.dumps(event,ensure_ascii=False),flush=True)
    emit(1,0,0,"start",stage_name="Подготовка списка видео")
    try:
      for idx,source in enumerate(videos,1):
        print(f"PROJECT ANALYSIS {idx}/{total}: {source.name}; CPU threads={args.threads}; backend={args.backend}",flush=True)
        # A separate automatic width is selected for each source video.
        from scripts.volleyball_ui.preview import target_width,video_info
        width=target_width(video_info(source.name),"auto") if args.analysis_width==0 else args.analysis_width
        segment=ranges.get(source.name)
        seg_start=float(segment["start"]) if segment else 0
        seg_end=float(segment["end"]) if segment else 0
        seg_length=(round(seg_end-seg_start,3) if segment and seg_end<video_info(source.name)["duration_sec"]-.01 else 0)
        from scripts.volleyball_ui.prepared_videos import ready_path
        # The shared project cache has one canonical normalized width. Even
        # when an analysis profile chooses a different inference resolution,
        # it must read this cached copy rather than fall back to the original.
        prepared = ready_path(args.project_id, source.name)
        if prepared is None:
            raise RuntimeError(
                "Видео ещё не подготовлено к анализу: " + source.name +
                ". Дождитесь готовности кэша проекта.")
        cmd=[sys.executable,"-m","scripts.volleyball_studio",
              "--input",str(source),"--output",str(output),
             "--work",str(work),"--analyze-only","--inference-only",
             "--non-interactive","--backend",args.backend,
             "--ball-model",args.ball_model,
             "--ball-threshold",str(args.ball_threshold),
             "--analysis-width",str(width),"--threads",str(args.threads),
             "--gpu-device-id",str(args.gpu_device_id),
             "--gpu-memory-limit-mb",str(args.gpu_memory_limit_mb),
             "--analysis-start",str(seg_start), *as_cli(advanced)]
        if cache_salt:
            cmd+=["--cache-salt",cache_salt]
            print("REBUILD AFTER INTERRUPTED ANALYSIS: "+source.name,flush=True)
        if seg_length>0:
            cmd+=["--analysis-seconds",str(seg_length)]
        cmd+=["--prepared-video",str(prepared)]
        print("REUSED PREPARED VIDEO: "+str(prepared),flush=True)
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
        current={"video":source.name,"video_index":idx,"status":"running",
                 # Immutable segment snapshot. Later project range edits must
                 # not relabel this original inference branch.
                 "range_start": seg_start, "range_end": seg_end,
                 "stages":[],"started_at":datetime.now(timezone.utc).isoformat()}
        summary["videos"].append(current)
        write_summary()
        current_stage=0
        stage_start=None
        stage_frames=0
        env={**os.environ,"PYTHONUNBUFFERED":"1"}
        process=subprocess.Popen(cmd,cwd=ROOT,stdout=subprocess.PIPE,
                                 stderr=subprocess.STDOUT,text=True,
                                 errors="replace",bufsize=1,env=env)
        assert process.stdout is not None
        for line in process.stdout:
            print(line,end="",flush=True)
            if not line.startswith("{"):
                continue
            try:
                event=json.loads(line)
            except ValueError:
                continue
            if event.get("event")=="ANALYSIS_STAGE":
                stage=int(event.get("stage_index") or 0)
                if not 1<=stage<=len(stage_names):
                    continue
                if event.get("phase")=="start":
                    current_stage=stage
                    stage_start=time.monotonic()
                    stage_frames=0
                    emit(idx,stage,0,"start")
                elif event.get("phase")=="complete":
                    if stage>len(current["stages"]):
                        stage_sec=round(time.monotonic()-(stage_start or started),2)
                        current["stages"].append({
                            "stage_index":stage,"stage":stage_names[stage-1],
                            "duration_sec":stage_sec,
                            "frames":stage_frames if stage_frames>0 else None,
                            "fps":round(stage_frames/stage_sec,1) if stage_frames>0 and stage_sec>0 else None
                        })
                        completed_steps+=1
                    emit(idx,stage,100,"complete",
                         stage_duration_sec=current["stages"][-1]["duration_sec"])
                    current_stage=0
                    write_summary()
            elif current_stage and "frames" in event and "total_frames" in event:
                try:
                    frames=int(event["frames"])
                    total_frames=int(event["total_frames"])
                except (ValueError,TypeError,OverflowError):
                    continue
                if total_frames>0 and 0<=frames<=total_frames:
                    stage_frames=max(stage_frames,frames)
                    emit(idx,current_stage,100*frames/total_frames,"running",
                         frames=frames,total_frames=total_frames,
                         eta_sec=event.get("eta_sec"))
        exit_code=process.wait()
        if exit_code:
            raise subprocess.CalledProcessError(exit_code,cmd)
        if len(current["stages"])!=len(stage_names):
            raise RuntimeError(f"Недостаточно подтверждённых этапов у {source.name}: "
                               f"{len(current['stages'])}/{len(stage_names)}")
        # Child studio writes the exact selected cache into processing.json.
        # Never infer it from the "latest" cache: repeated profiles reuse old
        # content-addressed directories, while each launch has a new record.
        entries = sorted(output.rglob("reports/processing.json"),
                         key=lambda file: file.stat().st_mtime_ns, reverse=True)
        for entry in entries:
            try:
                item = json.loads(entry.read_text(encoding="utf-8"))
                actual = Path(item["cache"]).resolve()
                if (Path(item.get("source", "")).resolve() == source.resolve()
                        and actual.parent == work.resolve()
                        and (actual / "inference.manifest.json").is_file()
                        and (actual / "players.json").is_file()):
                    current["cache_id"] = actual.name
                    break
            except (OSError, ValueError, KeyError, TypeError):
                continue
        if not current.get("cache_id"):
            raise RuntimeError("Не удалось сохранить ссылку на результаты анализа " + source.name)
        if args.player_identity_mode == "amateur":
            # Reuse immutable RAVEL outputs. CPU ReID is tied to the same
            # inference cache, never included in the neural cache signature.
            from scripts.volleyball_ui.player_identity_groups import (
                configure_identity, overview)
            import time as _identity_time
            configure_identity(args.project_id, source.name, current["cache_id"],
                mode="amateur",
                threshold_percent=args.player_reid_threshold_percent,
                auto_assign_threshold_percent=args.player_auto_assign_threshold_percent,
                auto_assign_enabled=args.player_auto_assign,
                session_gap_sec=args.player_session_gap_sec)
            current["identity_status"] = "running"
            write_summary()
            while True:
                identity = overview(args.project_id, source.name, current["cache_id"])
                if identity["status"] != "running":
                    break
                print(json.dumps({"event":"UI_PROGRESS","kind":"analyze",
                    "stage":"CPU ReID · связь игроков между розыгрышами",
                    "phase":"running", "overall_percent":99.0,
                    "stage_percent":identity.get("progress",0),
                    "elapsed_sec":round(time.monotonic()-started,1),
                    "profile_name":args.profile_name},ensure_ascii=False),flush=True)
                _identity_time.sleep(.4)
            current["identity_status"] = identity["status"]
            current["identity_groups"] = len(identity.get("groups",[]))
            current["identity_auto_linked_ids"] = sum(len(x)
                for x in identity.get("auto_links",{}).values())
            if identity["status"] == "error":
                current["identity_error"] = identity.get("error","Не удалось сопоставить игроков")
                print("CPU REID WARNING: "+current["identity_error"],flush=True)
        current.update(status="complete",duration_sec=round(
            sum(stage["duration_sec"] for stage in current["stages"]),2))
        write_summary()
        print(f"PROJECT ANALYSIS COMPLETE {idx}/{total}: {source.name}",flush=True)
      summary["status"]="complete"
      summary["gpu_finished_at"]=datetime.now(timezone.utc).isoformat()
      summary["finished_at"]=summary["gpu_finished_at"]
      summary["duration_sec"]=round(time.monotonic()-started,2)
      summary["gpu_duration_sec"]=summary["duration_sec"]
      write_summary()
      # GPU inference is finished and immutable. Create the recommended
      # interpretation as an independent, versioned CPU replay. This can be
      # tuned repeatedly later without rerunning either neural network.
      try:
          from scripts.run_project_reprocess import process_project
          print(json.dumps({"event":"UI_PROGRESS","kind":"reprocess",
              "stage":"Рекомендуемая CPU-интерпретация · касания и игроки",
              "phase":"running","overall_percent":0,
              "profile_name":args.profile_name},ensure_ascii=False),flush=True)
          from scripts.volleyball_ui.cpu_defaults import automatic_cpu_settings
          cpu = process_project(args.project_id,"run_"+run_id,
              **automatic_cpu_settings(args.player_model))
          summary["default_cpu_status"]="complete"
          summary["default_cpu_versions"]=cpu["videos"]
          print("DEFAULT CPU INTERPRETATION COMPLETE",flush=True)
      except Exception as cpu_error:
          summary["default_cpu_status"]="error"
          summary["default_cpu_error"]=str(cpu_error)
          print("DEFAULT CPU INTERPRETATION WARNING: "+str(cpu_error),flush=True)
      finally:
          summary["finished_at"]=datetime.now(timezone.utc).isoformat()
          summary["duration_sec"]=round(time.monotonic()-started,2)
          write_summary()
      print(json.dumps({"event":"UI_PROGRESS","kind":"analyze",
                        "stage":"Все видео обработаны","phase":"complete",
                        "overall_percent":100,"stage_percent":100,
                        "completed_stages":total_steps,"total_stages":total_steps,
                        "stage_index":len(stage_names),"stage_total":len(stage_names),
                        "video_index":total,"videos_total":total,
                        "elapsed_sec":summary["duration_sec"]},ensure_ascii=False),flush=True)
      print(f"PROJECT ANALYSIS FINISHED ({total} videos)",flush=True)
    except BaseException as error:
      summary["status"]="failed"
      summary["error"]=str(error)
      summary["finished_at"]=datetime.now(timezone.utc).isoformat()
      summary["duration_sec"]=round(time.monotonic()-started,2)
      write_summary()
      raise

if __name__=="__main__":
    main()
