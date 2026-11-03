"""Local source selection is by filesystem reference, not video upload."""
import json
import os
import subprocess
import urllib.request
from http.server import ThreadingHTTPServer
from threading import Thread
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui import data, local_sources, prepared_videos, projects, server


class LocalSourceTests(unittest.TestCase):
    def setUp(self):
        tmp=tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root=Path(tmp.name)
        self.originals=self.root/"originals"
        self.originals.mkdir()
        self.links=self.root/"project"
        self.links.mkdir()
        self.results=self.root/"results"
        self.results.mkdir()
        for obj,attribute,value in (
            (data,"PROJECT",self.links),
            (data,"RESULTS",self.results),
            (projects,"PROJECTS_DIR",self.results/"projects"),
            (projects,"REGISTER",self.results/"projects"/"projects.json"),
        ):
            m=patch.object(obj,attribute,value)
            m.start()
            self.addCleanup(m.stop)

    def test_register_links_no_copy_no_source_mutation(self):
        first=self.originals/"match.mp4"
        first.write_bytes(b"FIRST SOURCE")
        second=self.root/"other"/"match.mp4"
        second.parent.mkdir()
        second.write_bytes(b"SECOND SOURCE")
        registered=local_sources.register_paths([str(first),str(second)])
        self.assertEqual([v["name"] for v in registered],
                         ["match.mp4","match (2).mp4"])
        for record,source in zip(registered,[first,second]):
            link=self.links/record["name"]
            self.assertTrue(link.is_symlink())
            self.assertEqual(link.resolve(),source.resolve())
            self.assertEqual(data.video_path(record["name"]).read_bytes(),
                             source.read_bytes())
        self.assertEqual(local_sources.register_paths([str(first)])[0]["name"],
                         "match.mp4")
        self.assertEqual(first.read_bytes(),b"FIRST SOURCE")
        self.assertEqual(second.read_bytes(),b"SECOND SOURCE")
        self.assertFalse((self.root/"results"/"prepared").exists())

    def test_project_references_original_and_places_prepared_in_results(self):
        original=self.originals/"long_game.mp4"
        original.write_bytes(b"original content is not copied")
        record=local_sources.register_paths([str(original)])[0]
        info={"duration_sec":12.0,"width":1280,"height":720,"auto_width":640}
        with patch("scripts.volleyball_ui.preview.video_info",return_value=info),\
             patch.object(prepared_videos,"video_info",return_value=info):
            project=projects.create_project("Path-only", [record["name"]])
            source,metadata,output,json_file,identity=prepared_videos._entry(
                project["id"],record["name"])
        self.assertTrue(source.is_symlink())
        self.assertEqual(source.resolve(),original.resolve())
        self.assertEqual(output.parent, self.results/"prepared")
        self.assertNotEqual(output.resolve(),source.resolve())
        self.assertEqual(original.read_bytes(),b"original content is not copied")

    def test_real_normalization_reads_link_and_writes_only_into_results(self):
        source=self.originals/"real-video.mp4"
        subprocess.run([
            "ffmpeg","-hide_banner","-loglevel","error","-y",
            "-f","lavfi","-i","color=c=blue:s=160x90:r=12",
            "-t","0.7","-pix_fmt","yuv420p",str(source),
        ],check=True,timeout=20)
        initial=source.read_bytes()
        video=local_sources.register_paths([str(source)])[0]["name"]
        project=projects.create_project("External source", [video])
        prepared_videos.queue_project(project["id"])
        deadline=time.monotonic()+15
        while time.monotonic()<deadline:
            state=prepared_videos.status(project["id"])
            if state["videos"][0]["state"] in ("ready","failed"):
                break
            time.sleep(.1)
        self.assertEqual(state["videos"][0]["state"],"ready",state)
        prepared=prepared_videos.ready_path(project["id"],video)
        self.assertIsNotNone(prepared)
        self.assertEqual(prepared.parent,self.results/"prepared")
        self.assertNotEqual(prepared.resolve(),source.resolve())
        self.assertEqual(source.read_bytes(),initial)
        self.assertEqual([entry.name for entry in self.links.iterdir()],[video])
        self.assertTrue((self.links/video).is_symlink())

    def test_rejects_missing_relative_and_invalid_file_types(self):
        bad=["match.mp4",str(self.originals/"missing.mp4"),
             str(self.originals/"notes.txt"),"../unsafe.mp4"]
        for source in bad:
            with self.subTest(source=source),self.assertRaises(ValueError):
                local_sources.register_paths([source])
        with self.assertRaises(ValueError):
            local_sources.register_paths([])
        with self.assertRaises(ValueError):
            local_sources.register_paths(["/tmp/a.mp4"]*31)

    def test_windows_drive_path_maps_to_wsl_source(self):
        if not os.path.exists("/mnt/c"):
            self.skipTest("Requires WSL")
        # /tmp is a WSL UNC location; use a temporary path on the Windows drive.
        with tempfile.TemporaryDirectory(dir=Path(__file__).resolve().parents[1]) as folder:
            source=Path(folder)/"drive name.mp4"
            source.write_bytes(b"source")
            windows=subprocess.run(["wslpath","-w",str(source)],
                                   capture_output=True,text=True,check=True).stdout.strip()
            self.assertEqual(local_sources._source_path(windows),source.resolve())

    def test_http_path_registration_and_project_without_transfer(self):
        web=ThreadingHTTPServer(("127.0.0.1",0),server.Handler)
        thread=Thread(target=web.serve_forever,daemon=True)
        thread.start()
        self.addCleanup(web.server_close)
        self.addCleanup(web.shutdown)
        base=f"http://127.0.0.1:{web.server_port}"
        def request(route,obj):
            req=urllib.request.Request(
                base+route,method="POST",data=json.dumps(obj).encode("utf8"),
                headers={"Content-Type":"application/json"})
            with urllib.request.urlopen(req,timeout=6) as response:
                return json.load(response)
        original=self.originals/"v.mp4"
        original.write_bytes(b"unchanged source bytes")
        linked=request("/api/local-videos/add",{"paths":[str(original)]})["videos"]
        self.assertEqual(len(linked),1)
        self.assertEqual(linked[0]["source_path"],str(original.resolve()))
        self.assertTrue((self.links/"v.mp4").is_symlink())
        info={"duration_sec":5.0,"width":1280,"height":720}
        with patch("scripts.volleyball_ui.preview.video_info",return_value=info),\
             patch("scripts.volleyball_ui.prepared_videos.queue_project") as queued:
            project=request("/api/project-create",{"name":"No copy","videos":["v.mp4"]})["project"]
            queued.assert_called_once_with(project["id"])
        self.assertEqual(original.read_bytes(),b"unchanged source bytes")
        self.assertEqual(project["videos"],["v.mp4"])

    def test_windows_picker_starts_in_repo_project_directory(self):
        import base64
        from types import SimpleNamespace
        captured=[]
        def fake_run(command,**kwargs):
            if command[0]=="wslpath":
                return SimpleNamespace(stdout=r"C:\volley\project"+"\n")
            if "-EncodedCommand" in command:
                value=command[command.index("-EncodedCommand")+1]
                captured.append(base64.b64decode(value).decode("utf-16le"))
                encoded=base64.b64encode(b"[]")
                return SimpleNamespace(stdout=encoded+b"\n")
            raise AssertionError(command)
        with patch.object(local_sources.sys,"platform","linux"),\
             patch.object(local_sources.subprocess,"run",side_effect=fake_run),\
             patch.object(Path,"is_file",return_value=True):
            self.assertEqual(local_sources._open_windows_picker(),[])
            self.assertEqual(local_sources._open_windows_picker("audio"),[])
        self.assertEqual(len(captured),2)
        self.assertIn("$picker.InitialDirectory='C:\\volley\\project'",captured[0])
        self.assertIn("$picker.Multiselect=$true",captured[0])
        self.assertIn("Audio files|*.mp3;",captured[1])
        self.assertIn("$picker.InitialDirectory='C:\\volley\\project'",captured[1])

    def test_async_native_picker_returns_only_paths(self):
        original=self.originals/"volley.mp4"
        original.write_bytes(b"source")
        with patch.object(local_sources,"_open_windows_picker",
                          return_value=[str(original)]):
            identifier=local_sources.begin_picker()["picker_id"]
            for _ in range(50):
                state=local_sources.picker_status(identifier)
                if state["status"]!="pending":
                    break
                time.sleep(.01)
        self.assertEqual(state["status"],"done")
        self.assertEqual(state["paths"],[str(original)])
        self.assertEqual(list(self.links.iterdir()),[])
        with self.assertRaises(ValueError):
            local_sources.picker_status("../bad")

    def test_ui_keeps_path_only_workflow(self):
        root=Path(__file__).resolve().parents[1]
        html=(root/"scripts/volleyball_ui/static/index.html").read_text("utf8")
        js=(root/"scripts/volleyball_ui/static/app.js").read_text("utf8")
        server=(root/"scripts/volleyball_ui/server.py").read_text("utf8")
        self.assertIn('id="project-video-dropzone"',html)
        self.assertNotIn('id="project-local-paths"',html)
        self.assertNotIn('id="project-add-paths"',html)
        self.assertIn('id="new-project-video-options"',html)
        self.assertIn('edit?api("/api/videos"):Promise.resolve({videos:[]})',js)
        self.assertIn('target.hidden=videos.length===0;',js)
        self.assertNotIn('id="project-video-file-input"',html)
        self.assertIn('/api/local-videos/add',js)
        self.assertIn('/api/local-videos/picker/start',js)
        self.assertIn('/api/local-videos/picker/status',js)
        self.assertNotIn('/api/video-upload/chunk',js)
        self.assertIn('/api/local-videos/add',server)


if __name__=="__main__":
    unittest.main()
