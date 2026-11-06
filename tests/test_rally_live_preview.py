"""Preview rally boundary controls rebuild metadata, never replay the model or video."""
from __future__ import annotations

from http.server import ThreadingHTTPServer
import json
from pathlib import Path
import tempfile
import threading
import unittest
import urllib.error
import urllib.parse
import urllib.request
from unittest.mock import patch

from scripts.volleyball_ui.server import Handler


class LiveRallyBoundaryApiTests(unittest.TestCase):
    def setUp(self):
        temp=tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.cache=Path(temp.name)
        analysis=self.cache/"analysis"
        analysis.mkdir()
        (self.cache/"analysis.mp4").write_bytes(b"fake-not-decoded")
        self.events=analysis/"events.json"
        self.events.write_text(json.dumps({
            "schema":"volleyball-events/2","fps":30,
            "analysis_offset_sec":0,
            "coordinate_space":{"width":320,"height":180},
            "events":[]
        }),encoding="utf8")
        self.tracks=analysis/"game-tracks.json"
        self.tracks.write_text(json.dumps([
            {"track_id":1,"start_frame":0,"last_frame":60,
             "rally_classification":{"label":"rally","rally_confidence":.9}},
            {"track_id":2,"start_frame":80,"last_frame":140,
             "rally_classification":{"label":"rally","rally_confidence":.8}},
        ]),encoding="utf8")
        self.server=ThreadingHTTPServer(("127.0.0.1",0),Handler)
        worker=threading.Thread(target=self.server.serve_forever,daemon=True)
        worker.start()
        self.addCleanup(worker.join,3)
        self.addCleanup(self.server.server_close)
        self.addCleanup(self.server.shutdown)
        for target,return_value in [
            ("scripts.volleyball_ui.server.assert_project_video",None),
            ("scripts.volleyball_ui.analysis_runs.cache_for",self.cache),
            ("scripts.volleyball_ui.server._normalized_duration",8.0),
            ("scripts.volleyball_ui.timeline.data.video_path",self.cache/"video.mp4"),
            ("scripts.volleyball_ui.data.groups_for",[]),
        ]:
            thing=patch(target,return_value=return_value)
            thing.start();self.addCleanup(thing.stop)
        occupancy=patch("scripts.volleyball_ui.rally_players.player_info",
                        side_effect=lambda cache,ranges,**k:[
                            {"player_count":3,"player_track_ids":[]}
                            for _ in ranges])
        occupancy.start();self.addCleanup(occupancy.stop)

    def query(self, **values):
        arguments={
            "project_id":"project","video":"video.mp4",
            "cache_id":"cpu-1","gap_sec":0,"min_sec":1,"context_sec":0
        }
        arguments.update(values)
        url=("http://127.0.0.1:"+str(self.server.server_port)+
             "/api/analysis-rallies?"+urllib.parse.urlencode(arguments))
        with urllib.request.urlopen(url,timeout=7) as response:
            return json.load(response)

    def test_gap_joins_saved_tracks_without_updating_cpu_cache(self):
        before=self.events.read_bytes()
        first=self.query(gap_sec=0)
        second=self.query(gap_sec=1)
        self.assertEqual(len(first["rallies"]),2)
        self.assertEqual(len(second["rallies"]),1)
        self.assertTrue(second["reconstruction_available"])
        self.assertTrue(second["preview_only"])
        self.assertNotIn("media_url",second)
        self.assertEqual(self.events.read_bytes(),before)
        self.assertEqual(second["rally_summary"]["settings"]["gap_sec"],1)

    def test_context_changes_bounds_but_not_cpu_classification(self):
        unpadded=self.query(gap_sec=0,context_sec=0)
        padded=self.query(gap_sec=0,context_sec=3)
        self.assertEqual(len(unpadded["rallies"]),len(padded["rallies"]))
        self.assertNotEqual(unpadded["rallies"][0]["end"],
                            padded["rallies"][0]["end"])
        self.assertEqual(unpadded["rally_summary"]["classified_tracks"],
                         padded["rally_summary"]["classified_tracks"])

    def test_legacy_without_tracks_explicitly_reports_no_reconstruction(self):
        self.tracks.unlink()
        result=self.query(gap_sec=8)
        self.assertFalse(result["reconstruction_available"])
        self.assertEqual(result["rallies"],[])

    def test_requires_completed_cpu_and_valid_settings(self):
        def url(values):
            arguments={
                "project_id":"project","video":"video.mp4",
                "cache_id":"cpu-1",**values}
            return ("http://127.0.0.1:"+str(self.server.server_port)+
                   "/api/analysis-rallies?"+urllib.parse.urlencode(arguments))
        with self.assertRaises(urllib.error.HTTPError) as bad:
            urllib.request.urlopen(url({"gap_sec":50}),timeout=5)
        self.assertEqual(bad.exception.code,400)
        bad.exception.close()
        self.events.unlink()
        with self.assertRaises(urllib.error.HTTPError) as missing:
            urllib.request.urlopen(url({}),timeout=5)
        self.assertEqual(missing.exception.code,409)
        missing.exception.close()


if __name__=="__main__":
    unittest.main()
