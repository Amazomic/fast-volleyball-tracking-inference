import unittest

from src.inference_player_ball_openvino import PlayerHysteresis


def detection(x):
    return {"class_name": "player", "score": .9,
            "bbox_xyxy_norm": [x - .03, .36, x + .03, .68]}


class ThreeSecondGapTests(unittest.TestCase):
    def tracker(self):
        return PlayerHysteresis(.3, .05, 90, match_iou=.05, max_center_distance=.14)

    def observed_id(self, tracker, frame, x):
        return next(row["track_id"] for row in tracker.update(frame, [detection(x)])
                    if not row.get("interpolated"))

    def test_reacquires_at_three_second_boundary_at_30_fps(self):
        tracker = self.tracker()
        initial = self.observed_id(tracker, 0, .25)
        for frame in range(1, 90):
            tracker.update(frame, [])
        self.assertEqual(self.observed_id(tracker, 90, .25), initial)

    def test_expires_after_three_second_boundary(self):
        tracker = self.tracker()
        initial = self.observed_id(tracker, 0, .25)
        self.assertNotEqual(self.observed_id(tracker, 91, .25), initial)

    def test_long_gap_does_not_force_association_to_distant_player(self):
        tracker = self.tracker()
        for frame in range(10):
            initial = self.observed_id(tracker, frame, .20 + .004 * frame)
        self.assertNotEqual(self.observed_id(tracker, 99, .596), initial)

    def test_held_box_is_extrapolated_for_only_eight_frames(self):
        tracker = self.tracker()
        for frame in range(10):
            self.observed_id(tracker, frame, .20 + .004 * frame)
        near = tracker.update(17, [])[0]
        late = tracker.update(99, [])[0]
        self.assertEqual(near["bbox_xyxy_norm"], late["bbox_xyxy_norm"])
        self.assertTrue(late["interpolated"])
        self.assertLess(late["score"], .0001)


if __name__ == "__main__":
    unittest.main()
