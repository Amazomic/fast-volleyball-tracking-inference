"""RAVEL-013 default temporal peak selection and Track ID attribution."""
import unittest

from scripts.volleyball_studio.touch_peaks import (
    select_touch_peaks, associate_touch_players,
)


def row(frame, probability, player=7, bbox=None):
    return {
        "frame_index": frame, "class_name": "player",
        "action": "Touch", "action_score": probability, "track_id": player,
        "bbox_xyxy": bbox or [10, 10, 60, 100],
    }


class TouchPeakTests(unittest.TestCase):
    def test_five_frame_smoothing_and_ten_frame_gap(self):
        predictions = [row(t, 0.9) for t in range(10, 15)]
        predictions += [row(t, .8, 8) for t in range(14, 19)]
        predictions += [row(t, .95, 9) for t in range(40, 45)]
        found = select_touch_peaks(predictions, .5, smooth=5, min_gap=10)
        self.assertEqual(len(found), 2)
        self.assertGreaterEqual(found[0]["frame_index"], 10)
        self.assertLessEqual(found[0]["frame_index"], 18)
        self.assertEqual(found[1]["frame_index"], 42)
        self.assertAlmostEqual(found[1]["action_score"], .95, places=4)

    def test_isolated_high_frame_is_not_a_smoothed_contact(self):
        self.assertEqual(select_touch_peaks([row(15, .99)], .5), [])

    def test_raw_action_actor_can_be_mapped_to_stable_track_id(self):
        proposals = [row(frame, .9, None)
                     for frame in range(11, 16)]
        peaks = select_touch_peaks(proposals, .5)
        tracked = [dict(row(frame, .9, 28), action=None)
                   for frame in range(11, 16)]
        matched = associate_touch_players(peaks, tracked)
        self.assertEqual([item["track_id"] for item in matched], [28])

    def test_nonoverlapping_player_is_not_misattributed(self):
        proposals = [row(frame, .9, None)
                     for frame in range(11, 16)]
        peaks = select_touch_peaks(proposals, .5)
        unrelated = [row(frame, .9, 28, [700, 100, 750, 300])
                     for frame in range(11, 16)]
        matched = associate_touch_players(peaks, unrelated)
        self.assertIsNone(matched[0]["track_id"])


if __name__ == "__main__":
    unittest.main()
