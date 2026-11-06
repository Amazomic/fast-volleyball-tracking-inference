"""Unit + integration tests for common rally boundaries and portrait camera."""
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import numpy as np

from scripts.rally_boundaries import rebuild
from scripts.build_project_highlights import export_vertical_reels
from src.make_reels import (
    interpolate_positions, crop_frame, crop_and_save_track_payload,
)


def track(a,b,label="rally",tid=0,confidence=.88):
    return {"start_frame":a,"last_frame":b,"track_id":tid,
            "rally_classification":{"label":label,"rally_confidence":confidence}}


class RallyBoundaryTests(unittest.TestCase):
    def test_last_touch_keeps_follow_through_before_adjacent_return(self):
        barrier=track(706,790,"not_rally",tid=2)
        barrier["rally_classification"].update(
            not_rally_reason="technical_return",technical_return_confidence=.95)
        events=[{"type":"attack","frame":702,"confidence":.9,"contact":{}}]
        rows=rebuild([track(35,704,tid=1),barrier],events=events,
                     padding_frames=45,duration_frames=973)
        self.assertGreaterEqual(rows[0]["source_end_frame"],724)
        self.assertEqual(rows[0]["detected_end_frame"],704)

    def test_follow_through_never_crosses_new_serve_or_video_end(self):
        barrier=track(706,790,"not_rally",tid=2)
        barrier["rally_classification"].update(not_rally_reason="technical_return")
        events=[{"type":"attack","frame":702,"confidence":.9,"contact":{}},
                {"type":"serve","frame":712,"confidence":.95}]
        rows=rebuild([track(35,704),barrier],events=events,
                     padding_frames=45,duration_frames=973)
        self.assertLess(rows[0]["source_end_frame"],712)

    def test_merges_brief_dropout_and_keeps_full_extent(self):
        rows=rebuild([track(100,174,tid=1),track(183,260,tid=2)],
            max_gap_frames=90,padding_frames=45,duration_frames=400)
        self.assertEqual(len(rows),1)
        self.assertEqual(rows[0]["track_ids"],[1,2])
        self.assertEqual(rows[0]["detected_end_frame"],260)
        self.assertEqual(rows[0]["source_start_frame"],55)
        self.assertEqual(rows[0]["source_end_frame"],305)

    def test_explicit_technical_return_and_serve_split_point(self):
        barrier=track(195,220,"not_rally",tid=3)
        barrier["rally_classification"].update(not_rally_reason="technical_return",
                                                technical_return_confidence=.96)
        rows=rebuild([track(100,190,tid=1),barrier,track(225,300,tid=2)],
                     max_gap_frames=90,padding_frames=60,duration_frames=500)
        self.assertEqual(len(rows),2)
        self.assertLess(rows[0]["source_end_frame"],barrier["start_frame"])
        self.assertGreater(rows[1]["source_start_frame"],barrier["last_frame"])
        serves=[{"type":"serve","frame":225,"confidence":.92}]
        rows=rebuild([track(100,200,tid=1),track(225,300,tid=2)],
                     events=serves,max_gap_frames=90,
                     padding_frames=10,duration_frames=500)
        self.assertEqual(len(rows),2)

    def test_serve_anchors_only_when_supported(self):
        events=[{"type":"serve","frame":90,"confidence":.95}]
        rows=rebuild([track(104,240)],events=events,
                    max_gap_frames=90,padding_frames=25,duration_frames=300)
        self.assertEqual(rows[0]["serve_frame"],90)
        self.assertEqual(rows[0]["source_start_frame"],65)
        self.assertEqual(rows[0]["start_evidence"],"detected_serve")
        empty=rebuild([track(104,240)],events=[{"type":"serve","frame":90,"confidence":.2}],
                    duration_frames=300)
        self.assertIsNone(empty[0]["serve_frame"])
        self.assertEqual(empty[0]["start_evidence"],"first_rally_ball_track")

    def test_weak_nonrally_between_does_not_sever_rally(self):
        weak=track(201,215,"not_rally",tid=55)
        weak["rally_classification"].update(
            not_rally_reason="technical_return",technical_return_confidence=.15)
        groups=rebuild([track(100,200,tid=1),weak,track(216,300,tid=2)],
                       duration_frames=400)
        self.assertEqual(len(groups),1)

    def test_montage_exports_group_payload_not_individual_tracks(self):
        with tempfile.TemporaryDirectory() as tmp:
            d=Path(tmp)
            cache=d/"cache"
            folder=cache/"tracks/analysis/tracks"
            folder.mkdir(parents=True)
            for tid,a,b in [(1,10,20),(2,23,35)]:
                path=folder/("track_"+str(tid).zfill(4)+".json")
                path.write_text(json.dumps({
                    "track_id":tid,"start_frame":a,"last_frame":b,
                    "positions":[[[10+i,20+i],i] for i in range(a,b+1)],
                    "tracking_scale":{"frame_width_px":1080,"frame_height_px":1920}
                }),encoding="utf-8")
            group={"track_ids":[1,2],"source_start_frame":8,"source_end_frame":40,
                   "detected_start_frame":10,"detected_end_frame":35,
                   "boundary_method":"reconstructed_ball_tracks; serve/point not verified"}
            cfg={"reels_limit":10,"reels_export":True,"reels_smoothing":"moving_avg",
                 "reels_interpolation":"linear","reels_margin":0,"reels_padding":.33,
                 "reels_zoom_max":1.4,"reels_smooth_window":15,
                 "reels_smooth_polyorder":2,"reels_fade_transition":False}
            calls=[]
            with patch("scripts.build_project_highlights.subprocess.run",
                       side_effect=lambda cmd,**kwargs:calls.append(cmd)):
                export_vertical_reels([Path("match.mp4")],{"match.mp4":cache},
                    {"match.mp4":[group]},d,cfg)
            self.assertEqual(len(calls),1)
            cmd=calls[0]
            self.assertNotIn("--zoom-max",cmd)
            self.assertEqual(cmd[cmd.index("--padding")+1],"0")
            self.assertEqual(cmd[cmd.index("--interpolation")+1],"linear")
            path=Path(cmd[cmd.index("--track_jsons")+1])
            payload=json.loads(path.read_text(encoding="utf-8"))
            self.assertEqual(payload["track_ids"],[1,2])
            self.assertEqual(len(payload["positions"]),24)
            self.assertEqual((payload["start_frame"],payload["last_frame"]),(8,40))


