"""Sequential five-detector CPU/CUDA benchmarks with one common source sample."""
from __future__ import annotations

import tempfile
import unittest
from argparse import Namespace
from pathlib import Path
from unittest.mock import patch

import numpy as np
from scripts import benchmark_project as bench


class BenchmarkTests(unittest.TestCase):
    def args(self, **overrides):
        params={"project_id":"project","video":"sample.mp4","start":1.5,
                "threads":8,"warmup":2,"runs":5,"repeats":2}
        params.update(overrides)
        return Namespace(**params)

    def test_all_five_models_required(self):
        names=bench.available_models()
        self.assertEqual(names,list(bench.MODELS))
        self.assertEqual(len(bench.model_paths()),5)
        self.assertEqual(len(bench.BALL_MODELS),2)
        self.assertEqual(len(bench.RAVEL_MODELS),3)

    def test_defaults_and_rejection_of_invalid_settings(self):
        args=bench.parse_args(["--project-id","project"])
        self.assertEqual((args.warmup,args.runs,args.repeats),(3,12,2))
        self.assertEqual(bench.DEVICES,("cpu","cuda"))
        for changes in ({"threads":0},{"threads":129},{"runs":2},
                        {"warmup":31},{"repeats":6},{"start":-1}):
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                bench.validate(self.args(**changes))

    def test_same_real_frames_are_transformed_for_each_architecture(self):
        frames=[np.full((240,426,3),n,dtype=np.uint8) for n in range(9)]
        ball=bench.input_tensor(frames,("B",9,288,512))
        fast=bench.input_tensor(frames,("unk__64",9,288,512))
        self.assertEqual(fast.shape,ball.shape)
        rgb=bench.input_tensor(frames,(1,9,3,360,640))
        grey=bench.input_tensor(frames,(1,9,1,576,1024))
        self.assertEqual(ball.shape,(1,9,288,512))
        self.assertEqual(rgb.shape,(1,9,3,360,640))
        self.assertEqual(grey.shape,(1,9,1,576,1024))
        self.assertEqual(ball.dtype,np.float32)
        self.assertAlmostEqual(float(ball[0,4].mean()),4/255,places=4)
        self.assertAlmostEqual(float(grey[0,6].mean()),6/255,places=4)

    def test_sequential_ten_tests_and_history(self):
        with tempfile.TemporaryDirectory() as temp:
            root=Path(temp)
            paths=[root/name for name in bench.MODELS]
            for path in paths:path.write_bytes(b"model")
            frames=[np.zeros((60,100,3),dtype=np.uint8) for _ in range(9)]
            order=[]
            counter={"calls":0}
            def fake_loader(path,device,threads,samples):
                order.append((path.name,device))
                self.assertIs(frames,samples)
                self.assertEqual(threads,8)
                def run():
                    counter["calls"]+=1
                return run,{"provider":"CUDAExecutionProvider" if device=="cuda"
                            else "CPUExecutionProvider","shape":[1,9,12,16],
                            "seq":9,"session":object()}
            with (patch.object(bench,"get_project"),
                  patch.object(bench,"project_videos",return_value=[root/"sample.mp4"]),
                  patch.object(bench,"project_dir",return_value=root),
                  patch.object(bench,"model_paths",return_value=paths),
                  patch.object(bench,"source_frames",return_value=(frames,"a"*64)),
                  patch.object(bench,"hardware",return_value={"cpu":"test","gpu":"test"}),
                  patch.object(bench,"emit"),
                  patch.object(bench,"load_executor",side_effect=fake_loader)):
                result=bench.benchmark(self.args())
            expected=[(path.name,device) for path in paths for device in ("cpu","cuda")]
            self.assertEqual(order,[item for item in expected for _ in range(2)])
            self.assertEqual(len(result["results"]),10)
            self.assertEqual(counter["calls"],10*2*(2+5))
            self.assertEqual({r["input_hash"] for r in result["results"]},{"a"*64})
            self.assertEqual({r["status"] for r in result["results"]},{"complete"})
            self.assertEqual({r["threads"] for r in result["results"]},{8})
            self.assertEqual({r["repeats"] for r in result["results"]},{2})
            saved=bench.read_json(root/"benchmarks/history.json")
            self.assertEqual(len(saved["runs"]),10)
            self.assertTrue((root/"benchmarks"/"runs"/result["id"]/"result.json").is_file())

    def test_failed_cuda_never_generates_fictitious_fps(self):
        with tempfile.TemporaryDirectory() as temp:
            root=Path(temp)
            paths=[root/name for name in bench.MODELS]
            frames=[np.zeros((32,32,3),dtype=np.uint8) for _ in range(9)]
            def loader(path,device,threads,samples):
                if device=="cuda":
                    raise RuntimeError("CUDA unavailable")
                return (lambda:None),{"provider":"CPUExecutionProvider","shape":[1,9,32,32]}
            with (patch.object(bench,"get_project"),
                  patch.object(bench,"project_videos",return_value=[root/"sample.mp4"]),
                  patch.object(bench,"project_dir",return_value=root),
                  patch.object(bench,"model_paths",return_value=paths),
                  patch.object(bench,"source_frames",return_value=(frames,"a"*64)),
                  patch.object(bench,"hardware",return_value={}),
                  patch.object(bench,"emit"),
                  patch.object(bench,"load_executor",side_effect=loader)):
                result=bench.benchmark(self.args(runs=3,repeats=1,warmup=0))
            self.assertEqual([r["status"] for r in result["results"]].count("failed"),5)
            self.assertEqual([r["fps"] for r in result["results"] if r["device"]=="cuda"],[None]*5)

    def test_interrupted_series_retains_completed_measurements(self):
        with tempfile.TemporaryDirectory() as temp:
            root=Path(temp)
            paths=[root/name for name in bench.MODELS]
            frames=[np.zeros((32,32,3),dtype=np.uint8) for _ in range(9)]
            calls=[]
            def loader(path,device,threads,samples):
                calls.append(device)
                if device=="cuda":
                    raise KeyboardInterrupt()
                return (lambda:None),{"provider":"CPUExecutionProvider","shape":[1,9,32,32]}
            with (patch.object(bench,"get_project"),
                  patch.object(bench,"project_videos",return_value=[root/"sample.mp4"]),
                  patch.object(bench,"project_dir",return_value=root),
                  patch.object(bench,"model_paths",return_value=paths),
                  patch.object(bench,"source_frames",return_value=(frames,"a"*64)),
                  patch.object(bench,"hardware",return_value={}),
                  patch.object(bench,"emit"),
                  patch.object(bench,"load_executor",side_effect=loader)):
                with self.assertRaises(KeyboardInterrupt):
                    bench.benchmark(self.args(warmup=0,runs=3,repeats=1))
            history=bench.read_json(root/"benchmarks"/"history.json")
            self.assertEqual(len(history["runs"]),1)
            self.assertEqual(history["runs"][0]["model"],paths[0].name)
            self.assertEqual(history["runs"][0]["device"],"cpu")

    def test_server_wires_single_serial_benchmark(self):
        from unittest.mock import MagicMock
        from scripts.volleyball_ui import server
        with tempfile.TemporaryDirectory() as temp:
            process=MagicMock()
            process.poll.return_value=None
            job={"process":None,"id":None,"kind":None,"log":None,"started_at":None,"command":[]}
            source=Path(temp)/"sample.mp4"
            with (patch.object(server,"JOB",job),
                  patch.object(server,"project_videos",return_value=[source]),
                  patch.object(server,"project_logs_root",return_value=Path(temp)),
                  patch.object(server.subprocess,"Popen",return_value=process) as popen):
                server.start_job("benchmark",{"project_id":"project","video":"sample.mp4",
                                              "threads":26,"runs":5,"warmup":1,"repeats":1})
            cmd=popen.call_args.args[0]
            self.assertIn("benchmark_project.py",cmd[1])
            self.assertEqual(cmd[cmd.index("--threads")+1],"26")
            self.assertEqual(cmd[cmd.index("--video")+1],"sample.mp4")
            self.assertNotIn("--backend",cmd)


if __name__=="__main__":
    unittest.main()
