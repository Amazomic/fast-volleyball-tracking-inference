"""Isolated short-interval experiments, independent of the canonical analysis cache."""
from __future__ import annotations

import argparse
import csv
import json
import math
import os
import subprocess
import sys
import time
import uuid
from datetime import datetime, timezone
from pathlib import Path

from scripts.volleyball_ui.data import ROOT, video_path
from scripts.volleyball_ui.projects import assert_project_video, project_dir
from scripts.volleyball_ui.preview import video_info, target_width
from scripts.volleyball_studio.advanced import validated
from scripts.volleyball_studio.ball_models import BALL_MODELS, DEFAULT_BALL_MODEL, resolve_ball_model
from scripts.volleyball_studio.common import key

MODES={"ball":"Только мяч","players":"Только игроки","full":"Полный анализ фрагмента"}
STAGES={"ball":["Подготовка фрагмента","Обнаружение мяча"],
        "players":["Подготовка фрагмента","Обнаружение игроков"],
        "full":["Подготовка фрагмента","Повторное использование видео","Обнаружение мяча","Обнаружение игроков",
                "Трекинг и розыгрыши","Игровые события","Формирование результатов"]}


def validate_config(payload: dict):
    if not isinstance(payload,dict):
        raise ValueError("Неверный запрос эксперимента")
    project_id=payload.get("project_id")
    video=payload.get("video")
    if not isinstance(project_id,str) or not project_id or not isinstance(video,str):
        raise ValueError("Сначала выберите проект и исходное видео")
    assert_project_video(project_id,video)
    info=video_info(video)
    try:
        start=float(payload.get("start"))
        seconds=float(payload.get("seconds"))
        threshold=float(payload.get("ball_threshold",.5))
        threads=int(payload.get("threads",8))
    except (ValueError,TypeError,OverflowError) as error:
        raise ValueError("Неверные время, порог или число потоков") from error
    if not all(map(math.isfinite,(start,seconds,threshold))):
        raise ValueError("Параметры должны быть конечными числами")
    total=float(info["duration_sec"])
    if start<0 or seconds<2 or seconds>total or start+seconds>total+.1:
        raise ValueError("Выбранный диапазон должен быть от 2 секунд и находиться внутри видео")
    mode=payload.get("mode","full")
    if mode not in MODES:
        raise ValueError("Режим теста должен быть: мяч, игроки или полный")
    backend=payload.get("backend","onnx")
    if backend not in ("onnx","onnx-cpu","openvino"):
        raise ValueError("Неверный движок обнаружения мяча")
    model=payload.get("ball_model",DEFAULT_BALL_MODEL)
    if model not in BALL_MODELS:
        raise ValueError("Неверная модель мяча")
    if not .05<=threshold<=.95 or not 1<=threads<=32:
        raise ValueError("Порог мяча должен быть 0.05–0.95, потоки 1–32")
    requested=payload.get("analysis_width","auto")
    width=target_width(info,requested)
    if not (320<=width<=1920 and width%2==0):
        raise ValueError("Некорректная ширина анализа")
    advanced=validated(payload)
    # A full short test must have a visually inspectable player overlay.
    # Store this override in the experiment settings, not the main project.
    if mode=="full":advanced["player_output_video"]=True
    return {
        "project_id":project_id,"video":video,
        "profile_name":str(payload.get("profile_name") or "Рекомендуемый").strip()[:100],
        "start":round(start,3),"seconds":round(min(seconds,total-start),3),
        "mode":mode,"backend":backend,"ball_model":model,
        "ball_threshold":threshold,"threads":threads,
        "analysis_width":width,"beach":bool(payload.get("beach",True)),
        "advanced":advanced,
    }


def experiment_root(project_id: str) -> Path:
    return project_dir(project_id)/"experiments"


def history(project_id: str, limit=12):
    path=experiment_root(project_id)/"runs"
    if not path.exists():return []
    entries=[]
    for file in sorted(path.glob("*.json"),reverse=True)[:max(1,min(limit,50))]:
        try:
            data=json.loads(file.read_text(encoding="utf-8"))
            if isinstance(data,dict):
                entries.append({k:data.get(k) for k in (
                    "id","video","start","seconds","mode","status",
                    "started_at","elapsed_sec","metrics","settings",
                    "video_file","error","advanced_settings")})
        except (OSError,ValueError):
            continue
    return entries


