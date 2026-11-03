"""Regression tests for player tracklet review and tuning."""
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui.player_tracks import _compute, suggest_continuations
from scripts.volleyball_studio.advanced import validated, as_cli
from scripts.volleyball_studio.__main__ import parser
from src.inference_player_ball_openvino import PlayerHysteresis


class PlayerGroupingTests(unittest.TestCase):
    def test_summary_counts_unique_tracklets_not_players(self):
        with tempfile.TemporaryDirectory() as td:
            file=Path(td)/"players.json"
            file.write_text(json.dumps({
                "fps":30,"source_size":{"width":1000,"height":1000},
                "predictions":[
                    {"class_name":"player","track_id":10,"frame_index":0,"score":.9,
                     "bbox_xyxy":[100,100,170,300],"interpolated":False},
                    {"class_name":"player","track_id":10,"frame_index":10,"score":.8,
                     "bbox_xyxy":[120,110,190,310],"interpolated":False},
                    {"class_name":"player","track_id":10,"frame_index":11,"score":.7,
                     "bbox_xyxy":[120,110,190,310],"interpolated":True},
                    {"class_name":"player","track_id":11,"frame_index":50,"score":.9,
                     "bbox_xyxy":[125,105,195,305],"interpolated":False},
                ]
            }),encoding="utf8")
            item=_compute(str(file),file.stat().st_mtime_ns,file.stat().st_size)
            self.assertEqual(len(item["tracks"]),2)
            self.assertEqual(item["tracks"]["10"]["detected_frames"],2)
            self.assertEqual(item["tracks"]["10"]["first_frame"],0)
            self.assertEqual(item["tracks"]["10"]["last_frame"],11)

    def test_cautious_suggestions_never_merge_simultaneous_tracks(self):
        rows={
            "10":{"first_frame":10,"last_frame":120,"start_center":[.2,.2],"end_center":[.3,.3],
                   "start_height":.2,"end_height":.2,"detected_frames":110,"duration_sec":3.6,"mean_score":.9},
            "11":{"first_frame":125,"last_frame":220,"start_center":[.31,.31],"end_center":[.35,.35],
                   "start_height":.21,"end_height":.21,"detected_frames":80,"duration_sec":3.2,"mean_score":.85},
            "12":{"first_frame":90,"last_frame":200,"start_center":[.31,.31],"end_center":[.34,.34],
                   "start_height":.21,"end_height":.21,"detected_frames":70,"duration_sec":3.6,"mean_score":.8},
            "13":{"first_frame":130,"last_frame":230,"start_center":[.9,.9],"end_center":[.9,.9],
                   "start_height":.2,"end_height":.2,"detected_frames":70,"duration_sec":3.3,"mean_score":.85},
        }
        with patch("scripts.volleyball_ui.player_tracks.tracks_for",return_value={"fps":30,"tracks":rows}):
            result=suggest_continuations("video.mp4","project","10")
        self.assertEqual([row["track_id"] for row in result["candidates"]],["11"])

    def test_match_distance_changes_tracking_fragmentation(self):
        first={"class_name":"player","score":.9,
               "bbox_xyxy_norm":[.175,.1,.225,.3]}
        second={"class_name":"player","score":.9,
                "bbox_xyxy_norm":[.27,.1,.32,.3]}
        strict=PlayerHysteresis(.35,.2,2,max_center_distance=.08)
        relaxed=PlayerHysteresis(.35,.2,8,max_center_distance=.11)
        for tracker in (strict,relaxed):
            tracker.update(0,[first])
            tracker.update(1,[second])
        self.assertEqual(strict.next_track_id,2)
        self.assertEqual(relaxed.next_track_id,1)

    def test_model_parameters_reach_cli(self):
        v=validated({"player_match_center_distance":.12,"player_match_iou":.035,
                     "player_hysteresis_frames":12})
        args=parser().parse_args(as_cli(v))
        self.assertEqual(args.player_match_center_distance,.12)
        self.assertEqual(args.player_match_iou,.035)
        self.assertEqual(args.player_hysteresis_frames,12)
        for bad in ({"player_match_center_distance":.5},{"player_match_iou":-1}):
            with self.assertRaises(ValueError):
                validated(bad)


if __name__=="__main__":
    unittest.main()
