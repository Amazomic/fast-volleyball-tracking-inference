"""Preview uses only saved RAVEL-013 Touch predictions, not heuristic events."""
from pathlib import Path
import json
import tempfile
import unittest
from unittest.mock import patch

from scripts.volleyball_ui.model_touches import _touch_index, touch_predictions

ROOT=Path(__file__).resolve().parents[1]


class RavelTouchPreviewTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.cache=Path(self.temp.name)
        self.path=self.cache/"players.json"
        self.base={
          "model":"/models/RAVEL-VB-013-9f.onnx",
          "fps":30,"source_size":{"width":640,"height":360},
          "action_threshold":0.5,
          "predictions":[
            {"class_name":"player","frame_index":20,"track_id":7,
             "action":"Touch","action_score":0.81,"interpolated":False,
             "bbox_xyxy":[100,20,180,230]},
            {"class_name":"player","frame_index":120,"track_id":8,
             "action":"Touch","action_score":0.62,"interpolated":False,
             "bbox_xyxy":[200,30,280,250]},
            {"class_name":"player","frame_index":121,"track_id":8,
             "action":"Touch","action_score":0.98,"interpolated":True,
             "bbox_xyxy":[200,30,280,250]},
            {"class_name":"ball","frame_index":130,"action":"Touch","action_score":0.99,
             "bbox_xyxy":[1,1,2,2]},
            {"class_name":"player","frame_index":200,"track_id":8,
             "action":"none","action_score":0.1,"bbox_xyxy":[1,1,100,100]}
          ]}
        _touch_index.cache_clear()

    def query(self, **kwargs):
        with patch("scripts.volleyball_ui.model_touches.assert_project_video"), patch(
            "scripts.volleyball_ui.model_touches.cache_for",return_value=self.cache):
            return touch_predictions("pid","video.mp4","hash",**kwargs)

    def test_raw_touch_frames_not_filtered_to_rallies_or_heuristics(self):
        self.path.write_text(json.dumps(self.base),encoding="utf8")
        # No events.json is present. Model events still appear.
        summary=self.query()
        self.assertTrue(summary["available"])
        self.assertEqual(summary["count"],2)
        self.assertEqual(sum(summary["bins"]),2)
        first=self.query(start=0,length=2)
        self.assertEqual(first["window_total"],1)
        self.assertEqual(first["events"][0]["player_id"],7)
        self.assertAlmostEqual(first["events"][0]["time"],20/30,places=3)
        late=self.query(start=3,length=2)
        self.assertEqual(late["window_total"],1)
        self.assertEqual(late["events"][0]["frame"],120)

    def test_threshold_is_applied_without_recomputing_events(self):
        self.path.write_text(json.dumps(self.base),encoding="utf8")
        selected=self.query(start=0,length=8,threshold=0.8)
        self.assertEqual(selected["count"],1)
        self.assertEqual(selected["window_total"],1)
        self.assertEqual(selected["events"][0]["score"],0.81)
        # Predictions below the original .5 cutoff weren't saved.
        self.assertEqual(self.query(threshold=0)["threshold"],.5)

    def test_older_model_does_not_fake_touches(self):
        self.base["model"]="/models/RAVEL-VB-012-9f.onnx"
        self.path.write_text(json.dumps(self.base),encoding="utf8")
        self.assertEqual(self.query()["reason"],"different_model")

    def test_bad_numeric_parameters_rejected(self):
        self.path.write_text(json.dumps(self.base),encoding="utf8")
        for opts in ({"threshold":1.1},{"start":-5},{"length":30},{"threshold":"NaN"}):
            with self.subTest(opts=opts),self.assertRaises(ValueError):
                self.query(**opts)

    def test_ui_is_isolated_from_rally_events(self):
        html=(ROOT/"scripts/volleyball_ui/static/index.html").read_text(encoding="utf8")
        ui=(ROOT/"scripts/volleyball_ui/static/model-touches-ui.js").read_text(encoding="utf8")
        renderer=(ROOT/"scripts/volleyball_ui/static/results-ui.js").read_text(encoding="utf8")
        self.assertIn('value="model"',html)
        self.assertIn('id="result-model-touch-histogram"',html)
        self.assertIn('"/api/analysis-model-touches?"',ui)
        self.assertNotIn('"/api/analysis-view?"',ui)
        self.assertIn('modelTouches.active()',renderer)
        self.assertIn('modelTouches.setSource(',renderer)


if __name__=="__main__":
    unittest.main()
