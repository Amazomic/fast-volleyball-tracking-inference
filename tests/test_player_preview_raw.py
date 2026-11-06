"""Raw preview uses saved decoder scores without changing GPU artifacts."""
from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui.player_overlay import (
    player_boxes_window, player_detection_capability)


class PlayerRawPreviewTests(unittest.TestCase):
    def test_new_gpu_raw_floor_can_be_previewed_and_old_floor_cannot(self):
        with tempfile.TemporaryDirectory() as tmp:
            cache=Path(tmp)
            (cache/"players.json").write_text(json.dumps({
                "fps":30,"source_size":{"width":100,"height":200},
                "predictions":[{
                    "class_name":"player","frame_index":30,
                    "track_id":5,"score":.8,
                    "bbox_xyxy":[20,20,50,100]}]}),encoding="utf8")
            raw=cache/"players.raw.jsonl"
            raw.write_text(json.dumps([30,[
                [.8,.2,.1,.5,.5],[.03,.7,.1,.9,.5]]])+"\n",encoding="utf8")
            meta=cache/"players.raw.json"
            meta.write_text(json.dumps({
                "format":"ravel-raw-players-v1",
                "minimum_confidence":.02,
                "score_threshold":.42}),encoding="utf8")
            with patch("scripts.volleyball_ui.player_overlay.assert_project_video"), \
                 patch("scripts.volleyball_ui.analysis_runs.cache_for",
                       return_value=cache), \
                 patch("scripts.volleyball_ui.data.excluded_track_ids",
                       return_value=set()):
                cap=player_detection_capability(cache)
                self.assertTrue(cap["available"])
                self.assertAlmostEqual(cap["minimum_confidence"],.02)
                data=player_boxes_window("proj","match.mp4",1,2,
                                         cache_id="id",threshold=.02)
                self.assertEqual(len(data["frames"]),1)
                self.assertEqual(len(data["frames"][0][1]),2)
                self.assertIn(-1,[row[0] for row in data["frames"][0][1]])
                meta.write_text(json.dumps({
                    "format":"ravel-raw-players-v1",
                    "minimum_confidence":.05,
                    "score_threshold":.42}),encoding="utf8")
                self.assertAlmostEqual(
                    player_detection_capability(cache)["minimum_confidence"],.05)
                with self.assertRaisesRegex(ValueError,"Порог ниже"):
                    player_boxes_window("proj","match.mp4",1,2,
                                        cache_id="id",threshold=.02)


if __name__=="__main__":
    unittest.main()
