"""Stdlib-only smoke tests for legacy command dispatch and setup diagnostics."""

import importlib.util
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]


def load_file(module_name: str, path: Path):
    spec = importlib.util.spec_from_file_location(module_name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class EntryPointTests(unittest.TestCase):
    def setUp(self):
        self.entry = load_file("volley_legacy_entry", ROOT / "main.py")

    def test_track_runs_real_script_and_forwards_flags(self):
        with tempfile.TemporaryDirectory() as folder:
            video, model = Path(folder) / "match.mp4", Path(folder) / "ball.onnx"
            video.touch()
            model.touch()
            with patch.object(self.entry.subprocess, "run", return_value=subprocess.CompletedProcess([], 0)) as run:
                code = self.entry.main(["--mode", "track", "--video_path", str(video),
                                        "--model_path", str(model), "--only-csv",
                                        "--device", "cpu"])
        self.assertEqual(code, 0)
        command = run.call_args.args[0]
        self.assertEqual(command[:2], [sys.executable, str(ROOT / "src/inference_onnx_seq_gray_v2.py")])
        self.assertIn("--only_csv", command)
        self.assertEqual(command[-2:], ["--device", "cpu"])
        self.assertEqual(run.call_args.kwargs["cwd"], ROOT)

    def test_track_rejects_missing_video_without_silent_success(self):
        with patch.object(self.entry.subprocess, "run") as run:
            with self.assertRaises(SystemExit) as error:
                self.entry.main(["--video_path", "/nonexistent/match.mp4"])
            self.assertEqual(error.exception.code, 2)
            run.assert_not_called()

    def test_analyze_runs_registered_project_pipeline(self):
        with patch.object(self.entry.subprocess, "run", return_value=subprocess.CompletedProcess([], 7)) as run:
            self.assertEqual(self.entry.main(["--mode", "analyze", "--project-id", "demo",
                                              "--backend", "onnx-cpu"]), 7)
        self.assertEqual(run.call_args.args[0], [sys.executable, "-m", "scripts.run_project_analysis",
                                                  "--project-id", "demo", "--backend", "onnx-cpu"])

    def test_analyze_requires_project(self):
        with self.assertRaises(SystemExit) as error:
            self.entry.main(["--mode", "analyze"])
        self.assertEqual(error.exception.code, 2)

    def test_pose_never_reports_success_from_placeholder(self):
        with patch.object(self.entry.subprocess, "run") as run:
            with self.assertRaises(SystemExit) as error:
                self.entry.main(["--mode", "pose"])
            self.assertEqual(error.exception.code, 2)
            run.assert_not_called()


class SetupDoctorTests(unittest.TestCase):
    def setUp(self):
        self.doctor = load_file("volley_setup_doctor", ROOT / "src/check_setup.py")

    def test_missing_current_requirements_are_reported(self):
        with tempfile.TemporaryDirectory() as folder:
            with patch.object(self.doctor.shutil, "which", return_value=None), \
                    patch.object(self.doctor, "find_spec", return_value=None):
                missing = self.doctor.missing_requirements(Path(folder))
        self.assertIn("ffmpeg", missing)
        self.assertIn("onnxruntime", missing)
        self.assertTrue(any("RAVEL-VB-013" in value for value in missing))
        self.assertFalse(any("models_yolo" in value for value in missing))

    def test_ready_installation_passes(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            for relative in self.doctor.REQUIRED_FILES:
                path = root / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.touch()
            with patch.object(self.doctor.shutil, "which", return_value="/usr/bin/ffmpeg"), \
                    patch.object(self.doctor, "find_spec", return_value=object()):
                self.assertEqual(self.doctor.missing_requirements(root), [])
                self.assertTrue(self.doctor.check_files(root))


if __name__ == "__main__":
    unittest.main()
