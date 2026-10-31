"""Tests for UI report and identity reconciliation (no GPU / model inference)."""
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui import data
from scripts.full_rallies import choose_whole_groups


class IdentityTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.project = self.root / "project"
        self.project.mkdir()
        self.results = self.root / "results"
        self.results.mkdir()
        self.name = "match.mp4"
        (self.project / self.name).write_bytes(b"fake")
        cache = self.results / "analysis" / "cache-123"
        (cache / "analysis").mkdir(parents=True)
        (cache / "analysis/events.json").write_text(json.dumps({
            "source": str(self.project / self.name), "court_available": False,
            "events": [
                {"type": "rally_start", "frame": 0},
                {"type": "attack", "frame": 10, "player_track_id": 12},
                {"type": "attack", "frame": 50, "player_track_id": 34},
                {"type": "dig", "frame": 60, "player_track_id": 34},
            ]
        }), encoding="utf-8")
        self.patches = [
            patch.object(data, "PROJECT", self.project),
            patch.object(data, "RESULTS", self.results),
            patch.object(data, "IDS_FILE", self.results / "player_identities.json")
        ]
        for p in self.patches:
            p.start()
            self.addCleanup(p.stop)

    def test_report_finds_cache_and_action_ids(self):
        r = data.report(self.name)
        self.assertTrue(r["analyzed"])
        self.assertEqual(r["rallies"], 1)
        self.assertEqual(r["track_count"], 2)
        self.assertEqual(r["counts"]["attack"], 2)

    def test_assign_and_merge_by_same_name(self):
        data.assign(self.name, ["12"], "Иван")
        data.assign(self.name, ["34"], "Иван")
        r = data.report(self.name)
        self.assertEqual(len(r["groups"]), 1)
        self.assertEqual(r["groups"][0]["track_ids"], ["12", "34"])
        self.assertEqual(r["groups"][0]["total_actions"], 3)
        data.unassign(self.name, ["12"])
        self.assertEqual(data.report(self.name)["groups"][0]["track_ids"], ["34"])

    def test_moving_ids_between_people(self):
        data.assign(self.name, ["12", "34"], "Игрок A")
        data.assign(self.name, ["12"], "Игрок B")
        groups = data.report(self.name)["groups"]
        self.assertEqual(len(groups), 2)
        self.assertEqual(sorted(sum((g["track_ids"] for g in groups), [])), ["12", "34"])

    def test_invalid_video_and_id(self):
        with self.assertRaises(ValueError):
            data.assign("../other.mp4", ["12"], "test")
        with self.assertRaises(ValueError):
            data.assign(self.name, ["hello"], "test")


class RallyTests(unittest.TestCase):
    def test_fully_preserves_boundaries(self):
        groups = [
            {"ranked": True, "ranking_score": 9, "duration_sec": 12.0,
             "source_start_frame": 100, "source_end_frame": 459},
            {"ranked": True, "ranking_score": 8, "duration_sec": 8.0,
             "source_start_frame": 1000, "source_end_frame": 1239},
            {"ranked": True, "ranking_score": 4, "duration_sec": 22.0,
             "source_start_frame": 2000, "source_end_frame": 2659},
        ]
        selected = choose_whole_groups(groups, target_sec=20, max_over_sec=0)
        self.assertEqual([(g["source_start_frame"], g["source_end_frame"]) for g in selected],
                         [(100, 459), (1000, 1239)])
        self.assertEqual(sum(g["duration_sec"] for g in selected), 20)


if __name__ == "__main__":
    unittest.main()
