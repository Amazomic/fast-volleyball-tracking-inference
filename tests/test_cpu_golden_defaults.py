"""Regression checks for the shared golden CPU replay policy."""
import json
import sys
import tempfile
import types
import unittest
from unittest.mock import patch
from pathlib import Path
from scripts.volleyball_ui.cpu_defaults import golden_cpu_settings, automatic_cpu_settings

ROOT = Path(__file__).resolve().parents[1]


class GoldenCpuTests(unittest.TestCase):
    def test_versioned_golden_settings(self):
        manifest = json.loads((ROOT/"scripts/volleyball_ui/static/cpu-defaults.json").read_text("utf-8"))
        self.assertEqual(manifest["schema"],"volley-cpu-golden-defaults/1")
        self.assertGreaterEqual(manifest["revision"],1)
        a = golden_cpu_settings()
        a["touch_threshold"] = 0
        self.assertEqual(golden_cpu_settings()["touch_threshold"],.45)
        self.assertEqual(golden_cpu_settings()["player_threshold"],.30)
        self.assertEqual(golden_cpu_settings()["ball_threshold"],.44)  # Golden revision 4; see docs/CPU_GOLDEN_DEFAULTS.md
        self.assertEqual(golden_cpu_settings()["mode"],"combined")
        self.assertTrue(golden_cpu_settings()["beach"])

    def test_older_models_use_compatible_geometry_fallback(self):
        replay = automatic_cpu_settings("RAVEL-VB-012-9f")
        self.assertEqual(replay["mode"],"baseline")
        self.assertIsNone(replay["player_threshold"])
        self.assertEqual(automatic_cpu_settings("RAVEL-VB-013-9f"),golden_cpu_settings())

    def test_inference_and_quick_reel_share_one_source(self):
        analysis = (ROOT/"scripts/run_project_analysis.py").read_text("utf-8")
        quick = (ROOT/"scripts/volleyball_ui/quick_reel_analyze.py").read_text("utf-8")
        ui = (ROOT/"scripts/volleyball_ui/static/results-ui.js").read_text("utf-8")
        html = (ROOT/"scripts/volleyball_ui/static/index.html").read_text("utf-8")
        self.assertIn("**automatic_cpu_settings(args.player_model)",analysis)
        self.assertIn("cpu_settings = automatic_cpu_settings(args.player_model)",quick)
        self.assertIn("cache = reprocess(inference_cache, **cpu_settings)",quick)
        self.assertIn('fetch("/cpu-defaults.json"',ui)
        self.assertIn('id="result-reprocess-reset"',html)
        self.assertIn('player_hold_frames:settings.player_hold_frames??90',
                      (ROOT/"scripts/volleyball_ui/static/app.js").read_text("utf-8"))

    def test_quick_reel_consumes_golden_cpu_cache(self):
        from scripts.volleyball_ui import quick_reel_analyze as quick
        with tempfile.TemporaryDirectory() as tmp:
            folder=Path(tmp)
            source=folder/"game.mp4"
            source.write_bytes(b"video")
            prepared=folder/"prepared.mp4"
            prepared.write_bytes(b"prepared")
            gpu_cache=folder/"analysis"/"video_01"/"gpu"
            seen={}
            gpu_module=types.ModuleType("scripts.volleyball_studio.analysis")
            def fake_analyze(video,work,args):
                seen["fresh_source"]=video
                seen["cache_salt"]=args.cache_salt
                gpu_cache.mkdir(parents=True,exist_ok=True)
                return None, [], gpu_cache
            gpu_module.analyze=fake_analyze
            cpu_module=types.ModuleType("scripts.volleyball_studio.reprocess")
            def fake_reprocess(cache,**settings):
                seen["gpu_cache"]=cache
                seen["cpu_settings"]=settings
                output=cache/"postprocessing"/"golden"
                (output/"analysis").mkdir(parents=True)
                (output/"ranking.json").write_text("{}")
                (output/"analysis"/"events.json").write_text("{}")
                return output
            cpu_module.reprocess=fake_reprocess
            entry={"video":"game.mp4","source":str(source),
                   "size":source.stat().st_size,
                   "mtime_ns":source.stat().st_mtime_ns}
            with (patch.object(quick,"request_for",return_value={"videos":[entry]}),
                  patch.object(quick,"ready_source_path",return_value=prepared),
                  patch.object(quick,"prepared_entry",return_value=(source,{},prepared,folder/"marker",{"width":1280})),
                  patch.object(quick,"quick_dir",return_value=folder),
                  patch.dict(sys.modules,{"scripts.volleyball_studio.analysis":gpu_module,
                                          "scripts.volleyball_studio.reprocess":cpu_module})):
                result=quick.analyze_one("quick-test",1)
            self.assertEqual(seen["fresh_source"],source)
            self.assertEqual(seen["cache_salt"],"quick-test-1")
            self.assertEqual(seen["gpu_cache"],gpu_cache)
            self.assertEqual(seen["cpu_settings"],golden_cpu_settings())
            self.assertEqual(result["cpu_settings"],golden_cpu_settings())
            self.assertEqual(result["inference_cache"],str(gpu_cache))
            self.assertTrue(Path(result["cache"]).is_relative_to(folder))
            self.assertTrue((folder/"analysis"/"video_01"/"result.json").is_file())


if __name__=="__main__":
    unittest.main()
