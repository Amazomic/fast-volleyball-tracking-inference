"""API command routing: analysis settings vs montage filters."""
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui import server


class MockProcess:
    returncode = 0
    def poll(self):
        return 0


class MontageCommandTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.source = self.root / "one.mp4"
        self.source.write_bytes(b"video")
        job = {"process":None,"id":None,"kind":None,"log":None,"started_at":None,"command":[]}
        patches = [
            patch.object(server, "JOB", job),
            patch.object(server, "project_videos", return_value=[self.source]),
            patch.object(server, "project_logs_root", return_value=self.root/"logs"),
            patch.object(server, "find_cache", return_value=self.root/"cache"),
            patch.object(server, "video_info", return_value={
                "width":1080,"height":1920,"duration_sec":120,"auto_width":1080
            }),
            patch.object(server.subprocess, "Popen", return_value=MockProcess()),
            patch.object(server, "job_state", return_value={"status":"done"})
        ]
        for p in patches:
            p.start()
            self.addCleanup(p.stop)

    def test_analysis_does_not_receive_montage_options(self):
        result = server.start_job("analyze", {
            "project_id":"abc", "backend":"onnx","analysis_width":"auto",
            "threads":8,"beach":True
        })
        self.assertEqual(result, {"status":"done"})
        cmd=server.JOB["command"]
        self.assertIn("run_project_analysis.py",str(cmd))
        self.assertNotIn("--min-rally",cmd)
        self.assertNotIn("--min-confidence",cmd)
        self.assertIn("--beach",cmd)

    def test_analysis_onnx_cpu_explicit_backend_and_threads(self):
        server.start_job("analyze", {
            "project_id":"abc","backend":"onnx-cpu",
            "analysis_width":"auto","threads":10
        })
        cmd=server.JOB["command"]
        self.assertEqual(cmd[cmd.index("--backend")+1],"onnx-cpu")
        self.assertEqual(cmd[cmd.index("--threads")+1],"10")

    def test_benchmark_runs_upstream_synthetic_inference_only(self):
        result=server.start_job("benchmark",{
            "project_id":"abc",
            "model":"VballNetV4c_seq9_grayscale_20260908_213829.onnx",
            "backend":"onnx-cpu","threads":10,
            "warmup":10,"runs":60,"repeats":3
        })
        self.assertEqual(result,{"status":"done"})
        cmd=server.JOB["command"]
        self.assertIn("benchmark_project.py",str(cmd))
        self.assertEqual(cmd[cmd.index("--backend")+1],"onnx-cpu")
        self.assertEqual(cmd[cmd.index("--threads")+1],"10")
        self.assertEqual(cmd[cmd.index("--runs")+1],"60")
        self.assertNotIn("--mode",cmd)
        self.assertNotIn("--video",cmd)
        self.assertNotIn("--analysis-width",cmd)
        with self.assertRaises(ValueError):
            server.start_job("benchmark",{
                "project_id":"abc","model":"../private-model.onnx","backend":"onnx-cpu"
            })

    def test_montage_receives_filters_without_neural_pipeline(self):
        result = server.start_job("highlights", {
            "project_id":"abc","duration":120,"min_rally":4,
            "min_confidence":0.8
        })
        self.assertEqual(result, {"status":"done"})
        cmd = server.JOB["command"]
        self.assertIn("build_project_highlights.py",str(cmd))
        self.assertEqual(cmd[cmd.index("--min-rally")+1], "4.0")
        self.assertEqual(cmd[cmd.index("--min-confidence")+1], "0.8")
        self.assertNotIn("--backend",cmd)

    def test_invalid_montage_filters_are_rejected(self):
        for values in [
            {"min_rally":0}, {"min_rally":31}, {"min_confidence":0},
            {"min_confidence":1.1}, {"min_confidence":float("nan")}
        ]:
            with self.subTest(values=values):
                with self.assertRaises(ValueError):
                    server.start_job("highlights",{"project_id":"abc",**values})
                self.assertFalse(server.JOB["process"] and
                                 server.JOB["process"].poll() is None)


if __name__ == "__main__":
    unittest.main()