class OriginalCameraTests(unittest.TestCase):
    def test_interpolation_uses_original_horizontal_camera(self):
        points = {0: (100, 200), 2: (200, 400)}
        np.testing.assert_allclose(
            interpolate_positions(points, 0, 2, "hold"), [100, 100, 200])
        np.testing.assert_allclose(
            interpolate_positions(points, 0, 2, "linear"), [100, 150, 200])

    def test_original_crop_keeps_full_height_without_zoom(self):
        frame=np.zeros((160,300,3),dtype=np.uint8)
        frame[50:60,260:280]=255
        cropped=crop_frame(frame,270,90)
        self.assertEqual(cropped.shape,(160,90,3))
        self.assertEqual(cropped[55].max(),255)
        edge=crop_frame(frame,-50,90)
        self.assertEqual(edge.shape,(160,90,3))
        self.assertEqual(edge.max(),0)

    def test_original_portrait_video_stays_9_16_without_dynamic_zoom(self):
        import cv2
        with tempfile.TemporaryDirectory() as td:
            path=Path(td)
            source=str(path/"source.mp4")
            output=str(path/"original.mp4")
            writer=cv2.VideoWriter(source,cv2.VideoWriter_fourcc(*"mp4v"),30,(90,160))
            for i in range(50):
                frame=np.zeros((160,90,3),dtype=np.uint8)
                cv2.circle(frame,(20+i//3,30+i),(4),(250,250,250),-1)
                writer.write(frame)
            writer.release()
            payload={"start_frame":0,"last_frame":49,
                     "positions":[[[20+i//3,30+i],i] for i in range(50)],
                     "tracking_scale":{"frame_width_px":90,"frame_height_px":160}}
            crop_and_save_track_payload(
                source,payload,output,smoothing="moving_avg",padding=0)
            cap=cv2.VideoCapture(output)
            self.assertTrue(cap.isOpened())
            self.assertEqual(round(cap.get(cv2.CAP_PROP_FRAME_WIDTH)),90)
            self.assertEqual(round(cap.get(cv2.CAP_PROP_FRAME_HEIGHT)),160)
            self.assertGreaterEqual(cap.get(cv2.CAP_PROP_FRAME_COUNT),45)
            cap.release()


if __name__=="__main__":
    unittest.main()