def result_media(project_id: str, run_id: str):
    if not isinstance(run_id,str) or len(run_id)>45 or not run_id.replace("_","").isalnum():
        raise ValueError("Некорректный ID теста")
    root=experiment_root(project_id)
    file=root/"runs"/(run_id+".json")
    if not file.is_file():raise FileNotFoundError("Тест не найден")
    data=json.loads(file.read_text(encoding="utf-8"))
    candidate=data.get("video_file")
    if not candidate or not isinstance(candidate,str):
        raise FileNotFoundError("Для этого теста нет видео с разметкой")
    target=(root/candidate).resolve()
    if not target.is_relative_to(root.resolve()) or not target.is_file() or target.suffix.lower()!=".mp4":
        raise FileNotFoundError("Видео эксперимента недоступно")
    return target


def emit(index,total,phase,stage,started,*,frames=None,total_frames=None):
    completed=index if phase=="complete" else index-1
    progress=100 if phase=="complete" else 0
    print(json.dumps({
        "event":"UI_PROGRESS","kind":"experiment","stage":stage,"phase":phase,
        "overall_percent":round(100*(completed+(0 if phase=="complete" else progress/100))/total,2),
        "stage_percent":progress,"completed_stages":completed,"total_stages":total,
        "stage_index":index,"stage_total":total,
        "elapsed_sec":round(time.monotonic()-started,2),
        "frames":frames,"total_frames":total_frames,
    },ensure_ascii=False),flush=True)


def run_checked(cmd,log_path):
    # The parent UI job receives only progress/log summaries; command output is
    # kept per experiment instead of flooding the app's server log.
    print("Команда: "+" ".join(str(x) for x in cmd),flush=True)
    with log_path.open("w",encoding="utf-8") as output:
        result=subprocess.run([str(x) for x in cmd],cwd=ROOT,stdin=subprocess.DEVNULL,
                              stdout=output,stderr=subprocess.STDOUT,check=False)
    if result.returncode:
        tail=log_path.read_text(encoding="utf-8",errors="replace")[-3000:]
        raise RuntimeError(f"Этап завершился с кодом {result.returncode}: {tail}")
    print("Этап завершён: "+log_path.stem,flush=True)


def count_ball(csv_path):
    with csv_path.open(newline="",encoding="utf-8-sig") as file:
        rows=list(csv.DictReader(file))
    positive=sum(float(row.get("Visibility",row.get("visibility",0)) or 0)>0 for row in rows)
    return {"frames":len(rows),"ball_detected":positive,
            "ball_coverage_percent":round(100*positive/max(1,len(rows)),1)}


def count_players(path):
    data=json.loads(path.read_text(encoding="utf-8"))
    tracks={}
    observations=0
    for item in data.get("predictions",[]):
        if item.get("class_name")!="player" or item.get("track_id") is None or item.get("interpolated"):
            continue
        observations+=1
        identifier=str(item["track_id"])
        tracks[identifier]=tracks.get(identifier,0)+1
    fps=float(data.get("fps") or 30)
    return {"track_ids":len(tracks),"short_track_ids":sum(v<fps*3 for v in tracks.values()),
            "long_track_ids":sum(v>=fps*10 for v in tracks.values()),
            "player_observations":observations}


def _ball_args(prepared,folder,config):
    model,xml=resolve_ball_model(config["ball_model"])
    backend=config["backend"]
    args=[sys.executable,ROOT/"src"/(
        "inference_openvino_seq_gray_v2.py" if backend=="openvino"
        else "inference_onnx_seq_gray_v2.py"),
        "--video_path",prepared,"--output_dir",folder/"ball"]
    # Video with detection overlay is for reviewing quick experiments.
    if backend=="openvino":
        args.extend(["--model_xml",xml,"--device","CPU",
                     "--num-threads",str(config["threads"]),
                     "--threshold",str(config["ball_threshold"])])
    else:
        args.extend(["--model_path",model,"--device",
                     "cpu" if backend=="onnx-cpu" else "cuda",
                     "--num-threads",str(config["threads"]),
                     "--confidence_threshold",str(config["ball_threshold"])])
    return args


def _player_args(prepared,folder,config):
    adv=config["advanced"]
    args=[sys.executable,ROOT/"src/inference_player_ball_openvino.py",prepared,
          "--model",ROOT/"ov"/(adv["player_model"]+".xml"),
          "--output",folder/"players.json",
          "--output-video",folder/"players-annotated.mp4",
          "--device",adv["player_device"],"--num-threads",str(config["threads"]),
          "--score-threshold",str(adv["player_score_threshold"]),
          "--close-threshold",str(adv["player_close_threshold"]),
          "--hysteresis-frames",str(adv["player_hysteresis_frames"]),
          "--match-center-distance",str(adv["player_match_center_distance"]),
          "--match-iou",str(adv["player_match_iou"]),
          "--ball-threshold",str(adv["player_ball_threshold"]),
          "--head-threshold",str(adv["player_head_threshold"]),
          "--nms-iou",str(adv["player_nms_iou"]),
          "--stride",str(adv["player_stride"]),
          "--frame-step",str(adv["player_frame_step"]),
          "--warmup-runs",str(adv["player_warmup_runs"]),
          "--performance-hint",adv["player_performance_hint"]]
    if adv["player_include_features"]:args.append("--include-features")
    return args


