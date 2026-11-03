"""Performance budgets and built-in GPU profile contracts for Volley Studio."""
import unittest
from pathlib import Path

ROOT=Path(__file__).resolve().parents[1]
APP=(ROOT/"scripts/volleyball_ui/static/app.js").read_text(encoding="utf-8")
RESULTS=(ROOT/"scripts/volleyball_ui/static/results-ui.js").read_text(encoding="utf-8")
CSS=(ROOT/"scripts/volleyball_ui/static/style.css").read_text(encoding="utf-8")
PRESETS=(ROOT/"scripts/volleyball_ui/static/analysis-presets.js").read_text(encoding="utf-8")

class ClientPerformanceTests(unittest.TestCase):
    def test_one_adaptive_job_poll_and_no_idle_preparation_subscription(self):
        self.assertNotIn("setInterval(",APP)
        self.assertIn('document.hidden?30000:state.jobStatus==="running"?2500:12000',APP)
        self.assertIn("if(state.preparationActive&&state.project===pid)",APP)
        self.assertIn("if(jobPollPending)return jobPollPending;",APP)

    def test_job_stage_tree_is_only_rebuilt_on_change(self):
        self.assertIn("if(!showLive)return;",APP)
        self.assertIn("if(next.dataset.stageSignature!==stageSignature)",APP)
        self.assertIn("next.dataset.stageSignature=stageSignature;",APP)

    def test_checkbox_selection_does_not_recreate_player_tables(self):
        handler=APP.split('$("#raw-players").addEventListener("change"',1)[1].split(
            '$("#raw-players").addEventListener("click"',1)[0]
        self.assertIn("syncTrackSelection()",handler)
        self.assertNotIn("renderPlayers()",handler)
        self.assertIn("const groupById=new Map",APP)

    def test_photo_fetch_bursts_are_debounced(self):
        self.assertIn("function schedulePlayerPhotos(){",APP)
        self.assertIn("clearTimeout(photoRefreshTimer);",APP)
        self.assertEqual(APP.count("syncTrackSelection();\n    schedulePlayerPhotos();"),4)

    def test_rallies_and_video_overlays_use_indexed_events(self):
        self.assertIn("const count=bound(r.end,true)-bound(r.start,false)",RESULTS)
        self.assertIn("const stride=Math.max(1,Math.ceil(visibleActions.length/500))",RESULTS)
        self.assertIn("const near=closestVisibleAction(t)",RESULTS)
        self.assertIn('if(!data||!E("view-analysis-results").classList.contains("active"))return;',RESULTS)
        self.assertIn("sortedActions=(result.actions||[])",RESULTS)

    def test_large_offscreen_cards_have_containment(self):
        self.assertIn(".result-rally-item,.analysis-run-record,.player-identity-slot{",CSS)
        self.assertIn("content-visibility:auto;",CSS)

    def test_new_profile_is_gpu_pair(self):
        self.assertIn('name:"Новый"',PRESETS)
        self.assertIn('player_model:"RAVEL-VB-013-9f"',PRESETS)
        self.assertEqual(PRESETS.count('backend:"onnx",'),3)
        self.assertEqual(PRESETS.count('player_backend:"onnx-cuda"'),3)

    def test_013_is_part_of_sequential_5x2_benchmark(self):
        from scripts.benchmark_project import MODELS,DEVICES
        self.assertEqual(len(MODELS),5)
        self.assertIn("RAVEL-VB-013-9f.onnx",MODELS)
        self.assertEqual(DEVICES,("cpu","cuda"))
        self.assertEqual(len(MODELS)*len(DEVICES),10)

if __name__=="__main__": unittest.main()

