"""Regression: raw VballNet proposals remain re-filterable without inference."""
import csv
import json
import tempfile
import sys
import unittest
from pathlib import Path
from unittest.mock import patch

import numpy as np

from scripts.volleyball_ui import trajectory
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/"src"))
from inference_onnx_seq_gray_v2 import raw_ball_candidates


class RawBallCandidateTests(unittest.TestCase):
    def test_heatmap_preserves_subthreshold_candidates(self):
        output=np.zeros((1,9,8,10),dtype=np.float32)
        for frame,score in enumerate((.93,.12,.49,.52,.33,.77,.01,.64,.44)):
            output[0,frame,frame%8,frame%10]=score
        model={"family":"heatmap","seq":9,"planes":1}
        rows=raw_ball_candidates(output,model)
        self.assertEqual(len(rows),9)
        self.assertAlmostEqual(rows[1][0],.12,places=5)
        self.assertEqual(rows[1][1:3],(1.,1.))
        self.assertIsNone(rows[1][3])
        self.assertEqual(sum(score>=.5 for score,*_ in rows),4)

    def test_grid_preserves_lower_confidence(self):
        output=np.zeros((1,9*3*2*2),dtype=np.float32)
        model={"family":"grid","seq":9,"planes":3,"grid_rows":2,
               "grid_cols":2,"input_width":20,"input_height":20}
        grid=output.reshape(9,3,2,2)
        grid[0,0,1,1]=.16
        grid[0,1,1,1]=.5
        grid[0,2,1,1]=.25
        score,x,y,radius=raw_ball_candidates(output,model)[0]
        self.assertIsNone(radius)
        self.assertAlmostEqual(score,.16,places=5)
        self.assertEqual((x,y),(15.,12.5))


class ThresholdPreviewTests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.cache=Path(self.tmp.name)
        (self.cache/"ball/analysis").mkdir(parents=True)
        (self.cache/"analysis").mkdir()
        (self.cache/"analysis/events.json").write_text(json.dumps({
            "fps":30,"coordinate_space":{"width":1000,"height":800}
        }),encoding="utf-8")
        with (self.cache/"ball/analysis/ball.csv").open("w",newline="") as f:
            writer=csv.writer(f)
            writer.writerow(["Frame","Visibility","X","Y","Radius"])
            writer.writerows([[0,1,20,40,3],[1,0,-1,-1,0],[2,1,60,80,3]])
        self.raw=self.cache/"ball/analysis/ball.raw.csv"
        with self.raw.open("w",newline="") as f:
            writer=csv.writer(f)
            writer.writerow(["Frame","Confidence","X","Y","Radius"])
            writer.writerows([[0,.95,20,40,3],[1,.25,30,50,3],[2,.65,60,80,3]])
        self.meta=self.cache/"ball/analysis/ball.raw.json"
        self.meta.write_text(json.dumps({
            "format":"ball-raw-peak-v2","analysis_threshold":.5
        }),encoding="utf-8")
        pa=patch.object(trajectory,"assert_project_video",return_value=True)
        pb=patch.object(trajectory,"find_cache",return_value=self.cache)
        pa.start();pb.start()
        self.addCleanup(pa.stop);self.addCleanup(pb.stop)
        trajectory._load_raw_points.cache_clear()
        trajectory._load_points.cache_clear()

    def test_threshold_reuses_raw_candidates(self):
        high=trajectory.trajectory_window("proj","video.mp4",0,1,threshold=.8)
        low=trajectory.trajectory_window("proj","video.mp4",0,1,threshold=.2)
        default=trajectory.trajectory_window("proj","video.mp4",0,1)
        self.assertEqual([p[0] for p in high["points"]],[0])
        self.assertEqual([p[0] for p in low["points"]],[0,1,2])
        self.assertEqual([p[0] for p in default["points"]],[0,2])
        self.assertEqual(low["detections_total"],3)
        self.assertTrue(high["supports_threshold"])
        self.assertEqual(high["analysis_threshold"],.5)

    def test_legacy_does_not_fabricate_raw_scores(self):
        self.raw.unlink()
        self.assertFalse(trajectory.ball_detection_capability(self.cache)["available"])
        response=trajectory.trajectory_window("proj","video.mp4",0,1,threshold=.4)
        self.assertFalse(response["available"])
        self.assertEqual(response["reason"],"no_raw_candidates")
        self.assertTrue(trajectory.trajectory_window("proj","video.mp4",0,1)["available"])

    def test_invalid_threshold_rejected(self):
        for value in (-.1,1.5,float("nan"),"bad"):
            with self.subTest(value=value),self.assertRaises(ValueError):
                trajectory.trajectory_window("proj","video.mp4",0,1,threshold=value)


if __name__=="__main__":
    unittest.main()
