import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui.rally_view import from_tracks, settings
from scripts.volleyball_ui.timeline import timeline


class RallyReconstructionTests(unittest.TestCase):
    def test_filters_non_rally_and_joins_short_tracking_dropouts(self):
        with tempfile.TemporaryDirectory() as td:
            folder=Path(td)
            (folder / "analysis").mkdir()
            tracks=[
                {"start_frame":0,"last_frame":20,"rally_classification":{"label":"not_rally"}},
                {"start_frame":60,"last_frame":135,"rally_classification":{"label":"rally","rally_confidence":.8}},
                {"start_frame":170,"last_frame":240,"rally_classification":{"label":"rally","rally_confidence":.7}},
                {"start_frame":480,"last_frame":580,"rally_classification":{"label":"not_rally"}},
                {"start_frame":610,"last_frame":650,"rally_classification":{"label":"rally"}},
            ]
            (folder / "analysis/game-tracks.json").write_text(json.dumps(tracks))
            result=from_tracks(folder,120,30,40,gap_sec=2,min_sec=3,context_sec=1)
            self.assertEqual(result["raw_tracks"],5)
            self.assertEqual(result["classified_tracks"],3)
            self.assertEqual(len(result["rallies"]),1)
            self.assertEqual(result["rallies"][0]["joined_parts"],2)
            self.assertAlmostEqual(result["rallies"][0]["start"],121,places=2)
            self.assertAlmostEqual(result["rallies"][0]["end"],129.03,places=2)

    def test_technical_return_blocks_merge(self):
        with tempfile.TemporaryDirectory() as td:
            folder=Path(td)
            (folder / "analysis").mkdir()
            tracks=[
                {"start_frame":0,"last_frame":95,"rally_classification":{"label":"rally"}},
                {"start_frame":100,"last_frame":120,"rally_classification":{"label":"not_rally","not_rally_reason":"technical_return"}},
                {"start_frame":125,"last_frame":230,"rally_classification":{"label":"rally"}},
            ]
            (folder / "analysis/game-tracks.json").write_text(json.dumps(tracks))
            result=from_tracks(folder,0,30,20,gap_sec=5,min_sec=1,context_sec=1)
            self.assertEqual(len(result["rallies"]),2)

    def test_bounds_rejected(self):
        for bad in ({"gap":-1},{"gap":14},{"minimum":-1},{"context":11},{"gap":float("nan")}):
            with self.assertRaises(ValueError):
                settings(**bad)

    def test_future_events_do_not_label_non_rally_tracks_as_points(self):
        from scripts.volleyball_studio.events import event_timeline
        def track(begin, end, label):
            return {"start_frame":begin,"last_frame":end,
                    "rally_classification":{"label":label,"rally_confidence":.8},
                    "player_interaction":{"contacts":[]}}
        payload=event_timeline([track(0,45,"not_rally"),track(60,180,"rally")],
                               "video.mp4",0,1080,1920,True)
        starts=[e for e in payload["events"] if e["type"]=="rally_start"]
        ends=[e for e in payload["events"] if e["type"]=="rally_end"]
        self.assertEqual([e["frame"] for e in starts],[60])
        self.assertEqual([e["frame"] for e in ends],[180])

    def test_marker_uses_zoom_and_clips_to_picture(self):
        root=Path(__file__).resolve().parents[1]
        js=(root/"scripts/volleyball_ui/static/results-ui.js").read_text(encoding="utf-8")
        html=(root/"scripts/volleyball_ui/static/index.html").read_text(encoding="utf-8")
        css=(root/"scripts/volleyball_ui/static/result-media.css").read_text(encoding="utf-8")
        self.assertIn('id="result-ball-layer"',html)
        self.assertIn("const zoomX=shell.width/Math.max(1,overlay.offsetWidth)",js)
        self.assertIn("zoomY=shell.height/Math.max(1,overlay.offsetHeight)",js)
        self.assertIn("layer.style.left=",js)
        self.assertIn("overflow:hidden;",css)
        self.assertIn("Number(data.coordinate_space?.width)",js)

    def test_schema_v2_absolute_time_not_shifted_twice(self):
        with tempfile.TemporaryDirectory() as td:
            folder=Path(td)
            (folder/"analysis").mkdir()
            payload={"schema":"volleyball-events/2","source":"video.mp4","fps":30,
                     "analysis_offset_sec":120,"coordinate_space":{"width":1080,"height":1920},
                     "events":[
                       {"type":"rally_start","track_id":"t1","time_sec":126,"frame":180},
                       {"type":"attack","time_sec":127,"frame":210,"ball":{"x":540,"y":960}},
                       {"type":"rally_end","track_id":"t1","time_sec":133,"frame":390}
                     ]}
            (folder/"analysis/events.json").write_text(json.dumps(payload))
            with patch("scripts.volleyball_ui.timeline.data.video_path",return_value=folder/"video.mp4"),\
                 patch("scripts.volleyball_ui.timeline.data.find_cache",return_value=folder):
                value=timeline("video.mp4",duration_sec=20)
            self.assertEqual(value["actions"][0]["time"],127)
            self.assertEqual(value["rallies"][0]["start"],126)
            self.assertEqual(value["coordinate_space"],{"width":1080,"height":1920})


if __name__ == "__main__":
    unittest.main()
