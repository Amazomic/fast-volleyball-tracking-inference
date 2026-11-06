"""Quick Reel retention, cancellation isolation and no-transcode video acceptance."""
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui import (
    prepared_videos, quick_reel_jobs, quick_reel_prepare, server, video_compat
)


def metadata(**overrides):
    video = {
        "codec_type": "video", "codec_name": "h264",
        "pix_fmt": "yuv420p", "width": 1280, "height": 720,
        "sample_aspect_ratio": "1:1", "r_frame_rate": "30/1",
        "avg_frame_rate": "30/1", "start_time": "0.0",
        "field_order": "progressive",
    }
    video.update(overrides)
    return {"format": {"format_name": "mov,mp4,m4a,3gp,3g2,mj2"},
            "streams": [video, {"codec_type": "audio", "codec_name": "aac"}]}


class AlreadyPreparedTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.source = Path(self.tmp.name) / "match.mp4"
        self.source.write_bytes(b"valid-placeholder")
        video_compat._compatible.cache_clear()

    def test_only_exact_safe_mp4_is_directly_accepted(self):
        for field, value in (
            ("codec_name", "hevc"), ("pix_fmt", "yuv420p10le"),
            ("width", 1920), ("r_frame_rate", "30000/1001"),
            ("avg_frame_rate", "29/1"), ("sample_aspect_ratio", "4:3"),
            ("start_time", "0.5"), ("side_data_list", [{"rotation": 90}]),
        ):
            with self.subTest(field=field):
                video_compat._compatible.cache_clear()
                with patch.object(video_compat, "_ffprobe",
                                  return_value=metadata(**{field: value})):
                    self.assertFalse(video_compat.is_analysis_ready(self.source, 1280))
        video_compat._compatible.cache_clear()
        with patch.object(video_compat, "_ffprobe", return_value=metadata()):
            self.assertTrue(video_compat.is_analysis_ready(self.source, 1280))

    def test_quick_reel_skips_transcode_and_cache_when_source_ready(self):
        saved = self.source.parent / "prepared.mp4"
        marker = self.source.parent / "prepared.json"
        identity = {"width": 1280}
        events = []
        with (patch.object(quick_reel_prepare, "prepared_entry",
                           return_value=(self.source, {"duration_sec": 1}, saved, marker, identity)),
              patch.object(quick_reel_prepare, "is_analysis_ready", return_value=True),
              patch.object(quick_reel_prepare, "_valid",
                           side_effect=AssertionError("Should not read prepared cache"))):
            result = quick_reel_prepare.prepare_source(
                str(self.source), lambda n, reused: events.append((n, reused)))
            self.assertEqual(result, (self.source, True))
            self.assertEqual(events, [(100, True)])
            self.assertFalse(saved.exists())

    def test_project_ready_path_direct_source_without_proxy(self):
        cached = self.source.parent / "cached.mp4"
        marker = self.source.parent / "cached.json"
        with (patch.object(prepared_videos, "_entry",
                           return_value=(self.source, {}, cached, marker, {"width": 1280})),
              patch.object(prepared_videos, "is_analysis_ready", return_value=True)):
            self.assertEqual(prepared_videos.ready_path("any", "match.mp4"), self.source)
            self.assertFalse(cached.exists())

    @unittest.skipUnless(shutil.which("ffmpeg") and shutil.which("ffprobe"),
                         "FFmpeg and FFprobe required")
    def test_real_30fps_h264_mp4_is_used_without_new_file(self):
        source = self.source.parent / "clean.mp4"
        subprocess.run([
            "ffmpeg", "-hide_banner", "-loglevel", "error", "-y",
            "-f", "lavfi", "-i", "testsrc2=size=640x360:rate=30",
            "-t", "1", "-c:v", "libx264", "-pix_fmt", "yuv420p",
            "-preset", "ultrafast", "-an", str(source),
        ], check=True, timeout=30)
        self.assertTrue(video_compat.is_analysis_ready(source, 640))
        prepared, skipped = quick_reel_prepare.prepare_source(str(source))
        self.assertEqual(prepared, source.resolve())
        self.assertTrue(skipped)


class LatestOnlyTests(unittest.TestCase):
    def test_cleanup_drops_previous_run_and_selections_but_not_input(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder) / "quick_reels"
            old, new = "a" * 32, "b" * 32
            active_token = "d" * 32
            for ident in (old, new):
                (root / ident).mkdir(parents=True)
                (root / ident / "request.json").write_text(
                    '{"id":"' + ident + '","videos":[{"selection_id":"' +
                    active_token + '"}]}', encoding="utf-8")
            (root / old / "analysis.mp4").write_bytes(b"old data")
            (root / "selections").mkdir()
            (root / "selections" / ("c" * 32 + ".json")).write_text("{}")
            (root / "selections" / (active_token + ".json")).write_text("{}")
            originals = Path(folder) / "source.mp4"
            originals.write_bytes(b"do not remove")
            soundtrack = Path(folder) / "track.mp3"
            soundtrack.write_bytes(b"audio")
            (root / "audio").mkdir()
            (root / "audio" / "unused.mp3").write_bytes(b"unused")
            with patch.object(quick_reel_jobs, "QUICK_ROOT", root):
                quick_reel_jobs.publish_latest(new, str(soundtrack))
                self.assertEqual(quick_reel_jobs.latest_reel_id(), new)
                self.assertFalse((root / old).exists())
                self.assertTrue((root / "selections" / (active_token + ".json")).is_file())
                self.assertFalse((root / "selections" / ("c" * 32 + ".json")).exists())
                self.assertFalse((root / "audio" / "unused.mp3").exists())
                self.assertTrue((root / new / "request.json").exists())
                self.assertTrue(originals.is_file())
                self.assertTrue(soundtrack.is_file())

    def test_cancel_never_stops_unrelated_or_stale_job(self):
        class FakeProcess:
            def poll(self):
                return None

        active = "a" * 32
        with (patch.object(server, "JOB", {"process": FakeProcess(),
                                         "kind": "analyze",
                                         "quick_request_id": None}),
              patch.object(server, "cancel_job") as cancel):
            with self.assertRaises(ValueError):
                server.cancel_quick_reel(active)
            cancel.assert_not_called()
        with (patch.object(server, "JOB", {"process": FakeProcess(),
                                         "kind": "quick_reel",
                                         "quick_request_id": active}),
              patch.object(server, "cancel_job", return_value={"status": "cancelled"}) as cancel):
            with self.assertRaises(ValueError):
                server.cancel_quick_reel("b" * 32)
            self.assertEqual(server.cancel_quick_reel(active)["status"], "cancelled")
            cancel.assert_called_once()


if __name__ == "__main__":
    unittest.main()
