"""Shared project preparation: background progress, cache reuse and invalidation."""
import fcntl
import json
import subprocess
import tempfile
import urllib.error
import urllib.request
from threading import Thread
from http.server import ThreadingHTTPServer
import time
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui import data, projects, prepared_videos


class PreparedVideoTests(unittest.TestCase):
    def setUp(self):
        tmp=tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root=Path(tmp.name)
        source=self.root/"project"
        source.mkdir()
        results=self.root/"results"
        results.mkdir()
        for obj, attr, value in (
            (data,"PROJECT",source),
            (data,"RESULTS",results),
            (projects,"PROJECTS_DIR",results/"projects"),
            (projects,"REGISTER",results/"projects"/"projects.json"),
        ):
            p=patch.object(obj,attr,value)
            p.start()
            self.addCleanup(p.stop)
        self.source=source/"tiny.mp4"
        subprocess.run(["ffmpeg","-y","-v","error","-f","lavfi",
                        "-i","testsrc2=size=320x180:rate=24",
                        "-t","1.25","-c:v","mpeg4",str(self.source)],
                       check=True,timeout=20)
        self.project=projects.create_project("Tiny",["tiny.mp4"])

    def test_auto_prepare_and_reuse(self):
        project_id=self.project["id"]
        first=prepared_videos.status(project_id,auto_queue=True)
        self.assertEqual(first["total"],1)
        self.assertIn(first["videos"][0]["state"],("queued","preparing","ready"))
        deadline=time.monotonic()+25
        while time.monotonic()<deadline:
            current=prepared_videos.status(project_id)
            if current["videos"][0]["state"] in ("ready","failed"):
                break
            time.sleep(.15)
        self.assertEqual(current["videos"][0]["state"],"ready",current)
        converted=prepared_videos.ready_path(project_id,"tiny.mp4")
        self.assertIsNotNone(converted)
        self.assertTrue(converted.is_file())
        args=["ffprobe","-v","error","-select_streams","v:0",
              "-show_entries","stream=codec_name,avg_frame_rate,width",
              "-of","default=nw=1",str(converted)]
        text=subprocess.run(args,check=True,capture_output=True,text=True).stdout
        self.assertIn("codec_name=h264",text)
        self.assertIn("avg_frame_rate=30/1",text)
        stat=converted.stat()
        prepared_videos.queue_project(project_id)
        time.sleep(.1)
        self.assertEqual(converted.stat().st_mtime_ns,stat.st_mtime_ns)
        self.assertEqual(prepared_videos.status(project_id)["ready"],1)
        self.assertTrue(self.source.exists())

    def test_source_change_invalidates_cached_proxy(self):
        pid=self.project["id"]
        info=prepared_videos._entry(pid,"tiny.mp4")
        source, _, output, metadata, ident=info
        output.parent.mkdir(parents=True,exist_ok=True)
        output.write_bytes(b"prepared")
        prepared_videos._save(metadata,{"identity":ident,"status":"ready","progress":100})
        self.assertEqual(prepared_videos.ready_path(pid,"tiny.mp4"),output)
        source.write_bytes(source.read_bytes()+b"change")
        self.assertIsNone(prepared_videos.ready_path(pid,"tiny.mp4"))

    def test_two_servers_cannot_queue_same_video_at_once(self):
        pid=self.project["id"]
        _,_,output,metadata,identity=prepared_videos._entry(pid,"tiny.mp4")
        handle=prepared_videos._acquire_preparation_lock(output)
        self.assertIsNotNone(handle)
        try:
            prepared_videos._save(metadata,{
                "identity":identity,"status":"preparing","progress":42,
            })
            with patch.object(prepared_videos._POOL,"submit") as submit:
                prepared_videos.queue_project(pid)
                submit.assert_not_called()
            self.assertEqual(prepared_videos._manifest(metadata)["progress"],42)
            row=prepared_videos.status(pid)["videos"][0]
            self.assertEqual(row["state"],"preparing")
            self.assertEqual(row["progress"],42)
        finally:
            handle.close()

    def test_preview_waits_for_prepared_cache_and_serves_only_that_file(self):
        from scripts.volleyball_ui import server
        pid=self.project["id"]
        _,_,output,metadata,identity=prepared_videos._entry(pid,"tiny.mp4")
        web=ThreadingHTTPServer(("127.0.0.1",0),server.Handler)
        thread=Thread(target=web.serve_forever,daemon=True)
        thread.start()
        self.addCleanup(web.shutdown)
        self.addCleanup(web.server_close)
        base=f"http://127.0.0.1:{web.server_port}"
        view=base+"/api/video-view?project_id="+pid+"&video=tiny.mp4"
        media=base+"/api/video-media?project_id="+pid+"&video=tiny.mp4"
        with urllib.request.urlopen(view,timeout=6) as response:
            pending=json.load(response)
        self.assertEqual(pending["browser_preview"],"preparing")
        self.assertIsNone(pending["preview_url"])
        with self.assertRaises(urllib.error.HTTPError) as error:
            urllib.request.urlopen(media,timeout=6)
        self.assertEqual(error.exception.code,409)
        output.parent.mkdir(parents=True,exist_ok=True)
        output.write_bytes(b"this is the prepared video only")
        prepared_videos._save(metadata,{
            "identity":identity,"status":"ready","progress":100,
        })
        with urllib.request.urlopen(view,timeout=6) as response:
            completed=json.load(response)
        self.assertEqual(completed["browser_preview"],"prepared")
        with urllib.request.urlopen(media,timeout=6) as response:
            self.assertEqual(response.read(),b"this is the prepared video only")

    def test_membership_validation(self):
        with self.assertRaises(ValueError):
            prepared_videos.ready_path(self.project["id"],"not_project.mp4")


if __name__=="__main__":
    unittest.main()
