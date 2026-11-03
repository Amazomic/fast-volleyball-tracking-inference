from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))

from volleyball_studio.analysis import score_track, select
from volleyball_studio.common import cached, discover
from volleyball_studio.render import camera_targets, effect_state


class StudioTests(unittest.TestCase):
    def test_cache_rejects_corruption_and_changed_signature(self):
        with tempfile.TemporaryDirectory() as temp:
            folder = Path(temp)
            output = folder / "data"
            calls = []

            def action():
                calls.append(1)
                output.write_text("valid")
                return [output]

            cached(folder, "test", "a", action)
            cached(folder, "test", "a", action)
            self.assertEqual(len(calls), 1)
            output.write_text("bad")
            cached(folder, "test", "a", action)
            cached(folder, "test", "b", action)
            self.assertEqual(len(calls), 3)

    def test_discovery_does_not_reingest_old_renders(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / "match.MP4").touch()
            (root / "music.mp3").touch()
            (root / "result").mkdir()
            (root / "result/old.mp4").touch()
            videos, audios = discover(root)
            self.assertEqual(len(videos), 1)
            self.assertEqual(len(audios), 1)

    def test_effects_require_ball_and_contact_evidence(self):
        item = {"analysis_width": 1000, "ball": {9: {"x": 100, "y": 100, "confidence": .9}, 10: {"x": 120, "y": 100, "confidence": .9}}, "ball_rows": {}, "contacts": [{"frame": 10, "contact_type": "attack", "confidence": .9, "speed_after_px_frame": 25}]}
        self.assertTrue(effect_state(item, 10, "cinematic", .65)["fire"])
        self.assertTrue(effect_state(item, 10, "cinematic", .65)["slow"])
        item["ball"][10]["confidence"] = .2
        self.assertFalse(any(effect_state(item, 10, "cinematic", .65).values()))
        item["ball"][10]["confidence"] = .9
        item["ball_rows"][10] = (800, 100)
        self.assertFalse(any(effect_state(item, 10, "cinematic", .65).values()))
        self.assertFalse(any(effect_state(item, 10, "off", .65).values()))

    def test_camera_bounded_speed_and_crop(self):
        item = {"start": 0, "end": 99, "analysis_width": 1280, "analysis_height": 720, "track": {"positions": [[[100 if f < 30 else 1200, 300], f] for f in range(100)]}, "players": {}}
        values = camera_targets(item)
        self.assertTrue(all(202 <= x <= 1078 for x in values))
        self.assertLessEqual(max(abs(a-b) for a, b in zip(values, values[1:])), 1280/120 + .001)

    def test_long_static_track_is_not_highlight(self):
        track = {"start_frame": 0, "last_frame": 599, "positions": [[[100, 100], f] for f in range(600)], "rally_classification": {"rally_confidence": .99}}
        balls = {f: {"confidence": .99} for f in range(600)}
        self.assertIsNone(score_track(track, balls, 1000, 4, .65, False))

    def test_selection_has_no_overlap_and_respects_budget(self):
        items = [{"source": "a", "start": a, "end": b, "duration": (b-a+1)/30, "score": score, "contacts": []} for a, b, score in [(0, 299, 10), (50, 349, 9), (400, 699, 8), (800, 1099, 7)]]
        groups = select(items, 12, 2)
        flattened = [x for g in groups for x in g]
        self.assertEqual(len(groups), 2)
        self.assertTrue(all(sum(x["duration"] for x in g) <= 12 for g in groups))
        self.assertFalse(any(x["start"] == 50 for x in flattened))


if __name__ == "__main__":
    unittest.main()
