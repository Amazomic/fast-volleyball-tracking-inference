"""Tests for player thumbnail index and cache with synthetic tracker data."""
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui import data, photos


class PlayerPhotosTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.project = self.root/"project"
        self.project.mkdir()
        self.result_dir = self.root/"results"
        self.name = "game.mp4"
        (self.project/self.name).write_bytes(b"placeholder")
        self.cache = self.result_dir/"analysis"/"unit-cache"
        (self.cache/"analysis").mkdir(parents=True)
        (self.cache/"analysis"/"events.json").write_text(
            json.dumps({"source":str(self.project/self.name),"events":[]}),encoding="utf-8")
        records=[]
        for frame in range(0, 121, 5):
            records.append({"class_name":"player", "track_id": 12, "frame_index":frame,
                            "bbox_xyxy":[20.0,30.0,90.0,160.0], "score":0.96})
        records.append({"class_name":"head", "track_id":99,"frame_index":100,
                        "bbox_xyxy":[0,0,10,10],"score":1.0})
        (self.cache/"players.json").write_text(
            json.dumps({"fps":30,"predictions":records}),encoding="utf-8")
        patches=[patch.object(data,"PROJECT",self.project),
                 patch.object(data,"RESULTS",self.result_dir)]
        for p in patches:
            p.start()
            self.addCleanup(p.stop)

    def test_returns_three_samples_for_known_id(self):
        meta=photos.photos_metadata(self.name,"12")
        self.assertEqual(len(meta["samples"]),3)
        frames=[x["frame"] for x in meta["samples"]]
        self.assertEqual(frames,sorted(set(frames)))
        self.assertTrue(all(x["url"].startswith("/api/player-photo?") for x in meta["samples"]))

    def test_rejects_invalid_ids_and_video_path(self):
        with self.assertRaises(ValueError):
            photos.photos_metadata(self.name,"../../etc/passwd")
        with self.assertRaises(ValueError):
            photos.photos_metadata("../wrong.mp4","12")
        with self.assertRaises(ValueError):
            photos.photo_path(self.name,12,99)

    def test_stable_index(self):
        first=photos.get_index(self.cache)
        second=photos.get_index(self.cache)
        self.assertEqual(first["tracks"]["12"],second["tracks"]["12"])
        self.assertNotIn("99",first["tracks"])
        self.assertTrue((self.result_dir/"player_photos"/"unit-cache"/"index.json").is_file())


if __name__ == "__main__":
    unittest.main()
