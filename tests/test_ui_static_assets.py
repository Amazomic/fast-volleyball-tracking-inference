"""Integration regression: every UI script/style in index must be served."""
from __future__ import annotations

import re
import threading
import unittest
import urllib.error
import urllib.request
from http.server import ThreadingHTTPServer

from scripts.volleyball_ui.server import Handler


class HeaderAssetServingTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.http = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        cls.thread = threading.Thread(target=cls.http.serve_forever, daemon=True)
        cls.thread.start()
        cls.base = f"http://127.0.0.1:{cls.http.server_port}"

    @classmethod
    def tearDownClass(cls):
        cls.http.shutdown()
        cls.http.server_close()
        cls.thread.join(timeout=3)

    def test_all_linked_scripts_and_styles_have_successful_http_responses(self):
        with urllib.request.urlopen(self.base + "/", timeout=5) as response:
            html = response.read().decode("utf-8")
        assets = re.findall(r'<(?:script|link)[^>]+(?:src|href)="(/[^"]+\.(?:js|css)(?:\?[^"]*)?)"', html)
        self.assertGreaterEqual(len(assets), 10, "Expected UI JavaScript/CSS assets")
        self.assertIn("/header-chain-menu.js?v=3", assets)
        for asset in assets:
            with self.subTest(asset=asset):
                try:
                    with urllib.request.urlopen(self.base + asset, timeout=5) as response:
                        payload = response.read()
                        self.assertEqual(response.status, 200)
                        self.assertGreater(len(payload), 0)
                        if ".js" in asset:
                            self.assertIn("javascript", response.headers.get("Content-Type", ""))
                except urllib.error.HTTPError as error:
                    self.fail(f"{asset} returned HTTP {error.code}")


if __name__ == "__main__":
    unittest.main()
