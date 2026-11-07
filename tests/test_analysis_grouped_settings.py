"""Advanced controls stay grouped by analysis stage without changing request keys."""
from html.parser import HTMLParser
from pathlib import Path
import unittest

FILE = Path(__file__).resolve().parents[1] / "scripts/volleyball_ui/static/index.html"

STAGES = {
    "analysis-stage-ball": {"ball_model", "ball_threshold", "backend"},
    "analysis-pipeline-advanced": {
        "player_model", "player_score_threshold", "player_head_threshold",
        "player_ball_threshold", "player_nms_iou", "player_close_threshold",
        "player_hysteresis_frames", "player_match_center_distance", "player_match_iou",
        "player_action_threshold", "player_backend", "player_device",
        "player_identity_mode", "player_reid_threshold_percent", "player_auto_assign",
        "player_auto_assign_threshold_percent", "player_session_gap_sec"},
    "analysis-stage-performance": {
        "gpu_device_id", "gpu_memory_limit_mb", "player_stride", "player_frame_step",
        "threads", "analysis_width",
        "player_reader", "player_decode_hwaccel", "player_decode_threads"},
    "analysis-stage-outputs": {
        "player_output_video", "ball_output_video", "player_include_features"}
}


class StageParser(HTMLParser):
    def __init__(self):
        super().__init__()
        self.stack = []
        self.named = {}
        self.stages = {}
        self.technical = False

    def handle_starttag(self, tag, attributes):
        attrs = dict(attributes)
        if tag == "details":
            self.stack.append(attrs.get("id"))
            if attrs.get("id") in STAGES:
                self.stages[attrs["id"]] = "open" in attrs
        if tag in ("input", "select") and attrs.get("name"):
            active = [key for key in self.stack if key in STAGES]
            if active:
                self.named.setdefault(attrs["name"], []).append(active[-1])

    def handle_endtag(self, tag):
        if tag == "details" and self.stack:
            self.stack.pop()


class GroupedAnalysisTests(unittest.TestCase):
    def test_settings_grouped_by_stage_with_preserved_names(self):
        doc = FILE.read_text(encoding="utf-8")
        section = doc.split('id="analysis-technical-settings"', 1)[1].split(
            'id="analysis-start-submit"', 1)[0]
        parser = StageParser()
        parser.feed(section)
        self.assertEqual(set(parser.stages), set(STAGES))
        self.assertEqual(
            {name for name, opened in parser.stages.items() if opened},
            {"analysis-stage-ball"}
        )
        expected = {name: [group] for group, names in STAGES.items() for name in names}
        self.assertEqual(parser.named, expected)
        self.assertIn('id="analysis-advanced"', section)
        self.assertIn('id="analysis-cpu-row"', section)
        self.assertIn('id="auto-size-value"', section)
        self.assertIn('id="analysis-context-summary"', section)
        for name in STAGES:
            self.assertIn('data-analysis-tab="'+name+'"', section)
        self.assertEqual(section.count('class="analysis-stage-tab"'), 4)
        # Real CPU controls are only available at the later postprocessing step.
        for field in ("track_max_distance", "track_min_duration",
                      "track_player_threshold", "track_ball_threshold",
                      "track_no_ball_fill", "beach", "player_touch_smooth",
                      "player_touch_min_gap", "player_touch_hold",
                      "player_strip_seconds"):
            self.assertNotIn('name="'+field+'"', section)
        self.assertIn('name="player_action_threshold"', section)


if __name__ == "__main__":
    unittest.main()
