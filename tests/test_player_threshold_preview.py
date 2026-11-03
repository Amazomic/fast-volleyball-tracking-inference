"""RAVEL raw player candidates can be re-filtered without rerunning inference."""
from pathlib import Path
import json
import tempfile
import unittest
from unittest.mock import patch

from scripts.volleyball_ui import player_overlay


class RawPlayersPreviewTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.cache=Path(self.temp.name)
        self.players=self.cache/"players.json"
        self.players.write_text(json.dumps({
            "fps":30,"source_size":{"width":1000,"height":600},
            "predictions":[{"class_name":"player","track_id":5,"score":.92,
                "frame_index":0,"bbox_xyxy":[100,60,200,240]}],
        }),encoding="utf-8")
        self.raw=self.cache/"players.raw.jsonl"
        self.raw.write_text(
            '[0,[[0.92,0.1,0.1,0.2,0.4],[0.2,0.5,0.3,0.6,0.6]]]\n'
            '[1,[[0.4,0.52,0.3,0.62,0.6]]]\n',
            encoding="utf-8")
        (self.cache/"players.raw.json").write_text(json.dumps({
            "format":"ravel-raw-players-v1","minimum_confidence":.05,
            "score_threshold":.65,"close_threshold":.18
        }),encoding="utf-8")
        pa=patch.object(player_overlay,"assert_project_video",return_value=None)
        pb=patch("scripts.volleyball_ui.analysis_runs.cache_for",
                 return_value=self.cache)
        pa.start();pb.start()
        self.addCleanup(pa.stop);self.addCleanup(pb.stop)
        player_overlay._index.cache_clear()
        player_overlay._raw_offsets.cache_clear()

    def window(self, threshold=None):
        return player_overlay.player_boxes_window(
            "testproject","video.mp4",0,.2,"testcache",threshold=threshold)

    def test_raw_threshold_changes_boxes_and_preserves_matched_id(self):
        baseline=self.window()
        low=self.window(.15)
        high=self.window(.85)
        self.assertEqual(len(baseline["frames"][0][1]),1)
        self.assertEqual(len(low["frames"][0][1]),2)
        self.assertEqual(low["frames"][0][1][0][0],5)
        self.assertEqual(low["frames"][0][1][1][0],-1)
        self.assertEqual(len(low["frames"][1][1]),1)
        self.assertEqual(len(high["frames"]),1)
        self.assertEqual(len(high["frames"][0][1]),1)

    def test_capability_and_legacy_fail_closed(self):
        self.assertTrue(player_overlay.player_detection_capability(self.cache)["available"])
        self.raw.unlink()
        self.assertFalse(player_overlay.player_detection_capability(self.cache)["available"])
        self.assertEqual(self.window(.20)["reason"],"no_raw_players")
        self.assertTrue(self.window()["available"])

    def test_invalid_threshold_rejected(self):
        for value in (-1,0,1,float("nan"),"hello"):
            with self.subTest(value=value), self.assertRaises(ValueError):
                self.window(value)

    def test_seek_index_skips_previous_frames(self):
        # Sparse frame index gives correct results beyond the first chunk.
        rows=["["+str(i)+",[[0.5,0.1,0.1,0.2,0.4]]]" for i in range(401)]
        self.raw.write_text("\n".join(rows)+"\n",encoding="utf-8")
        player_overlay._raw_offsets.cache_clear()
        result=player_overlay.player_boxes_window(
            "testproject","video.mp4",9,1,"testcache",threshold=.4)
        self.assertTrue(result["frames"])
        self.assertGreaterEqual(result["frames"][0][0],270)
        self.assertLessEqual(result["frames"][-1][0],300)


if __name__=="__main__":
    unittest.main()
