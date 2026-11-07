import json
import tempfile
import unittest
from pathlib import Path

from scripts.volleyball_ui.capabilities import analysis_capabilities


class AnalysisCapabilitiesTests(unittest.TestCase):
    def test_only_modes_supported_by_the_saved_model_are_selectable(self):
        with tempfile.TemporaryDirectory() as directory:
            cache = Path(directory)
            names = ["players.json", "ball/analysis/ball.csv"]
            for name in names:
                target = cache / name
                target.parent.mkdir(parents=True, exist_ok=True)
                target.touch()
            manifest = {"schema": "volley-inference/1", "outputs": dict.fromkeys(names, "hash"),
                        "player_model": "RAVEL-VB-012-9f", "player_action_threshold": .5}
            metadata = cache / "inference.manifest.json"
            metadata.write_text(json.dumps(manifest))
            self.assertEqual(analysis_capabilities(cache)["reprocess_modes"], ["baseline"])
            manifest["player_model"] = "RAVEL-VB-013-9f"
            metadata.write_text(json.dumps(manifest))
            cap = analysis_capabilities(cache)
            self.assertIn("combined", cap["reprocess_modes"])
            self.assertEqual(cap["touch_floor"], .5)
            (cache / "players.raw.v2.jsonl").touch()
            manifest["outputs"]["players.raw.v2.jsonl"] = "hash"
            metadata.write_text(json.dumps(manifest))
            self.assertEqual(analysis_capabilities(cache)["touch_floor"], 0)
            (cache / "players.json").unlink()
            self.assertEqual(analysis_capabilities(cache)["reprocess_modes"], [])

    def test_missing_manifest_has_no_replay_choices(self):
        with tempfile.TemporaryDirectory() as directory:
            self.assertEqual(analysis_capabilities(Path(directory))["reprocess_modes"], [])


if __name__ == "__main__":
    unittest.main()
