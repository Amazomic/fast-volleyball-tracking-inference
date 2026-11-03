"""History is saved per invocation, grouped by profile in UI, with stage times."""
import json
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

from scripts.volleyball_ui import analysis_history


class AnalysisHistoryTests(unittest.TestCase):
    def setUp(self):
        self.tmp=TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root=Path(self.tmp.name)
        self.project=self.root/"project"
        self.project.mkdir()
        for target, value in [
            ("scripts.volleyball_ui.analysis_history.get_project",
             {"videos":["game.mp4"]}),
            ("scripts.volleyball_ui.analysis_history.project_run_root",
             self.project/"runs"),
            ("scripts.volleyball_ui.analysis_history.project_dir",
             self.project),
        ]:
            if isinstance(value,dict):
                func=lambda project_id, data=value: data
            else:
                func=lambda project_id, path=value: path
            started=patch(target,side_effect=func)
            started.start()
            self.addCleanup(started.stop)

    def save(self, folder, name, record):
        path=self.project/folder/(name+".json")
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(record,ensure_ascii=False),encoding="utf-8")

    def test_profiles_and_stage_timing_from_both_job_types(self):
        self.save("runs/analysis_timings","old",{
            "status":"complete",
            "profile_name":"Точность","settings":{"backend":"onnx"},
            "started_at":"2026-11-01T13:00:00+00:00",
            "finished_at":"2026-11-01T13:02:00+00:00",
            "duration_sec":120.0,
            "stage_names":["Подготовка","Мяч"],
            "videos_total":1,
            "videos":[{"video":"game.mp4","status":"complete",
                       "duration_sec":110,
                       "stages":[{"stage_index":1,"stage":"Подготовка","duration_sec":2.5},
                                 {"stage_index":2,"stage":"Мяч","duration_sec":107.5}]}],
        })
        self.save("experiments/runs","new",{
            "status":"complete",
            "profile_name":"Скорость","settings":{"backend":"onnx"},
            "video":"game.mp4",
            "mode":"players",
            "started_at":"2026-11-01T14:00:00+00:00",
            "finished_at":"2026-11-01T14:00:20+00:00",
            "elapsed_sec":20,
            "stages":[{"stage_index":1,"stage":"Подготовка фрагмента","duration_sec":3},
                      {"stage_index":2,"stage":"Обнаружение игроков","duration_sec":17}],
        })
        rows=analysis_history.list_history("demo")
        self.assertEqual(len(rows),2)
        self.assertEqual([x["profile"] for x in rows],["Скорость","Точность"])
        self.assertEqual([x["duration_sec"] for x in rows],[20,120])
        self.assertEqual([s["duration_sec"] for s in rows[0]["videos"][0]["stages"]],[3,17])
        self.assertEqual([s["duration_sec"] for s in rows[1]["videos"][0]["stages"]],[2.5,107.5])
        self.assertEqual(rows[0]["stage_names"],["Подготовка фрагмента","Обнаружение игроков"])
        self.assertIn("13:00",rows[1]["started_at"])

    def test_legacy_invocations_are_hidden_without_deleting_files(self):
        self.save("runs/analysis_timings","old",{
            "status":"failed","started_at":"2026-11-01T12:00:00+00:00",
            "duration_sec":0,"videos":[],"error":"failed"})
        self.save("experiments/runs","older",{
            "video":"game.mp4","mode":"ball","status":"complete",
            "started_at":"2026-11-01T10:00:00+00:00",
            "elapsed_sec":15})
        rows=analysis_history.list_history("demo")
        self.assertEqual(rows,[])
        self.assertTrue((self.project/"experiments/runs/older.json").is_file())

    def test_running_cancelled_and_failed_runs_are_not_displayed(self):
        for state in ("running","failed","cancelled"):
            self.save("runs/analysis_timings","interrupted_"+state,{
                "status":state,"profile_name":"Скорость","videos_total":1,
                "started_at":"2026-11-01T15:00:00+00:00",
                "videos":[{"video":"game.mp4","status":state,"stages":[]}],
            })
            self.save("experiments/runs","fragment_"+state,{
                "video":"game.mp4","status":state,"mode":"ball",
                "started_at":"2026-11-01T15:00:00+00:00",
            })
        self.assertEqual(analysis_history.list_history("demo"),[])

    def test_incomplete_videos_rejects_parent_complete(self):
        self.save("runs/analysis_timings","bad",{
            "status":"complete","profile_name":"Скорость","videos_total":1,
            "started_at":"2026-11-01T15:00:00+00:00",
            "videos":[{"video":"game.mp4","status":"running","stages":[]}],
        })
        self.assertEqual(analysis_history.list_history("demo"),[])

    def test_no_cross_project_experiments(self):
        self.save("experiments/runs","wrong",{
            "video":"another.mp4","status":"complete","mode":"ball"})
        self.assertEqual(analysis_history.list_history("demo"),[])

    def test_stage_reporter_records_pipeline_timings_for_full_fragments(self):
        import io
        import os
        import time
        from contextlib import redirect_stdout
        from scripts.volleyball_studio.analysis_stages import StageReporter
        path=self.root/"studio-stages.jsonl"
        reporter=StageReporter()
        reporter.active=2
        reporter.started=time.monotonic()-0.1
        with patch.dict(os.environ, {"VOLLEY_EXPERIMENT_STAGES_FILE":str(path)}), redirect_stdout(io.StringIO()):
            reporter._emit("complete")
        records=[json.loads(line) for line in path.read_text(encoding="utf-8").splitlines()]
        self.assertEqual(len(records),1)
        self.assertEqual(records[0]["stage_index"],2)
        self.assertEqual(records[0]["phase"],"complete")
        self.assertGreater(records[0]["duration_sec"],0)


if __name__=="__main__":
    unittest.main()
