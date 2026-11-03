"""Regression cases for player ID association across real-frame gaps and crossings."""
import unittest

from scripts.volleyball_studio.advanced import validated
from src.inference_player_ball_openvino import PlayerHysteresis


def player(label, x, score=.92):
    return {
        "class_name": "player", "identity": label, "score": score,
        "bbox_xyxy_norm": [x - .03, .36, x + .03, .68],
    }


class PlayerIdStabilityTests(unittest.TestCase):
    def tracker(self):
        config = validated({})
        return PlayerHysteresis(
            config["player_score_threshold"], config["player_close_threshold"],
            config["player_hysteresis_frames"],
            match_iou=config["player_match_iou"],
            max_center_distance=config["player_match_center_distance"],
        )

    def test_crossing_players_keep_ids_even_when_input_order_flips(self):
        tracker = self.tracker()
        identities = {}
        for frame in range(25):
            entries = [
                player("A", .24 + .021 * frame),
                player("B", .73 - .019 * frame),
            ]
            if frame % 2:
                entries.reverse()
            result = tracker.update(frame, entries)
            observed = {x["identity"]: x["track_id"] for x in result
                        if x["class_name"] == "player" and not x["interpolated"]}
            self.assertEqual(set(observed), {"A", "B"})
            for label, track_id in observed.items():
                identities.setdefault(label, track_id)
                self.assertEqual(track_id, identities[label], (frame, label))
        self.assertNotEqual(identities["A"], identities["B"])
        self.assertEqual(tracker.next_track_id, 2)

    def test_keeps_id_through_fifteen_frame_occlusion(self):
        tracker = self.tracker()
        before = None
        for frame in range(24):
            entries = [] if 5 <= frame <= 17 else [player("A", .20 + .008 * frame)]
            observed = [
                p for p in tracker.update(frame, entries)
                if p["class_name"] == "player" and not p.get("interpolated")
            ]
            if frame == 4:
                before = observed[0]["track_id"]
            if frame == 18:
                self.assertEqual(observed[0]["track_id"], before)
        self.assertEqual(tracker.next_track_id, 1)

    def test_genuine_disappearance_expires_track(self):
        tracker = self.tracker()
        old = next(p["track_id"] for p in tracker.update(0, [player("A", .25)])
                   if p["class_name"] == "player")
        for frame in range(1, 25):
            tracker.update(frame, [])
        new = next(p["track_id"] for p in tracker.update(25, [player("B", .25)])
                   if p["class_name"] == "player")
        self.assertNotEqual(old, new)

    def test_no_two_detected_players_share_one_id_in_same_frame(self):
        tracker = self.tracker()
        for frame in range(22):
            detections = [player("A", .16 + .02 * frame),
                          player("B", .72 - .02 * frame),
                          player("C", .85)]
            result = tracker.update(frame, detections)
            ids = [p["track_id"] for p in result
                   if p["class_name"] == "player" and not p.get("interpolated")]
            self.assertEqual(len(ids), len(set(ids)))


if __name__ == "__main__":
    unittest.main()
