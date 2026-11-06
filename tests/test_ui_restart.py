"""UI restart must be in-place, idempotent and safe for active jobs."""
import json
import threading
import unittest
from http.server import ThreadingHTTPServer
from urllib.error import HTTPError
from urllib.request import Request, urlopen
from unittest.mock import patch

from scripts.volleyball_ui import server as volley_server


class RunningProcess:
    def poll(self):
        return None


class RestartTests(unittest.TestCase):
    def setUp(self):
        self.new_restart = threading.Event()
        self.patch = patch.object(volley_server, "_RESTART_PENDING", self.new_restart)
        self.patch.start()
        self.addCleanup(self.patch.stop)
        self.httpd = ThreadingHTTPServer(("127.0.0.1", 0), volley_server.Handler)
        self.httpd.reload_requested = threading.Event()
        self.url = "http://127.0.0.1:" + str(self.httpd.server_port)
        self.thread = threading.Thread(target=self.httpd.serve_forever, daemon=True)
        self.thread.start()
        self.addCleanup(self.httpd.server_close)
        self.addCleanup(self.httpd.shutdown)
        self.original = volley_server.JOB["process"]
        self.addCleanup(lambda: volley_server.JOB.__setitem__("process", self.original))
        volley_server.JOB["process"] = None

    def post(self, path):
        req = Request(self.url+path, data=b"{}", method="POST",
                      headers={"Content-Type": "application/json"})
        return urlopen(req, timeout=3)

    def test_restart_rejected_while_processing(self):
        volley_server.JOB["process"] = RunningProcess()
        with self.assertRaises(HTTPError) as caught:
            self.post("/api/system/restart")
        self.assertEqual(caught.exception.code, 409)
        self.assertFalse(self.new_restart.is_set())
        self.assertFalse(self.httpd.reload_requested.is_set())

    def test_restart_shuts_down_one_server_after_ack(self):
        with urlopen(self.url+"/api/health", timeout=3) as response:
            before = json.load(response)
        self.assertEqual(before["instance_id"], volley_server._INSTANCE_ID)
        self.assertGreaterEqual(before["api_version"], 28)
        with self.post("/api/system/restart") as response:
            answer = json.load(response)
        self.assertEqual(answer, {"accepted": True, "instance_id": before["instance_id"]})
        self.assertTrue(self.new_restart.is_set())
        self.assertTrue(self.httpd.reload_requested.is_set())
        self.thread.join(3)
        self.assertFalse(self.thread.is_alive(), "the old server must stop after its ACK")


if __name__ == "__main__":
    unittest.main()
