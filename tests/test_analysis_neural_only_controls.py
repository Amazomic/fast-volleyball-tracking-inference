"""Primary analysis config contains only inputs used by neural inference.

Legacy profile records may still include CPU options, but the browser must
not display, resend, or count them as overrides of a neural profile.
"""
from html.parser import HTMLParser
from pathlib import Path
import unittest

from scripts.volleyball_ui.analysis_profiles import _clean_values

ROOT = Path(__file__).resolve().parents[1]
STATIC = ROOT / "scripts" / "volleyball_ui" / "static"

CPU_ONLY = {
    "track_max_distance", "track_min_duration", "track_player_threshold",
    "track_ball_threshold", "track_no_ball_fill", "beach",
    "player_touch_smooth", "player_touch_min_gap",
    "player_touch_hold", "player_strip_seconds",
}
# Identity-stage options are part of the global analysis profile, but execute
# only after inference and do not modify GPU/CPU touch postprocessing.
IDENTITY_STAGE_INPUTS = {
    "player_identity_mode", "player_reid_threshold_percent",
    "player_auto_assign", "player_auto_assign_threshold_percent",
    "player_session_gap_sec",
}
NEURAL_INPUTS = {
    "ball_model", "ball_threshold", "backend", "player_model",
    "player_backend", "player_device", "player_score_threshold",
    "player_head_threshold", "player_action_threshold",
    "player_ball_threshold", "player_nms_iou", "player_close_threshold",
    "player_hysteresis_frames", "player_match_center_distance",
    "player_match_iou", "player_stride", "player_frame_step",
    "player_reader", "player_decode_hwaccel", "player_decode_threads",
    "gpu_device_id", "gpu_memory_limit_mb", "threads",
    "analysis_width", "player_output_video", "ball_output_video",
    "player_include_features",
}

class FieldParser(HTMLParser):
    def __init__(self):
        super().__init__()
        self.names = set()
        self.tabs = []
        self.panels = []

    def handle_starttag(self, tag, attrs):
        a = dict(attrs)
        if tag in ("select", "input") and a.get("name"):
            self.names.add(a["name"])
        if a.get("role") == "tab":
            self.tabs.append(a["data-analysis-run"])
        if a.get("role") == "tabpanel":
            self.panels.append(a["id"])


class NeuralControlsTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.html = (STATIC / "index.html").read_text(encoding="utf8")
        cls.form = cls.html.split('<form id="analysis-form">', 1)[1].split(
            "</form>", 1)[0]
        cls.parsed = FieldParser()
        cls.parsed.feed(cls.form)

    def test_no_cpu_controls_are_present_or_hidden_in_gpu_form(self):
        self.assertEqual(self.parsed.names, NEURAL_INPUTS | IDENTITY_STAGE_INPUTS)
        self.assertFalse(self.parsed.names & CPU_ONLY)
        self.assertNotIn('analysis-stage-tracking', self.form)
        self.assertEqual(
            self.parsed.tabs,
            ["ball", "players", "performance", "outputs"],
        )
        self.assertEqual(len(self.parsed.panels), 4)
        self.assertNotIn('class="panel analysis-reprocess"', self.html)
        for cpu_control in ("result-reprocess-mode", "result-track-min-duration",
                            "result-track-max-distance", "result-touch-min-interval",
                            "result-track-beach"):
            self.assertIn('id="'+cpu_control+'"', self.html)

    def test_gpu_action_floor_stays_explicit(self):
        self.assertIn('name="player_action_threshold"', self.form)
        self.assertIn('порог сохранения Touch (GPU)', self.form)
        self.assertIn('порог итоговых касаний', self.form)
        self.assertIn('name="player_score_threshold"', self.form)
        self.assertIn('name="ball_threshold"', self.form)

    def test_primary_launch_does_not_require_cpu_form_or_experiment(self):
        js = (STATIC / "app.js").read_text(encoding="utf8")
        start = js.split("async function launchAnalysis(){", 1)[1].split(
            '$("#analysis-form").addEventListener("submit"', 1)[0]
        self.assertIn('await submitTask("/api/analyze",params,"analysis")', start)
        self.assertNotIn('analysis-experiment', start)
        self.assertNotIn('querySelector(\'[name="beach"]\')', start)
        self.assertNotIn('#reprocess-start', js)

    def test_legacy_cpu_profile_keys_do_not_count_as_neural_edits(self):
        # The old profile validator still accepts legacy records for reading.
        saved = _clean_values({
            "player_model": "RAVEL-VB-013-9f",
            "track_max_distance": 175,
            "beach": True,
            "player_touch_smooth": 7,
        })
        self.assertIn("track_max_distance", saved)
        js = (STATIC / "analysis-presets.js").read_text(encoding="utf8")
        self.assertIn("Object.keys(current).filter(", js)
        self.assertNotIn("Object.keys(base).filter(", js)
        self.assertEqual(NEURAL_INPUTS & CPU_ONLY, set())

if __name__ == "__main__":
    unittest.main()
