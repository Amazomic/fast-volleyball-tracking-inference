from pathlib import Path
import sys
import unittest
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))

from volleyball_studio.__main__ import parser
from volleyball_studio.analysis import select
from volleyball_studio.request import parse_request, themed_candidates
from volleyball_studio.vfx import effect_state
from volleyball_studio.composition import protect_timing


class RequestTests(unittest.TestCase):
    def candidate(self, contacts, court=True):
        return {"id": "rally", "source": "match.mp4", "source_content": "same", "start": 0, "end": 1499, "duration": 50., "score": 10., "court_available": court, "contacts": contacts}

    def test_russian_request(self):
        result = parse_request("Сделай один ролик на тридцать секунд, только атаки и блоки, кинематографический, с музыкой и огнем")
        self.assertEqual(result["duration"], 30.)
        self.assertEqual(result["count"], 1)
        self.assertEqual(result["events"], ["attack", "block"])
        self.assertEqual((result["style"], result["effects"], result["music"]), ("cinematic", "cinematic", "yes"))

    def test_minimum_and_final_duration_are_distinct(self):
        result = parse_request("Длинные розыгрыши не короче 12 секунд, два ролика на 45 секунд без музыки")
        self.assertEqual((result["min_rally"], result["duration"], result["count"]), (12., 45., 2))
        self.assertTrue(result["long_rallies"])
        self.assertEqual(result["music"], "no")

    def test_sound_and_effects(self):
        result = parse_request("Лучшие подачи, 60-секундный ролик, без эффектов, только оригинальный звук")
        self.assertEqual(result["events"], ["serve"])
        self.assertEqual(result["duration"], 60)
        self.assertEqual((result["effects"], result["music"], result["accent_volume"]), ("off", "no", 0))

    def test_no_substitution_for_missing_attacks(self):
        contacts = [{"frame": 50, "contact_type": "dig", "confidence": .99}, {"frame": 100, "contact_type": "attack", "confidence": .74}]
        self.assertEqual(themed_candidates([self.candidate(contacts)], ["attack"], .75), [])

    def test_no_blocks_without_geometry(self):
        contact = {"frame": 50, "contact_type": "block", "confidence": .99}
        self.assertEqual(themed_candidates([self.candidate([contact], False)], ["block"], .75), [])

    def test_one_thirty_second_montage_from_many_attacks(self):
        contacts = [{"frame": 50+i*90, "contact_type": "attack", "confidence": .99} for i in range(15)]
        moments = themed_candidates([self.candidate(contacts)], ["attack"], .75)
        groups = select(moments, 30, 1)
        self.assertEqual(len(groups), 1)
        self.assertGreater(len(groups[0]), 1)
        self.assertAlmostEqual(sum(c["duration"] for c in groups[0]), 30.)

    def test_eight_cpus_default(self):
        self.assertEqual(parser().parse_args([]).threads, 8)

    def test_retiming_does_not_force_cut_contact_for_exact_duration(self):
        item = {"start": 0, "end": 59, "analysis_width": 1000, "ball": {i: {"x": 100+i, "y": 100, "confidence": .99} for i in range(60)}, "ball_rows": {}, "contacts": [{"frame": 40, "contact_type": "attack", "confidence": .99, "speed_after_px_frame": 30}]}
        args = SimpleNamespace(effects="cinematic", min_confidence=.75, speed_ramp=False)
        self.assertEqual(protect_timing([[item]], 2, args), [])

    def test_longer_effect_stops_when_ball_lost(self):
        item = {"analysis_width": 1000, "ball": {i: {"x": 100+i, "y": 100, "confidence": .99} for i in range(80)}, "ball_rows": {}, "contacts": [{"frame": 10, "contact_type": "attack", "confidence": .99, "speed_after_px_frame": 30}]}
        self.assertTrue(effect_state(item, 39, "cinematic", .75)["fire"])
        item["ball"].pop(39)
        self.assertFalse(effect_state(item, 39, "cinematic", .75)["fire"])


if __name__ == "__main__":
    unittest.main()
