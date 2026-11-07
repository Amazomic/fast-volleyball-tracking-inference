import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui import diagnostics


class SelectedDiagnosticCacheTests(unittest.TestCase):
    def test_selected_cache_is_used_and_retained_in_download_links(self):
        with tempfile.TemporaryDirectory() as directory:
            selected = Path(directory) / "selected"
            (selected / "tracks/analysis/tracks").mkdir(parents=True)
            (selected / "analysis.mp4").touch()
            output = selected / "exports/diagnostics"
            output.mkdir(parents=True)
            (output / "rallies-combined.mp4").write_bytes(b"test")
            with patch.object(diagnostics, "assert_project_video"), patch(
                "scripts.volleyball_ui.analysis_runs.cache_for", return_value=selected
            ) as resolve:
                files = diagnostics.outputs("project", "match.mp4", cache_id="chosen")
                resolve.assert_called_with("match.mp4", "project", "chosen")
                self.assertIn("cache_id=chosen", files[0]["url"])
                self.assertEqual(
                    diagnostics.output_path("project", "match.mp4", "rallies-combined.mp4", cache_id="chosen"),
                    output / "rallies-combined.mp4",
                )

    def test_missing_selected_cache_never_falls_back_to_latest(self):
        with patch.object(diagnostics, "assert_project_video"), patch(
            "scripts.volleyball_ui.analysis_runs.cache_for", return_value=None
        ), patch.object(diagnostics, "find_cache") as latest:
            with self.assertRaises(FileNotFoundError):
                diagnostics.paths("project", "match.mp4", cache_id="deleted")
            latest.assert_not_called()


if __name__ == "__main__":
    unittest.main()
