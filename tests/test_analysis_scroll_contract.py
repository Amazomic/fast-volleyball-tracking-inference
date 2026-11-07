"""Regression guards for page scroll chaining inside the Analysis workspace."""
from pathlib import Path
import re
import unittest

STATIC = Path(__file__).resolve().parents[1] / "scripts" / "volleyball_ui" / "static"


def declarations(css: str, selector: str) -> str:
    matches = list(re.finditer(re.escape(selector) + r"\s*\{([^{}]*)\}", css))
    if not matches:
        raise AssertionError(f"Missing scroll selector: {selector}")
    return matches[-1].group(1)


class AnalysisScrollContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.style = (STATIC / "style.css").read_text(encoding="utf-8")
        cls.analysis = (STATIC / "analysis-presets.css").read_text(encoding="utf-8")
        cls.html = (STATIC / "index.html").read_text(encoding="utf-8")

    def test_settings_scroll_chains_to_main_page(self):
        block = declarations(self.analysis, ".analysis-profile-editor .analysis-stage-list")
        self.assertIn("overflow-y:auto", block)
        self.assertIn("overscroll-behavior-y:auto", block)
        self.assertNotIn("overscroll-behavior:contain", block)

    def test_log_scroll_chains_to_main_page(self):
        block = declarations(self.style, "#analysis-job-panel #job-log")
        self.assertIn("overflow-y:auto", block)
        self.assertIn("overscroll-behavior-y:auto", block)
        self.assertNotIn("overscroll-behavior:contain", block)

    def test_touch_timeline_allows_vertical_page_pan(self):
        block = declarations(self.style, ".timeline-input-only .timeline-canvas")
        self.assertIn("touch-action:pan-y", block)

    def test_css_cache_versions_updated(self):
        self.assertIn('/style.css?v=15', self.html)
        self.assertIn('/analysis-presets.css?v=3', self.html)


if __name__ == "__main__":
    unittest.main()
