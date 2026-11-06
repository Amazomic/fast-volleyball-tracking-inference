"""Project creation opens a browser-native picker and links sources, without uploads."""
from __future__ import annotations

import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui import data, file_selection


class ProjectBrowserPickerTests(unittest.TestCase):
    def test_browser_selected_video_is_linked_without_copy(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            originals = root / "Desktop"
            originals.mkdir()
            video = originals / "example.mp4"
            video.write_bytes(b"original video bytes")
            link_dir = root / "project"
            with patch.object(data, "PROJECT", link_dir):
                selected = [{"name": video.name, "size": video.stat().st_size,
                             "last_modified": video.stat().st_mtime * 1000}]
                response = file_selection.resolve_files(
                    selected, kind="video", roots=[originals], project_sources=True)
                row = response["videos"][0]
                self.assertEqual(row["source_path"], str(video.resolve()))
                self.assertEqual(row["name"], video.name)
                self.assertEqual((link_dir / video.name).resolve(), video)
                self.assertTrue((link_dir / video.name).is_symlink())
                self.assertEqual(video.read_bytes(), b"original video bytes")

    def test_ui_uses_browser_picker_and_not_headless_powershell(self):
        root = Path(__file__).resolve().parents[1]
        html = (root / "scripts/volleyball_ui/static/index.html").read_text("utf8")
        app = (root / "scripts/volleyball_ui/static/app.js").read_text("utf8")
        self.assertIn('id="project-video-file-input"', html)
        self.assertIn('window.showOpenFilePicker', app)
        self.assertIn('"/api/project/resolve-files"', app)
        self.assertIn('acceptProjectFiles(event.dataTransfer?.files)', app)
        self.assertIn('selected.map(file=>({name:file.name,size:file.size,', app)
        self.assertNotIn('await api("/api/local-videos/picker/start",{});', app)


if __name__ == "__main__":
    unittest.main()
