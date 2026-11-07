"""Differential S02a motion kernel tests: same RAVEL CPU interpretation."""
from __future__ import annotations

import os
import random
from pathlib import Path
import unittest
from unittest.mock import patch

from scripts.volleyball_studio.reprocess import _ball_motion_change, _ravel_contacts
from scripts.volleyball_studio.rust_core import library_path, rust_ball_motion_changes


def trajectory(kind: str) -> list[tuple[int, float, float]]:
    points = []
    for frame in range(31):
        d = frame - 15
        if kind == "straight":
            xy = (100 + frame * 5, 180)
        elif kind == "gravity":
            xy = (180, 300 - 15 * frame + frame * frame / 2)
        elif kind == "reversal":
            xy = (200 + (7 if d <= 0 else -6) * d, 180)
        elif kind == "speed_gain":
            xy = (200 + (2 if d <= 0 else 12) * d, 180)
        elif kind == "sparse":
            if frame % 8:
                continue
            xy = (200 + (7 if d <= 0 else -6) * d, 180)
        else:
            raise ValueError(kind)
        points.append((frame, float(xy[0]), float(xy[1])))
    return points


class MotionRustContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not library_path().is_file():
            if os.environ.get("VOLLEY_RUST_REQUIRED") == "1":
                raise AssertionError("Rust DLL is required for the motion parity gate")
            raise unittest.SkipTest("Compile Rust DLL to check S02a motion parity")

    def oracle(self, observations, frames):
        with patch.dict(os.environ, {"VOLLEY_RUST_MOTION": "0"}):
            return [_ball_motion_change(observations, frame) for frame in frames]

    def test_motion_equivalence_for_reversals_apex_and_gaps(self):
        for kind in ("straight", "gravity", "reversal", "speed_gain", "sparse"):
            samples = trajectory(kind)
            frames = list(range(5, 26))
            with self.subTest(kind=kind):
                expected = self.oracle(samples, frames)
                got = rust_ball_motion_changes(samples, frames)
                self.assertEqual(got, expected)
                with patch.dict(os.environ, {"VOLLEY_RUST_MOTION": "1"}):
                    self.assertEqual(
                        [_ball_motion_change(samples, frame) for frame in frames],
                        expected,
                    )

    def test_seeded_noisy_tracks_use_identical_motion_evidence(self):
        rng = random.Random(20261107)
        for scenario in range(90):
            # Mixture of gradual acceleration, gravity and abrupt kicks.
            base_x, base_y = rng.uniform(10, 500), rng.uniform(10, 500)
            dx, dy = rng.uniform(-10, 10), rng.uniform(-10, 10)
            kick = rng.randint(10, 35)
            samples = []
            for frame in range(45):
                if rng.random() < .07:
                    continue
                if frame == kick:
                    dx += rng.uniform(-16, 16)
                    dy += rng.uniform(-16, 16)
                base_x += dx
                base_y += dy + (frame - 20) * .02
                samples.append((frame, base_x, base_y))
            frames = list(range(7, 40, 3))
            with self.subTest(scenario=scenario):
                self.assertEqual(
                    rust_ball_motion_changes(samples, frames),
                    self.oracle(samples, frames),
                )

    def test_ravel_contacts_identical_for_python_and_rust(self):
        for kind in ("straight", "gravity", "reversal", "speed_gain"):
            observations = trajectory(kind)
            track = {
                "start_frame": 0,
                "last_frame": 30,
                "frame_observations": [
                    {"frame": frame, "x": x, "y": y}
                    for frame, x, y in observations
                ],
            }
            x, y = observations[15][1:]
            proposal = {
                "class_name": "player", "action": "Touch",
                "frame_index": 15, "action_score": .99,
                "bbox_xyxy": [x - 30, y - 65, x + 30, y + 65],
                "track_id": 5,
            }
            with patch.dict(os.environ, {"VOLLEY_RUST_MOTION": "0"}):
                expected = _ravel_contacts(track, [], .45, selected_peaks=[proposal])
            with patch.dict(os.environ, {"VOLLEY_RUST_MOTION": "1"}):
                result = _ravel_contacts(track, [], .45, selected_peaks=[proposal])
            with self.subTest(kind=kind):
                self.assertEqual(result, expected)

    def test_invalid_batch_does_not_silently_fall_back(self):
        with self.assertRaisesRegex(ValueError, "sorted"):
            rust_ball_motion_changes([(5, 50., 50.), (1, 10., 10.)], [3])
        with patch.dict(os.environ, {
            "VOLLEY_RUST_CORE_LIBRARY": str(Path(__file__).parent / "missing_motion.dll"),
        }):
            with self.assertRaisesRegex(FileNotFoundError, "not built"):
                rust_ball_motion_changes(trajectory("reversal"), [15])


if __name__ == "__main__":
    unittest.main()
