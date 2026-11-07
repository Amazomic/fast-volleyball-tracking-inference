"""Parity/ABI gates for the first opt-in Rust CPU kernel."""
from __future__ import annotations

import os
from pathlib import Path
import random
import unittest
from unittest.mock import patch

from scripts.volleyball_studio import rust_core


class RustCoreContractTests(unittest.TestCase):
    BOX_PAIRS = (
        ((0., 0., 1., 1.), (0., 0., 1., 1.), 1.0),
        ((0., 0., 1., 1.), (2., 2., 3., 3.), 0.0),
        ((0., 0., 1., 1.), (1., 0., 2., 1.), 0.0),
        ((0., 0., 2., 2.), (1., 1., 3., 3.), 1 / 7),
        ((0., 0., 0., 2.), (0., 0., 2., 2.), 0.0),
        ((.125, .20, .30, .80), (.20, .25, .35, .75), None),
    )

    def test_python_oracle_and_explicit_default(self):
        with patch.dict(os.environ, {"VOLLEY_RUST_IOU": "0"}):
            for a, b, expected in self.BOX_PAIRS:
                with self.subTest(a=a, b=b):
                    result = rust_core.bbox_iou(a, b)
                    self.assertGreaterEqual(result, 0.0)
                    self.assertLessEqual(result, 1.0)
                    self.assertEqual(result, rust_core.python_bbox_iou(a, b))
                    if expected is not None:
                        self.assertAlmostEqual(result, expected)

    def test_opt_in_never_silently_falls_back(self):
        with patch.dict(os.environ, {
            "VOLLEY_RUST_IOU": "1",
            "VOLLEY_RUST_CORE_LIBRARY": str(Path(__file__).parent / "absent_volley_core.dll"),
        }):
            with self.assertRaisesRegex(FileNotFoundError, "Rust kernel not built"):
                rust_core.bbox_iou((0, 0, 1, 1), (0, 0, 1, 1))

    def test_compiled_rust_matches_python_oracle(self):
        path = rust_core.library_path()
        if not path.is_file():
            if os.environ.get("VOLLEY_RUST_REQUIRED") == "1":
                self.fail(f"Rust library required for this gate, missing: {path}")
            self.skipTest("Compile rust/volley-core first to run ABI parity")
        samples = [(a, b) for a, b, _expected in self.BOX_PAIRS]
        rng = random.Random(20261107)
        for _ in range(2500):
            a = tuple(rng.random() for _ in range(4))
            b = tuple(rng.random() for _ in range(4))
            # Both valid boxes and degenerate/reversed inputs exercise
            # the existing Python implementation's documented semantics.
            samples.append((a, b))
        with patch.dict(os.environ, {"VOLLEY_RUST_IOU": "1"}):
            for a, b in samples:
                with self.subTest(a=a, b=b):
                    self.assertAlmostEqual(
                        rust_core.bbox_iou(a, b),
                        rust_core.python_bbox_iou(a, b),
                        places=14,
                    )


if __name__ == "__main__":
    unittest.main()
