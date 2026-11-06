"""All-track similarity, threshold control, and project-roster recommendations."""
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import numpy as np

from scripts.volleyball_ui import player_identity_groups as g
from scripts.volleyball_ui import player_reference_matching as refs


def track(start, end, count=20):
    return {"first_frame": start, "last_frame": end, "detected_frames": count,
            "fps": 30, "start_center": [.3, .5], "end_center": [.3, .5]}


class ReIdRecommendationTests(unittest.TestCase):
    def test_close_candidates_still_have_a_unique_best_match(self):
        roster = [{"id": "p1", "name": "Первый"}, {"id": "p2", "name": "Второй"}]
        album = {"p1": [(np.array([.96, .28]), ("old", "v", "c", "1"))],
                 "p2": [(np.array([.955, .2966]), ("old", "v", "c", "2"))]}
        with patch.object(refs.data, "players_for", return_value=roster), \
             patch.object(refs.data, "groups_for", return_value=[]), \
             patch.object(refs, "_album_features", side_effect=lambda pid, *a: album[pid]):
            matches, _ = refs.match_players({"3": np.array([1., 0.])},
                "project", "video", "cache", "openvino_reid", lambda: None)
        self.assertEqual(matches["3"]["player_id"], "p1")
        self.assertFalse(matches["3"]["ambiguous"])

    def test_all_tracks_compared_including_short_and_unassigned(self):
        vec = {"1": np.array([1., 0.]), "2": np.array([.90, .436]),
               "3": np.array([0., 1.]), "4": np.array([.1, .995])}
        tracks = {"1": track(0, 15), "2": track(20, 22, count=2),
                  "3": track(27, 52), "4": track(58, 78), "5": track(90, 100)}
        result = g.build_groups(vec, tracks, threshold_percent=99)
        self.assertEqual(result["embedded_count"], 4)
        self.assertEqual(result["total_tracks"], 5)
        self.assertEqual(len(result["nearest_tracks"]), 5)
        self.assertEqual(result["nearest_tracks"]["1"][0]["track_id"], "2")
        self.assertEqual(result["nearest_tracks"]["2"][0]["track_id"], "1")
        self.assertEqual(result["nearest_tracks"]["5"], [])
        self.assertIn("5", result["unmatched_ids"])

    def test_threshold_changes_groups_not_embeddings(self):
        vectors = {"1": np.array([1., 0.]), "2": np.array([.90, .43589])}
        tracks = {"1": track(0, 10), "2": track(13, 30)}
        loose = g.build_groups(vectors, tracks, threshold_percent=85)
        strict = g.build_groups(vectors, tracks, threshold_percent=96)
        self.assertEqual(loose["groups"][0]["track_ids"], ["1", "2"])
        self.assertEqual(strict["groups"], [])
        self.assertEqual(loose["nearest_tracks"]["1"], strict["nearest_tracks"]["1"])

    def test_manual_groups_survive_threshold_rebuild(self):
        raw = g.build_groups({"1": np.array([1., 0.]), "2": np.array([0., 1.]),
                              "3": np.array([.7, .7])},
                             {"1": track(0, 10), "2": track(11, 20), "3": track(21, 30)},
                             threshold_percent=99)
        old = {"groups": [{"id": "manual-kept", "track_ids": ["1", "2"],
                           "source": "manual", "similarity": None}],
               "manual_excluded_ids": ["3"]}
        computed = g.preserve_manual_groups(raw, old)
        self.assertEqual(computed["groups"][0]["track_ids"], ["1", "2"])
        self.assertEqual(computed["unmatched_ids"], ["3"])

    def test_only_selected_project_players_can_be_guessed(self):
        features = {
            "1": np.array([1.0, 0.0], dtype=np.float32),
            "2": np.array([.995, .100], dtype=np.float32),
            "3": np.array([.02, .999], dtype=np.float32),
        }
        roster = [{"id": "p1", "name": "Алекс"}, {"id": "p2", "name": "Валера"}]
        album = {
            "p1": [], "p2": [(np.array([0., 1.], np.float32),
                             ("old-project", "old.mp4", "cache", "8"))],
        }
        with patch.object(refs.data, "players_for", return_value=roster), \
             patch.object(refs.data, "groups_for", return_value=[
                 {"id": "p1", "track_ids": ["1"]}]), \
             patch.object(refs, "_album_features", side_effect=lambda pid, *a: album[pid]):
            result, counts = refs.match_players(features, "project", "video", "inference",
                                                "openvino_reid", lambda: None,
                                                threshold_percent=85)
        self.assertTrue(result["1"]["confirmed"])
        self.assertEqual(result["1"]["player_id"], "p1")
        self.assertEqual(result["2"]["player_id"], "p1")
        self.assertTrue(result["2"]["above_threshold"])
        self.assertEqual(result["3"]["player_id"], "p2")
        self.assertEqual(counts, {"p1": 1, "p2": 1})
        self.assertNotIn("non-roster-id", [x["player_id"] for x in result.values()])

    def test_no_album_or_confirmed_id_no_guess(self):
        with patch.object(refs.data, "players_for", return_value=[{"id": "p1", "name": "Алекс"}]), \
             patch.object(refs.data, "groups_for", return_value=[]), \
             patch.object(refs, "_album_features", return_value=[]):
            result, counts = refs.match_players({"100": np.array([1., 0.])},
                                                "project", "video", "cache", "openvino_reid",
                                                lambda: None)
        self.assertEqual(result, {})
        self.assertEqual(counts, {"p1": 0})

    def test_threshold_is_saved_and_reprocessing_preserves_manual_edits(self):
        with tempfile.TemporaryDirectory() as folder:
            base = Path(folder)
            source = base / "players.json"
            source.write_text("{}", encoding="utf-8")
            target = base / "group.json"
            g.data.save_json(target, {
                "schema": g.VERSION, "stamp": g._stamp(source),
                "similarity_schema": g.SIMILARITY_SCHEMA,
                "groups": [{"id": "manual", "track_ids": ["1", "2"], "source": "manual"}],
                "unmatched_ids": ["3"], "threshold_percent": 85,
            })
            with patch.object(g, "_context", return_value=(base, source, target)), \
                 patch.object(g, "_start") as start, \
                 patch.object(g, "overview", return_value={"status": "running", "threshold_percent": 91}):
                outcome = g.set_threshold("project", "video", "cache", 91)
                self.assertEqual(outcome["threshold_percent"], 91)
                self.assertEqual(g.data.read_json(target)["threshold_percent"], 91)
                self.assertEqual(g.data.read_json(target)["groups"][0]["source"], "manual")
                self.assertTrue(start.call_args.kwargs["preserve"])

    def test_threshold_validation(self):
        for bad in (-1, 100, 54, "85.2", None, True):
            with self.assertRaises(ValueError):
                g.validate_threshold(bad)
        self.assertEqual(g.validate_threshold("87"), 87)


if __name__ == "__main__":
    unittest.main()
