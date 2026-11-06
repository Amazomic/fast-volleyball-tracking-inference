"""RAVEL Touch proposals must not become contacts without ball-flight evidence."""
import unittest

from scripts.volleyball_studio.reprocess import (
    _ball_motion_change, _ravel_contacts, _merge_contacts,
)


class TouchMotionTests(unittest.TestCase):
    @staticmethod
    def track(x, y=lambda _: 180.0):
        observations = [
            {"frame": f, "x": float(x(f)), "y": float(y(f))}
            for f in range(31)
        ]
        return {
            "start_frame": 0, "last_frame": 30,
            "frame_observations": observations,
        }

    @staticmethod
    def peak(track, frame=15, in_reach=True):
        point = track["frame_observations"][frame]
        x, y = point["x"], point["y"]
        if not in_reach:
            x += 200
        return {
            "class_name": "player", "action": "Touch",
            "frame_index": frame, "action_score": 0.99,
            "bbox_xyxy": [x - 30, y - 65, x + 30, y + 65],
            "track_id": 5,
        }

    def contacts(self, track, *, frame=15, in_reach=True):
        peak = self.peak(track, frame=frame, in_reach=in_reach)
        return _ravel_contacts(track, [], 0.45, selected_peaks=[peak])

    def test_straight_flight_through_player_is_not_touch(self):
        track = self.track(lambda f: 100 + 5 * f)
        self.assertEqual(self.contacts(track), [])

    def test_gravity_apex_is_not_touch(self):
        track = self.track(lambda f: 180, lambda f: 300 - 15 * f + f * f / 2)
        self.assertEqual(self.contacts(track), [])

    def test_reversed_heading_is_a_touch(self):
        track = self.track(lambda f: 200 + 7 * (f - 15) if f <= 15
                           else 200 - 6 * (f - 15))
        result = self.contacts(track)
        self.assertEqual(len(result), 1)
        self.assertEqual(result[0]["player_track_id"], 5)
        self.assertGreater(result[0]["direction_change_deg"], 120)
        self.assertGreater(result[0]["velocity_residual_px_frame"], 6)
        self.assertGreater(result[0]["speed_before_px_frame"], 0)
        self.assertEqual(result[0]["origin"], "ravel_touch")

    def test_rapid_speed_increase_is_a_touch(self):
        track = self.track(lambda f: 200 + 2 * (f - 15) if f <= 15
                           else 200 + 12 * (f - 15))
        result = self.contacts(track)
        self.assertEqual(len(result), 1)
        self.assertGreater(result[0]["speed_after_px_frame"],
                           result[0]["speed_before_px_frame"] * 1.5)
        self.assertLess(result[0]["direction_change_deg"], 10)

    def test_motion_without_nearby_player_is_not_touch(self):
        track = self.track(lambda f: 200 + 7 * (f - 15) if f <= 15
                           else 200 - 6 * (f - 15))
        self.assertEqual(self.contacts(track, in_reach=False), [])

    def test_missing_trajectory_samples_does_not_confirm_touch(self):
        self.assertIsNone(_ball_motion_change(
            [(14, 200., 180.), (15, 200., 180.)], 15))

    def test_model_peak_may_be_two_frames_ahead_of_motion(self):
        track = self.track(lambda f: 200 + 7 * (f - 15) if f <= 15
                           else 200 - 6 * (f - 15))
        result = self.contacts(track, frame=17)
        self.assertEqual(len(result), 1)
        self.assertLessEqual(abs(result[0]["motion_frame"] - 15), 3)


if __name__ == "__main__":
    unittest.main()
