"""Regression tests: historical project preview and quick-reel video share a GET handler."""
from __future__ import annotations

import json
import tempfile
import threading
import unittest
import urllib.parse
import urllib.request
from http.server import ThreadingHTTPServer
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui.server import Handler


class ExistingProjectVideoViewTests(unittest.TestCase):
    def setUp(self):
        self.folder = tempfile.TemporaryDirectory()
        self.addCleanup(self.folder.cleanup)
        self.media = Path(self.folder.name) / "prepared.mp4"
        self.media.write_bytes(b"test-video" * 256)
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.addCleanup(self.stop_server)
        self.root = f"http://127.0.0.1:{self.server.server_port}"

    def stop_server(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)

    def get(self, route, *, range_bytes=False):
        headers = {"Range": "bytes=0-15"} if range_bytes else {}
        request = urllib.request.Request(self.root + route, headers=headers)
        return urllib.request.urlopen(request, timeout=5)

    def test_existing_projects_share_prepared_video(self):
        with (
            patch("scripts.volleyball_ui.server.assert_project_video"),
            patch("scripts.volleyball_ui.server.video_path", return_value=self.media),
            patch("scripts.volleyball_ui.server.video_info", return_value={"duration_sec": 42.0}),
            patch("scripts.volleyball_ui.prepared_videos.ready_path", return_value=self.media),
        ):
            for project_id in ("historical_a", "historical_b"):
                query = urllib.parse.urlencode({
                    "project_id": project_id, "video": "match.mp4"
                })
                with self.subTest(project_id=project_id):
                    with self.get("/api/video-view?" + query) as response:
                        self.assertEqual(response.status, 200)
                        view = json.load(response)
                    self.assertEqual(view["preparation_state"], "ready")
                    self.assertIn(project_id, view["media_url"])
                    with self.get(view["media_url"], range_bytes=True) as response:
                        self.assertEqual(response.status, 206)
                        self.assertEqual(len(response.read()), 16)

    def test_quick_reel_video_uses_its_own_path(self):
        reel_id = "a" * 32
        with (
            patch("scripts.volleyball_ui.quick_reel_jobs.request_for", return_value={}),
            patch("scripts.volleyball_ui.quick_reel_jobs.result_for",
                  return_value=({"duration": 14.0}, self.media)),
        ):
            with self.get("/api/quick-reel/media?id=" + reel_id,
                          range_bytes=True) as response:
                self.assertEqual(response.status, 206)
                self.assertEqual(len(response.read()), 16)


if __name__ == "__main__":
    unittest.main()
