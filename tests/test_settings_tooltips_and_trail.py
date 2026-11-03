"""Regression: tooltip coverage and real SVG visibility control."""
from pathlib import Path
import unittest

from scripts.volleyball_studio.advanced import SPECS

ROOT = Path(__file__).resolve().parents[1]
STATIC = ROOT / "scripts" / "volleyball_ui" / "static"


class TooltipsAndTrajectoryTests(unittest.TestCase):
    def test_every_advanced_setting_has_a_specific_explanation(self):
        js = (STATIC / "settings-tooltips.js").read_text(encoding="utf-8")
        for field in SPECS:
            with self.subTest(field=field):
                self.assertIn('    ' + field + ':"', js)
        for field in (
            "ball_model", "ball_threshold", "backend", "analysis_width",
            "threads", "beach", "duration", "min_rally", "min_confidence",
            "warmup", "runs", "repeats", "gap_sec", "min_sec", "context_sec",
        ):
            with self.subTest(field=field):
                self.assertIn('    ' + field + ':"', js)
        self.assertIn('По умолчанию:', js)
        self.assertIn('Влияние:', js)
        self.assertIn('mouseenter', js)
        self.assertIn('focus', js)

    def test_tooltips_are_served(self):
        html = (STATIC / "index.html").read_text(encoding="utf-8")
        css = (STATIC / "settings-tooltips.css").read_text(encoding="utf-8")
        server = (ROOT / "scripts" / "volleyball_ui" / "server.py").read_text(encoding="utf-8")
        for asset in ("settings-tooltips.js", "settings-tooltips.css"):
            self.assertIn('/' + asset, html)
            self.assertIn('"/' + asset + '"', server)
        self.assertIn('.setting-help-label:hover::after', css)
        self.assertIn('.setting-help-label:focus-visible::after', css)

    def test_svg_attribute_is_removed_instead_of_setting_expando_property(self):
        js = (STATIC / "results-ui.js").read_text(encoding="utf-8")
        self.assertIn('svg.removeAttribute("hidden")', js)
        self.assertIn('svg.setAttribute("hidden","")', js)
        self.assertNotIn('svg.hidden=false', js)
        self.assertNotIn('svg.hidden=true', js)
        html = (STATIC / "index.html").read_text(encoding="utf-8")
        self.assertIn('id="result-trajectory-indicator"', html)
        self.assertIn('id="result-trajectory-jump"', html)
        self.assertIn('id="result-ball-trail"', html)

    def test_trajectory_next_detection_is_exposed(self):
        backend = (ROOT / "scripts" / "volleyball_ui" / "trajectory.py").read_text(encoding="utf-8")
        self.assertIn('"first_frame"', backend)
        self.assertIn('"next_after_window"', backend)
        self.assertIn('"detections_total"', backend)


if __name__ == "__main__":
    unittest.main()
