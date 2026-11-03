"""Two selectable ball models must reach the full analysis pipeline."""
import tempfile
import unittest
from pathlib import Path
from unittest.mock import MagicMock, patch

from scripts.volleyball_studio.ball_models import BALL_MODELS, DEFAULT_BALL_MODEL, FAST_BALL_MODEL, resolve_ball_model
from scripts.volleyball_studio.__main__ import parser
from scripts.volleyball_ui import server


class BallModelSelectionTests(unittest.TestCase):
    def test_only_two_models_and_default(self):
        self.assertEqual(BALL_MODELS, (DEFAULT_BALL_MODEL, FAST_BALL_MODEL))
        self.assertEqual(parser().parse_args([]).ball_model, DEFAULT_BALL_MODEL)
        for name in BALL_MODELS:
            self.assertEqual(parser().parse_args(["--ball-model", name]).ball_model, name)
            onnx, openvino = resolve_ball_model(name)
            self.assertTrue(onnx.is_file())
            self.assertTrue(openvino.is_file())
        for unsafe in ("../bad.onnx", "RAVEL-VB-012-9f.onnx", "VballNetGridV3_seq9_grayscale_20260908_225156.onnx"):
            with self.assertRaises(ValueError):
                resolve_ball_model(unsafe)

    def test_analyze_endpoint_forwards_model_to_project_runner(self):
        for name in BALL_MODELS:
            with self.subTest(name=name), tempfile.TemporaryDirectory() as temp:
                process = MagicMock()
                process.poll.return_value = None
                fake_job = dict(process=None, id=None, kind=None, log=None, started_at=None, command=[])
                with patch.object(server, "JOB", fake_job), \
                     patch.object(server, "project_videos", return_value=[Path("example.mp4")]), \
                     patch.object(server, "project_logs_root", return_value=Path(temp)), \
                     patch.object(server.subprocess, "Popen", return_value=process) as popen:
                    server.start_job("analyze", {"project_id": "test-project", "ball_model": name, "analysis_width": "auto"})
                    cmd = popen.call_args.args[0]
                    self.assertIn("run_project_analysis.py", cmd[1])
                    self.assertEqual(cmd[cmd.index("--ball-model") + 1], name)
                    self.assertIn("--backend", cmd)

    def test_unknown_model_rejected_before_job_start(self):
        with patch.object(server, "project_videos", return_value=[Path("example.mp4")]):
            with self.assertRaises(ValueError):
                server.start_job("analyze", {"project_id": "project", "ball_model": "../other.onnx"})


if __name__ == "__main__":
    unittest.main()
