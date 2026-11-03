"""Global custom analysis profiles preserve validated settings and reject malformed data."""
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui import data
from scripts.volleyball_ui import analysis_profiles as profiles


class AnalysisProfilesTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        original = patch.object(data, "RESULTS", Path(temp.name))
        original.start()
        self.addCleanup(original.stop)

    def test_round_trip_global_and_delete(self):
        entry = profiles.save_profile("Свои настройки", {
            "player_score_threshold": .41,
            "player_hysteresis_frames": 9,
            "backend": "openvino",
            "ball_threshold": .55,
            "beach": True,
            "player_output_video": False,
            "analysis_width": 960,
            "manual_width_enabled": True,
            "threads": 12,
        })
        self.assertEqual(len(profiles.all_profiles()), 1)
        self.assertEqual(profiles.all_profiles()[0]["values"]["threads"], 12)
        self.assertEqual(profiles.all_profiles()[0]["values"]["player_score_threshold"], .41)
        profiles.remove_profile(entry["id"])
        self.assertEqual(profiles.all_profiles(), [])

    def test_invalid_and_duplicate_profiles_are_rejected(self):
        profiles.save_profile("Точный", {"player_frame_step": 1})
        invalid = [
            ("ТОЧНЫЙ", {"player_frame_step": 2}),
            ("bad", {"backend": "bash"}),
            ("bad", {"threads": "NaN"}),
            ("bad", {"analysis_width": "../path"}),
            ("bad", {"track_min_duration": -1}),
            ("bad", {"reels_smooth_window": 14}),
            ("bad", {"unknown_path": "evil"}),
            ("bad", {"beach": "yes"}),
            ("", {"player_frame_step": 1}),
        ]
        for name, values in invalid:
            with self.subTest(name=name, values=values):
                with self.assertRaises((ValueError, TypeError)):
                    profiles.save_profile(name, values)
        with self.assertRaises(ValueError):
            profiles.remove_profile("baseline")


if __name__ == "__main__":
    unittest.main()
