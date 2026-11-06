"""Postprocessing branches must not silently alter stored analysis/montage runs."""
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui import analysis_runs


class PostprocessingSelectionTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.cache_root = self.root / "caches"
        self.run_root = self.root / "runs"
        self.cache_root.mkdir()
        self.history = self.run_root / "analysis_timings"
        self.history.mkdir(parents=True)
        self.project = {"id": "test-project", "videos": ["match.mp4"]}
        self.base_id = "a" * 24
        self.version_id = "b" * 24
        self.base = self.cache_root / self.base_id
        self.base.mkdir()
        self.write_json(self.base / "analysis/events.json", {
            "source": "match.mp4", "events": []})
        self.write_json(self.history / "20261104_000000.json", {
            "project_id": "test-project", "status": "complete",
            "profile_name": "New", "profile_id": "profile-13",
            "started_at": "2026-11-04T00:00:00+00:00",
            "videos": [{"video": "match.mp4", "status": "complete",
                        "cache_id": self.base_id}],
        })
        self.patches = [
            patch.object(analysis_runs, "get_project", return_value=self.project),
            patch.object(analysis_runs, "project_cache_root",
                         return_value=self.cache_root),
            patch.object(analysis_runs, "project_run_root",
                         return_value=self.run_root),
            patch.object(analysis_runs, "assert_project_video"),
            patch.object(analysis_runs.data, "source_matches_video",
                         return_value=True),
        ]
        for item in self.patches:
            item.start()
            self.addCleanup(item.stop)

    @staticmethod
    def write_json(path, data):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(data), encoding="utf-8")

    def add_replay(self):
        folder = self.base / "postprocessing" / self.version_id
        self.write_json(folder / "analysis/events.json", {
            "source": "match.mp4", "events": []})
        self.write_json(folder / "ranking.json", [])
        self.write_json(folder / "postprocess.manifest.json", {
            "schema": "volley-postprocessing/1",
            "version_id": self.version_id,
            "inference_cache": str(self.base),
            "settings": {"mode": "ravel-touch", "touch_threshold": .5},
        })
        return folder

    def test_original_run_is_unchanged_and_replay_gets_its_own_id(self):
        self.assertEqual(len(analysis_runs.list_runs("test-project")), 1)
        replay = self.add_replay()
        runs = analysis_runs.list_runs("test-project")
        self.assertEqual(len(runs), 2)
        original = next(run for run in runs if "parent_run_id" not in run)
        variant = next(run for run in runs if "parent_run_id" in run)
        self.assertEqual(original["videos"]["match.mp4"], self.base_id)
        self.assertEqual(variant["parent_run_id"], original["id"])
        self.assertEqual(variant["videos"]["match.mp4"],
                         self.base_id + "." + self.version_id)
        self.assertEqual(analysis_runs.cache_for(
            "match.mp4", "test-project", variant["videos"]["match.mp4"]), replay)
        with self.assertRaises(ValueError):
            analysis_runs.cache_for("match.mp4", "test-project", "../unsafe")

    def test_project_runner_uses_selected_parent_and_all_videos(self):
        from scripts.run_project_reprocess import process_project
        base_run = analysis_runs.list_runs("test-project")[0]
        output = self.base / "postprocessing" / self.version_id

        def completed_replay(*args, **kwargs):
            # The real CPU worker atomically publishes a complete manifest.
            # Mocking it must preserve this publication contract.
            self.write_json(output / "analysis/events.json", {
                "source": "match.mp4", "events": []})
            self.write_json(output / "ranking.json", [])
            self.write_json(output / "postprocess.manifest.json", {
                "schema": "volley-postprocessing/1",
                "version_id": self.version_id,
                "inference_cache": str(self.base),
                "settings": {"mode": kwargs["mode"],
                             "touch_threshold": kwargs["touch_threshold"]},
            })
            return output

        with (patch("scripts.run_project_reprocess.list_runs", return_value=[base_run]),
              patch("scripts.run_project_reprocess.cache_for", return_value=self.base),
              patch("scripts.run_project_reprocess.reprocess",
                    side_effect=completed_replay) as replay):
            answer = process_project("test-project", base_run["id"], "combined")
        self.assertEqual(replay.call_count, 1)
        self.assertEqual(answer["videos"]["match.mp4"],
                         self.base_id + "." + self.version_id)
        self.assertEqual(answer["status"], "complete")

    def test_incomplete_cpu_replay_is_not_published(self):
        from scripts.run_project_reprocess import process_project
        base_run = analysis_runs.list_runs("test-project")[0]
        output = self.base / "postprocessing" / self.version_id
        with (patch("scripts.run_project_reprocess.list_runs", return_value=[base_run]),
              patch("scripts.run_project_reprocess.cache_for", return_value=self.base),
              patch("scripts.run_project_reprocess.reprocess", return_value=output),
              patch("scripts.volleyball_ui.analysis_runs.restore_postprocessing_version")
              as restore):
            with self.assertRaisesRegex(ValueError, "incomplete"):
                process_project("test-project", base_run["id"], "combined")
            restore.assert_not_called()

    def test_video_range_is_snapshot_of_original_inference(self):
        # A later change to the project's current editing range must not
        # rewrite the segment described by an old analysis branch.
        history_file = self.history / "20261104_000000.json"
        row = json.loads(history_file.read_text(encoding="utf-8"))
        row["videos"][0]["range_start"] = 123.0
        row["videos"][0]["range_end"] = 185.0
        self.write_json(history_file, row)
        original = analysis_runs.list_runs("test-project")[0]
        self.assertEqual(original["video_ranges"]["match.mp4"],
                         {"start": 123.0, "end": 185.0})
        self.add_replay()
        replay = next(run for run in analysis_runs.list_runs("test-project")
                      if run.get("parent_run_id"))
        self.assertEqual(replay["video_ranges"]["match.mp4"],
                         {"start": 123.0, "end": 185.0})

    def test_incomplete_version_is_not_selectable(self):
        replay = self.add_replay()
        (replay / "ranking.json").unlink()
        runs = analysis_runs.list_runs("test-project")
        self.assertEqual(len(runs), 1)
        with self.assertRaises(ValueError):
            analysis_runs.cache_for(
                "match.mp4", "test-project", self.base_id + "." + self.version_id)


if __name__ == "__main__":
    unittest.main()
