"""Upstream-style neural inference benchmark regression tests."""
from __future__ import annotations

import tempfile
import unittest
from argparse import Namespace
from pathlib import Path
from unittest.mock import patch

from scripts import benchmark_project as bench


class BenchmarkTests(unittest.TestCase):
    def params(self,**overrides):
        defaults={"project_id":"project","model":"VballNetV4c_seq9_grayscale_20260908_213829.onnx",
                  "backend":"onnx-cpu","threads":10,"warmup":2,"runs":5,"repeats":3}
        defaults.update(overrides)
        return Namespace(**defaults)

    def test_backend_and_upstream_defaults(self):
        for runtime in bench.BACKENDS:
            args=bench.parse_args(["--project-id","project","--backend",runtime])
            self.assertEqual(args.backend,runtime)
            self.assertEqual((args.warmup,args.runs,args.repeats),(10,60,3))
        self.assertEqual(bench.DEFAULT_RUNS,60)

    def test_available_models_are_vballnet_files_only(self):
        models=bench.available_models()
        self.assertIn("VballNetV4c_seq9_grayscale_20260908_213829.onnx",models)
        self.assertNotIn("RAVEL-VB-012-9f.onnx",models)
        self.assertEqual(len(bench.model_paths("all")),len(models))
        for bad in ("../private","RAVEL-VB-011-9f.onnx","foo.onnx"):
            with self.assertRaises(ValueError):
                bench.model_paths(bad)

    def test_calculates_seq_per_median_latency_and_stores_history(self):
        with tempfile.TemporaryDirectory() as td:
            model=Path(td)/"benchmark.onnx"
            model.touch()
            counter={"load":0,"calls":0}
            def fake_loader(path,backend,threads):
                counter["load"]+=1
                def execute(tensor):
                    counter["calls"]+=1
                    self.assertEqual(tensor.shape,(1,9,32,64))
                return execute,{"seq":9,"shape":(1,9,32,64),"provider":"CPU",
                                "_owner":object()}
            with patch.object(bench,"get_project"), \
                 patch.object(bench,"project_dir",return_value=Path(td)), \
                 patch.object(bench,"model_paths",return_value=[model]), \
                 patch.object(bench,"load_executor",side_effect=fake_loader):
                outcome=bench.benchmark(self.params())
            res=outcome["results"][0]
            self.assertEqual(res["mode"],"inference")
            self.assertEqual(res["seq"],9)
            self.assertEqual(res["repeats"],3)
            self.assertEqual(res["repeat_fps"].__len__(),3)
            self.assertGreater(res["fps"],0)
            self.assertEqual(counter["load"],3)
            self.assertEqual(counter["calls"],(5+2)*3)
            self.assertTrue((Path(td)/"benchmarks"/"history.json").is_file())
            self.assertFalse((Path(td)/"benchmarks"/"runs"/outcome["id"]/"input.mp4").exists())

    def test_no_decode_pipeline_or_video_options(self):
        a=bench.parse_args(["--project-id","p","--model","all","--threads","8"])
        self.assertFalse(hasattr(a,"video"))
        self.assertFalse(hasattr(a,"duration"))
        self.assertFalse(hasattr(a,"mode"))
        self.assertEqual(a.model,"all")

    def test_invalid_measurement_settings(self):
        for changes in ({"threads":0},{"warmup":31},{"runs":4},{"repeats":6}):
            with self.subTest(changes=changes):
                with self.assertRaises(ValueError):
                    bench.validate(self.params(**changes))

if __name__=="__main__":
    unittest.main()
