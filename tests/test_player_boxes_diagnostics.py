"""Regressions for local RAVEL box overlays and opt-in diagnostics."""
from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui.player_overlay import _index, player_boxes_window
from scripts.volleyball_ui.diagnostics import validated, outputs, output_path


class PlayerOverlayTests(unittest.TestCase):
    def test_bbox_window_uses_saved_frames_and_clips_coordinates(self):
        with tempfile.TemporaryDirectory() as folder:
            base=Path(folder)
            file=base/"players.json"
            file.write_text(json.dumps({
                "fps":30,"source_size":{"width":1080,"height":1920},
                "predictions":[
                  {"class_name":"player","track_id":42,"frame_index":30,"score":.85,
                   "bbox_xyxy":[100,200,500,800],"interpolated":False},
                  {"class_name":"player","track_id":42,"frame_index":31,"score":.73,
                   "bbox_xyxy":[100,200,510,820],"interpolated":True},
                  {"class_name":"player","track_id":90,"frame_index":45,"score":.91,
                   "bbox_xyxy":[-10,150,330,800],"interpolated":False},
                  {"class_name":"head","track_id":999,"frame_index":45,"score":.99,
                   "bbox_xyxy":[100,200,300,500]},
                  {"class_name":"player","track_id":91,"frame_index":60,"score":.02,
                   "bbox_xyxy":[100,200,300,500]},
                ]
            }),encoding="utf-8")
            with patch("scripts.volleyball_ui.player_overlay.assert_project_video"),\
                 patch("scripts.volleyball_ui.player_overlay.find_cache",return_value=base):
                out=player_boxes_window("project","file.mp4",0.95,1)
            self.assertEqual(out["width"],1080)
            self.assertEqual(out["height"],1920)
            self.assertEqual([x[0] for x in out["frames"]],[30,45])
            self.assertEqual(out["frames"][0][1][0][0],42)
            self.assertEqual(out["frames"][1][1][0][1],0)
            with patch("scripts.volleyball_ui.player_overlay.assert_project_video"),\
                 patch("scripts.volleyball_ui.player_overlay.find_cache",return_value=base):
                for start,length in ((-1,2),(0,12),(0,0),(float("nan"),1)):
                    with self.assertRaises(ValueError):
                        player_boxes_window("project","file.mp4",start,length)

    def test_diagnostics_validation(self):
        opts=validated({"tracks_export_mode":"split","tracks_padding":.7,
                        "tracks_include_not_rally":True,"tracks_mark_ball":False})
        self.assertEqual(opts["tracks_export_mode"],"split")
        self.assertEqual(opts["tracks_padding"],.7)
        self.assertTrue(opts["tracks_include_not_rally"])
        self.assertFalse(opts["tracks_mark_ball"])
        for bad in (
            {"tracks_export_mode":"none"},
            {"tracks_export_mode":"dangerous"},
            {"tracks_export_mode":"split","tracks_padding":-1},
            {"tracks_export_mode":"split","show_rally_track":-1},
        ):
            with self.subTest(bad=bad),self.assertRaises(ValueError):
                validated(bad)

    def test_diagnostics_outputs_only_cached_allowlisted_files(self):
        with tempfile.TemporaryDirectory() as folder:
            base=Path(folder)
            directory=base/"exports/diagnostics"
            directory.mkdir(parents=True)
            (directory/"a.mp4").write_bytes(b"test")
            (directory/"snapshot.png").write_bytes(b"png")
            (directory/"bad.txt").write_text("nope")
            with patch("scripts.volleyball_ui.diagnostics.paths",
                       return_value=(base,base/"analysis.mp4",base/"tracks",directory)):
                found=outputs("proj","video.mp4")
                self.assertEqual(len(found),2)
                self.assertEqual(output_path("proj","video.mp4","a.mp4"),directory/"a.mp4")
                with self.assertRaises(FileNotFoundError):
                    output_path("proj","video.mp4","../bad.txt")

    def test_diagnostics_controls_live_in_analysis_not_preview(self):
        root=Path(__file__).resolve().parents[1]
        html=(root/"scripts/volleyball_ui/static/index.html").read_text(encoding="utf8")
        a=html.index('<form id="analysis-form">')
        a_end=html.index("</form>",a)
        diag=html.index('id="results-diagnostics"')
        r=html.index('id="view-analysis-results"')
        player=html.index('id="view-players"')
        self.assertTrue(a_end<diag<r<player)
        self.assertNotIn('name="tracks_export_mode"',html[a:a_end])
        self.assertIn('name="tracks_export_mode"',html[diag:r])
        self.assertNotIn('id="diagnostics-form"',html[r:player])
        self.assertIn('id="diagnostics-source-select"',html[diag:r])
        self.assertIn('data-result-overlay="players"',html)
        self.assertIn('id="result-player-boxes"',html)
        js=(root/"scripts/volleyball_ui/static/results-ui.js").read_text(encoding="utf8")
        self.assertIn('requestPlayerBoxes',js)
        self.assertIn('paintPlayerBoxes',js)


if __name__=="__main__":
    unittest.main()