def execute(config: dict, run_id: str | None=None):
    config=validate_config(config)
    started=time.monotonic()
    root=experiment_root(config["project_id"])
    root.mkdir(parents=True,exist_ok=True)
    identifier=run_id or datetime.now(timezone.utc).strftime("%Y%m%d_%H%M%S_")+uuid.uuid4().hex[:8]
    output=root/"variants"/identifier
    output.mkdir(parents=True,exist_ok=False)
    run_path=root/"runs"/(identifier+".json")
    run_path.parent.mkdir(parents=True,exist_ok=True)
    initial={"id":identifier,"video":config["video"],"start":config["start"],
             "seconds":config["seconds"],"mode":config["mode"],
             "status":"running","started_at":datetime.now(timezone.utc).isoformat(),
             "profile_name":config["profile_name"],"stages":[],
             "settings":{k:v for k,v in config.items() if k!="advanced"},
             "advanced_settings":config["advanced"],
             "metrics":{},"video_file":None}
    def save():
        tmp=run_path.with_suffix(".json.tmp")
        tmp.write_text(json.dumps(initial,ensure_ascii=False,indent=2),encoding="utf-8")
        os.replace(tmp,run_path)
    save()
    step_names=STAGES[config["mode"]]
    stage_started={}
    def emit_stage(index,total,phase,stage,started,**kwargs):
        if phase=="start":
            stage_started[index]=time.monotonic()
            initial["stages"].append({"stage_index":index,"stage":stage,
                                      "duration_sec":None})
            save()
        elif phase=="complete":
            elapsed=round(time.monotonic()-stage_started.pop(index,started),2)
            for row in reversed(initial["stages"]):
                if row["stage_index"]==index and row["duration_sec"] is None:
                    row["duration_sec"]=max(0,elapsed)
                    break
            save()
        emit(index,total,phase,stage,started,**kwargs)
    try:
        source=video_path(config["video"])
        from scripts.volleyball_ui.prepared_videos import ready_path
        available = ready_path(config["project_id"], config["video"])
        if available is None:
            raise RuntimeError("Видео ещё не подготовлено к анализу: " +
                               config["video"])
        stat=available.stat()
        # v2 keys never reuse older experiment clips derived from raw sources.
        prep_key=key(["project-prepared-v2",str(available),stat.st_size,
                      stat.st_mtime_ns,config["start"],config["seconds"],
                      config["analysis_width"],30])
        clip=root/"prepared"/(prep_key+".mp4")
        clip.parent.mkdir(parents=True,exist_ok=True)
        emit_stage(1,len(step_names),"start",step_names[0],started)
        if not clip.is_file() or clip.stat().st_size<20000:
            temp=clip.with_suffix(".part.mp4")
            if temp.exists():temp.unlink()
            cmd=["ffmpeg","-nostdin","-hide_banner","-loglevel","error","-y",
                 "-ss",str(config["start"]),"-i",str(available),"-t",str(config["seconds"]),
                 "-vf",f"fps=30,scale={config['analysis_width']}:-2,setsar=1",
                 "-an","-c:v","libx264","-preset","ultrafast","-crf","18",
                 "-threads",str(config["threads"]),str(temp)]
            run_checked(cmd,output/"prepare.log")
            if not temp.is_file() or temp.stat().st_size<20000:
                raise RuntimeError("Не удалось создать короткий фрагмент")
            temp.replace(clip)
        else:
            print("Повторно использован подготовленный фрагмент: "+str(clip),flush=True)
        initial["prepared_video"]=str(clip.relative_to(root))
        emit_stage(1,len(step_names),"complete",step_names[0],started)
        mode=config["mode"]
        if mode=="ball":
            (output/"ball").mkdir(parents=True,exist_ok=True)
            emit_stage(2,2,"start",step_names[1],started)
            run_checked(_ball_args(clip,output,config),output/"ball.log")
            candidates=list((output/"ball").rglob("*.csv"))
            if not candidates:raise RuntimeError("Детектор мяча не записал CSV")
            initial["metrics"]=count_ball(candidates[0])
            videos=list((output/"ball").rglob("*.mp4"))
            if videos:initial["video_file"]=str(videos[0].relative_to(root))
            emit_stage(2,2,"complete",step_names[1],started)
        elif mode=="players":
            emit_stage(2,2,"start",step_names[1],started)
            run_checked(_player_args(clip,output,config),output/"players.log")
            initial["metrics"]=count_players(output/"players.json")
            if (output/"players-annotated.mp4").is_file():
                initial["video_file"]=str((output/"players-annotated.mp4").relative_to(root))
            emit_stage(2,2,"complete",step_names[1],started)
        else:
            emit_stage(2,2,"start","Полный анализ фрагмента",started)
            from scripts.volleyball_studio.__main__ import parser as studio_parser
            from scripts.volleyball_studio.analysis import analyze
            from scripts.volleyball_studio.advanced import as_cli
            argv=["--non-interactive","--analyze-only",
                  "--backend",config["backend"],"--ball-model",config["ball_model"],
                  "--ball-threshold",str(config["ball_threshold"]),
                  "--threads",str(config["threads"]),
                  "--analysis-width",str(config["analysis_width"]),
                  "--no-court-fallback",*as_cli(config["advanced"])]
            if config["beach"]:argv.append("--beach")
            args=studio_parser().parse_args(argv)
            args.analysis_start=0
            args.analysis_seconds=None
            args.prepared_input=True
            from scripts.volleyball_ui.court import court_path
            saved_court=court_path(config["project_id"],config["video"])
            args.court_json=saved_court if saved_court.is_file() else None
            # Run-scoped pipeline caches: changed thresholds must never overwrite
            # artifacts referenced by an earlier saved A/B variant.
            old_env=os.environ.get("VOLLEY_EXPERIMENT")
            old_stages=os.environ.get("VOLLEY_EXPERIMENT_STAGES_FILE")
            stage_log=output/"pipeline-stages.jsonl"
            os.environ["VOLLEY_EXPERIMENT"]="1"
            os.environ["VOLLEY_EXPERIMENT_STAGES_FILE"]=str(stage_log)
            try:
                _,_,cache=analyze(clip,root/"pipeline"/identifier,args)
            finally:
                if old_env is None:os.environ.pop("VOLLEY_EXPERIMENT",None)
                else:os.environ["VOLLEY_EXPERIMENT"]=old_env
                if old_stages is None:os.environ.pop("VOLLEY_EXPERIMENT_STAGES_FILE",None)
                else:os.environ["VOLLEY_EXPERIMENT_STAGES_FILE"]=old_stages
            emit_stage(2,2,"complete","Полный анализ фрагмента",started)
            if stage_log.is_file():
                pipeline_stages=[]
                for line in stage_log.read_text(encoding="utf-8").splitlines():
                    try:
                        stage_event=json.loads(line)
                    except ValueError:
                        continue
                    if stage_event.get("phase")=="complete":
                        pipeline_stages.append({
                            "stage_index":int(stage_event["stage_index"])+1,
                            "stage":stage_event["stage"],
                            "duration_sec":round(float(stage_event["duration_sec"]),2)
                        })
                if pipeline_stages:
                    initial["stages"]=[
                        stage for stage in initial["stages"]
                        if stage["stage"]!="Полный анализ фрагмента"
                    ]+pipeline_stages
                    save()
            initial["metrics"]={}
            ballcsv=cache/"ball"/"analysis"/"ball.csv"
            if ballcsv.is_file():initial["metrics"].update(count_ball(ballcsv))
            players=cache/"players.json"
            if players.is_file():initial["metrics"].update(count_players(players))
            events=cache/"analysis"/"events.json"
            if events.is_file():
                obj=json.loads(events.read_text(encoding="utf-8"))
                initial["metrics"]["events"]=sum(
                    e.get("type") not in ("rally_start","rally_end")
                    for e in obj.get("events",[]))
            tracks=cache/"analysis"/"game-tracks.json"
            if tracks.is_file():
                obj=json.loads(tracks.read_text(encoding="utf-8"))
                initial["metrics"]["rallies"]=sum(
                    (t.get("rally_classification") or {}).get("label")=="rally"
                    for t in obj)
            overlay=cache/"players-annotated.mp4"
            initial["video_file"]=str((overlay if overlay.is_file() else clip).relative_to(root))
        initial["status"]="complete"
    except BaseException as error:
        initial["status"]="failed"
        initial["error"]=str(error)
        raise
    finally:
        initial["elapsed_sec"]=round(time.monotonic()-started,2)
        initial["finished_at"]=datetime.now(timezone.utc).isoformat()
        save()
    print("БЫСТРЫЙ ТЕСТ ГОТОВ: "+str(run_path),flush=True)


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument("--config",required=True,type=Path)
    a=parser.parse_args()
    execute(json.loads(a.config.read_text(encoding="utf-8")))


if __name__=="__main__":
    main()
