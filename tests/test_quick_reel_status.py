"""Persistent Quick Reel header state and local file URI API."""
import json
import threading
import tempfile
import unittest
import urllib.request
from http.server import ThreadingHTTPServer
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui import server, quick_reel_jobs
from scripts.volleyball_ui import data
from scripts.volleyball_ui.local_sources import _source_path
from scripts.volleyball_ui.montage_audio import resolve_audio


class QuickReelStatusApiTests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root=Path(self.tmp.name)
        self.reel_id="a"*32
        self.project=self.root/"quick_reels"/self.reel_id
        self.project.mkdir(parents=True)
        self.patch_root=patch.object(quick_reel_jobs,"QUICK_ROOT",
                                     self.root/"quick_reels")
        self.patch_root.start()
        self.addCleanup(self.patch_root.stop)
        data.save_json(self.project/"request.json",{
            "id":self.reel_id,
            "videos":[{"video":"match.mp4"},{"video":"second.mp4"}],
            "audio_path":str(self.root/"music.mp3"),"duration":20.0,
        })
        self.status={
            "status":"running","stage":"Подготовка видео: 1/2",
            "progress":8.4,"id":self.reel_id,
            "videos":[{"video":"match.mp4","state":"ready","progress":100},
                      {"video":"second.mp4","state":"preparing","progress":20}],
        }
        data.save_json(self.project/"status.json",self.status)
        self.library=self.root/"library"
        self.library.mkdir()
        (self.library/"match.mp4").write_bytes(b"small-unchanged-video")
        self.patches=[
            patch.object(server,"job_state",
                         return_value={"kind":None,"status":"idle","project_id":None}),
            patch.object(data,"PROJECT",self.library),
        ]
        for value in self.patches:
            value.start();self.addCleanup(value.stop)
        self.http=ThreadingHTTPServer(("127.0.0.1",0),server.Handler)
        self.addCleanup(self.http.server_close)
        self.thread=threading.Thread(target=self.http.serve_forever,daemon=True)
        self.thread.start()
        self.addCleanup(self.http.shutdown)
        self.root_url="http://127.0.0.1:"+str(self.http.server_address[1])

    def get(self,url):
        with urllib.request.urlopen(self.root_url+url,timeout=10) as response:
            return json.loads(response.read())

    def test_header_status_survives_server_restart(self):
        result=self.get("/api/quick-reel/status?id="+self.reel_id)
        self.assertEqual(result["status"],"running")
        self.assertEqual(result["progress"],8.4)
        self.assertEqual(len(result["videos"]),2)
        self.assertEqual(result["stage"],"Подготовка видео: 1/2")
        self.assertFalse(result["available"])

    def test_previous_project_files_are_not_suggested(self):
        import urllib.error
        with self.assertRaises(urllib.error.HTTPError) as caught:
            self.get("/api/quick-reel/sources")
        self.assertEqual(caught.exception.code,404)

    def test_independent_result_serves_mp4_without_project(self):
        finished=self.project/"montage"/"highlights_reel.mp4"
        finished.parent.mkdir()
        finished.write_bytes(b"mock-mp4-content")
        data.save_json(self.project/"result.json",{
            "id":self.reel_id, "video":"montage/highlights_reel.mp4",
            "duration":19.0,"music_duration":20.0,
            "videos":["match.mp4","second.mp4"],
        })
        metadata=self.get("/api/quick-reel/result?id="+self.reel_id)
        self.assertTrue(metadata["available"])
        self.assertIn("?id="+self.reel_id,metadata["url"])
        with urllib.request.urlopen(self.root_url+metadata["url"]) as response:
            self.assertEqual(response.read(),b"mock-mp4-content")
        self.assertFalse((self.project/"project.json").exists())

    def test_history_and_archived_media_survive_a_new_run(self):
        first=self.reel_id
        output=self.project/"montage"/"highlights_reel.mp4"
        output.parent.mkdir()
        output.write_bytes(b"archived-mp4-content")
        data.save_json(self.project/"result.json",{
            "id":first,"video":"montage/highlights_reel.mp4",
            "duration":16.5,"videos":["match.mp4"],
        })
        (self.project/"analysis"/"gpu_cache").mkdir(parents=True)
        (self.project/"analysis"/"gpu_cache"/"scratch.bin").write_bytes(b"scratch")
        second="b"*32
        folder=quick_reel_jobs.quick_dir(second)
        folder.mkdir()
        data.save_json(folder/"request.json",{
            "id":second,"created_at":1800000000,
            "videos":[{"selection_id":"d"*32}],
        })
        quick_reel_jobs.publish_latest(second,str(self.root/"music.mp3"))
        self.assertFalse(self.project.exists())
        history=self.get("/api/quick-reel/history")["results"]
        self.assertEqual(len(history),1)
        self.assertEqual(history[0]["id"],first)
        with urllib.request.urlopen(self.root_url+history[0]["url"]) as response:
            self.assertEqual(response.read(),b"archived-mp4-content")
        with urllib.request.urlopen(self.root_url+history[0]["download_url"]) as response:
            self.assertEqual(response.read(),b"archived-mp4-content")
        self.assertFalse((self.root/"quick_reels"/"completed"/first/"analysis").exists())

    def test_delete_completed_reel_api_removes_only_archived_video(self):
        output=self.project/"montage"/"highlights_reel.mp4"
        output.parent.mkdir()
        output.write_bytes(b"completed-video")
        data.save_json(self.project/"result.json",{
            "id":self.reel_id,"video":"montage/highlights_reel.mp4",
            "duration":20,"videos":["match.mp4"]})
        data.save_json(self.project/"status.json",{
            **self.status,"status":"complete"})
        self.assertEqual(len(self.get("/api/quick-reel/history")["results"]),1)
        self.assertTrue((self.root/"quick_reels"/"completed"/
                         (self.reel_id+".mp4")).exists())
        payload=json.dumps({"id":self.reel_id}).encode()
        request=urllib.request.Request(
            self.root_url+"/api/quick-reel/delete",data=payload,
            headers={"Content-Type":"application/json"},method="POST")
        with urllib.request.urlopen(request,timeout=10) as reply:
            self.assertEqual(json.loads(reply.read()),{"deleted":self.reel_id})
        self.assertEqual(self.get("/api/quick-reel/history")["results"],[])
        self.assertTrue((self.project/"request.json").is_file())
        self.assertTrue((self.library/"match.mp4").is_file())

    def test_file_uris_resolve_to_original_not_new_copy(self):
        original=self.root/"source recording.mp4"
        soundtrack=self.root/"sound track.mp3"
        original.write_bytes(b"real-source")
        soundtrack.write_bytes(b"music")
        self.assertEqual(_source_path(original.as_uri()),original.resolve())
        self.assertEqual(resolve_audio(soundtrack.as_uri()),soundtrack.resolve())


if __name__=="__main__":
    unittest.main()
