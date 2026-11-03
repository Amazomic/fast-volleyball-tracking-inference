"""CPU thread quota is auto-calculated on the analysis host and editable."""
import json
import os
from pathlib import Path
from http.server import ThreadingHTTPServer
from threading import Thread
from urllib.request import urlopen
import unittest

from scripts.volleyball_ui.server import Handler


ROOT=Path(__file__).resolve().parents[1]


class CPUResourcesTests(unittest.TestCase):
    def test_cpu_threads_endpoint_reports_80_percent_of_logical_cores(self):
        server=ThreadingHTTPServer(("127.0.0.1",0),Handler)
        thread=Thread(target=server.serve_forever,daemon=True)
        thread.start()
        try:
            with urlopen(f"http://127.0.0.1:{server.server_port}/api/analysis-resources",timeout=5) as response:
                data=json.load(response)
            available=getattr(os, "process_cpu_count", os.cpu_count)() or 1
            self.assertEqual(data["logical_cpu_count"],available)
            self.assertEqual(data["default_threads"],min(128,max(1,round(available*0.8))))
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=3)

    def test_threads_setting_is_editable_even_for_gpu_backend(self):
        html=(ROOT/"scripts/volleyball_ui/static/index.html").read_text(encoding="utf-8")
        app=(ROOT/"scripts/volleyball_ui/static/app.js").read_text(encoding="utf-8")
        profiles=(ROOT/"scripts/volleyball_ui/static/analysis-presets.js").read_text(encoding="utf-8")
        self.assertIn('name="threads" id="analysis-threads"',html)
        self.assertIn('$("#analysis-threads").disabled=false',app)
        self.assertIn('await fetch("/api/analysis-resources"',profiles)
        self.assertLess(profiles.index('threads.value=String(Math.min(128,Math.max(1,Number(resources.default_threads)||1)))'),
                        profiles.index('snapshotDefaults();'))


if __name__=="__main__":
    unittest.main()
