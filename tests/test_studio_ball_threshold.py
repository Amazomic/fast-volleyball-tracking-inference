"""Regression: UI confidence cutoff reaches the selected inference backend."""
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch, MagicMock

from scripts.volleyball_studio.__main__ import parser
from scripts.volleyball_studio import analysis
from scripts.volleyball_ui import server


class StopAfterBall(Exception):
    pass


class BallThresholdTests(unittest.TestCase):
    def test_cli_default_and_custom(self):
        self.assertEqual(parser().parse_args([]).ball_threshold, 0.5)
        self.assertEqual(parser().parse_args(["--ball-threshold", "0.75"]).ball_threshold, 0.75)

    def test_server_forwards_threshold_for_project(self):
        with tempfile.TemporaryDirectory() as tmp:
            proc=MagicMock()
            proc.poll.return_value=None
            job={"process":None,"id":None,"kind":None,"log":None,
                 "started_at":None,"command":[]}
            with patch.object(server,"JOB",job), \
                 patch.object(server,"project_videos",return_value=[Path("match.mp4")]), \
                 patch("scripts.volleyball_ui.prepared_videos.status",return_value={"ready":1,"total":1,"videos":[]}), \
                 patch.object(server,"project_logs_root",return_value=Path(tmp)), \
                 patch.object(server.subprocess,"Popen",return_value=proc) as popen:
                server.start_job("analyze",{"project_id":"project","ball_threshold":0.75})
                cmd=popen.call_args.args[0]
                self.assertIn("run_project_analysis.py",cmd[1])
                self.assertEqual(cmd[cmd.index("--ball-threshold")+1],"0.75")

    def test_bad_values_rejected(self):
        for value in (-1,0,0.049,0.951,2,float("nan"),float("inf")):
            with self.subTest(value=value):
                with patch.object(server,"project_videos",return_value=[Path("match.mp4")]):
                    with self.assertRaises(ValueError):
                        server.start_job("analyze",{"project_id":"project","ball_threshold":value})

    def test_each_backend_gets_threshold_and_cache_key_changes(self):
        for backend,option in (("onnx","--confidence_threshold"),("onnx-cpu","--confidence_threshold"),("openvino","--threshold")):
            keys=[]
            for threshold in (0.5,0.8):
                with self.subTest(backend=backend,threshold=threshold),tempfile.TemporaryDirectory() as td:
                    video=Path(td)/"match.mp4"
                    video.write_bytes(b"test")
                    args=SimpleNamespace(ball_model="VballNetV4c_seq9_grayscale_20260908_213829.onnx",
                      analysis_seconds=None,analysis_start=0,analysis_width=1280,
                      threads=8,backend=backend,ball_threshold=threshold,
                      scratch_dir=None,court_json=None,no_court_fallback=True)
                    def fake_cache(folder,stage,signature,action):
                        if stage=="ball":
                            keys.append(signature)
                            action()
                        return []
                    def fake_run(command,*_):
                        if "--only_csv" in command:
                            self.assertIn(option,command)
                            self.assertEqual(command[command.index(option)+1],str(threshold))
                            raise StopAfterBall()
                    with patch.object(analysis,"identity",return_value=["identity"]), \
                         patch.object(analysis,"digest",return_value="sha"), \
                         patch.object(analysis,"probe",return_value={"streams":[{"codec_type":"video","width":1280,"height":720}],"format":{"duration":"10"}}), \
                         patch.object(analysis,"cached",side_effect=fake_cache), \
                         patch.object(analysis,"run",side_effect=fake_run), \
                         patch("scripts.volleyball_studio.video_inputs.prepare_proxy",return_value="reused"):
                        with self.assertRaises(StopAfterBall):
                            analysis.analyze(video,Path(td)/"cache",args)
            self.assertEqual(len(keys),2)
            self.assertNotEqual(keys[0],keys[1],"Changing threshold must invalidate ball detection cache")


if __name__ == "__main__":
    unittest.main()
