"""Regression coverage for configured pipeline options."""
from html.parser import HTMLParser
from pathlib import Path
from unittest import TestCase

from scripts.volleyball_studio.advanced import SPECS, validated, as_cli, from_namespace
from scripts.volleyball_studio.__main__ import parser

ROOT = Path(__file__).resolve().parents[1]


class FormFields(HTMLParser):
    def __init__(self):
        super().__init__()
        self.fields = set()

    def handle_starttag(self, tag, attrs):
        if tag in ("input", "select"):
            data = dict(attrs)
            if data.get("name"):
                self.fields.add(data["name"])


class AdvancedOptionsTests(TestCase):
    def test_all_settings_round_trip(self):
        values = validated({
            "player_model": "RAVEL-VB-011-9f",
            "track_max_distance": 180,
            "tracks_export_mode": "split",
            "tracks_mark_ball": False,
            "reels_export": True,
            "reels_smoothing": "kalman",
            "reels_interpolation": "hold",
            "reels_padding": 0.7,
        })
        args = parser().parse_args(as_cli(values))
        self.assertEqual(from_namespace(args), values)

    def test_reject_invalid_settings(self):
        for invalid in (
            {"player_model": "../unsafe"},
            {"track_min_duration": -1},
            {"player_frame_step": 0},
            {"player_score_threshold": 1.01},
            {"reels_smooth_window": 14},
            {"reels_smooth_polyorder": 5, "reels_smooth_window": 5},
            {"tracks_export_mode": "unknown"},
        ):
            with self.subTest(invalid=invalid):
                with self.assertRaises(ValueError):
                    validated(invalid)

    def test_all_exposed_fields_exist_in_ui(self):
        parser_html = FormFields()
        parser_html.feed((ROOT / "scripts/volleyball_ui/static/index.html").read_text(encoding="utf-8"))
        # Historical SPECS remains accepted by the CLI for replaying old
        # runs. New primary analysis exposes only parameters that affect GPU
        # inference, while contact tracking is configured in the CPU stage.
        html = (ROOT / "scripts/volleyball_ui/static/index.html").read_text(encoding="utf-8")
        primary = FormFields()
        primary.feed(html.split('<form id="analysis-form">', 1)[1].split("</form>", 1)[0])
        self.assertTrue(primary.fields <= (set(SPECS) | {
            "ball_model", "ball_threshold", "backend",
            "gpu_device_id", "gpu_memory_limit_mb", "threads", "analysis_width"}))
        for name in ("track_max_distance", "track_min_duration",
                     "track_player_threshold", "track_ball_threshold",
                     "track_no_ball_fill", "beach",
                     "player_touch_smooth", "player_touch_min_gap",
                     "player_touch_hold", "player_strip_seconds"):
            self.assertNotIn(name, primary.fields, name)
        for name in ("player_model", "player_score_threshold",
                     "player_action_threshold", "ball_threshold", "backend"):
            self.assertIn(name, primary.fields, name)
        self.assertIn('id="result-track-min-duration"', html)
        self.assertIn('id="result-touch-min-interval"', html)

    def test_models_present(self):
        for name in ("RAVEL-VB-011-9f", "RAVEL-VB-012-9f"):
            self.assertTrue((ROOT / "ov" / (name + ".xml")).is_file())
