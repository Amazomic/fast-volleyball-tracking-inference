"""Regression: 2026-11-04 upstream Touch settings and scoped diagnostic MP4."""
from __future__ import annotations

import json
import tempfile
import threading
import unittest
import urllib.error
import urllib.request
from http.server import ThreadingHTTPServer
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_studio.advanced import SPECS, as_cli, validated
from scripts.volleyball_studio.__main__ import parser
from scripts.volleyball_ui.analysis_profiles import _clean_values, profile_schema
from scripts.volleyball_ui import server


class UpstreamTouchSettingsTests(unittest.TestCase):
    def test_new_touch_settings_are_validated_and_forwarded(self):
        options={
            "player_model":"RAVEL-VB-013-9f",
            "player_backend":"onnx-cuda",
            "player_device":"GPU",
            "player_action_threshold":.7,
            "player_frame_step":1,
            "player_touch_smooth":5,
            "player_touch_min_gap":10,
            "player_touch_hold":6,
            "player_strip_seconds":4.0,
            "player_reader":"ffmpeg",
            "player_decode_hwaccel":"none",
            "player_decode_threads":0,
            "player_output_video":True,
        }
        cleaned=_clean_values(options)
        self.assertEqual(cleaned,options)
        parsed=parser().parse_args(as_cli(options))
        config=validated(vars(parsed))
        for name,value in options.items():
            self.assertEqual(config[name],value,name)
            self.assertIn(name,profile_schema())
            self.assertIn(name,SPECS)
        runner=(Path(__file__).resolve().parents[1]/
                "scripts/volleyball_studio/analysis.py").read_text(encoding="utf-8")
        for option in ("--touch-smooth", "--touch-min-gap", "--touch-hold",
                       "--strip-seconds", "--reader", "--decode-hwaccel",
                       "--decode-threads"):
            with self.subTest(flag=option):
                self.assertIn('"'+option+'"',runner)

    def test_invalid_upstream_options_rejected(self):
        for name,value in (("player_touch_smooth",0),
                           ("player_touch_min_gap",-1),
                           ("player_touch_hold",-1),
                           ("player_strip_seconds",100),
                           ("player_reader","unknown"),
                           ("player_decode_hwaccel","malicious"),
                           ("player_decode_threads",500)):
            with self.subTest(name=name,value=value):
                with self.assertRaises(ValueError):
                    _clean_values({name:value})


class DiagnosticMediaApiTests(unittest.TestCase):
    def setUp(self):
        tmp=tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root=Path(tmp.name)
        self.cache=self.root/"isolated-cache-a"
        self.cache.mkdir()
        (self.cache/"analysis.mp4").write_bytes(b"regular-preview")
        (self.cache/"analysis").mkdir()
        (self.cache/"analysis/events.json").write_text(
            json.dumps({"source": "test.mp4", "events": []}), encoding="utf-8")
        (self.cache/"players-annotated.mp4").write_bytes(b"ravel-touch-markers")
        patches=[
            patch.object(server,"assert_project_video",return_value=None),
            patch("scripts.volleyball_ui.analysis_runs.cache_for",
                  side_effect=lambda name,project,cache_id:
                  self.cache if (name=="test.mp4" and
                                 project=="project-1" and
                                 cache_id=="isolated-cache-a") else None),
            patch.object(server,"_normalized_duration",return_value=3.0),
            patch.object(server,"timeline",
                         return_value={"analyzed":True,"rallies":[],"actions":[]}),
            patch("scripts.volleyball_ui.trajectory.ball_detection_capability",
                  return_value={}),
            patch("scripts.volleyball_ui.player_overlay.player_detection_capability",
                  return_value={}),
        ]
        for stub in patches:
            stub.start()
            self.addCleanup(stub.stop)
        http=ThreadingHTTPServer(("127.0.0.1",0),server.Handler)
        self.addCleanup(http.server_close)
        thread=threading.Thread(target=http.serve_forever,daemon=True)
        thread.start()
        self.addCleanup(http.shutdown)
        self.url="http://127.0.0.1:"+str(http.server_address[1])
        self.args="?project_id=project-1&video=test.mp4&cache_id=isolated-cache-a"

    def test_selected_analysis_exposes_annotated_touch_mp4(self):
        with urllib.request.urlopen(self.url+"/api/analysis-view"+self.args) as stream:
            body=json.loads(stream.read())
        self.assertTrue(body["analyzed"])
        self.assertTrue(body["upstream_touch_video_url"].endswith(self.args))
        with urllib.request.urlopen(self.url+body["upstream_touch_video_url"]) as stream:
            self.assertEqual(stream.read(),b"ravel-touch-markers")
            self.assertEqual(stream.headers.get_content_type(),"video/mp4")

    def test_wrong_cache_has_no_access_to_other_branch(self):
        bad="?project_id=project-1&video=test.mp4&cache_id=different-run"
        with self.assertRaises(urllib.error.HTTPError) as ctx:
            urllib.request.urlopen(self.url+"/api/analysis-upstream-touch-media"+bad)
        self.assertEqual(ctx.exception.code,404)

    def test_no_link_when_diagnostic_export_disabled(self):
        (self.cache/"players-annotated.mp4").unlink()
        with urllib.request.urlopen(self.url+"/api/analysis-view"+self.args) as stream:
            body=json.loads(stream.read())
        self.assertNotIn("upstream_touch_video_url",body)


if __name__=="__main__":
    unittest.main()
