"""The profile UI and backend agree on all accepted settings."""
import json
from html.parser import HTMLParser
from http.server import ThreadingHTTPServer
from pathlib import Path
from threading import Thread
from urllib.request import urlopen
import unittest

from scripts.volleyball_ui.analysis_profiles import _clean_values, profile_schema
from scripts.volleyball_ui.server import Handler

ROOT = Path(__file__).resolve().parents[1]
STATIC = ROOT / "scripts/volleyball_ui/static"


class Controls(HTMLParser):
    def __init__(self):
        super().__init__()
        self.fields = {}
    def handle_starttag(self, tag, attributes):
        a = dict(attributes)
        if tag in ("input", "select") and "name" in a:
            self.fields[a["name"]] = {"tag": tag, **a}


class ProfileValidationContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        html = (STATIC / "index.html").read_text(encoding="utf-8")
        editor = html.split('<div class="analysis-profile-editor">', 1)[1].split(
            '<div class="analysis-profile-save"', 1)[0]
        parser = Controls()
        parser.feed(editor)
        cls.fields = parser.fields
        cls.schema = profile_schema()

    def test_all_editor_inputs_are_known_to_backend(self):
        self.assertFalse(set(self.fields) - set(self.schema))
        self.assertIn("gpu_device_id", self.fields)
        self.assertIn("gpu_memory_limit_mb", self.fields)
        self.assertIn("threads", self.fields)
        self.assertIn("analysis_width", self.fields)

    def test_html_ranges_and_choices_match_server(self):
        for name, control in self.fields.items():
            spec = self.schema[name]
            if control.get("type") != "number":
                continue
            with self.subTest(name=name):
                self.assertEqual(float(control["min"]), spec["min"])
                self.assertEqual(float(control["max"]), spec["max"])
                if spec["type"] == "int":
                    expected = spec.get("multiple_of", 1)
                    self.assertEqual(int(control["step"]), expected)

    def test_complete_form_can_be_saved(self):
        values = {}
        for name, field in self.fields.items():
            if field.get("type") == "number":
                values[name] = float(field["value"])
            elif field.get("type") == "checkbox":
                values[name] = "checked" in field
            elif field.get("type") == "hidden":
                values[name] = field["value"]
            elif field["tag"] == "select":
                choices = self.schema[name]["choices"]
                values[name] = choices[0] if choices else None
        values["manual_width_enabled"] = False
        self.assertEqual(set(_clean_values(values)), set(values))

    def test_supported_gpu_and_cpu_limits(self):
        self.assertEqual(_clean_values({
            "threads": 128,
            "gpu_device_id": 15,
            "gpu_memory_limit_mb": 131072,
            "analysis_width": 1920
        })["threads"], 128)
        for name, value in (
            ("threads", 129),
            ("gpu_device_id", 16),
            ("gpu_memory_limit_mb", -1),
            ("analysis_width", 321),
            ("analysis_width", 1921),
            ("ball_threshold", 1.0),
            ("player_hysteresis_frames", 2.5),
            ("reels_smooth_window", 14),
        ):
            with self.subTest(name=name, value=value):
                with self.assertRaisesRegex(ValueError, ".*"):
                    _clean_values({name: value})

    def test_schema_endpoint_and_live_validation(self):
        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        thread = Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            with urlopen(
                f"http://127.0.0.1:{server.server_port}/api/analysis-profile-schema",
                timeout=5
            ) as response:
                result = json.load(response)
            self.assertEqual(result["fields"], self.schema)
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=3)
        js = (STATIC / "analysis-presets.js").read_text(encoding="utf-8")
        app = (STATIC / "app.js").read_text(encoding="utf-8")
        for snippet in (
            'fetch("/api/analysis-profile-schema"',
            'function validationMessage(el)',
            'function correctNumber(el)',
            'function revealField(el)',
            'validate:validateAll',
        ):
            self.assertIn(snippet, js)
        self.assertIn('VolleyballAnalysisPresets?.validate?', app)


if __name__ == "__main__":
    unittest.main()
