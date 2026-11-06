"""End-to-end regression for independent neural inference, CPU runs and identities."""
from __future__ import annotations

import io
import json
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import patch

from scripts import run_project_analysis
from scripts.volleyball_studio.player_continuity import group_tracklets
from scripts.volleyball_ui import data, projects
from scripts.volleyball_ui.analysis_runs import cache_for, list_runs


class IndependentStagesTest(unittest.TestCase):
    def setUp(self):
        temp=tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        base=Path(temp.name)
        self.video_root=base/"source"
        self.video_root.mkdir()
        self.source=self.video_root/"match.mp4"
        self.source.write_bytes(b"fake-test-source")
        self.results=base/"results"
        self.results.mkdir()
        for mod, attr, value in (
            (data,"PROJECT",self.video_root),
            (data,"RESULTS",self.results),
            (projects,"PROJECTS_DIR",self.results/"projects"),
            (projects,"REGISTER",self.results/"projects"/"projects.json"),
        ):
            p=patch.object(mod,attr,value)
            p.start();self.addCleanup(p.stop)
        p=patch("scripts.volleyball_ui.preview.video_info",
                return_value={"duration_sec":15,"width":320,"height":180,
                              "auto_width":320})
        p.start();self.addCleanup(p.stop)
        self.pid=projects.create_project("Test",["match.mp4"])["id"]
        self.cache=projects.project_cache_root(self.pid)/("a"*24)
        (self.cache/"ball/analysis").mkdir(parents=True)
        (self.cache/"analysis.mp4").write_bytes(b"tiny-analysis-proxy")
        (self.cache/"players.json").write_text(json.dumps({
            "model":"RAVEL-VB-013-9f","fps":30,"source_size":{"width":320,"height":180},
            "predictions":[]}),encoding="utf8")
        (self.cache/"ball/analysis/ball.csv").write_text(
            "Frame,Visibility,X,Y,Radius\n",encoding="utf8")
        (self.cache/"inference.manifest.json").write_text(json.dumps({
            "schema":"volley-inference/1","source":str(self.source.resolve()),
            "width":320,"height":180,"fps":30,"ball_model":"VballNetV4c",
        }),encoding="utf8")

    def _make_cpu(self, suffix):
        folder=self.cache/"postprocessing"/suffix
        (folder/"analysis").mkdir(parents=True)
        (folder/"postprocess.manifest.json").write_text(json.dumps({
            "schema":"volley-postprocessing/1","version_id":suffix,
            "inference_cache":str(self.cache.resolve()),
            "settings":{"mode":"combined"}}),encoding="utf8")
        (folder/"ranking.json").write_text("[]",encoding="utf8")
        (folder/"analysis/events.json").write_text(json.dumps({
            "source":str(self.source.resolve()),"events":[]}),encoding="utf8")
        (folder/"analysis/game-tracks.json").write_text("[]",encoding="utf8")
        return folder

    def test_first_run_finishes_without_any_cpu_events(self):
        stage_rows=[]
        for i in range(1,4):
            stage_rows.extend([
                json.dumps({"event":"ANALYSIS_STAGE","stage_index":i,"phase":"start"}),
                json.dumps({"event":"ANALYSIS_STAGE","stage_index":i,"phase":"complete"}),
            ])
        commands=[]
        cache=self.cache
        output=projects.project_run_root(self.pid)
        class FakeProcess:
            def __init__(self,cmd,**kwargs):
                commands.append(cmd)
                target=output/"fake-video"/"reports/processing.json"
                target.parent.mkdir(parents=True,exist_ok=True)
                target.write_text(json.dumps({
                    "cache":str(cache.resolve()),"source":str(cache_base_source),
                    "status":"inference_ready"}),encoding="utf8")
                self.stdout=io.StringIO("\n".join(stage_rows)+"\n")
            def wait(self):return 0
        cache_base_source=self.source.resolve()
        pins=[
            patch("scripts.volleyball_ui.prepared_videos.ready_path",
                  return_value=self.source),
            patch("scripts.run_project_analysis.subprocess.Popen", FakeProcess),
            patch.object(sys,"argv",["run_project_analysis",
                  "--project-id",self.pid,"--profile-name","GPU only",
                  "--analysis-width","320"]),
        ]
        for item in pins:item.start();self.addCleanup(item.stop)
        with redirect_stdout(io.StringIO()):
            run_project_analysis.main()
        self.assertEqual(len(commands),1)
        self.assertIn("--inference-only",commands[0])
        self.assertNotIn("--csv_path",commands[0])
        self.assertFalse((self.cache/"analysis/events.json").exists())
        history=projects.project_run_root(self.pid)/"analysis_timings"
        records=list(history.glob("*.json"))
        self.assertEqual(len(records),1)
        timing=json.loads(records[0].read_text(encoding="utf8"))
        self.assertTrue(timing["inference_only"])
        self.assertEqual(len(timing["stage_names"]),3)
        self.assertEqual(len(timing["videos"][0]["stages"]),3)
        runs=list_runs(self.pid)
        self.assertEqual(len(runs),1)
        self.assertTrue(runs[0]["inference_only"])
        self.assertEqual(cache_for("match.mp4",self.pid,self.cache.name),self.cache)
        self.assertEqual(runs[0]["videos"]["match.mp4"],self.cache.name)

    def test_cpu_version_shares_manually_confirmed_player_id(self):
        suffix1="b"*16
        suffix2="c"*16
        self._make_cpu(suffix1)
        self._make_cpu(suffix2)
        data.create_player("Player A")
        person=next(item for item in data.global_players()
                    if item["name"]=="Player A")
        data.attach_player(person["id"],self.pid)
        composite=self.cache.name+"."+suffix1
        other=self.cache.name+"."+suffix2
        linked=data.assign("match.mp4",["101"],project_id=self.pid,
                           player_id=person["id"],cache_id=composite)
        self.assertEqual(linked[0]["track_ids"],["101"])
        self.assertEqual(data.groups_for("match.mp4",self.pid,other)[0]["id"],
                         person["id"])
        self.assertEqual(data.groups_for("match.mp4",self.pid,self.cache.name)[0]
                         ["track_ids"],["101"])
        storage=data._identities(self.pid)["videos"]
        self.assertIn("match.mp4::"+self.cache.name,storage)
        self.assertNotIn("match.mp4::"+composite,storage)
        data.unassign("match.mp4",["101"],self.pid,other)
        self.assertEqual(data.groups_for("match.mp4",self.pid,composite),[])

    def test_legacy_cpu_links_migrate_without_conflicting_owners(self):
        a,b=self._make_cpu("b"*16),self._make_cpu("c"*16)
        legacy={
            "schema":2,
            "videos":{
                "match.mp4::"+self.cache.name+"."+a.name:[
                    {"id":"person-a","name":"A","track_ids":["11","12"]}],
                "match.mp4::"+self.cache.name+"."+b.name:[
                    {"id":"person-a","name":"A","track_ids":["11"]},
                    {"id":"person-b","name":"B","track_ids":["12","13"]}],
            }
        }
        data.save_json(data.identity_file(self.pid),legacy)
        with patch("scripts.volleyball_ui.data.global_players",
                   return_value=[{"id":"person-a","name":"A"},
                                 {"id":"person-b","name":"B"}]), patch(
            "scripts.volleyball_ui.data.project_player_ids",
            return_value=["person-a","person-b"]):
            groups=data.groups_for("match.mp4",self.pid,
                                  self.cache.name+"."+a.name)
        self.assertEqual(
            {g["id"]:g["track_ids"] for g in groups},
            {"person-a":["11"],"person-b":["13"]},
            "Conflicting older identities must not be assigned silently")

    def test_http_preview_shows_gpu_only_without_fake_rallies(self):
        import threading
        import urllib.request
        from http.server import ThreadingHTTPServer
        from scripts.volleyball_ui.server import Handler
        server=ThreadingHTTPServer(("127.0.0.1",0),Handler)
        thread=threading.Thread(target=server.serve_forever,daemon=True)
        thread.start()
        self.addCleanup(thread.join,3)
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        from unittest.mock import patch
        from urllib.parse import urlencode
        with patch("scripts.volleyball_ui.server._normalized_duration",return_value=15):
            query=urlencode({"project_id":self.pid,"video":"match.mp4",
                             "cache_id":self.cache.name})
            with urllib.request.urlopen(
                    f"http://127.0.0.1:{server.server_port}/api/analysis-view?{query}",
                    timeout=6) as response:
                result=json.load(response)
        self.assertFalse(result["analyzed"])
        self.assertTrue(result["inference_ready"])
        self.assertFalse(result["postprocessed"])
        self.assertEqual(result["rallies"],[])
        self.assertEqual(result["actions"],[])
        self.assertIn("analysis-media",result["media_url"])
        self.assertEqual(result["duration_sec"],15)

    def test_http_job_command_forwards_all_cpu_geometry_options(self):
        from contextlib import nullcontext
        from types import SimpleNamespace
        from scripts.volleyball_ui import server
        captured=[]
        def fake_popen(argv, **kwargs):
            captured.extend(argv)
            return SimpleNamespace(poll=lambda:0,returncode=0,pid=1234)
        options={
            "project_id":self.pid,"run_id":"run_20261104_123000_123456",
            "mode":"combined","touch_threshold":.7,
            "touch_min_interval_sec":.45,"touch_max_reach_ratio":.25,
            "ball_threshold":.55,"player_threshold":.65,
            "track_min_duration":1.1,"track_max_distance":160,
            "track_player_threshold":.4,"track_ball_threshold":.6,
            "track_no_ball_fill":True,"beach":True,
        }
        with patch.object(server,"project_videos",return_value=[self.source]), patch(
            "scripts.volleyball_ui.project_lifecycle.job_launch_guard",
            return_value=nullcontext()), patch.object(
            server,"project_logs_root",return_value=self.results/"logs"), patch.object(
            server,"JOB",{**server.JOB,"process":None}), patch.object(
            server.subprocess,"Popen",side_effect=fake_popen):
            server.start_job("reprocess", options)
        self.assertIn("scripts.run_project_reprocess",captured)
        for key,expected in (
            ("--touch-threshold","0.7"),
            ("--touch-min-interval-sec","0.45"),
            ("--touch-max-reach-ratio","0.25"),
            ("--ball-threshold","0.55"),
            ("--player-threshold","0.65"),
            ("--track-min-duration","1.1"),
            ("--track-max-distance","160.0"),
            ("--track-player-threshold","0.4"),
            ("--track-ball-threshold","0.6"),
        ):
            self.assertIn(key,captured)
            self.assertEqual(captured[captured.index(key)+1],expected)
        self.assertIn("--track-no-ball-fill",captured)
        self.assertIn("--beach",captured)
        self.assertNotIn("--inference-only",captured)

    def test_verified_player_identity_flows_to_preview_and_montage(self):
        from scripts.volleyball_ui.rally_view import from_tracks
        from scripts.volleyball_ui.montage_project import plan_rallies
        cache=self._make_cpu("b"*16)
        tracked={
            "track_id":1,"start_frame":0,"last_frame":90,
            "positions":[[[20,20],5],[[40,20],45],[[30,30],80]],
            "rally_classification":{"label":"rally","is_rally":True,
                                    "rally_confidence":.95},
            "player_interaction":{"contacts":[]}
        }
        (cache/"analysis/game-tracks.json").write_text(
            json.dumps([tracked]),encoding="utf8")
        data.create_player("Alice")
        person=next(p for p in data.global_players() if p["name"]=="Alice")
        data.attach_player(person["id"],self.pid)
        data.assign("match.mp4",["101"],project_id=self.pid,
                    player_id=person["id"],cache_id=self.cache.name+"."+cache.name)
        fake_occupancy=[{
            "player_count":5,
            "player_track_ids":["101","102","103"],
            "linked_player_ids":[]}]
        with patch("scripts.volleyball_ui.rally_players.player_info",
                   return_value=fake_occupancy):
            shown=from_tracks(cache,0,30,10,gap_sec=0,min_sec=1,context_sec=0,
                              source_video="match.mp4",project_id=self.pid)
            exported=plan_rallies([{"cache":cache,"video":"match.mp4",
                                    "project_id":self.pid}],60,
                                  list_candidates=True)
        self.assertEqual(len(shown["rallies"]),1)
        self.assertEqual(shown["rallies"][0]["player_count"],5)
        self.assertEqual(shown["rallies"][0]["linked_player_ids"],[person["id"]])
        self.assertEqual(shown["rallies"][0]["linked_players"][0]["name"],"Alice")
        self.assertEqual(shown["rallies"][0]["unlinked_track_ids"],["102","103"])
        self.assertTrue(exported)
        self.assertEqual(exported[0]["linked_player_ids"],[person["id"]])
        self.assertEqual(exported[0]["linked_players"][0]["name"],"Alice")

    def test_provisional_links_never_become_real_identities(self):
        def row(tid,frame,x):
            return {"class_name":"player","track_id":tid,"frame_index":frame,
                    "score":.99,"bbox_xyxy":[x,20,x+20,80]}
        sample=(
            [row(101,f,100) for f in range(0,9)] +
            [row(102,f,101) for f in range(12,22)] +
            [row(103,f,220) for f in range(12,22)]
        )
        result=group_tracklets(sample,320,180)
        self.assertEqual(result["raw_track_ids"],3)
        self.assertFalse(result["confirmed"])
        self.assertNotIn("player_id",result)
        self.assertEqual(result["track_to_candidate"]["101"],
                         result["track_to_candidate"]["102"])
        self.assertNotEqual(result["track_to_candidate"]["101"],
                            result["track_to_candidate"]["103"])
        self.assertEqual(result["candidate_groups"],2)


if __name__=="__main__":
    unittest.main()
