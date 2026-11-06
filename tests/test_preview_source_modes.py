"""Preview separates saved neural predictions from CPU-derived rally events."""
from __future__ import annotations

from http.server import ThreadingHTTPServer
import json
from pathlib import Path
import tempfile
import threading
import unittest
import urllib.parse
import urllib.request
from unittest.mock import patch

from scripts.volleyball_ui import model_touches, ravel_ball, trajectory
from scripts.volleyball_ui.server import Handler

ROOT=Path(__file__).resolve().parents[1]


class PreviewSourceModesTests(unittest.TestCase):
    def setUp(self):
        tmp=tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.cache=Path(tmp.name)
        (self.cache/"ball/analysis").mkdir(parents=True)
        (self.cache/"analysis.mp4").write_bytes(b"synthetic")
        (self.cache/"inference.manifest.json").write_text(json.dumps({
            "schema":"volley-inference/1","fps":30,"width":100,
            "height":200,"ball_model":"VballNetV4c","ball_threshold":.5,
        }),encoding="utf8")
        (self.cache/"ball/analysis/ball.csv").write_text(
            "Frame,Visibility,X,Y,Radius\n"
            "0,1,11,20,3\n5,1,12,21,4\n",encoding="utf8")
        (self.cache/"ball/analysis/ball.raw.csv").write_text(
            "Frame,Confidence,X,Y\n"
            "0,0.6,11,20\n1,0.3,20,21\n5,0.9,12,21\n",encoding="utf8")
        (self.cache/"ball/analysis/ball.raw.json").write_text(json.dumps({
            "format":"ball-raw-peak-v2","analysis_threshold":.5,
        }),encoding="utf8")
        (self.cache/"players.json").write_text(json.dumps({
            "model":"RAVEL-VB-013-9f","fps":30,
            "source_size":{"width":100,"height":200},
            "action_threshold":.7,"ball_threshold":.35,
            "predictions":[
                {"class_name":"ball","frame_index":0,"score":.6,
                 "bbox_xyxy":[40,80,50,100]},
                {"class_name":"ball","frame_index":1,"score":.22,
                 "bbox_xyxy":[38,82,48,101]},
                {"class_name":"ball","frame_index":5,"score":.9,
                 "bbox_xyxy":[45,85,49,95]},
                {"class_name":"ball","frame_index":6,"score":.9,
                 "bbox_xyxy":[45,85,49,95],"interpolated":True},
                {"class_name":"player","frame_index":0,"track_id":23,
                 "score":.9,"action":"Touch","action_score":.82,
                 "bbox_xyxy":[10,20,30,80]},
            ]
        }),encoding="utf8")
        self.assertFalse((self.cache/"analysis/events.json").exists())

    def test_gpu_only_vballnet_points_and_scores_without_cpu_events(self):
        with patch.object(trajectory,"assert_project_video"), patch(
            "scripts.volleyball_ui.analysis_runs.cache_for",
            return_value=self.cache):
            result=trajectory.trajectory_window(
                "project","video.mp4",0,1,cache_id="cache",
                threshold=.5,include_scores=True)
            self.assertTrue(result["available"])
            self.assertEqual(result["fps"],30)
            self.assertEqual((result["width"],result["height"]),(100,200))
            self.assertEqual(result["points"],[[0,11,20,.6],[5,12,21,.9]])
            stricter=trajectory.trajectory_window(
                "project","video.mp4",0,1,cache_id="cache",
                threshold=.85,include_scores=True)
            self.assertEqual(stricter["points"],[[5,12,21,.9]])
            old=trajectory.trajectory_window("project","video.mp4",0,1,
                                              cache_id="cache")
            self.assertEqual(old["points"],[[0,11,20],[5,12,21]])
            self.assertEqual(trajectory.ball_detection_capability(self.cache)
                             ["analysis_threshold"],.5)

    def test_ravel_ball_is_separate_and_original_floor_is_preserved(self):
        with patch.object(ravel_ball,"assert_project_video"),patch.object(
                ravel_ball,"cache_for",return_value=self.cache):
            first=ravel_ball.ravel_ball_window("project","video.mp4",0,1,
                                                cache_id="original")
            self.assertTrue(first["available"])
            self.assertEqual(first["source"],"RAVEL-013")
            self.assertEqual(first["stored_threshold"],.35)
            self.assertEqual([r[0] for r in first["frames"]],[0,5])
            self.assertEqual(first["frames"][0][1][0],[45,90,.6])
            second=ravel_ball.ravel_ball_window("project","video.mp4",0,1,
                                                 cache_id="original",threshold=.8)
            self.assertEqual([r[0] for r in second["frames"]],[5])
            with self.assertRaises(ValueError):
                ravel_ball.ravel_ball_window(
                    "project","video.mp4",-1,1,cache_id="original")

    def test_model_touches_are_raw_predictions_not_cpu_contacts(self):
        with patch.object(model_touches,"assert_project_video"), patch.object(
                model_touches,"cache_for",return_value=self.cache):
            result=model_touches.touch_predictions(
                "project","video.mp4","original",threshold=.5)
            self.assertTrue(result["available"])
            self.assertEqual(result["count"],1)
            self.assertEqual(result["stored_threshold"],.7)
            self.assertEqual(result["threshold"],.7)
            no_touch=model_touches.touch_predictions(
                "project","video.mp4","original",threshold=.9)
            self.assertEqual(no_touch["count"],0)

    def test_http_ravel_ball_rejects_invalid_window_and_serves_neural_output(self):
        http=ThreadingHTTPServer(("127.0.0.1",0),Handler)
        thread=threading.Thread(target=http.serve_forever,daemon=True)
        thread.start()
        self.addCleanup(thread.join,3)
        self.addCleanup(http.server_close)
        self.addCleanup(http.shutdown)
        def url(start):
            q=urllib.parse.urlencode({"project_id":"p","video":"v.mp4",
                  "cache_id":"original","start":start,"length":1})
            return f"http://127.0.0.1:{http.server_port}/api/analysis-ravel-ball?{q}"
        with patch.object(ravel_ball,"assert_project_video"),patch.object(
                ravel_ball,"cache_for",return_value=self.cache):
            with urllib.request.urlopen(url(0),timeout=5) as response:
                payload=json.load(response)
                self.assertEqual(response.status,200)
                self.assertEqual(payload["source"],"RAVEL-013")
                self.assertEqual(len(payload["frames"]),2)
            with self.assertRaises(urllib.error.HTTPError) as error:
                urllib.request.urlopen(url(-3),timeout=5)
            self.assertEqual(error.exception.code,400)
            error.exception.close()

    def test_html_has_explicit_modes_and_provenance_controls(self):
        html=(ROOT/"scripts/volleyball_ui/static/index.html").read_text("utf8")
        js=(ROOT/"scripts/volleyball_ui/static/results-ui.js").read_text("utf8")
        css=(ROOT/"scripts/volleyball_ui/static/result-media.css").read_text("utf8")
        self.assertEqual(html.count('name="result-preview-mode"'),2)
        self.assertIn('value="model"',html)
        self.assertIn('value="events" checked',html)
        for kind in ("vballnet","ravelBall","players","touch"):
            self.assertEqual(html.count('data-result-raw="'+kind+'"'),1)
        self.assertIn('id="result-raw-controls"',html)
        self.assertEqual(html.count('name="result-raw-source"'),2)
        self.assertIn('data-result-raw-group="vballnet"',html)
        self.assertIn('data-result-raw-group="ravel" hidden',html)
        self.assertIn('const rawEnabled=kind=>',js)
        self.assertIn('if(!rawEnabled(kind)||!geometry)continue;',js)
        self.assertIn('if(rawEnabled("players"))requestPlayerBoxes(elapsed);',js)
        self.assertIn('modelTouches.setTouchVisible?.(raw&&rawEnabled("touch"))',js)
        self.assertIn('[data-result-raw-group][hidden]',css)
        self.assertIn('id="result-raw-vball"',html)
        self.assertIn('id="result-raw-ravel"',html)
        self.assertIn("paintRawBalls(elapsed,geometry)",js)
        for element in ("result-raw-vball-threshold",
                        "result-raw-ravel-threshold",
                        "result-raw-player-threshold",
                        "result-raw-trail-seconds",
                        "result-raw-ball-trail"):
            self.assertIn('id="'+element+'"',html)
        self.assertIn('query.set("threshold",String(rawThresholds.ravelBall))',js)
        self.assertIn('query.set("threshold",String(rawThresholds.vballnet))',js)
        self.assertIn('if(inRawMode()&&score<rawThresholds.players)continue',js)
        self.assertIn('drawRawBallTrail(kind,win,time,geometry,points)',js)
        self.assertIn('result-raw-ball-trail[data-source="ravelBall"]',css)
        # GPU overlay controls must not inherit the CPU-only hidden selector.
        style=(ROOT/"scripts/volleyball_ui/static/style.css").read_text("utf8")
        self.assertNotIn(".raw-touch-mode .result-overlay-controls,",style)
        self.assertIn(".raw-touch-mode .result-cpu-overlays,",style)
        self.assertIn('id="result-raw-source-switch"',html)
        self.assertIn('.result-raw-source-switch[hidden]',css)
        self.assertIn('E("result-raw-source-switch").hidden=!raw',js)
        self.assertNotIn('id="result-raw-trail"',html)
        self.assertIn('const rawTrail={seconds:2}',js)
        self.assertIn('if(!rawEnabled(kind)||!win?.available||!geometry)return',js)
        self.assertIn("cache_id:rawCacheId()",js)
        self.assertIn('const endpoint=kind==="ravelBall"',js)
        self.assertIn("CPU · ",js)
        self.assertIn(".raw-touch-mode .result-cpu-tools",css)
        self.assertIn(".raw-touch-mode .result-rally-sidebar",css)


if __name__ == "__main__":
    unittest.main()
