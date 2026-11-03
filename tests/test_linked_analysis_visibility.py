"""Imported symlink aliases must keep their completed project analysis visible."""
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui import analysis_runs, data, local_sources, montage_runner, projects


class LinkedAnalysisVisibilityTests(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = Path(tmp.name)
        self.original_a = root / "a" / "match.mp4"
        self.original_b = root / "b" / "match.mp4"
        for original in (self.original_a, self.original_b):
            original.parent.mkdir()
            original.write_bytes(b"source video")
        project_files = root / "project"
        project_files.mkdir()
        results = root / "results"
        results.mkdir()
        for obj, attr, val in (
            (data, "PROJECT", project_files),
            (data, "RESULTS", results),
            (projects, "PROJECTS_DIR", results / "projects"),
            (projects, "REGISTER", results / "projects" / "projects.json"),
        ):
            patcher = patch.object(obj, attr, val)
            patcher.start()
            self.addCleanup(patcher.stop)
        info = patch("scripts.volleyball_ui.preview.video_info",
                     return_value={"duration_sec": 40})
        info.start()
        self.addCleanup(info.stop)
        linked = local_sources.register_paths(
            [str(self.original_a), str(self.original_b)])
        self.first, self.alias = (entry["name"] for entry in linked)
        self.assertEqual([self.first, self.alias],
                         ["match.mp4", "match (2).mp4"])
        self.project = projects.create_project("Linked", [self.first, self.alias])
        self.pid = self.project["id"]
        self.history = projects.project_run_root(self.pid) / "analysis_timings"
        self.history.mkdir(parents=True)
        self.cache = {}
        for index, (name, original) in enumerate((
            (self.first, self.original_a), (self.alias, self.original_b))):
            cache = projects.project_cache_root(self.pid) / (str(index + 1) * 16)
            (cache / "analysis").mkdir(parents=True)
            (cache / "analysis" / "events.json").write_text(
                json.dumps({"source": str(original.resolve()), "events": [
                    {"type": "rally_start", "frame": 4, "player_track_id": index + 1}
                ]}), encoding="utf-8")
            self.cache[name] = cache
        (self.history / "20261102_123456.json").write_text(json.dumps({
            "status": "complete", "profile_name": "Точность",
            "started_at": "2026-11-02T12:34:56+00:00",
            "videos": [{"video": name, "cache_id": path.name, "status": "complete"}
                       for name, path in self.cache.items()],
        }), encoding="utf-8")

    def test_imported_alias_cache_is_visible_in_report_and_montage(self):
        self.assertEqual(data.find_cache(self.alias, self.pid), self.cache[self.alias])
        runs = analysis_runs.list_runs(self.pid)
        self.assertEqual(len(runs), 1)
        self.assertEqual(runs[0]["videos"][self.alias], self.cache[self.alias].name)
        project_summary = projects.project_summary(self.pid)
        self.assertTrue(all(row["analyzed"] for row in project_summary["analyses"]))
        result = projects.project_report(self.pid, runs[0]["id"])
        self.assertEqual(result["analyzed_videos"], 2)
        self.assertEqual(result["rallies"], 2)
        payload = {"project_id": self.pid, "run_id": runs[0]["id"],
                   "mode": "montage", "duration": 30}
        sources = montage_runner.sources_for(montage_runner.validate_request(payload))
        self.assertEqual([row["cache"] for row in sources],
                         [self.cache[self.first], self.cache[self.alias]])

    def test_same_basename_does_not_cross_sources(self):
        self.assertNotEqual(self.original_a.resolve(), self.original_b.resolve())
        self.assertNotEqual(data.find_cache(self.first, self.pid),
                            data.find_cache(self.alias, self.pid))
        with self.assertRaisesRegex(ValueError, "не принадлежит"):
            analysis_runs.cache_for(self.first, self.pid, self.cache[self.alias].name)
        with self.assertRaisesRegex(ValueError, "не принадлежит"):
            analysis_runs.cache_for(self.alias, self.pid, self.cache[self.first].name)
        (self.original_b).unlink()
        self.assertIsNone(data.find_cache(self.alias, self.pid))
        self.assertEqual(analysis_runs.list_runs(self.pid), [])


if __name__ == "__main__":
    unittest.main()
