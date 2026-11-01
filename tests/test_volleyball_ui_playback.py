"""Browser playback without existing analysis or full-length transcoded copies."""
import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui import data, playback, preview


class OnDemandPlaybackTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        source_dir = self.root / "project"
        source_dir.mkdir()
        output_dir = self.root / "results"
        output_dir.mkdir()
        self.video = source_dir / "test.mp4"
        subprocess.run([
            "ffmpeg", "-nostdin", "-hide_banner", "-loglevel", "error", "-y",
            "-f", "lavfi", "-i", "testsrc2=size=320x180:rate=30",
            "-t", "2", "-c:v", "mpeg4", "-q:v", "4", str(self.video)
        ], check=True, timeout=20)
        original = self.video.stat()
        self.original_signature = (original.st_size, original.st_mtime_ns)
        for m in [
            patch.object(data, "PROJECT", source_dir),
            patch.object(data, "RESULTS", output_dir),
        ]:
            m.start()
            self.addCleanup(m.stop)
        preview._read_info.cache_clear()

    def test_media_generated_only_on_demand_and_shared_by_source(self):
        info = preview.video_info(self.video.name)
        self.assertEqual(info["codec"], "mpeg4")
        self.assertFalse((self.root/"results"/"ui_stream").exists())
        segment = playback.get_segment(self.video.name, 0, info["duration_sec"])
        self.assertTrue(segment.is_file())
        self.assertLess(segment.stat().st_size, 2 * 1024 * 1024)
        before = segment.stat().st_mtime_ns
        self.assertEqual(playback.get_segment(self.video.name,0,info["duration_sec"]),segment)
        self.assertEqual(segment.stat().st_mtime_ns,before)
        probe = subprocess.run([
            "ffprobe","-v","error","-select_streams","v:0",
            "-show_entries","stream=codec_name,width,height",
            "-of","json",str(segment)
        ], check=True,capture_output=True,text=True,timeout=10)
        video = json.loads(probe.stdout)["streams"][0]
        self.assertEqual(video["codec_name"],"h264")
        self.assertEqual(video["width"],480)
        original = self.video.stat()
        self.assertEqual((original.st_size,original.st_mtime_ns),
                         self.original_signature)

    def test_invalid_segments_do_not_spawn_transcoding(self):
        info = preview.video_info(self.video.name)
        for position in (-1,1,18,999999):
            with self.subTest(position=position):
                with self.assertRaises(ValueError):
                    playback.get_segment(self.video.name,position,info["duration_sec"])
        with self.assertRaises(ValueError):
            playback.get_segment(self.video.name,0,float("nan"))


if __name__=="__main__":
    unittest.main()
