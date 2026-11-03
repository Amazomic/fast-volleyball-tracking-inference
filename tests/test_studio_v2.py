import json
import shutil
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))

from volleyball_studio.common import cached, encoder, run
from volleyball_studio.analysis import select
from volleyball_studio.quality import check
from volleyball_studio.events import contacts_for, event_timeline
from volleyball_studio.preview import review_filter
from volleyball_studio.resources import adaptive_workers
from volleyball_studio.styles import apply_style
from volleyball_studio.vfx import effect_state, playback_speed


class StudioV2(unittest.TestCase):
    def test_events_keep_unknown_sides_and_absolute_time(self):
        track = {"start_frame": 0, "last_frame": 90, "rally_classification": {"rally_confidence": .9}, "player_interaction": {"contacts": [{"frame": 30, "contact_type": "block", "confidence": .9, "ball_x": 30, "ball_y": 20, "player_track_id": 1}]}}
        result = event_timeline([track], "match.mp4", 10., 1280, 720, False)
        contact = next(e for e in result["events"] if "contact" in e)
        self.assertEqual(contact["time_sec"], 11.)
        self.assertEqual(contact["type"], "unknown_touch")
        self.assertLess(contact["confidence"], .9)
        self.assertEqual(contacts_for(result, track)[0]["event_id"], contact["id"])

    def test_interrupted_stage_restarts_without_complete_marker(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            def interrupted():
                raise KeyboardInterrupt()
            with self.assertRaises(KeyboardInterrupt):
                cached(root, "ball", "signature", interrupted)
            self.assertFalse((root / "ball.complete.json").exists())
            self.assertEqual(json.loads((root / "ball.status.json").read_text())["status"], "interrupted")
            output = root / "ball.csv"
            def recovered():
                output.write_text("valid")
                return [output]
            cached(root, "ball", "signature", recovered)
            self.assertTrue((root / "ball.complete.json").exists())

    def test_nvenc_failure_falls_back_without_cuda_installation(self):
        with patch("volleyball_studio.common.subprocess.run", return_value=SimpleNamespace(returncode=1, stderr=b"NVENC unavailable")):
            self.assertIn("libx264", encoder("auto"))
            with self.assertRaises(RuntimeError):
                encoder("nvenc")

    def test_styles_do_not_mutate_recognition_options(self):
        for style, expected in (("classic", "off"), ("dynamic", "subtle"), ("cinematic", "cinematic")):
            args = SimpleNamespace(style=style, effects=None, accents=False, backend="onnx", analysis_width=1280)
            apply_style(args)
            self.assertEqual(args.effects, expected)
            self.assertEqual((args.backend, args.analysis_width), ("onnx", 1280))

    def test_manual_review_and_unknown_identifiers(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "selection.json"
            path.write_text(json.dumps({"exclude": ["b"]}))
            self.assertEqual(review_filter([{"id": "a"}, {"id": "b"}], path), [{"id": "a"}])
            path.write_text(json.dumps({"include": ["unknown"]}))
            with self.assertRaises(ValueError):
                review_filter([{"id": "a"}], path)

    def test_uncertain_attack_never_enables_fire_or_ramp(self):
        item = {"analysis_width": 1000, "ball": {f: {"x": 100+f*20, "y": 100, "confidence": .99} for f in range(40)}, "ball_rows": {}, "contacts": [{"frame": 15, "contact_type": "attack", "confidence": .7, "speed_after_px_frame": 30, "recommended_effect": "flash"}]}
        args = SimpleNamespace(speed_ramp=True, effects="cinematic", min_confidence=.65)
        state = effect_state(item, 15, "cinematic", .65)
        self.assertFalse(state["fire"])
        self.assertEqual(playback_speed(item, 10, args, state), 1.)
        item["contacts"][0].update(confidence=.99, recommended_effect="fire")
        self.assertEqual(playback_speed(item, 10, args, state), .5)
        self.assertEqual(playback_speed(item, 26, args, state), 1.5)
        item["ball"].pop(26)
        self.assertEqual(playback_speed(item, 26, args, state), 1.)

    def test_workers_respect_memory_limit(self):
        with patch("volleyball_studio.resources.measure_resources", return_value={"available_ram_bytes": 2*1024**3}), patch("volleyball_studio.resources.os.cpu_count", return_value=32):
            self.assertEqual(adaptive_workers(8, 4, "h264_nvenc"), 1)

    def test_selection_context_stays_inside_safe_gap(self):
        item = {"source": "a", "start": 30, "end": 89, "duration": 2., "score": 10., "contacts": [], "safe_context_start": 28, "safe_context_end": 93}
        selected = select([item], 5, 1)[0][0]
        self.assertEqual((selected["start"], selected["end"]), (28, 93))
        self.assertEqual(selected["context_frames"], 6)

    @unittest.skipUnless(shutil.which("ffmpeg"), "FFmpeg required")
    def test_qc_rejects_long_black_video(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "black.mp4"
            run(["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i", "color=s=180x320:r=30:d=2", "-f", "lavfi", "-i", "anullsrc=r=48000:cl=stereo", "-t", "2", "-c:v", "libx264", "-c:a", "aac", path])
            result = check(path, {"mapping": [{} for _ in range(60)], "effects": []}, 2, .65)
            self.assertFalse(result["passed"])
            self.assertFalse(result["checks"]["no_long_black"])


if __name__ == "__main__":
    unittest.main()
