"""Structured, explicit analysis stages emitted to the UI's local project runner."""
from __future__ import annotations

import json
import os
import time
from pathlib import Path

STAGES = (
    "Проверка подготовленного видео / диапазона",
    "Обнаружение мяча",
    "Обнаружение и отслеживание игроков",
    "Построение траекторий и розыгрышей",
    "Классификация игровых событий",
    "Подготовка результатов анализа",
)


class StageReporter:
    def __init__(self):
        self.active = 0
        self.started = 0.0

    def begin(self, index: int):
        from .common import progress_context
        if not 1 <= index <= len(STAGES):
            raise ValueError("Неверный номер этапа")
        self.active = index
        self.started = time.monotonic()
        progress_context(stage=STAGES[index - 1],
                         stage_index=index, stage_total=len(STAGES))
        self._emit("start")

    def end(self):
        if not self.active:
            raise RuntimeError("Нет запущенного этапа")
        self._emit("complete")
        self.active = 0

    def _emit(self, phase):
        print(json.dumps({
            "event": "ANALYSIS_STAGE",
            "phase": phase,
            "stage_index": self.active,
            "stage_total": len(STAGES),
            "stage": STAGES[self.active - 1],
            "stage_elapsed_sec": round(time.monotonic() - self.started, 2),
        }, ensure_ascii=False), flush=True)
        stage_output=os.environ.get("VOLLEY_EXPERIMENT_STAGES_FILE")
        if stage_output:
            payload={"stage_index":self.active,"stage":STAGES[self.active-1],
                     "phase":phase,"duration_sec":round(time.monotonic()-self.started,2)}
            with Path(stage_output).open("a",encoding="utf-8") as file:
                file.write(json.dumps(payload,ensure_ascii=False)+"\n")
        if os.environ.get("VOLLEY_EXPERIMENT")=="1":
            total=len(STAGES)+1
            completed=self.active+1 if phase=="complete" else self.active
            print(json.dumps({
                "event":"UI_PROGRESS","kind":"experiment","phase":phase,
                "stage":STAGES[self.active-1],"stage_index":self.active+1,
                "stage_total":total,"stage_percent":100 if phase=="complete" else 0,
                "completed_stages":completed,"total_stages":total,
                "overall_percent":round(100*completed/total,2),
            },ensure_ascii=False),flush=True)
