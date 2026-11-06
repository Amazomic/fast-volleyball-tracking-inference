"""Default CPU reprocessing must use temporal Touch peaks for RAVEL-013."""
from __future__ import annotations

import json
import tempfile
import unittest
from contextlib import nullcontext
from pathlib import Path
from unittest.mock import MagicMock, patch

from scripts.run_project_reprocess import process_project
from scripts.volleyball_ui import server


class CpuTouchPeakDefaultTests(unittest.TestCase):
    def api_command(self, settings):
        with tempfile.TemporaryDirectory() as folder:
            proc = MagicMock()
            proc.poll.return_value = None
            job = {"process": None, "id": None, "kind": None,
                   "log": None, "started_at": None, "command": []}
            with (patch.object(server, "JOB", job),
                  patch.object(server, "project_videos",
                               return_value=[Path("match.mp4")]),
                  patch.object(server, "project_logs_root",
                               return_value=Path(folder)),
                  patch("scripts.volleyball_ui.project_lifecycle.job_launch_guard",
                        return_value=nullcontext()),
                  patch.object(server.subprocess, "Popen",
                               return_value=proc) as launched):
                server.start_job("reprocess", {"project_id": "project",
                                               "run_id": "run_example",
                                               **settings})
            return launched.call_args.args[0]

    def test_api_unspecified_mode_uses_model_aware_default(self):
        command = self.api_command({})
        self.assertNotIn("--mode", command)
        self.assertIn("--touch-threshold", command)

    def test_api_explicit_geometry_mode_is_preserved(self):
        command = self.api_command({"mode": "baseline"})
        self.assertEqual(command[command.index("--mode") + 1], "baseline")

    def test_project_runner_uses_effective_mode_from_cpu_manifest(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory) / ("a" * 24)
            output = base / "postprocessing" / ("b" * 24)
            (output / "analysis").mkdir(parents=True)
            (output / "ranking.json").write_text("[]", encoding="utf-8")
            (output / "analysis/events.json").write_text("{}", encoding="utf-8")
            (output / "postprocess.manifest.json").write_text(json.dumps({
                "schema": "volley-postprocessing/1",
                "version_id": output.name,
                "settings": {"mode": "combined", "touch_threshold": .5},
            }), encoding="utf-8")
            run = {"id": "run_example", "videos": {"match.mp4": base.name}}
            with (patch("scripts.run_project_reprocess.list_runs",
                        return_value=[run]),
                  patch("scripts.run_project_reprocess.cache_for",
                        return_value=base),
                  patch("scripts.run_project_reprocess.reprocess",
                        return_value=output) as compute,
                  patch("scripts.volleyball_ui.analysis_runs.restore_postprocessing_version"),
                  patch("scripts.volleyball_ui.analysis_runs.publish_postprocessing_snapshot",
                        return_value={"id": "run_example_pp_snapshot"})):
                result = process_project("project", run["id"])
            self.assertIsNone(compute.call_args.kwargs["mode"])
            self.assertEqual(result["mode"], "combined")


if __name__ == "__main__":
    unittest.main()

