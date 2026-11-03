"""Analysis selection uses immutable per-project caches, not the latest match."""
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui import data, projects
from scripts.volleyball_ui.analysis_runs import cache_for, list_runs


class VersionedAnalysisTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        root = Path(temp.name)
        videos = root / "source"
        videos.mkdir()
        (videos / "match.mp4").write_bytes(b"video")
        results = root / "results"
        results.mkdir()
        for obj, name, value in (
            (data, "PROJECT", videos),
            (data, "RESULTS", results),
            (projects, "PROJECTS_DIR", results / "projects"),
            (projects, "REGISTER", results / "projects" / "projects.json"),
        ):
            pin = patch.object(obj, name, value)
            pin.start()
            self.addCleanup(pin.stop)
        info = patch("scripts.volleyball_ui.preview.video_info",
                     return_value={"duration_sec": 300})
        info.start()
        self.addCleanup(info.stop)
        self.pid = projects.create_project("Match", ["match.mp4"])["id"]
        self.caches = []
        for n, score in enumerate((2, 5)):
            cache = projects.project_cache_root(self.pid) / (str(n + 10) * 16)
            (cache / "analysis").mkdir(parents=True)
            (cache / "analysis" / "events.json").write_text(json.dumps({
                "source": str(videos / "match.mp4"),
                "events": [{"type": "attack", "frame": 4, "player_track_id": n + 1}
                           for _ in range(score)],
            }), encoding="utf-8")
            self.caches.append(cache)
        self.history = projects.project_run_root(self.pid) / "analysis_timings"
        self.history.mkdir(parents=True)

    def test_saved_runs_group_by_profile_and_retain_cache(self):
        for i, cache in enumerate(self.caches):
            (self.history / f"20261101_00000{i}.json").write_text(json.dumps({
                "status": "complete", "profile_name": "Точный" if i else "Базовый",
                "started_at": f"2026-11-01T12:00:0{i}+00:00",
                "videos": [{"video": "match.mp4", "cache_id": cache.name,"status":"complete"}],
            }), encoding="utf-8")
        runs = list_runs(self.pid)
        self.assertEqual(len(runs), 2)
        self.assertEqual([r["profile"] for r in runs], ["Точный", "Базовый"])
        self.assertEqual(runs[0]["videos"]["match.mp4"], self.caches[1].name)
        self.assertEqual(data.report("match.mp4", self.pid, self.caches[0].name)["counts"]["attack"], 2)
        self.assertEqual(data.report("match.mp4", self.pid, self.caches[1].name)["counts"]["attack"], 5)
        self.assertEqual(projects.project_report(self.pid, runs[0]["id"])["counts"]["attack"], 5)
        self.assertEqual(projects.project_report(self.pid, runs[1]["id"])["counts"]["attack"], 2)

    def test_cache_selection_cannot_escape_project_or_video(self):
        self.assertEqual(cache_for("match.mp4", self.pid, self.caches[0].name), self.caches[0])
        with self.assertRaises(ValueError):
            cache_for("match.mp4", self.pid, "../other")
        with self.assertRaises(ValueError):
            cache_for("match.mp4", self.pid, "0" * 16)
        with self.assertRaises(ValueError):
            cache_for("outside.mp4", self.pid, self.caches[0].name)

    def test_legacy_caches_not_offered_as_named_profiles(self):
        runs = list_runs(self.pid)
        self.assertEqual(runs, [])
        self.assertEqual(len(self.caches), 2)
        self.assertTrue(all(cache.exists() for cache in self.caches))


if __name__ == "__main__":
    unittest.main()
