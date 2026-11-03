"""Browser uploads must stream safely and start project preparation automatically."""
import io
import json
import tempfile
import unittest
from http.server import ThreadingHTTPServer
from pathlib import Path
from threading import Thread
from urllib.error import HTTPError
from urllib.request import Request, urlopen
from unittest.mock import patch

from scripts.volleyball_ui import data, projects, server, video_uploads


class BrowserVideoUploadTests(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        self.source = self.root / "project"
        self.source.mkdir()
        result = self.root / "results"
        result.mkdir()
        for module, key, value in (
            (data, "PROJECT", self.source),
            (data, "RESULTS", result),
            (projects, "PROJECTS_DIR", result/"projects"),
            (projects, "REGISTER", result/"projects"/"projects.json"),
        ):
            m = patch.object(module, key, value)
            m.start()
            self.addCleanup(m.stop)
        video_uploads._SESSIONS.clear()
        self.addCleanup(video_uploads._SESSIONS.clear)
        fake_probe = patch.object(video_uploads.subprocess, "run",
                                  return_value=type("Probe", (), {"stdout": "video\n"})())
        fake_probe.start()
        self.addCleanup(fake_probe.stop)
        info = patch("scripts.volleyball_ui.preview.video_info",
                     return_value={"duration_sec": 12.0})
        info.start()
        self.addCleanup(info.stop)

    def test_streaming_multiple_files_and_no_overwrite(self):
        original = self.source/"match.mp4"
        original.write_bytes(b"original content")
        contents = [b"one"*200, b"two"*400]
        produced = []
        for blob in contents:
            record = video_uploads.start("match.mp4", len(blob))
            token = record["upload_id"]
            self.assertNotEqual(record["filename"], "match.mp4")
            split = len(blob)//2
            with self.assertRaises(ValueError):
                video_uploads.append(token, split, io.BytesIO(blob), split)
            video_uploads.append(token, 0, io.BytesIO(blob[:split]), split)
            with self.assertRaises(ValueError):
                video_uploads.finish(token)
            video_uploads.append(token, split, io.BytesIO(blob[split:]), len(blob)-split)
            result = video_uploads.finish(token)
            self.assertEqual((self.source/result["filename"]).read_bytes(), blob)
            produced.append(result["filename"])
        self.assertEqual(original.read_bytes(), b"original content")
        self.assertEqual(len(set(produced)), 2)
        self.assertEqual(set(produced), {"match (2).mp4","match (3).mp4"})
        self.assertFalse(list((self.source/".incoming").glob("*.part")))

    def test_invalid_inputs_and_abort(self):
        for filename in ("../traversal.mp4","not-video.txt","c:\\evil.mp4",""):
            with self.subTest(name=filename):
                with self.assertRaises(ValueError):
                    video_uploads.start(filename, 12)
        with self.assertRaises(ValueError):
            video_uploads.start("valid.mp4", 0)
        with self.assertRaises(ValueError):
            video_uploads.start("valid.mp4", video_uploads.MAX_FILE_SIZE+1)
        record=video_uploads.start("valid.mp4", 100)
        token=record["upload_id"]
        with self.assertRaises(ValueError):
            video_uploads.append(token,0,io.BytesIO(b"x"),
                                 video_uploads.CHUNK_LIMIT+1)
        with self.assertRaises(ValueError):
            video_uploads.append(token,0,io.BytesIO(b"x"),100)
        self.assertEqual(video_uploads._SESSIONS[token]["written"],0)
        video_uploads.abort(token)
        self.assertFalse(list((self.source/".incoming").glob("*.part")))

    def test_http_upload_then_project_creation_queues_video(self):
        web = ThreadingHTTPServer(("127.0.0.1",0),server.Handler)
        thread=Thread(target=web.serve_forever,daemon=True)
        thread.start()
        self.addCleanup(web.server_close)
        self.addCleanup(web.shutdown)
        def request(path, payload, method="POST", content_type="application/json"):
            raw = (json.dumps(payload).encode("utf8")
                   if content_type=="application/json" else payload)
            req=Request(f"http://127.0.0.1:{web.server_port}{path}",
                        data=raw,method=method,
                        headers={"Content-Type":content_type})
            with urlopen(req,timeout=8) as response:
                return json.load(response)
        first = request("/api/video-upload/start",
                        {"filename":"new.mp4","size":6})
        token=first["upload_id"]
        request(f"/api/video-upload/chunk?upload_id={token}&offset=0",
                b"abcdef",method="PUT",content_type="application/octet-stream")
        with patch("scripts.volleyball_ui.prepared_videos.queue_project") as queue:
            saved=request("/api/video-upload/finish",{"upload_id":token})
            self.assertEqual(saved["filename"],"new.mp4")
            project=request("/api/project-create",{
                "name":"Uploaded recordings","videos":[saved["filename"]],
                "player_ids":[],
            })["project"]
            self.assertEqual(project["videos"],["new.mp4"])
            queue.assert_called_once_with(project["id"])
        self.assertEqual((self.source/"new.mp4").read_bytes(),b"abcdef")
        with self.assertRaises(HTTPError) as error:
            request("/api/video-upload/start",{"filename":"evil.txt","size":20})
        self.assertEqual(error.exception.code,400)

    def test_legacy_upload_api_remains_available_but_ui_does_not_copy(self):
        root=Path(__file__).resolve().parents[1]
        html=(root/"scripts/volleyball_ui/static/index.html").read_text(encoding="utf8")
        app=(root/"scripts/volleyball_ui/static/app.js").read_text(encoding="utf8")
        server_code=(root/"scripts/volleyball_ui/server.py").read_text(encoding="utf8")
        self.assertNotIn('id="project-video-file-input"',html)
        self.assertNotIn('id="project-local-paths"',html)
        self.assertNotIn('id="project-add-paths"',html)
        self.assertIn('id="project-video-dropzone"',html)
        self.assertNotIn('/api/video-upload/chunk',app)
        self.assertIn('/api/local-videos/add',app)
        self.assertIn('/api/video-upload/start',server_code)


if __name__=="__main__":
    unittest.main()
