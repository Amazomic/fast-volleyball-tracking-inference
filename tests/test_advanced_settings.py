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
        missing = set(SPECS).difference(parser_html.fields)
        self.assertFalse(missing, f"Options missing from both forms: {sorted(missing)}")

    def test_models_present(self):
        for name in ("RAVEL-VB-011-9f", "RAVEL-VB-012-9f"):
            self.assertTrue((ROOT / "ov" / (name + ".xml")).is_file())
