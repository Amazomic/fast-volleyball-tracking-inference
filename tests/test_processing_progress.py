"""Contract tests for per-stage timing/progress across analysis and montage."""
from __future__ import annotations
import io
import json
import sys
import tempfile
import time
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import patch

from scripts import run_project_analysis
from scripts.volleyball_ui import server
from scripts.volleyball_studio.analysis_stages import STAGES


def events_from(output):
    return [json.loads(line) for line in output.splitlines()
            if line.startswith("{") and '"event": "UI_PROGRESS"' in line]


class StageProgressTests(unittest.TestCase):
    def test_analysis_emits_six_stages_per_video_with_monotonic_total_and_persists_duration(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder)
            videos=[root/"one.mp4",root/"two.mp4"]
            class FakeProcess:
                def __init__(self,*args,**kwargs):
                    self.stdout=[]
                    for index,name in enumerate(STAGES,1):
                        self.stdout.append(json.dumps({"event":"ANALYSIS_STAGE","phase":"start",
                            "stage_index":index,"stage_total":6,"stage":name})+"\n")
                        if index==2:self.stdout.append(
                            json.dumps({"frames":50,"total_frames":100,"eta_sec":3})+"\n")
                        self.stdout.append(json.dumps({"event":"ANALYSIS_STAGE",
                            "phase":"complete","stage_index":index,"stage_total":6,"stage":name})+"\n")
                def wait(self):return 0
            buffer=io.StringIO()
            with patch.object(run_project_analysis,"project_videos",return_value=videos),\
                 patch.object(run_project_analysis,"project_ranges",return_value={}),\
                 patch.object(run_project_analysis,"project_cache_root",return_value=root/"cache"),\
                 patch.object(run_project_analysis,"project_run_root",return_value=root/"output"),\
                 patch.object(run_project_analysis,"court_path",create=True,return_value=root/"missing.json"),\
                 patch("scripts.volleyball_ui.court.court_path",return_value=root/"missing.json"),\
                 patch.object(run_project_analysis.subprocess,"Popen",side_effect=FakeProcess),\
                 patch.object(sys,"argv",["run_project_analysis.py","--project-id","demo",
                         "--analysis-width","1280"]),redirect_stdout(buffer):
                run_project_analysis.main()
            output=events_from(buffer.getvalue())
            self.assertEqual(len([e for e in output if e["phase"]=="complete" and
                e.get("stage_index",0)>0]),13)
            self.assertEqual(output[-1]["overall_percent"],100)
            self.assertEqual(output[-1]["completed_stages"],12)
            self.assertEqual(output[-1]["total_stages"],12)
            self.assertEqual([e["overall_percent"] for e in output],
                             sorted(e["overall_percent"] for e in output))
            half=[e for e in output if e.get("frames")==50]
            self.assertEqual(len(half),2)
            self.assertGreater(half[0]["overall_percent"],0)
            file=list((root/"output"/"analysis_timings").glob("*.json"))
            self.assertEqual(len(file),1)
            record=json.loads(file[0].read_text(encoding="utf-8"))
            self.assertEqual(record["status"],"complete")
            self.assertEqual(len(record["videos"]),2)
            self.assertEqual(len(record["videos"][0]["stages"]),6)
            self.assertGreaterEqual(record["duration_sec"],0)

    def test_server_progress_log_stream_survives_chunk_boundaries(self):
        class FakeProcess:
            returncode=None
            def poll(self):return None
        with tempfile.TemporaryDirectory() as folder:
            file=Path(folder)/"job.log"
            old=server.JOB.copy()
            try:
                server.JOB.update(process=FakeProcess(),id="demo",kind="analyze",log=file,
                    started_at="2026-11-01 12:00:00",started_epoch=time.time()-8,
                    finished_epoch=None,progress_event=None,progress_offset=0,
                    progress_buffer=b"")
                first={"event":"UI_PROGRESS","kind":"analyze","stage":"Мяч",
                       "overall_percent":21.5,"stage_percent":80,"stage_index":2,
                       "stage_total":6,"completed_stages":1,"total_stages":12,
                       "video_index":1,"videos_total":2,"frames":80,"total_frames":100}
                data=(json.dumps(first,ensure_ascii=False)+"\n").encode("utf-8")
                file.write_bytes(data[:17])
                self.assertEqual(server.job_state()["progress"],0)
                with file.open("ab") as f:f.write(data[17:])
                state=server.job_state()
                self.assertEqual(state["progress"],21.5)
                self.assertEqual(state["stage_percent"],80)
                self.assertEqual(state["completed_stages"],1)
                self.assertGreaterEqual(state["elapsed_sec"],7)
                later={**first,"overall_percent":19,"stage_percent":0}
                with file.open("ab") as f:f.write(
                    (json.dumps(later,ensure_ascii=False)+"\n").encode("utf-8"))
                self.assertEqual(server.job_state()["progress"],21.5)
            finally:
                server.JOB.clear()
                server.JOB.update(old)

    def test_montage_emits_steps_and_persists_elapsed_in_manifest(self):
        from scripts import build_project_highlights as montage
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp)
            source=root/"match.mp4"
            cache=root/"cache"
            cache.mkdir()
            (cache/"ranking.json").write_text("[]",encoding="utf-8")
            group={"ranked":True,"ranking_score":1,"duration_sec":4,
                   "source_start_frame":0,"source_end_frame":119,
                   "detected_start_frame":0,"detected_end_frame":119,
                   "confidence":.8}
            def fake_render(src,normalized,entries,*,output_path,segments_folder,progress_callback):
                progress_callback("clip","start",1,1,0,120)
                progress_callback("clip","running",1,1,50,120)
                progress_callback("clip","complete",1,1,120,120)
                progress_callback("join","start",1,1,0,1)
                output_path.write_bytes(b"fake")
                progress_callback("join","complete",1,1,1,1)
            buffer=io.StringIO()
            with patch.object(montage,"project_videos",return_value=[source]),\
                 patch.object(montage,"project_dir",return_value=root),\
                 patch.object(montage,"project_output",return_value=root/"final.mp4"),\
                 patch.object(montage,"locate_cache",return_value=cache),\
                 patch.object(montage,"load_groups",return_value=[group]),\
                 patch.object(montage,"choose_whole_groups",return_value=[{**group,"video":"match.mp4"}]),\
                 patch.object(montage,"render",side_effect=fake_render),\
                 patch.object(sys,"argv",["build_project_highlights.py","--project-id","demo"]),\
                 redirect_stdout(buffer):
                montage.main()
            values=events_from(buffer.getvalue())
            self.assertEqual(values[0]["total_stages"],4)
            self.assertEqual(values[-1]["overall_percent"],100)
            self.assertEqual(values[-1]["completed_stages"],4)
            self.assertEqual([e["overall_percent"] for e in values],
                             sorted(e["overall_percent"] for e in values))
            self.assertTrue(any(e.get("frames")==50 and
                0<e["stage_percent"]<100 for e in values))
            final=json.loads((root/"highlights_manifest.json").read_text(encoding="utf-8"))
            self.assertIn("elapsed_sec",final)
            self.assertEqual(len(final["step_durations"]),3)
            self.assertTrue((root/"final.mp4").is_file())


if __name__=="__main__":
    unittest.main()
