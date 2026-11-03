"""Quick Reel standalone pipeline: shared prepared videos, ALWAYS fresh inference."""
from __future__ import annotations

import argparse
import json
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

from . import data
from .quick_reel_jobs import quick_dir, request_for
from .quick_reel_prepare import prepare_source

STAGES = ("Подготовка видео", "Анализ матча с настройками по умолчанию",
          "Монтаж под музыку")
_ACTIVE_ID = None
_STATUS = {}
_LAST_WRITE = 0.


def _save(force=False):
    global _LAST_WRITE
    if not _ACTIVE_ID:
        return
    now = time.monotonic()
    if not force and now-_LAST_WRITE < .6:
        return
    data.save_json(quick_dir(_ACTIVE_ID) / "status.json", _STATUS)
    _LAST_WRITE = now


def update(stage, fraction, *, detail="", force=False):
    if not 1 <= stage <= 3:
        raise ValueError("Недопустимая стадия быстрого рилса")
    progress = (0, 15, 85)[stage-1] + (15, 70, 15)[stage-1] * max(0, min(1, fraction))
    _STATUS.update(status="running", stage=detail or STAGES[stage-1],
                   stage_index=stage, stage_total=3,
                   progress=round(progress, 2), updated_at=time.time())
    print(json.dumps({
        "event":"UI_PROGRESS", "kind":"quick_reel",
        "stage":_STATUS["stage"], "stage_index":stage,"stage_total":3,
        "completed_stages":stage-1 if fraction < 1 else stage,
        "total_stages":3, "overall_percent":_STATUS["progress"]
    }, ensure_ascii=False),flush=True)
    _save(force=force or fraction>=1)


def _analyze(reel_id: str, files: list) -> None:
    from scripts.volleyball_studio.analysis_stages import STAGES as analysis_stages
    n = len(files)
    for index, row in enumerate(files, 1):
        name = row["video"]
        row["state"] = "analyzing"
        row["analysis_progress"] = 0
        update(2, (index-1)/n, detail=f"Анализ {index}/{n}: {name}", force=True)
        command = [sys.executable, "-m", "scripts.volleyball_ui.quick_reel_analyze",
                   "--id", reel_id, "--video-index", str(index)]
        process = subprocess.Popen(command, cwd=data.ROOT,
                                   stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                   text=True, errors="replace", bufsize=1,
                                   stdin=subprocess.DEVNULL)
        assert process.stdout is not None
        for line in process.stdout:
            print(line, end="", flush=True)
            if not line.startswith("{"):
                continue
            try:
                event=json.loads(line)
            except ValueError:
                continue
            if event.get("event")!="ANALYSIS_STAGE":
                continue
            stage_index = event.get("stage_index",0)
            if not isinstance(stage_index,int) or not 1<=stage_index<=len(analysis_stages):
                continue
            fraction = (stage_index if event.get("phase")=="complete"
                        else stage_index-1)/len(analysis_stages)
            row["analysis_progress"] = round(100*fraction,1)
            update(2, ((index-1)+fraction)/n,
                   detail=f"Видео {index}/{n}: {event['stage']}")
        if process.wait():
            raise RuntimeError("Анализ видео завершился с ошибкой: "+name)
        row["state"] = "analyzed"
        row["analysis_progress"] = 100
        update(2,index/n,detail=f"Проанализировано: {name}",force=True)


def execute(config: dict):
    global _ACTIVE_ID, _STATUS
    reel_id = config["id"]
    saved = request_for(reel_id)
    if saved != config:
        raise ValueError("Параметры быстрого рилса изменены")
    _ACTIVE_ID = reel_id
    folder = quick_dir(reel_id)
    files = [{"video": item["video"], "state": "queued",
              "progress": 0} for item in saved["videos"]]
    _STATUS = {
        "id": reel_id, "status": "running", "stage": "Проверка исходных файлов",
        "stage_index":0, "stage_total":3, "progress":0,
        "videos":files, "updated_at":time.time(),
    }
    _save(force=True)
    from .quick_reel_audio import inspect_audio
    soundtrack = inspect_audio(saved["audio_path"])
    if abs(soundtrack["duration"]-saved["duration"])>.2:
        raise ValueError("Музыка изменилась после выбора")
    for item in saved["videos"]:
        source = Path(item["source"]).resolve(strict=True)
        stat = source.stat()
        if (str(source)!=item["source"] or stat.st_size!=item["size"]
                or stat.st_mtime_ns!=item["mtime_ns"]):
            raise ValueError("Исходное видео изменено: "+item["video"])

    update(1,0)
    n=len(files)
    for index,row in enumerate(files,1):
        row["state"]="preparing"
        update(1,(index-1)/n,detail=f"Подготовка {index}/{n}: {row['video']}")
        def prepared(percent, reused):
            row["state"]="ready" if reused else "preparing"
            row["progress"]=round(percent,1)
            row["prepared_reused"]=bool(reused)
            update(1,(index-1+percent/100)/n,
                   detail=("Используется готовый кэш: " if reused else
                           "Подготовка: ")+row["video"])
        source = saved["videos"][index-1]["source"]
        prepared_path, reused = prepare_source(source,progress=prepared)
        row["state"]="ready"
        row["progress"]=100
        row["prepared_reused"]=reused
        row["prepared_path"]=str(prepared_path)
        update(1,index/n,detail=f"Подготовлено {index}/{n}: {row['video']}",force=True)
    update(2,0)
    # Deliberately NO analysis cache lookup. Every independent reel reruns
    # VballNet, RAVEL, tracking, event classification and rally scoring.
    _analyze(reel_id,files)
    update(2,1)
    update(3,0)
    from .quick_reel_render import render_reel
    from .montage_runner import STAGES as montage_stages
    def progress(stage,done):
        update(3,(stage if done else stage-1)/len(montage_stages),
               detail=montage_stages[stage-1])
    output=render_reel(reel_id,Path(soundtrack["path"]),saved["duration"],
                       progress=progress)
    if not output.is_file() or not output.stat().st_size:
        raise RuntimeError("Не создан MP4 быстрого рилса")
    from .montage_audio import _streams
    actual=float(_streams(output).get("format",{}).get("duration") or 0)
    manifest={
        "id":reel_id,"video":output.relative_to(folder).as_posix(),
        "duration":round(actual,3),"music_duration":saved["duration"],
        "videos":[r["video"] for r in saved["videos"]],
        "audio_name":Path(soundtrack["path"]).name,
        "default_analysis":True,
    }
    data.save_json(folder/"result.json",manifest)
    update(3,1)
    _STATUS.update(status="complete",stage="Рилс готов",progress=100,
                   output=str(output),updated_at=time.time())
    _save(force=True)
    print("QUICK REEL COMPLETE:",output,flush=True)


def _terminate(signum, _frame):
    _STATUS.update(status="cancelled",stage="Обработка остановлена",
                   updated_at=time.time())
    _save(force=True)
    signal.signal(signum,signal.SIG_DFL)
    os.kill(os.getpid(),signum)


def main():
    signal.signal(signal.SIGTERM,_terminate)
    parser=argparse.ArgumentParser()
    parser.add_argument("--id",required=True)
    args=parser.parse_args()
    try:
        execute(request_for(args.id))
    except Exception as error:
        _STATUS.update(status="failed",stage="Ошибка обработки",
                       error=str(error),updated_at=time.time())
        _save(force=True)
        raise


if __name__=="__main__":
    main()
