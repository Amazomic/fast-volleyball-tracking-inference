"""Completed profile results remain visible while a different profile runs."""
import json
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import patch
import unittest

from scripts import run_project_analysis
from scripts.volleyball_ui import server

ROOT=Path(__file__).resolve().parents[1]

class AnalysisResultIntegrityTests(unittest.TestCase):
    def test_no_global_job_status_gate_in_results(self):
        script=(ROOT/"scripts/volleyball_ui/static/results-ui.js").read_text(encoding="utf-8")
        self.assertIn("if(!result.analyzed)",script)
        self.assertNotIn('const status=await fetch("/api/job"',script)
        self.assertNotIn('status?.kind==="analyze"',script)

    def test_preview_header_is_static_and_introduction_removed(self):
        html=(ROOT/"scripts/volleyball_ui/static/index.html").read_text(encoding="utf-8")
        script=(ROOT/"scripts/volleyball_ui/static/results-ui.js").read_text(encoding="utf-8")
        page=html.split('id="view-analysis-results"',1)[1].split('id="view-players"',1)[0]
        self.assertIn('<h3 id="result-video-name">Предпросмотр</h3>',page)
        self.assertNotIn('<h2>Результат анализа</h2>',page)
        self.assertNotIn('Выберите видео в списке справа.',page)
        self.assertNotIn('E("result-video-name").textContent=name',script)

    def test_complete_stage_durations_stream_to_job_state(self):
        with TemporaryDirectory() as tmp:
            log=Path(tmp)/"job.log"
            events=[
                {"event":"UI_PROGRESS","kind":"analyze","phase":"complete",
                 "stage":"Проверка видео","overall_percent":16.67,
                 "profile_name":"Скорость","video_index":1,"stage_index":1,
                 "stage_duration_sec":2.15},
                {"event":"UI_PROGRESS","kind":"analyze","phase":"running",
                 "stage":"Мяч","overall_percent":25.0,
                 "profile_name":"Скорость","video_index":1,"stage_index":2}
            ]
            log.write_text("".join(json.dumps(e,ensure_ascii=False)+"\n" for e in events),
                           encoding="utf-8")
            job={"process":None,"kind":"analyze","log":log,"id":"test",
                 "started_epoch":None,"finished_epoch":None,"started_at":None,
                 "progress_buffer":b"","progress_offset":0,"progress_event":None,
                 "stage_durations":{}}
            with patch.object(server,"JOB",job):
                state=server.job_state()
            self.assertEqual(state["stage_durations"][0]["duration_sec"],2.15)
            self.assertEqual(state["stage_durations"][0]["stage_index"],1)
            self.assertEqual(state["profile_name"],"Скорость")
            self.assertEqual(state["stage"],"Мяч")
            self.assertEqual(len(state["stage_durations"]),1)

    def test_retry_recreates_interrupted_profile_not_completed(self):
        with TemporaryDirectory() as tmp:
            folder=Path(tmp)
            def write(name,profile,status):
                (folder/name).write_text(json.dumps({"profile_name":profile,"status":status}))
            write("20261101_120000_00.json","Скорость","complete")
            write("20261101_120100_00.json","Точность","running")
            self.assertTrue(run_project_analysis.rebuild_after_interrupted(folder,"Точность"))
            self.assertFalse(run_project_analysis.rebuild_after_interrupted(folder,"Скорость"))
            write("20261101_120200_00.json","Точность","complete")
            self.assertFalse(run_project_analysis.rebuild_after_interrupted(folder,"Точность"))
            write("20261101_120300_00.json","Скорость","failed")
            self.assertTrue(run_project_analysis.rebuild_after_interrupted(folder,"Скорость"))

if __name__=="__main__":
    unittest.main()
