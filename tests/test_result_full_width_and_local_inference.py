"""Regressions: full-width results timeline and local inference assets."""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ResultTimelineAndLocalInference(unittest.TestCase):
    def test_results_timeline_is_directly_below_video_only(self):
        html = (ROOT / "scripts/volleyball_ui/static/index.html").read_text(encoding="utf-8")
        results = html.split('<section id="view-analysis-results"', 1)[1].split(
            '<section id="view-players"', 1)[0]
        self.assertEqual(results.count('class="result-timeline"'), 1)
        self.assertEqual(results.count('id="result-rally-track"'), 1)
        self.assertEqual(results.count('id="result-timeline-ticks"'), 1)
        self.assertEqual(results.count('class="panel result-media-panel"'), 1)
        self.assertNotIn('id="result-source-list"', results)
        self.assertNotIn('id="result-source-select"', results)
        self.assertNotIn('class="result-project-sources"', results)
        self.assertIn('id="chain-inference"', html)
        self.assertNotIn('id="chain-video"', html)
        self.assertEqual(results.count('id="result-rally-list"'), 1)
        self.assertLess(results.index('class="result-video-layout"'), results.index('id="result-rally-list"'))
        self.assertIn('class="result-rally-sidebar result-rallies-panel"', results)
        self.assertNotIn('class="panel result-rallies-panel"', results)
        self.assertNotIn('class="analysis-video-divider"', results)
        self.assertRegex(
            results,
            r'(?s)id="result-playback-mode".*?</button>\s*</div>\s*<div class="result-timeline">',
            "Timeline should be immediately after the video player wrapper")
        self.assertLess(results.index('id="result-video-player"'),
                        results.index('class="result-timeline"'))
        self.assertLess(results.index('class="result-timeline"'),
                        results.index('class="result-rally-sidebar result-rallies-panel"'))
        self.assertLess(results.index('id="result-rally-track"'),
                        results.index('id="result-timeline-ticks"'))
        self.assertLess(results.index('id="result-timeline-ticks"'),
                        results.index('id="result-rally-settings"'))
        self.assertLess(results.index('class="result-rally-sidebar result-rallies-panel"'),
                        results.index('class="result-cpu-tools"'))
        css = (ROOT / "scripts/volleyball_ui/static/result-media.css").read_text(encoding="utf-8")
        self.assertIn(".result-media-panel .result-video-main > .result-timeline{", css)
        rule = css.split(".result-media-panel .result-video-main > .result-timeline{", 1)[1].split("}", 1)[0]
        self.assertRegex(rule, r"width:\s*100%")
        self.assertIn("margin:10px 0 0;", rule)
        self.assertIn("flex:0 0 auto;", rule)
        self.assertNotIn(".result-video-layout > .result-timeline", css)
        self.assertIn(".result-media-panel .result-rally-sidebar", css)
        self.assertIn(".result-media-panel .result-rallies-panel .result-rally-list", css)

    def test_rally_sidebar_reaches_video_bottom_without_status_strip(self):
        static = ROOT / "scripts/volleyball_ui/static"
        html = (static / "index.html").read_text(encoding="utf-8")
        js = (static / "results-ui.js").read_text(encoding="utf-8")
        css = (static / "result-media.css").read_text(encoding="utf-8")
        self.assertNotIn('id="result-rally-now"', html)
        self.assertNotIn('E("result-rally-now")', js)
        self.assertNotIn("Выберите розыгрыш ниже", js)
        side = css.split(".result-media-panel .result-rally-sidebar{", 1)[1].split("}", 1)[0]
        self.assertIn("height:0;", side)
        self.assertIn("min-height:100%;", side)
        self.assertIn("max-height:none;", side)
        self.assertIn("overflow:hidden;", side)
        scroller = css.split(".result-media-panel .result-rallies-panel .result-rally-list{", 1)[1].split("}", 1)[0]
        self.assertIn("flex:1 1 auto;", scroller)
        self.assertIn("overflow-y:auto;", scroller)
        mobile = css.split("@media(max-width:780px)", 1)[1]
        self.assertIn("height:auto;", mobile)
        self.assertIn("max-height:250px;", mobile)

    def test_result_labels_and_rally_text(self):
        static = ROOT / "scripts/volleyball_ui/static"
        html = (static / "index.html").read_text(encoding="utf-8")
        app = (static / "app.js").read_text(encoding="utf-8")
        results = (static / "results-ui.js").read_text(encoding="utf-8")
        self.assertNotIn("<h3>Видео проекта</h3>", html)
        self.assertIn("<h3>Найденные розыгрыши</h3>", html)
        self.assertNotIn("???", html)
        self.assertNotIn("???", app)
        self.assertNotIn("подача не подтверждена", results)
        self.assertIn('?" · подача найдена":""', results)

    def test_models_resolve_to_files_in_repository(self):
        from scripts.volleyball_studio.ball_models import BALL_MODELS, resolve_ball_model
        for model in BALL_MODELS:
            onnx, openvino = resolve_ball_model(model)
            self.assertTrue(onnx.is_file())
            self.assertTrue(onnx.resolve().is_relative_to((ROOT / "models").resolve()))
            self.assertTrue(openvino.resolve().is_relative_to(ROOT.resolve()))
        for name in ("RAVEL-VB-011-9f", "RAVEL-VB-012-9f"):
            self.assertTrue((ROOT / "ov" / (name + ".xml")).is_file())
            self.assertTrue((ROOT / "ov" / (name + ".bin")).is_file())

    def test_no_remote_inference_in_active_code_paths(self):
        files = [
            "scripts/volleyball_studio/analysis.py",
            "scripts/volleyball_studio/render.py",
            "scripts/volleyball_ui/server.py",
            "scripts/benchmark_project.py",
            "scripts/build_project_highlights.py",
            "src/inference_onnx_seq_gray_v2.py",
            "src/inference_openvino_seq_gray_v2.py",
            "src/inference_player_ball_openvino.py",
            "src/track_calculator_with_court.py",
            "src/track_processor.py",
            "src/make_reels.py",
            "src/show_rally.py",
        ]
        forbidden = re.compile(
            r"(?:requests\.(?:post|get)|httpx\.|torch\.hub\.load|"
            r"from_pretrained\(|huggingface_hub|urllib\.request|urlopen\(|"
            r"socket\.create_connection\(|grpc\.(?:insecure|secure)_channel\(|api\.openai\.com|"
            r"api\.anthropic\.com|generativelanguage\.googleapis\.com)"
        )
        for name in files:
            source = (ROOT / name).read_text(encoding="utf-8")
            self.assertIsNone(forbidden.search(source), name)


if __name__ == "__main__":
    unittest.main()
